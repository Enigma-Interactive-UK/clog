#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Tauri commands take `State` by value by convention; the lint fires on every
// command signature otherwise.
#![allow(clippy::needless_pass_by_value)]

mod channels;

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use clog_core::{
    auto_detect, index_file, sample_lines, scan_records, CompiledPattern, CoreError, HeaderFields,
    Level, LineSource, PatternError, RecordHeader, RecordScanner, RegexScanner, RegexScannerError,
    TailEvent, TailState, BUILTIN_PATTERNS, DEFAULT_POLL_INTERVAL_MS,
};
use serde::Serialize;
use tauri::async_runtime::JoinHandle;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tokio::sync::oneshot;

use crate::channels::TailEmitter;

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

/// Which kind of scanner is currently in force for a file. Stored alongside
/// the source string so the tail task can recompile the scanner on every
/// tick (cheap relative to disk I/O) without having to keep a non-Send
/// trait object around.
#[derive(Debug, Clone)]
enum ScannerKind {
    Pattern(String),
    Regex(String),
}

impl ScannerKind {
    fn compile(&self) -> Result<CompiledScanner, IpcError> {
        match self {
            Self::Pattern(s) => Ok(CompiledScanner::Pattern(CompiledPattern::compile(s)?)),
            Self::Regex(s) => Ok(CompiledScanner::Regex(RegexScanner::compile(s)?)),
        }
    }
}

/// Concrete sum type so we can hand a sized `RecordScanner` to
/// `index_file`/`scan_records` from a runtime-selected branch without
/// boxing a trait object.
enum CompiledScanner {
    Pattern(CompiledPattern),
    Regex(RegexScanner),
}

