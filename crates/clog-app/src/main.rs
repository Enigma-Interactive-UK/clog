#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Tauri commands take `State` by value by convention; the lint fires on every
// command signature otherwise.
#![allow(clippy::needless_pass_by_value)]

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use clog_core::{
    auto_detect, index_file, sample_lines, scan_records, CompiledPattern, CoreError, LineSource,
    PatternError, RecordHeader, RegexScanner, RegexScannerError, BUILTIN_PATTERNS,
};
use serde::Serialize;
use tauri::State;

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum IpcError {
    #[error("{message}")]
    Io { message: String, path: String },
    #[error("file_id {file_id} is not open")]
    UnknownFile { file_id: u64 },
    #[error("requested range is out of bounds")]
    OutOfRange,
    #[error("pattern compile failed: {message}")]
    BadPattern { message: String },
    #[error("regex compile failed: {message}")]
    BadRegex { message: String },
}

impl From<CoreError> for IpcError {
    fn from(err: CoreError) -> Self {
        match err {
            CoreError::Io { path, source } => Self::Io {
                message: source.to_string(),
                path: path.display().to_string(),
            },
        }
    }
}

impl From<PatternError> for IpcError {
    fn from(err: PatternError) -> Self {
        Self::BadPattern {
            message: err.to_string(),
        }
    }
}

impl From<RegexScannerError> for IpcError {
    fn from(err: RegexScannerError) -> Self {
        Self::BadRegex {
            message: err.to_string(),
        }
    }
}

struct OpenedFile {
    path: PathBuf,
    records: Vec<RecordHeader>,
    /// Cumulative starting physical line for each record. `record_first_line[i]
    /// == records[i].line_offset as u64`. Cached so `get_lines` can binary
    /// search to map line index -> record index in O(log n).
    record_first_line: Vec<u64>,
    /// Total physical line count. Cached so we can answer the tail of the
    /// file without re-scanning.
    line_count: u64,
    bytes: Vec<u8>,
    /// Line-start byte offsets, parallel to `LineIndex::line_offsets`. Used
    /// to slice physical line text out of `bytes` on demand.
    line_offsets: Vec<u64>,
    pattern_source: String,
    pattern_name: Option<String>,
}

impl OpenedFile {
    fn rebuild_line_caches(&mut self, line_count: u64, line_offsets: Vec<u64>) {
        self.line_count = line_count;
        self.line_offsets = line_offsets;
        self.record_first_line = self
            .records
            .iter()
            .map(|r| u64::from(r.line_offset))
            .collect();
    }
}

#[derive(Default)]
struct AppState {
    files: Mutex<HashMap<u64, OpenedFile>>,
    next_id: AtomicU64,
}

#[derive(Debug, Serialize)]
struct OpenedFilePayload {
    file_id: u64,
    path: PathBuf,
    size_bytes: u64,
    line_count: u64,
    record_count: u64,
    /// Name of the auto-detected builtin pattern (`"wsl-oink"`, `"prod"`,
    /// `"log4j2-default"`) or `None` if none matched and we fell back to
    /// best effort.
    pattern_name: Option<String>,
    pattern_source: String,
    /// Match-score (0.0..=1.0) of the chosen pattern against a 64KB sample.
    pattern_score: f32,
}

#[derive(Debug, Serialize)]
struct RecordsPayload {
    /// Index of `headers[0]` in the file's full record list.
    start: u64,
    /// Byte offset of the first byte of `text` in the original file.
    base_offset: u64,
    headers: Vec<RecordHeader>,
    /// UTF-8 (lossy) text of the byte range covered by `headers`.
    text: String,
}

#[derive(Debug, Serialize)]
struct PatternTestPayload {
    score: f32,
    sample_size: u32,
}

#[derive(Debug, Serialize)]
struct ApplyPatternPayload {
    record_count: u64,
    pattern_source: String,
}