impl RecordScanner for CompiledScanner {
    fn try_parse_header(&self, line: &[u8]) -> Option<clog_core::ParsedHeader> {
        match self {
            Self::Pattern(p) => p.try_parse_header(line),
            Self::Regex(r) => r.try_parse_header(line),
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
    scanner_kind: ScannerKind,
    /// Shutdown signal for the running tail task, if any.
    tail_shutdown: Option<oneshot::Sender<()>>,
    /// `JoinHandle` for the running tail task, retained so we can drop it
    /// cleanly on close.
    tail_join: Option<JoinHandle<()>>,
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

    /// Tear down any running tail task. Safe to call regardless of whether
    /// one is active.
    fn stop_tail(&mut self) {
        if let Some(tx) = self.tail_shutdown.take() {
            let _ = tx.send(());
        }
        // The join handle is dropped without awaiting; the task observes
        // shutdown next tick and exits on its own. We don't block the IPC
        // thread to wait.
        self.tail_join = None;
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

/// Per-tick payload emitted on the `start_tail` Channel. The UI uses
/// `line_count` and `record_count` to resize its virtualiser, and `rotated`
/// to clear page caches before re-fetching.
#[derive(Debug, Clone, Serialize)]
pub struct TailDelta {
    /// Number of *new* records added since the last delta (0 if rotated).
    pub new_record_count: u64,
    /// File's current line count after applying this delta.
    pub line_count: u64,
    /// File's current record count after applying this delta.
    pub record_count: u64,
    /// Last byte offset of the in-memory buffer (i.e. file size for an
    /// append-only growth; 0 immediately after a rotation re-index).
    pub last_offset: u64,
    /// `true` iff a rotation was detected this tick. The UI should drop
    /// page caches and re-fetch from the top.
    pub rotated: bool,
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
        pattern_source: pattern_source.clone(),
        pattern_name: name,
        scanner_kind: ScannerKind::Pattern(pattern_source),
        tail_shutdown: None,
        tail_join: None,
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
    level: Level,
    /// Populated only when `line_within_record == 0`. Spans are relative to
    /// the line's first byte, so the UI can slice directly out of `text`.
    fields: Option<HeaderFields>,
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
    build_lines_payload(file, start, end)
}

/// Pure helper that builds the page payload from an `OpenedFile`. Split out
/// so tests can exercise the line/record/byte invariants without going
/// through Tauri state.
fn build_lines_payload(file: &OpenedFile, start: u64, end: u64) -> Result<LinesPayload, IpcError> {
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

#[derive(Debug, Serialize)]
struct LevelMinimapPayload {
    /// One level per bucket, top-of-file first. Length == requested
    /// `bucket_count` (clamped to >= 1). When the file is empty every
    /// bucket is `Level::Unknown`.
    buckets: Vec<Level>,
    /// The line span this minimap was computed over. UIs compare this to
    /// the current `line_count` to know whether a refetch is warranted.
    line_count: u64,
}

/// Rank for the "worst severity wins" bucket aggregation. Higher = more
/// important. Tie semantics: warn/error/fatal beat info/debug/trace even
/// when they're a tiny minority in the bucket (the user explicitly wants
/// warnings and errors to pop out of the minimap).
fn level_rank(l: Level) -> u8 {
    match l {
        Level::Fatal => 7,
        Level::Error => 6,
        Level::Warn => 5,
        Level::Info => 3,
        Level::Debug => 2,
        Level::Trace => 1,
        Level::All => 4,
        Level::Off => 0,
        Level::Unknown => 0,
    }
}

#[tauri::command]
fn get_level_minimap(
    state: State<'_, AppState>,
    file_id: u64,
    bucket_count: u32,
) -> Result<LevelMinimapPayload, IpcError> {
    let guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;
    let bucket_count = bucket_count.max(1) as usize;
    let mut buckets = vec![Level::Unknown; bucket_count];
    let line_count = file.line_count;
    if line_count == 0 || file.records.is_empty() {
        return Ok(LevelMinimapPayload {
            buckets,
            line_count,
        });
    }

    // Map each record's physical-line span onto the bucket grid. The bucket
    // for line `i` is `i * bucket_count / line_count`. We compute the
    // first/last bucket a record touches and bump every bucket in that
    // range to the record's level if it outranks what's there.
    let lc = line_count;
    let bc = bucket_count as u64;
    for rec in &file.records {
        let first_line = u64::from(rec.line_offset);
        let last_line = first_line + u64::from(rec.line_count.max(1)) - 1;
        let first_bucket = (first_line.saturating_mul(bc) / lc) as usize;
        let last_bucket = ((last_line.saturating_mul(bc) / lc) as usize).min(bucket_count - 1);
        for b in &mut buckets[first_bucket..=last_bucket] {
            if level_rank(rec.level) > level_rank(*b) {
                *b = rec.level;
            }
        }
    }

    Ok(LevelMinimapPayload {
        buckets,
        line_count,
    })
}

#[tauri::command]
fn close_file(state: State<'_, AppState>, file_id: u64) {
    let mut guard = state.files.lock().expect("files mutex poisoned");
    if let Some(mut f) = guard.remove(&file_id) {
        f.stop_tail();
    }
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

fn score_with<'a, S: RecordScanner>(scanner: &S, lines: impl Iterator<Item = &'a [u8]>) -> f32 {
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
    let (records, source_string, kind) = match (pattern, regex) {
        (Some(p), _) => {
            let scanner = CompiledPattern::compile(&p)?;
            let src = scanner.source.clone();
            (
                scan_records(&scanner, &line_index, &file.bytes),
                src.clone(),
                ScannerKind::Pattern(src),
            )
        }
        (_, Some(r)) => {
            let scanner = RegexScanner::compile(&r)?;
            (
                scan_records(&scanner, &line_index, &file.bytes),
                format!("regex:{r}"),
                ScannerKind::Regex(r),
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
    file.scanner_kind = kind;
    let line_count = line_index.line_count() as u64;
    file.rebuild_line_caches(line_count, line_index.line_offsets);
    Ok(ApplyPatternPayload {
        record_count: count,
        pattern_source: source_string,
    })
}

/// Start (or restart) tailing `file_id`. Subsequent `TailDelta` events are
/// emitted on the supplied channel until `stop_tail` is called or the file
/// is closed.
#[tauri::command]
fn start_tail(
    app: AppHandle,
    state: State<'_, AppState>,
    file_id: u64,
    on_delta: Channel<TailDelta>,
) -> Result<(), IpcError> {
    let (path, initial_size) = {
        let mut guard = state.files.lock().expect("files mutex poisoned");
        let file = guard
            .get_mut(&file_id)
            .ok_or(IpcError::UnknownFile { file_id })?;
        // Tear down a prior tail before starting a new one.
        file.stop_tail();
        (file.path.clone(), file.bytes.len() as u64)
    };

    let mut tail_state = TailState::new(&path, initial_size).map_err(|source| IpcError::Io {
        message: source.to_string(),
        path: path.display().to_string(),
    })?;

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let app_handle = app.clone();
    let join = tauri::async_runtime::spawn(async move {
        let mut emitter = TailEmitter::new(on_delta);
        let poll_interval = Duration::from_millis(DEFAULT_POLL_INTERVAL_MS);
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                () = tokio::time::sleep(poll_interval) => {}
            }
            match tail_state.poll() {
                Ok(TailEvent::Appended { from_offset, bytes }) => {
                    if let Some(delta) = apply_appended(&app_handle, file_id, from_offset, &bytes) {
                        let _ = emitter.emit(delta);
                    }
                }
                Ok(TailEvent::Rotated) => {
                    if let Ok((line_count, record_count, new_size)) =
                        apply_rotation(&app_handle, file_id)
                    {
                        // Re-anchor the tail state so subsequent polls
                        // measure growth from the new size.
                        let _ = tail_state.reset_to(new_size);
                        let _ = emitter.emit(TailDelta {
                            new_record_count: 0,
                            line_count,
                            record_count,
                            last_offset: new_size,
                            rotated: true,
                        });
                    }
                    // If the rotated file is briefly unreadable, we'll
                    // retry next tick.
                }
                // NoChange and transient I/O errors are equivalent here:
                // wait for the next tick and try again.
                Ok(TailEvent::NoChange) | Err(_) => {}
            }
            emitter.flush();
        }
    });

    let mut guard = state.files.lock().expect("files mutex poisoned");
    if let Some(file) = guard.get_mut(&file_id) {
        file.tail_shutdown = Some(shutdown_tx);
        file.tail_join = Some(join);
    }
    Ok(())
}

#[tauri::command]
fn stop_tail(state: State<'_, AppState>, file_id: u64) -> Result<(), IpcError> {
    let mut guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get_mut(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;
    file.stop_tail();
    Ok(())
}

/// Apply appended bytes to the file's in-memory state. Returns the delta to
/// emit, or `None` if the file went away between polls (e.g. concurrent
/// `close_file`).
fn apply_appended(
    app: &AppHandle,
    file_id: u64,
    from_offset: u64,
    appended: &[u8],
) -> Option<TailDelta> {
    let state = app.state::<AppState>();
    let mut guard = state.files.lock().expect("files mutex poisoned");
    let file = guard.get_mut(&file_id)?;

    // Recompile the scanner each tick. Cheap (pattern strings are short)
    // and side-steps the Send/Sync awkwardness of caching a trait object.
    let Ok(scanner) = file.scanner_kind.compile() else {
        return None;
    };

    let prev_records = file.records.len();
    extend_with_appended(file, &scanner, from_offset, appended);
    let new_record_count = (file.records.len() - prev_records) as u64;
    Some(TailDelta {
        new_record_count,
        line_count: file.line_count,
        record_count: file.records.len() as u64,
        last_offset: file.bytes.len() as u64,
        rotated: false,
    })
}

/// Mutate `file` to reflect that `appended` bytes have arrived starting at
/// `from_offset` (which must equal the current `file.bytes.len()`). Updates
/// `bytes`, `line_offsets`, `records`, `record_first_line`, `line_count`
/// such that the invariants `get_lines`/`get_records` depend on still hold.
fn extend_with_appended<S: RecordScanner>(
    file: &mut OpenedFile,
    scanner: &S,
    from_offset: u64,
    appended: &[u8],
) {
    debug_assert_eq!(from_offset, file.bytes.len() as u64);
    file.bytes.extend_from_slice(appended);
    let new_total = file.bytes.len() as u64;
    let first_new_record_idx = file.records.len();

    // Walk the appended payload, splitting on '\n'. The tail layer only
    // ships complete lines, so the buffer ends in '\n'.
    let mut local = 0usize;
    while local < appended.len() {
        let nl_rel = appended[local..]
            .iter()
            .position(|&b| b == b'\n')
            .unwrap_or(appended.len() - local - 1);
        let nl_abs = local + nl_rel; // position of '\n' (or last byte if no '\n')
        let abs_line_start = from_offset + local as u64;
        // Line content without trailing \r\n.
        let mut clean_end = nl_abs;
        if clean_end > local && appended[clean_end - 1] == b'\r' {
            clean_end -= 1;
        }
        let line_slice = &appended[local..clean_end];
        let line_idx = file.line_offsets.len();
        file.line_offsets.push(abs_line_start);

        if let Some(parsed) = scanner.try_parse_header(line_slice) {
            file.records.push(RecordHeader {
                byte_offset: abs_line_start,
                byte_len: 0,
                line_offset: u32::try_from(line_idx).unwrap_or(u32::MAX),
                line_count: 1,
                level: parsed.level,
                fields: parsed.fields,
            });
            file.record_first_line.push(line_idx as u64);
        } else if let Some(last) = file.records.last_mut() {
            last.line_count = last.line_count.saturating_add(1);
        } else {
            // Orphan continuation before any header.
            file.records.push(RecordHeader {
                byte_offset: abs_line_start,
                byte_len: 0,
                line_offset: u32::try_from(line_idx).unwrap_or(u32::MAX),
                line_count: 1,
                level: Level::Unknown,
                fields: HeaderFields::default(),
            });
            file.record_first_line.push(line_idx as u64);
        }

        local = nl_abs + 1;
    }

    // Fix up byte_len for any record whose end falls inside the appended
    // range. That's the last record before the append (if it gained
    // continuation lines) and every new record we just pushed.
    let recount_start = first_new_record_idx.saturating_sub(1);
    let n = file.records.len();
    for i in recount_start..n {
        let end = if i + 1 < n {
            file.records[i + 1].byte_offset
        } else {
            new_total
        };
        file.records[i].byte_len =
            u32::try_from(end - file.records[i].byte_offset).unwrap_or(u32::MAX);
    }

    file.line_count = file.line_offsets.len() as u64;
}

/// Re-index a rotated file from disk and swap state under the lock. Returns
/// `(line_count, record_count, new_size)`.
fn apply_rotation(app: &AppHandle, file_id: u64) -> Result<(u64, u64, u64), IpcError> {
    let state = app.state::<AppState>();
    let (path, scanner_kind) = {
        let guard = state.files.lock().expect("files mutex poisoned");
        let file = guard
            .get(&file_id)
            .ok_or(IpcError::UnknownFile { file_id })?;
        (file.path.clone(), file.scanner_kind.clone())
    };

    let scanner = scanner_kind.compile()?;
    // Re-read everything. The file may transiently be very small or even
    // empty between rotation steps; that's fine, we'll re-index again on
    // the next growth event.
    let (mut source, line_index, records) = index_file(&path, &scanner)?;
    let bytes = source.read_all()?;
    let new_size = source.file_size();
    let line_count = line_index.line_count() as u64;
    let record_count = records.len() as u64;

    let mut guard = state.files.lock().expect("files mutex poisoned");
    if let Some(file) = guard.get_mut(&file_id) {
        file.records = records;
        file.bytes = bytes;
        file.rebuild_line_caches(line_count, line_index.line_offsets);
    }
    Ok((line_count, record_count, new_size))
}

fn main() {
    tauri::Builder::default()
        .manage(AppState::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            open_file,
            get_records,
            get_lines,
            close_file,
            test_pattern,
            set_pattern,
            start_tail,
            stop_tail,
            get_level_minimap
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use clog_core::builtin_pattern;

    /// Build a fresh `OpenedFile` with no content, wired up with the
    /// wsl-oink pattern so a stream of header lines can be appended via
    /// `extend_with_appended`.
    fn fresh_file() -> (OpenedFile, CompiledScanner) {
        let pattern_src = builtin_pattern("wsl-oink").expect("wsl-oink builtin");
        let scanner_kind = ScannerKind::Pattern(pattern_src.to_string());
        let scanner = scanner_kind.compile().expect("compile wsl-oink");
        let file = OpenedFile {
            path: PathBuf::from("test.log"),
            records: Vec::new(),
            record_first_line: Vec::new(),
            line_count: 0,
            bytes: Vec::new(),
            line_offsets: Vec::new(),
            pattern_source: pattern_src.to_string(),
            pattern_name: Some("wsl-oink".to_string()),
            scanner_kind,
            tail_shutdown: None,
            tail_join: None,
        };
        (file, scanner)
    }

    fn extend(file: &mut OpenedFile, scanner: &CompiledScanner, payload: &[u8]) {
        let from = file.bytes.len() as u64;
        match scanner {
            CompiledScanner::Pattern(p) => extend_with_appended(file, p, from, payload),
            CompiledScanner::Regex(r) => extend_with_appended(file, r, from, payload),
        }
    }

    /// Reproduces the symptom the user reported: each tail tick adds one
    /// new line and `get_lines(0, line_count)` must always report content
    /// matching exactly what's been appended -- no empty rows, no stale
    /// "previous tick's content lagging" effect.
    #[test]
    fn appending_lines_one_at_a_time_keeps_get_lines_consistent() {
        let (mut file, scanner) = fresh_file();
        let body = [
            "[INFO ] 2026-05-22 16:28:59.246 [main] play - Starting /var/play/sites/solopress\n",
            "[INFO ] 2026-05-22 16:28:59.390 [main] play - Module crud is available\n",
            "[INFO ] 2026-05-22 16:28:59.391 [main] play - Module secure is available\n",
            "[INFO ] 2026-05-22 16:28:59.392 [main] play - Module crud is available\n",
        ];
        let expected_text: Vec<String> = body.iter().map(|l| l.trim_end().to_string()).collect();

        for (tick, payload) in body.iter().enumerate() {
            extend(&mut file, &scanner, payload.as_bytes());
            assert_eq!(
                file.line_count,
                (tick + 1) as u64,
                "line_count after tick {tick}"
            );
            assert_eq!(file.records.len(), tick + 1, "records after tick {tick}");
            assert_eq!(file.record_first_line.len(), tick + 1);
            // Every line that has ever been appended must read back exactly,
            // not just the freshly-appended one. This is the property the
            // UI race violated -- intermediate ticks left stale page data.
            let lines =
                build_lines_payload(&file, 0, file.line_count).expect("build_lines_payload");
            assert_eq!(lines.lines.len(), tick + 1);
            for (i, got) in lines.lines.iter().enumerate() {
                assert_eq!(got.text, expected_text[i], "tick {tick}, line {i}");
                assert_eq!(got.line_within_record, 0, "tick {tick}, line {i}");
                assert!(
                    got.fields.is_some(),
                    "tick {tick}, line {i} should have axis-1 fields"
                );
            }
        }
    }

    /// Appending two lines in one tail batch (the realistic case when the
    /// writer flushes several records between polls) must still produce
    /// one record per header line, in order, with the byte offsets
    /// chained without gaps.
    #[test]
    fn appending_two_lines_in_one_batch_lands_them_both() {
        let (mut file, scanner) = fresh_file();
        let first = "[INFO ] 2026-05-22 16:28:59.246 [main] play - one\n";
        let second = "[INFO ] 2026-05-22 16:28:59.247 [main] play - two\n";
        let combined = format!("{first}{second}");
        extend(&mut file, &scanner, combined.as_bytes());

        assert_eq!(file.line_count, 2);
        assert_eq!(file.records.len(), 2);
        // The two records must abut: end of record 0 == start of record 1.
        let r0 = &file.records[0];
        let r1 = &file.records[1];
        assert_eq!(r0.byte_offset + u64::from(r0.byte_len), r1.byte_offset);

        let lines = build_lines_payload(&file, 0, 2).expect("build_lines_payload");
        assert!(lines.lines[0].text.ends_with("- one"));
        assert!(lines.lines[1].text.ends_with("- two"));
    }

    /// Stack-trace continuation lines (no header pattern) must extend the
    /// preceding record's `line_count` rather than create new records. The
    /// physical-line count still grows -- one virtual row per line.
    #[test]
    fn continuation_lines_extend_the_preceding_record() {
        let (mut file, scanner) = fresh_file();
        let header = "[ERROR] 2026-05-22 16:28:59.246 [main] play - boom\n";
        extend(&mut file, &scanner, header.as_bytes());
        assert_eq!(file.records.len(), 1);
        assert_eq!(file.records[0].line_count, 1);

        // Two stack-trace lines arriving in the next tail tick.
        let stack =
            "\tat com.example.Foo.bar(Foo.java:42)\n\tat com.example.Foo.baz(Foo.java:17)\n";
        extend(&mut file, &scanner, stack.as_bytes());

        assert_eq!(file.line_count, 3);
        assert_eq!(
            file.records.len(),
            1,
            "stack lines must not create new records"
        );
        assert_eq!(file.records[0].line_count, 3);

        let lines = build_lines_payload(&file, 0, 3).expect("build_lines_payload");
        assert_eq!(lines.lines[0].line_within_record, 0);
        assert!(lines.lines[0].fields.is_some());
        assert_eq!(lines.lines[1].line_within_record, 1);
        assert!(lines.lines[1].fields.is_none());
        assert!(lines.lines[1].text.contains("Foo.java:42"));
        assert_eq!(lines.lines[2].line_within_record, 2);
        assert!(lines.lines[2].text.contains("Foo.java:17"));
    }

    /// Simulates rotation: the file is truncated and a different shape of
    /// content is re-read from disk. After `rebuild_line_caches` (which
    /// `apply_rotation` calls), the `OpenedFile` must be fully consistent
    /// AND a subsequent tail-style append must keep working without
    /// leaking the pre-rotation state.
    #[test]
    fn rotation_then_append_starts_clean() {
        let (mut file, scanner) = fresh_file();
        // Seed pre-rotation content.
        extend(
            &mut file,
            &scanner,
            b"[INFO ] 2026-05-22 16:28:59.246 [main] play - old\n",
        );
        assert_eq!(file.line_count, 1);

        // Rotation: byte content shrinks back to empty, then a fresh first
        // line is written. `apply_rotation` would re-read this from disk
        // via index_file; here we mimic it by hand-resetting the in-memory
        // state to the post-rotation file shape.
        file.bytes.clear();
        file.records.clear();
        file.line_offsets.clear();
        let post = b"[WARN ] 2026-05-22 16:29:01.000 [main] play - rotated\n";
        // Build line_offsets the same way `index_file` would for the new
        // content: one entry at offset 0.
        let new_line_offsets = vec![0u64];
        // Manually push the post-rotation record.
        file.bytes.extend_from_slice(post);
        let parsed = match &scanner {
            CompiledScanner::Pattern(p) => p
                .try_parse_header(post.strip_suffix(b"\n").unwrap_or(post))
                .expect("parse post-rotation header"),
            CompiledScanner::Regex(_) => unreachable!(),
        };
        file.records.push(RecordHeader {
            byte_offset: 0,
            byte_len: u32::try_from(post.len()).expect("post fits in u32"),
            line_offset: 0,
            line_count: 1,
            level: parsed.level,
            fields: parsed.fields,
        });
        file.rebuild_line_caches(1, new_line_offsets);

        assert_eq!(file.line_count, 1);
        let lines = build_lines_payload(&file, 0, 1).expect("build_lines_payload after rotation");
        assert!(lines.lines[0].text.ends_with("- rotated"));
        assert_eq!(lines.lines[0].level, Level::Warn);

        // Now a fresh tail tick on the rotated file. Must append cleanly
        // alongside the post-rotation record without re-introducing any
        // pre-rotation content.
        extend(
            &mut file,
            &scanner,
            b"[INFO ] 2026-05-22 16:29:02.000 [main] play - next\n",
        );
        assert_eq!(file.line_count, 2);
        let lines =
            build_lines_payload(&file, 0, 2).expect("build_lines_payload after rotated append");
        assert!(lines.lines[0].text.ends_with("- rotated"));
        assert!(lines.lines[1].text.ends_with("- next"));
        assert!(!lines.lines[0].text.contains("- old"));
        assert!(!lines.lines[1].text.contains("- old"));
    }
}