#[tauri::command]
fn open_file(state: State<'_, AppState>, path: String) -> Result<OpenedFilePayload, IpcError> {
    let path_buf = PathBuf::from(&path);
    // Sample first to decide which pattern to use.
    let sample = sample_lines(&path_buf, 64 * 1024)?;
    let sample_refs: Vec<&[u8]> = sample.iter().map(Vec::as_slice).collect();
    let (name, scanner, score) = if let Some(hit) = auto_detect(sample_refs.iter().copied()) {
        (Some(hit.0.to_string()), hit.1, hit.2)
    } else {
        // Fallback: still build a wsl-oink scanner so records are at
        // least segmented per line. User can paste the real pattern.
        let scanner = CompiledPattern::compile(BUILTIN_PATTERNS[0].1).expect("builtin valid");
        (None, scanner, 0.0)
    };

    let (source, line_index, records) = index_file(&path_buf, &scanner)?;

    let pattern_source = scanner.source.clone();

    let file_id = state.next_id.fetch_add(1, Ordering::Relaxed);
    let payload = OpenedFilePayload {
        file_id,
        path: source.path().to_path_buf(),
        size_bytes: source.file_size(),
        line_count: line_index.line_count() as u64,
        record_count: records.len() as u64,
        pattern_name: name.clone(),
        pattern_source: pattern_source.clone(),
        pattern_score: score,
    };
    let mut source = source;
    let bytes = source.read_all()?;
    let mut opened = OpenedFile {
        path: source.path().to_path_buf(),
        records,
        record_first_line: Vec::new(),
        line_count: 0,
        bytes,
        line_offsets: Vec::new(),
        pattern_source,
        pattern_name: name,
    };
    opened.rebuild_line_caches(
        line_index.line_count() as u64,
        line_index.line_offsets.clone(),
    );
    state
        .files
        .lock()
        .expect("files mutex poisoned")
        .insert(file_id, opened);
    Ok(payload)
}

#[tauri::command]
fn get_records(
    state: State<'_, AppState>,
    file_id: u64,
    start: u64,
    end: u64,
) -> Result<RecordsPayload, IpcError> {
    let guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;

    let total = file.records.len() as u64;
    if start >= total || end > total || start >= end {
        return Err(IpcError::OutOfRange);
    }

    let start_usz = usize::try_from(start).unwrap_or(usize::MAX);
    let end_usz = usize::try_from(end).unwrap_or(usize::MAX);
    let slice = &file.records[start_usz..end_usz];
    let base_offset = slice.first().map_or(0, |h| h.byte_offset);
    let last = slice.last().expect("non-empty by guard above");
    let stop = last.byte_offset + u64::from(last.byte_len);
    let len = usize::try_from(stop - base_offset).unwrap_or(usize::MAX);

    // Prefer the in-memory copy if it covers the range; fall back to a disk
    // read for hot reloads after the cache has been dropped.
    let bytes_slice: Vec<u8> = {
        let base = usize::try_from(base_offset).unwrap_or(usize::MAX);
        let stop_usz = base.saturating_add(len);
        if stop_usz <= file.bytes.len() {
            file.bytes[base..stop_usz].to_vec()
        } else {
            read_range(&file.path, base_offset, len).map_err(|source| IpcError::Io {
                message: source.to_string(),
                path: file.path.display().to_string(),
            })?
        }
    };

    Ok(RecordsPayload {
        start,
        base_offset,
        headers: slice.to_vec(),
        text: String::from_utf8_lossy(&bytes_slice).into_owned(),
    })
}

fn read_range(path: &Path, start: u64, len: usize) -> std::io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

#[derive(Debug, Serialize)]
struct LinePayload {
    record_idx: u64,
    line_within_record: u32,
    level: clog_core::Level,
    /// Populated only when `line_within_record == 0`. Spans are relative to
    /// the line's first byte, so the UI can slice directly out of `text`.
    fields: Option<clog_core::HeaderFields>,
    text: String,
}

#[derive(Debug, Serialize)]
struct LinesPayload {
    start_line: u64,
    lines: Vec<LinePayload>,
}

#[tauri::command]
fn get_lines(
    state: State<'_, AppState>,
    file_id: u64,
    start: u64,
    end: u64,
) -> Result<LinesPayload, IpcError> {
    let guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;

    let total = file.line_count;
    if start >= total || end > total || start >= end {
        return Err(IpcError::OutOfRange);
    }

    let mut lines: Vec<LinePayload> = Vec::with_capacity(usize::try_from(end - start).unwrap_or(0));
    // Use partition_point so the first record search is O(log n); subsequent
    // ones advance forward.
    let mut rec_idx = file
        .record_first_line
        .partition_point(|&fl| fl <= start)
        .saturating_sub(1);
    for line_idx in start..end {
        while rec_idx + 1 < file.records.len() && file.record_first_line[rec_idx + 1] <= line_idx {
            rec_idx += 1;
        }
        let rec = &file.records[rec_idx];
        let line_within_record = u32::try_from(line_idx - u64::from(rec.line_offset)).unwrap_or(0);
        let li = usize::try_from(line_idx).unwrap_or(usize::MAX);
        let line_start = file.line_offsets[li];
        let line_end = if li + 1 < file.line_offsets.len() {
            file.line_offsets[li + 1]
        } else {
            file.bytes.len() as u64
        };
        let s = usize::try_from(line_start).unwrap_or(usize::MAX);
        let mut e = usize::try_from(line_end).unwrap_or(usize::MAX);
        // Strip trailing newline so the UI doesn't render an empty visual row.
        if e > s && file.bytes[e - 1] == b'\n' {
            e -= 1;
        }
        if e > s && file.bytes[e - 1] == b'\r' {
            e -= 1;
        }
        let text = String::from_utf8_lossy(&file.bytes[s..e]).into_owned();
        let fields = if line_within_record == 0 {
            Some(rec.fields.clone())
        } else {
            None
        };
        lines.push(LinePayload {
            record_idx: rec_idx as u64,
            line_within_record,
            level: rec.level,
            fields,
            text,
        });
    }
    Ok(LinesPayload {
        start_line: start,
        lines,
    })
}

#[tauri::command]
fn close_file(state: State<'_, AppState>, file_id: u64) {
    state
        .files
        .lock()
        .expect("files mutex poisoned")
        .remove(&file_id);
}

/// Score `pattern` (`PatternLayout`) or `regex` against the file's first 64KB.
/// One of the two must be set.
#[tauri::command]
fn test_pattern(
    state: State<'_, AppState>,
    file_id: u64,
    pattern: Option<String>,
    regex: Option<String>,
) -> Result<PatternTestPayload, IpcError> {
    let guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;
    let sample = sample_lines(&file.path, 64 * 1024)?;
    let refs: Vec<&[u8]> = sample.iter().map(Vec::as_slice).collect();
    let sample_size = u32::try_from(refs.len()).unwrap_or(u32::MAX);
    let score = match (pattern, regex) {
        (Some(p), _) => CompiledPattern::compile(&p)?.match_score(refs),
        (_, Some(r)) => {
            let scanner = RegexScanner::compile(&r)?;
            score_with(&scanner, refs.into_iter())
        }
        _ => 0.0,
    };
    Ok(PatternTestPayload { score, sample_size })
}

fn score_with<'a, S: clog_core::RecordScanner>(
    scanner: &S,
    lines: impl Iterator<Item = &'a [u8]>,
) -> f32 {
    let mut total: u32 = 0;
    let mut hit: u32 = 0;
    for line in lines {
        total += 1;
        if scanner.try_parse_header(line).is_some() {
            hit += 1;
        }
    }
    if total == 0 {
        0.0
    } else {
        f32::from(u16::try_from(hit).unwrap_or(u16::MAX))
            / f32::from(u16::try_from(total).unwrap_or(u16::MAX))
    }
}

/// Apply a new pattern (`PatternLayout` or regex) to an already-open file. The
/// record array is rebuilt in place; the byte cache is reused. Returns the
/// new record count so the UI can resize its virtualiser.
#[tauri::command]
fn set_pattern(
    state: State<'_, AppState>,
    file_id: u64,
    pattern: Option<String>,
    regex: Option<String>,
) -> Result<ApplyPatternPayload, IpcError> {
    let mut guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get_mut(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;

    let line_index =
        clog_core::LineIndex::build(std::io::Cursor::new(&file.bytes)).map_err(|source| {
            IpcError::Io {
                message: source.to_string(),
                path: file.path.display().to_string(),
            }
        })?;
    let (records, source_string) = match (pattern, regex) {
        (Some(p), _) => {
            let scanner = CompiledPattern::compile(&p)?;
            let src = scanner.source.clone();
            (scan_records(&scanner, &line_index, &file.bytes), src)
        }
        (_, Some(r)) => {
            let scanner = RegexScanner::compile(&r)?;
            (
                scan_records(&scanner, &line_index, &file.bytes),
                format!("regex:{r}"),
            )
        }
        _ => {
            return Err(IpcError::BadPattern {
                message: "neither pattern nor regex supplied".into(),
            })
        }
    };
    let count = records.len() as u64;
    file.records = records;
    file.pattern_source.clone_from(&source_string);
    file.pattern_name = None;
    let line_count = line_index.line_count() as u64;
    file.rebuild_line_caches(line_count, line_index.line_offsets);
    Ok(ApplyPatternPayload {
        record_count: count,
        pattern_source: source_string,
    })
}

fn main() {
    tauri::Builder::default()
        .manage(AppState::default())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            open_file,
            get_records,
            get_lines,
            close_file,
            test_pattern,
            set_pattern
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
