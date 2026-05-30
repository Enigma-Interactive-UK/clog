#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Tauri commands take `State` by value by convention; the lint fires on every
// command signature otherwise.
#![allow(clippy::needless_pass_by_value)]

mod channels;
mod paths;
mod persistence;
mod update;

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use clog_core::{
    auto_detect, classify_thread, index_file, sample_lines, scan_records, search_records,
    CacheFingerprint, CompiledPattern, CoreError, HeaderFields, HitRef, Level, LevelMask,
    LineSource, LoadOutcome, LooseScanner, PatternError, RecordHeader, RecordScanner, RegexScanner,
    RegexScannerError, SearchError, SearchMode, SearchOptions, StreamedFile, TailEvent, TailState,
    ThreadGroup, ThreadGroupMask, BUILTIN_PATTERNS, DEFAULT_POLL_INTERVAL_MS,
};
use persistence::{
    HighlightRulesFile, PatternOverride, PatternsFile, PerFileRulesFile, Session, Settings,
};
use serde::Serialize;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::async_runtime::JoinHandle;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::oneshot;

use crate::channels::{SearchEmitter, TailEmitter};

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
    #[error("empty search query")]
    EmptyQuery,
}

impl From<SearchError> for IpcError {
    fn from(err: SearchError) -> Self {
        match err {
            SearchError::EmptyQuery => Self::EmptyQuery,
            SearchError::BadRegex(m) => Self::BadRegex { message: m },
        }
    }
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

#[derive(Debug, Clone)]
struct SlowRequestCache {
    /// Snapshot signature: `(records.len(), bytes.len(), pattern_hash)`.
    signature: (u64, u64, u64),
    occurrences: Vec<clog_core::SlowRequestOccurrence>,
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
    /// When true, every physical line is treated as its own record (an
    /// `Unknown`-level orphan rather than a continuation of the previous
    /// record) so a custom / unconfirmed pattern does not visually merge
    /// unrelated lines together. Mirrors `pattern_name.is_none()`.
    loose: bool,
    /// Shutdown signal for the running tail task, if any.
    tail_shutdown: Option<oneshot::Sender<()>>,
    /// `JoinHandle` for the running tail task, retained so we can drop it
    /// cleanly on close.
    tail_join: Option<JoinHandle<()>>,
    /// Monotonic id assigned to the current in-flight search, if any. Each
    /// `start_search` call bumps this so stale `SearchDelta` messages can
    /// be discriminated by the UI.
    current_search_id: u64,
    /// Cancellation flag for the current search task. Set to true by
    /// `cancel_search` or by the next `start_search` call. The search
    /// task polls this between chunks and aborts early when it flips.
    search_cancel: Option<Arc<AtomicBool>>,
    /// `JoinHandle` for the running search task. Dropped on close.
    search_join: Option<JoinHandle<()>>,
    /// Cached slow-request occurrences. Rebuilt lazily on first call
    /// after any change to (records, bytes, pattern). Both
    /// `get_slow_requests` and `get_slow_request_speeds` read this.
    slow_request_cache: Option<SlowRequestCache>,
    /// Inclusive lower bound (first visible physical line) of the truncate
    /// window. `None` = no "above" cut. Snapped to a record's first line.
    truncate_before: Option<u64>,
    /// Exclusive upper bound (one past the last visible physical line) of the
    /// truncate window. `None` = no "below" cut. Snapped to a record boundary.
    truncate_after: Option<u64>,
}

impl OpenedFile {
    /// Resolve the visible physical-line window, defaulting unset bounds to the
    /// full file. Returns `(lo_inclusive, hi_exclusive)`.
    fn truncate_window(&self) -> (u64, u64) {
        let lo = self.truncate_before.unwrap_or(0);
        let hi = self.truncate_after.unwrap_or(self.line_count);
        (lo, hi)
    }

    /// Number of physical lines visible inside the window. Used only for the
    /// windowed line/record counts returned to the UI.
    fn windowed_line_count(&self) -> u64 {
        let (lo, hi) = self.truncate_window();
        hi.saturating_sub(lo).min(self.line_count)
    }

    /// Set (or clear) the truncate window. `before` is the first visible
    /// physical line; `after` is one past the last visible physical line. Both
    /// are expected pre-snapped to record boundaries by the caller. Rejects an
    /// empty or inverted window. `(None, None)` clears the window.
    fn apply_truncate(
        &mut self,
        before: Option<u64>,
        after: Option<u64>,
    ) -> Result<TruncatePayload, IpcError> {
        if let (Some(b), Some(a)) = (before, after) {
            if b >= a {
                return Err(IpcError::BadPattern {
                    message: format!("invalid truncate window: before={b} >= after={a}"),
                });
            }
        }
        self.truncate_before = before;
        self.truncate_after = after;
        let (lo, hi) = self.truncate_window();
        let record_count = self
            .records
            .iter()
            .filter(|r| {
                let f = u64::from(r.line_offset);
                f >= lo && f < hi
            })
            .count() as u64;
        Ok(TruncatePayload {
            before,
            after,
            line_count: self.windowed_line_count(),
            record_count,
        })
    }

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

    /// Cancel the current search task, if any. The flag flips; the
    /// running task will notice on its next chunk boundary and exit. We
    /// don't await the join handle here -- the next `start_search` will
    /// just allocate a fresh flag.
    fn cancel_search(&mut self) {
        if let Some(flag) = &self.search_cancel {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.search_cancel = None;
        self.search_join = None;
    }
}

#[derive(Default)]
struct AppState {
    files: Mutex<HashMap<u64, OpenedFile>>,
    next_id: AtomicU64,
    /// Paths the binary was launched with. Drained by the UI on boot via
    /// `take_startup_paths` so a `clog.exe path1 path2` invocation opens
    /// each as a tab. Empty when launched with no args.
    startup_paths: Mutex<Vec<String>>,
}

/// Filter a list of argv strings down to plausible file paths -- anything
/// that exists on disk and isn't the executable itself. Used at startup
/// and on the single-instance forward.
fn filter_paths(argv: &[String]) -> Vec<String> {
    argv.iter()
        .skip(1) // executable
        .filter(|a| !a.starts_with('-'))
        .filter(|a| std::path::Path::new(a).is_file())
        .cloned()
        .collect()
}

#[tauri::command]
fn take_startup_paths(state: State<'_, AppState>) -> Vec<String> {
    let mut guard = state.startup_paths.lock().expect("startup_paths mutex");
    std::mem::take(&mut *guard)
}

#[derive(Debug, Serialize)]
struct OpenedFilePayload {
    file_id: u64,
    path: PathBuf,
    size_bytes: u64,
    line_count: u64,
    record_count: u64,
    /// Name of the auto-detected builtin pattern (`"wsl-dev"`, `"prod"`,
    /// `"log4j2-default"`) or `None` if none matched and we fell back to
    /// best effort.
    pattern_name: Option<String>,
    pattern_source: String,
    /// Match-score (0.0..=1.0) of the chosen pattern against a 64KB sample.
    pattern_score: f32,
    /// True iff the records/line-offsets came from the persistent index
    /// cache rather than a fresh `index_file` walk. Surfaced so the UI can
    /// show a "cached" hint or telemetry can track hit rate.
    cache_hit: bool,
    /// True iff the active pattern is "custom" (no auto-detected builtin
    /// and / or a user override) -- the UI uses this to suppress the
    /// continuation-line styling that would otherwise visually merge
    /// physical lines into the preceding record.
    loose: bool,
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
    loose: bool,
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
#[allow(clippy::too_many_lines)]
fn open_file(state: State<'_, AppState>, path: String) -> Result<OpenedFilePayload, IpcError> {
    let path_buf = PathBuf::from(&path);

    // 1. Resolve which scanner to use: per-file override beats auto-detect.
    let patterns = PatternsFile::load();
    let path_key = path_buf.to_string_lossy().to_string();
    let mut override_score: Option<f32> = None;
    let (name, scanner, score, scanner_kind) = if let Some(ov) =
        patterns.overrides.get(&path_key).cloned()
    {
        tracing::debug!(target: "clog::open", path = %path_key, kind = %ov.kind, "using per-file pattern override");
        if ov.kind.as_str() == "regex" {
            // Validate the regex compiles cleanly so a broken override
            // surfaces as a clear error here rather than a confusing
            // "no records parsed" downstream.
            let _ = RegexScanner::compile(&ov.source)?;
            override_score = Some(1.0);
            (
                None,
                CompiledPattern::compile(BUILTIN_PATTERNS[0].1).expect("builtin compiles"),
                1.0,
                ScannerKind::Regex(ov.source.clone()),
            )
        } else {
            let p = CompiledPattern::compile(&ov.source)?;
            (None, p, 1.0, ScannerKind::Pattern(ov.source.clone()))
        }
    } else {
        let sample = sample_lines(&path_buf, 64 * 1024)?;
        let sample_refs: Vec<&[u8]> = sample.iter().map(Vec::as_slice).collect();
        if let Some(hit) = auto_detect(sample_refs.iter().copied()) {
            let kind = ScannerKind::Pattern(hit.1.source.clone());
            (Some(hit.0.to_string()), hit.1, hit.2, kind)
        } else {
            let scanner = CompiledPattern::compile(BUILTIN_PATTERNS[0].1).expect("builtin valid");
            let kind = ScannerKind::Pattern(scanner.source.clone());
            (None, scanner, 0.0, kind)
        }
    };
    let _ = override_score;

    // 2. Resolve which compiled scanner to actually index with (matches the
    //    kind selected above). When no builtin pattern matched (`name` is
    //    `None`) the file is "custom" -- we don't trust the scanner to know
    //    what is and isn't a continuation, so wrap it in `LooseScanner` so
    //    every line becomes its own record.
    let compiled = scanner_kind.compile()?;
    let loose = name.is_none();

    // 3. Try the persistent index cache first. A hit skips the full
    //    `index_file` walk; a miss falls back to a fresh index + cache write.
    //    Loose vs strict produces different record shapes, so the fingerprint
    //    string distinguishes them.
    let pattern_source_for_fp = {
        let raw = match &scanner_kind {
            ScannerKind::Pattern(s) => s.clone(),
            ScannerKind::Regex(s) => format!("regex:{s}"),
        };
        if loose {
            format!("loose:{raw}")
        } else {
            raw
        }
    };
    let cache_path = paths::index_cache_path(&path_buf);
    let cache_load_t = std::time::Instant::now();
    let (line_index, records, cache_hit) = if let Ok(fp) =
        CacheFingerprint::for_path(&path_buf, &pattern_source_for_fp)
    {
        match clog_core::load_index_cache(&cache_path, &fp) {
            LoadOutcome::Hit {
                line_index,
                records,
            } => {
                tracing::info!(
                    target: "clog::cache",
                    path = %path_key,
                    elapsed_ms = u64::try_from(cache_load_t.elapsed().as_millis())
                        .unwrap_or(u64::MAX),
                    records = records.len(),
                    "index cache hit"
                );
                (line_index, records, true)
            }
            LoadOutcome::Miss => {
                let (_src, li, recs) = if loose {
                    index_file(&path_buf, &LooseScanner::new(&compiled))?
                } else {
                    index_file(&path_buf, &compiled)?
                };
                if let Err(e) = clog_core::save_index_cache(&cache_path, &fp, &li, &recs) {
                    tracing::warn!(target: "clog::cache", error = %e, "index cache write failed");
                }
                (li, recs, false)
            }
        }
    } else {
        let (_src, li, recs) = if loose {
            index_file(&path_buf, &LooseScanner::new(&compiled))?
        } else {
            index_file(&path_buf, &compiled)?
        };
        (li, recs, false)
    };

    let pattern_source = match &scanner_kind {
        ScannerKind::Pattern(s) => s.clone(),
        ScannerKind::Regex(s) => format!("regex:{s}"),
    };

    let mut source = StreamedFile::open(&path_buf)?;
    let size_bytes = source.file_size();
    let bytes = source.read_all()?;

    let file_id = state.next_id.fetch_add(1, Ordering::Relaxed);
    let payload = OpenedFilePayload {
        file_id,
        path: source.path().to_path_buf(),
        size_bytes,
        line_count: line_index.line_count() as u64,
        record_count: records.len() as u64,
        pattern_name: name.clone(),
        pattern_source: pattern_source.clone(),
        pattern_score: score,
        cache_hit,
        loose,
    };
    let _ = scanner; // suppress unused warning when override path discarded it
    let mut opened = OpenedFile {
        path: source.path().to_path_buf(),
        records,
        record_first_line: Vec::new(),
        line_count: 0,
        bytes,
        line_offsets: Vec::new(),
        pattern_source: pattern_source.clone(),
        pattern_name: name,
        scanner_kind,
        loose,
        tail_shutdown: None,
        tail_join: None,
        current_search_id: 0,
        search_cancel: None,
        search_join: None,
        slow_request_cache: None,
        truncate_before: None,
        truncate_after: None,
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

    // 4. Touch the recent-files list (best-effort).
    let abs_path_str = payload.path.to_string_lossy().to_string();
    let mut settings = Settings::load();
    settings.touch_recent(&abs_path_str);
    if let Err(e) = settings.save() {
        tracing::warn!(target: "clog::settings", error = %e, "settings write failed");
    }

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
    /// Byte offset of this physical line's first byte relative to the
    /// owning record's `byte_offset`. The UI uses this to map search-hit
    /// ranges (which are record-relative) onto line-relative char
    /// offsets. ASCII is assumed; multi-byte characters in a log line
    /// will skew the overlay -- acceptable for P6.
    byte_offset_in_record: u32,
    level: Level,
    /// Populated only when `line_within_record == 0`. Spans are relative to
    /// the line's first byte, so the UI can slice directly out of `text`.
    fields: Option<HeaderFields>,
    /// Full byte length of this physical line. When it exceeds `text.len()`
    /// the line was truncated to `LINE_TEXT_CAP` for transport (a multi-MB
    /// single-record JSON payload would otherwise stall the IPC bridge and
    /// the renderer). The UI shows a "show full record" affordance and can
    /// fetch a slice around any point via `get_line_window`; the full text
    /// still flows through `get_record_lines`. ASCII is assumed, matching
    /// the existing offset handling, so bytes == chars in practice.
    full_len: u64,
    /// True when `text` was cut to fit the transport cap. Byte-exact, so the
    /// UI must not re-derive truncation from `full_len` vs the char length.
    truncated: bool,
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

#[tauri::command]
fn get_record_lines(
    state: State<'_, AppState>,
    file_id: u64,
    record_idx: u64,
) -> Result<LinesPayload, IpcError> {
    let guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;
    let total = file.records.len() as u64;
    if record_idx >= total {
        return Err(IpcError::OutOfRange);
    }
    let rec_usz = usize::try_from(record_idx).unwrap_or(usize::MAX);
    let rec = &file.records[rec_usz];
    let start = u64::from(rec.line_offset);
    let end = start + u64::from(rec.line_count);
    // Uncapped: the full-record modal must show the complete text, however
    // large. The page path (`get_lines`) caps for scroll performance.
    build_lines_payload_capped(file, start, end, None)
}

/// Maximum bytes of a single physical line's text shipped to the UI per page.
/// Lines longer than this are truncated for transport; the UI renders a
/// truncation affordance and can fetch a slice around any offset via
/// `get_line_window`, while the full text remains available through
/// `get_record_lines`. Sized well above a normal log line but far below the
/// multi-MB single-record JSON payloads (see research/biscuit.out) that
/// otherwise stall the IPC bridge, the highlight engine, and DOM layout.
const LINE_TEXT_CAP: usize = 4096;

/// Truncate `text` to at most `cap` bytes on a UTF-8 char boundary, leaving
/// shorter lines untouched.
fn cap_line_text(mut text: String, cap: usize) -> String {
    if text.len() <= cap {
        return text;
    }
    let mut cut = cap;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
    text
}

/// Build the page payload from an `OpenedFile`, capping every line's text to
/// `LINE_TEXT_CAP`. This is the paginated scroll path (`get_lines`); the full
/// record text is served uncapped by `get_record_lines`. Existing callers and
/// tests use this 3-arg form unchanged.
fn build_lines_payload(file: &OpenedFile, start: u64, end: u64) -> Result<LinesPayload, IpcError> {
    build_lines_payload_capped(file, start, end, Some(LINE_TEXT_CAP))
}

/// Pure helper that builds the page payload from an `OpenedFile`. `cap`
/// truncates each line's transported text (`None` = full text). Split out so
/// tests can exercise the line/record/byte invariants without going through
/// Tauri state.
fn build_lines_payload_capped(
    file: &OpenedFile,
    start: u64,
    end: u64,
    cap: Option<usize>,
) -> Result<LinesPayload, IpcError> {
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
        let full_len = text.len() as u64;
        let text = match cap {
            Some(c) => cap_line_text(text, c),
            None => text,
        };
        // Byte-based so it is correct for multi-byte UTF-8, unlike a UI-side
        // `full_len > text.length` (chars) comparison.
        let truncated = (text.len() as u64) < full_len;
        let fields = if line_within_record == 0 {
            Some(rec.fields.clone())
        } else {
            None
        };
        let byte_offset_in_record = u32::try_from(line_start - rec.byte_offset).unwrap_or(u32::MAX);
        lines.push(LinePayload {
            record_idx: rec_idx as u64,
            line_within_record,
            byte_offset_in_record,
            level: rec.level,
            fields,
            full_len,
            truncated,
            text,
        });
    }
    Ok(LinesPayload {
        start_line: start,
        lines,
    })
}

#[derive(Debug, Serialize)]
struct LineWindowPayload {
    /// Byte offset within the physical line where `text` begins.
    start: u64,
    /// Full byte length of the physical line (newline stripped).
    full_len: u64,
    text: String,
}

/// Return a bounded slice of a single physical line centred on `center`
/// (a byte offset within the line), extending `radius` bytes each way. Lets
/// the UI peek at a search match buried past `LINE_TEXT_CAP` in a monster
/// line without dragging the whole multi-MB line across the IPC bridge.
#[tauri::command]
fn get_line_window(
    state: State<'_, AppState>,
    file_id: u64,
    line_index: u64,
    center: u64,
    radius: u64,
) -> Result<LineWindowPayload, IpcError> {
    let guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;
    build_line_window(file, line_index, center, radius)
}

/// Pure helper behind `get_line_window`, split out for testing.
fn build_line_window(
    file: &OpenedFile,
    line_index: u64,
    center: u64,
    radius: u64,
) -> Result<LineWindowPayload, IpcError> {
    if line_index >= file.line_count {
        return Err(IpcError::OutOfRange);
    }
    let li = usize::try_from(line_index).unwrap_or(usize::MAX);
    let line_start = file.line_offsets[li];
    let line_end = if li + 1 < file.line_offsets.len() {
        file.line_offsets[li + 1]
    } else {
        file.bytes.len() as u64
    };
    let s = usize::try_from(line_start).unwrap_or(usize::MAX);
    let mut e = usize::try_from(line_end).unwrap_or(usize::MAX);
    // Strip trailing newline so offsets line up with `build_lines_payload`.
    if e > s && file.bytes[e - 1] == b'\n' {
        e -= 1;
    }
    if e > s && file.bytes[e - 1] == b'\r' {
        e -= 1;
    }
    let line_len = e - s;
    let center = usize::try_from(center).unwrap_or(0).min(line_len);
    let radius = usize::try_from(radius).unwrap_or(0);
    let win_start = center.saturating_sub(radius);
    let win_end = center.saturating_add(radius).min(line_len);
    // `from_utf8_lossy` over a byte slice never panics on a split char, so no
    // boundary alignment is needed; under the ASCII assumption the offsets are
    // exact anyway.
    let text = String::from_utf8_lossy(&file.bytes[s + win_start..s + win_end]).into_owned();
    Ok(LineWindowPayload {
        start: win_start as u64,
        full_len: line_len as u64,
        text,
    })
}

#[derive(Debug, Clone, Copy, Serialize)]
struct BucketStat {
    /// Worst severity touching this bucket. Same semantics as the
    /// previous scalar minimap payload. Drives hue UI-side.
    worst: Level,
    /// Record count in this bucket at level ERROR or FATAL. Counted per
    /// record, not per physical line -- a multi-line ERROR contributes
    /// 1 to every bucket it touches.
    error: u32,
    /// Record count in this bucket at level WARN.
    warn: u32,
    /// Total record count in this bucket. Reserved for a future
    /// density wash; emitted now so the UI doesn't need another IPC
    /// round trip.
    total: u32,
}

#[derive(Debug, Serialize)]
struct LevelMinimapPayload {
    /// One stat per bucket, top-of-file first. Length == requested
    /// `bucket_count` (clamped to >= 1). When the file is empty every
    /// bucket reads as `Level::Unknown` with zeroed counts.
    buckets: Vec<BucketStat>,
    /// The line span this minimap was computed over. UIs compare this to
    /// the current `line_count` to know whether a refetch is warranted.
    line_count: u64,
    /// Maximum value of `(error + warn)` across all buckets. The UI uses
    /// this to normalise hot-overlay alpha. Zero means "no error/warn
    /// anywhere" -- UI falls back to the dim wash only.
    max_error_warn_sum: u32,
    /// Maximum `total` across all buckets. Reserved for a future
    /// density wash.
    max_total: u32,
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
        Level::Off | Level::Unknown => 0,
    }
}

/// Pure rollup used by `get_level_minimap` and by tests. Maps each
/// record's physical line span onto a `bucket_count`-wide grid and keeps
/// the worst severity per bucket. Equivalent to the inlined logic that
/// used to live inside the command.
fn build_level_minimap_payload(
    records: &[RecordHeader],
    line_count: u64,
    bucket_count: usize,
) -> LevelMinimapPayload {
    let bucket_count = bucket_count.max(1);
    let empty = BucketStat {
        worst: Level::Unknown,
        error: 0,
        warn: 0,
        total: 0,
    };
    let mut buckets = vec![empty; bucket_count];
    if line_count == 0 || records.is_empty() {
        return LevelMinimapPayload {
            buckets,
            line_count,
            max_error_warn_sum: 0,
            max_total: 0,
        };
    }
    let lc = line_count;
    let bc = bucket_count as u64;
    for rec in records {
        let first_line = u64::from(rec.line_offset);
        let last_line = first_line + u64::from(rec.line_count.max(1)) - 1;
        let first_bucket =
            usize::try_from(first_line.saturating_mul(bc) / lc).unwrap_or(bucket_count - 1);
        let last_bucket = usize::try_from(last_line.saturating_mul(bc) / lc)
            .unwrap_or(bucket_count - 1)
            .min(bucket_count - 1);
        for b in &mut buckets[first_bucket..=last_bucket] {
            if level_rank(rec.level) > level_rank(b.worst) {
                b.worst = rec.level;
            }
            b.total = b.total.saturating_add(1);
            match rec.level {
                Level::Error | Level::Fatal => {
                    b.error = b.error.saturating_add(1);
                }
                Level::Warn => {
                    b.warn = b.warn.saturating_add(1);
                }
                _ => {}
            }
        }
    }
    let mut max_error_warn_sum = 0u32;
    let mut max_total = 0u32;
    for b in &buckets {
        let heat = b.error.saturating_add(b.warn);
        if heat > max_error_warn_sum {
            max_error_warn_sum = heat;
        }
        if b.total > max_total {
            max_total = b.total;
        }
    }
    LevelMinimapPayload {
        buckets,
        line_count,
        max_error_warn_sum,
        max_total,
    }
}

fn pattern_hash(pattern_source: &str) -> u64 {
    let h = blake3::hash(pattern_source.as_bytes());
    let bytes = h.as_bytes();
    u64::from_le_bytes(bytes[..8].try_into().unwrap_or([0; 8]))
}

/// Pull a timestamp out of a record by re-rendering the slice covered
/// by `RecordHeader.fields.timestamp` and feeding it through a cheap
/// `YYYY-MM-DD HH:MM:SS.sss` parser. Returns `None` when the pattern
/// produced no timestamp field or the bytes don't match the expected
/// shape.
fn extract_record_timestamp_ms(rec: &RecordHeader, bytes: &[u8]) -> Option<i64> {
    let (s, e) = rec.fields.timestamp?;
    let base = usize::try_from(rec.byte_offset).ok()?;
    let start = base.saturating_add(s as usize);
    let end = base.saturating_add(e as usize);
    let slice = bytes.get(start..end)?;
    let text = std::str::from_utf8(slice).ok()?;
    parse_yyyy_mm_dd_hh_mm_ss_sss(text)
}

fn parse_yyyy_mm_dd_hh_mm_ss_sss(s: &str) -> Option<i64> {
    if s.len() != 23 {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: i64 = s[5..7].parse().ok()?;
    let day: i64 = s[8..10].parse().ok()?;
    let hour: i64 = s[11..13].parse().ok()?;
    let min: i64 = s[14..16].parse().ok()?;
    let sec: i64 = s[17..19].parse().ok()?;
    let ms: i64 = s[20..23].parse().ok()?;
    Some(
        ((year * 372 + (month - 1) * 31 + (day - 1)) * 86_400 + hour * 3600 + min * 60 + sec)
            * 1000
            + ms,
    )
}

fn rebuild_slow_request_cache(file: &mut OpenedFile) -> &[clog_core::SlowRequestOccurrence] {
    let signature = (
        file.records.len() as u64,
        file.bytes.len() as u64,
        pattern_hash(&file.pattern_source),
    );
    let needs_rebuild = file
        .slow_request_cache
        .as_ref()
        .is_none_or(|c| c.signature != signature);
    if needs_rebuild {
        let summary = clog_core::extract_slow_requests(
            &file.records,
            &file.bytes,
            &file.line_offsets,
            clog_core::PathMode::Raw,
            extract_record_timestamp_ms,
        );
        let mut occurrences: Vec<clog_core::SlowRequestOccurrence> = Vec::new();
        for entry in summary.entries {
            occurrences.extend(entry.occurrences);
        }
        file.slow_request_cache = Some(SlowRequestCache {
            signature,
            occurrences,
        });
    }
    &file
        .slow_request_cache
        .as_ref()
        .expect("cache built above")
        .occurrences
}

#[tauri::command]
fn get_slow_requests(
    state: State<'_, AppState>,
    file_id: u64,
    mode: clog_core::PathMode,
) -> Result<clog_core::SlowRequestSummary, IpcError> {
    let mut guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get_mut(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;
    let (lo, hi) = file.truncate_window();
    let _ = rebuild_slow_request_cache(file);
    let occs: Vec<clog_core::SlowRequestOccurrence> = file
        .slow_request_cache
        .as_ref()
        .expect("rebuild leaves cache populated")
        .occurrences
        .iter()
        .filter(|o| o.line_index >= lo && o.line_index < hi)
        .cloned()
        .collect();
    Ok(reaggregate_from_cache(&occs, mode))
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn reaggregate_from_cache(
    occs: &[clog_core::SlowRequestOccurrence],
    mode: clog_core::PathMode,
) -> clog_core::SlowRequestSummary {
    use std::collections::HashMap;
    struct G {
        path: String,
        raw_paths: Vec<String>,
        occs: Vec<clog_core::SlowRequestOccurrence>,
    }
    let mut groups: HashMap<String, G> = HashMap::new();
    for occ in occs {
        let key = match mode {
            clog_core::PathMode::Normalised => clog_core::normalise_path(&occ.raw_path),
            clog_core::PathMode::Raw => occ.raw_path.clone(),
        };
        let g = groups.entry(key.clone()).or_insert_with(|| G {
            path: key.clone(),
            raw_paths: Vec::new(),
            occs: Vec::new(),
        });
        if !g.raw_paths.contains(&occ.raw_path) {
            g.raw_paths.push(occ.raw_path.clone());
        }
        g.occs.push(occ.clone());
    }
    let mut entries: Vec<clog_core::SlowRequestEntry> = groups
        .into_values()
        .map(|mut g| {
            let count = u32::try_from(g.occs.len()).unwrap_or(u32::MAX);
            let mut durations: Vec<u32> = g.occs.iter().map(|o| o.duration_ms).collect();
            let total_ms: u64 = durations.iter().copied().map(u64::from).sum();
            let min_ms = *durations.iter().min().unwrap_or(&0);
            let max_ms = *durations.iter().max().unwrap_or(&0);
            let avg_ms = if count == 0 {
                0
            } else {
                u32::try_from(total_ms / u64::from(count)).unwrap_or(u32::MAX)
            };
            durations.sort_unstable();
            let n = durations.len();
            let p95_ms = if n == 0 {
                0
            } else {
                let idx = (((n as f64) * 0.95).ceil() as usize)
                    .saturating_sub(1)
                    .min(n - 1);
                durations[idx]
            };
            g.occs
                .sort_unstable_by(|a, b| b.duration_ms.cmp(&a.duration_ms));
            let longest_line = g.occs.first().map_or(0, |o| o.line_index);
            g.occs.truncate(clog_core::slow_requests::OCCURRENCE_CAP);
            clog_core::SlowRequestEntry {
                path: g.path,
                raw_paths: g.raw_paths,
                count,
                total_ms,
                min_ms,
                max_ms,
                avg_ms,
                p95_ms,
                longest_line,
                occurrences: g.occs,
            }
        })
        .collect();
    entries.sort_unstable_by(|a, b| b.total_ms.cmp(&a.total_ms));
    let total_hits = entries.iter().map(|e| e.count).sum();
    let total_ms = entries.iter().map(|e| e.total_ms).sum();
    let deduped = occs.iter().map(|o| o.dup_count.saturating_sub(1)).sum();
    clog_core::SlowRequestSummary {
        entries,
        total_hits,
        deduped,
        total_ms,
    }
}

#[tauri::command]
fn get_slow_request_speeds(
    state: State<'_, AppState>,
    file_id: u64,
    bucket_count: u32,
) -> Result<clog_core::SpeedGrid, IpcError> {
    let mut guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get_mut(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;
    let (lo, hi) = file.truncate_window();
    let span = hi.saturating_sub(lo);
    let _ = rebuild_slow_request_cache(file);
    let occs: Vec<clog_core::SlowRequestOccurrence> = file
        .slow_request_cache
        .as_ref()
        .expect("rebuild leaves cache populated")
        .occurrences
        .iter()
        .filter(|o| o.line_index >= lo && o.line_index < hi)
        .map(|o| {
            let mut c = o.clone();
            c.line_index = o.line_index.saturating_sub(lo);
            c
        })
        .collect();
    Ok(clog_core::build_speed_grid(
        &occs,
        span,
        bucket_count as usize,
    ))
}

#[derive(Debug, Clone, Serialize)]
struct EffectiveThresholds {
    source: &'static str,
    effective: clog_core::SlowRequestThresholds,
    per_file: Option<clog_core::SlowRequestThresholds>,
    global: Option<clog_core::SlowRequestThresholds>,
}

#[tauri::command]
fn get_slow_request_thresholds(
    state: State<'_, AppState>,
    file_id: u64,
) -> Result<EffectiveThresholds, IpcError> {
    let path = {
        let guard = state.files.lock().expect("files mutex poisoned");
        let file = guard
            .get(&file_id)
            .ok_or(IpcError::UnknownFile { file_id })?;
        file.path.clone()
    };
    let per_file = persistence::PerFileRulesFile::load(&path).slow_request_thresholds;
    let global = persistence::Settings::load().slow_request_thresholds;
    let (effective, source) = if let Some(t) = per_file {
        (t, "per_file")
    } else if let Some(t) = global {
        (t, "global")
    } else {
        // Auto-tier defaults. Any bucket with no slow requests reads
        // green at 0 ms; the gradient is driven from yellow up to red
        // at 10 s. The UI's paint logic treats `source == "auto"` as
        // "non-empty buckets start at mid (yellow), not green" so a
        // single hit is immediately visible regardless of its duration.
        (
            clog_core::SlowRequestThresholds::new(2_000, 10_000).expect("2000/10000 is valid"),
            "auto",
        )
    };
    Ok(EffectiveThresholds {
        source,
        effective,
        per_file,
        global,
    })
}

#[tauri::command]
fn save_slow_request_thresholds(
    state: State<'_, AppState>,
    file_id: u64,
    thresholds: Option<clog_core::SlowRequestThresholds>,
) -> Result<(), IpcError> {
    let path = {
        let guard = state.files.lock().expect("files mutex poisoned");
        let file = guard
            .get(&file_id)
            .ok_or(IpcError::UnknownFile { file_id })?;
        file.path.clone()
    };
    let validated = match thresholds {
        Some(t) => match clog_core::SlowRequestThresholds::new(t.fast_ms, t.slow_ms) {
            Some(v) => Some(v),
            None => {
                return Err(IpcError::BadPattern {
                    message: format!("invalid thresholds: fast={} slow={}", t.fast_ms, t.slow_ms),
                })
            }
        },
        None => None,
    };
    let mut f = persistence::PerFileRulesFile::load(&path);
    f.slow_request_thresholds = validated;
    if f.is_effectively_empty() {
        persistence::PerFileRulesFile::forget(&path).map_err(|e| IpcError::Io {
            message: e.to_string(),
            path: path.display().to_string(),
        })?;
    } else {
        f.save(&path).map_err(|e| IpcError::Io {
            message: e.to_string(),
            path: path.display().to_string(),
        })?;
    }
    Ok(())
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
    let (lo, hi) = file.truncate_window();
    if file.truncate_before.is_none() && file.truncate_after.is_none() {
        return Ok(build_level_minimap_payload(
            &file.records,
            file.line_count,
            bucket_count as usize,
        ));
    }
    let span = hi.saturating_sub(lo);
    let windowed: Vec<RecordHeader> = file
        .records
        .iter()
        .filter(|r| {
            let f = u64::from(r.line_offset);
            f >= lo && f < hi
        })
        .map(|r| {
            let mut c = r.clone();
            c.line_offset =
                u32::try_from(u64::from(r.line_offset).saturating_sub(lo)).unwrap_or(u32::MAX);
            c
        })
        .collect();
    Ok(build_level_minimap_payload(
        &windowed,
        span,
        bucket_count as usize,
    ))
}

/// Kind of significant event marker overlaid on the viewport's left rail.
/// New kinds are added by appending here and to `BUILTIN_MARKER_RULES`; the
/// UI picks up the new variant through the serialised tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum MarkerKind {
    /// Site restart, currently detected via "Core Plugin Load" appearing
    /// in a record's first physical line.
    Restart,
}

/// Rule used to flag records as markers. `needle` is a case-sensitive
/// literal substring matched against the first physical line of a record.
/// The substring form is intentional - the patterns we care about for v1
/// (site restart banners, lifecycle events) are stable strings; the
/// machinery can promote to regex later without changing the wire shape.
struct MarkerRule {
    kind: MarkerKind,
    needle: &'static str,
}

const BUILTIN_MARKER_RULES: &[MarkerRule] = &[MarkerRule {
    kind: MarkerKind::Restart,
    needle: "Core Plugin Load",
}];

#[derive(Debug, Clone, Serialize)]
struct MarkerRef {
    kind: MarkerKind,
    /// Physical line index of the marker (== the record's first line).
    line_index: u64,
    /// Index into `OpenedFile.records`.
    record_idx: u32,
}

/// Walk every record's first physical line and emit one `MarkerRef` per
/// (record, matching rule) pair. Records are scanned once; the first
/// matching rule wins, so a record never produces two markers. Linear in
/// `records.len() * sum(needle_len)`; the call is cheap relative to the
/// minimap rollup.
fn scan_markers(
    records: &[RecordHeader],
    bytes: &[u8],
    line_offsets: &[u64],
    rules: &[MarkerRule],
) -> Vec<MarkerRef> {
    let mut out = Vec::new();
    if rules.is_empty() {
        return out;
    }
    let total_lines = line_offsets.len();
    for (rec_idx, rec) in records.iter().enumerate() {
        let line_idx = rec.line_offset as usize;
        if line_idx >= total_lines {
            continue;
        }
        let line_start = usize::try_from(line_offsets[line_idx]).unwrap_or(usize::MAX);
        // Bound the line by the next line offset (excluding the trailing
        // newline) or, for the last line, by the record's byte end.
        let line_end_raw = if line_idx + 1 < total_lines {
            let next = usize::try_from(line_offsets[line_idx + 1]).unwrap_or(usize::MAX);
            next.saturating_sub(1)
        } else {
            usize::try_from(rec.byte_offset + u64::from(rec.byte_len)).unwrap_or(usize::MAX)
        };
        let line_end = line_end_raw.min(bytes.len()).max(line_start);
        let line = &bytes[line_start..line_end];
        for rule in rules {
            let needle = rule.needle.as_bytes();
            if needle.is_empty() || needle.len() > line.len() {
                continue;
            }
            if line.windows(needle.len()).any(|w| w == needle) {
                out.push(MarkerRef {
                    kind: rule.kind,
                    line_index: u64::from(rec.line_offset),
                    record_idx: u32::try_from(rec_idx).unwrap_or(u32::MAX),
                });
                break;
            }
        }
    }
    out
}

#[tauri::command]
fn get_markers(state: State<'_, AppState>, file_id: u64) -> Result<Vec<MarkerRef>, IpcError> {
    let guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;
    let (lo, hi) = file.truncate_window();
    let mut markers = scan_markers(
        &file.records,
        &file.bytes,
        &file.line_offsets,
        BUILTIN_MARKER_RULES,
    );
    markers.retain(|m| m.line_index >= lo && m.line_index < hi);
    Ok(markers)
}

/// Lightweight projection of a record. Used by the UI's level-mask + filter
/// path so it can build the visible-line set without going through the
/// full search engine.
#[derive(Debug, Clone, Serialize)]
pub struct RecordRef {
    pub record_idx: u64,
    pub record_first_line: u64,
    pub record_line_count: u32,
    pub level: Level,
}

#[derive(Debug, Serialize)]
struct RecordRefsPayload {
    refs: Vec<RecordRef>,
}

#[derive(Debug, Serialize)]
struct TruncatePayload {
    before: Option<u64>,
    after: Option<u64>,
    /// Number of physical lines inside the window.
    line_count: u64,
    /// Number of records whose first line falls inside the window.
    record_count: u64,
}

/// Return every record's `(record_idx, first_line, line_count)` whose
/// level passes `level_mask` AND whose thread group passes
/// `thread_group_mask`. The UI uses this to build `filteredLineIndices`
/// when the search query is empty -- the masks alone narrow the view
/// without needing a fake "match all" search.
#[tauri::command]
fn list_records_by_filters(
    state: State<'_, AppState>,
    file_id: u64,
    level_mask: u32,
    thread_group_mask: u32,
) -> Result<RecordRefsPayload, IpcError> {
    let guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;
    let lmask = LevelMask(u16::try_from(level_mask & 0xFFFF).unwrap_or(0xFFFF));
    let tmask = ThreadGroupMask(u8::try_from(thread_group_mask & 0xFF).unwrap_or(0x3F));
    let (lo, hi) = file.truncate_window();
    let bytes = &file.bytes;
    let refs = file
        .records
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            let f = u64::from(r.line_offset);
            if f < lo || f >= hi {
                return false;
            }
            if !lmask.allows(r.level) {
                return false;
            }
            if tmask == ThreadGroupMask::ALL {
                return true;
            }
            let group = thread_group_of(r, bytes);
            tmask.allows(group)
        })
        .map(|(i, r)| RecordRef {
            record_idx: i as u64,
            record_first_line: u64::from(r.line_offset),
            record_line_count: r.line_count,
            level: r.level,
        })
        .collect();
    Ok(RecordRefsPayload { refs })
}

fn thread_group_of(rec: &clog_core::RecordHeader, bytes: &[u8]) -> ThreadGroup {
    match rec.fields.thread {
        Some((s, e)) => {
            let base = usize::try_from(rec.byte_offset).unwrap_or(usize::MAX);
            let start = base.saturating_add(s as usize);
            let end = base.saturating_add(e as usize).min(bytes.len());
            if start > end || start >= bytes.len() {
                ThreadGroup::Other
            } else {
                classify_thread(&bytes[start..end])
            }
        }
        None => ThreadGroup::Other,
    }
}

/// Set (or clear) the truncate window for `file_id`. `before` is the first
/// visible physical line; `after` is one past the last visible physical line.
/// Both are expected pre-snapped to record boundaries by the caller. Rejects an
/// empty or inverted window. `(None, None)` clears the window.
#[tauri::command]
fn set_truncate(
    state: State<'_, AppState>,
    file_id: u64,
    before: Option<u64>,
    after: Option<u64>,
) -> Result<TruncatePayload, IpcError> {
    let mut guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get_mut(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;
    file.apply_truncate(before, after)
}

#[tauri::command]
fn close_file(state: State<'_, AppState>, file_id: u64) {
    let mut guard = state.files.lock().expect("files mutex poisoned");
    if let Some(mut f) = guard.remove(&file_id) {
        f.stop_tail();
        f.cancel_search();
    }
}

/// Streamed per-batch payload for `start_search`. The UI accumulates
/// `hits` into its hit map and uses `total` for the count badge; the
/// terminal message carries `done = true` and `hits = []` (or whatever
/// remained in the buffer at `finish` time).
#[derive(Debug, Clone, Serialize)]
pub struct SearchDelta {
    /// Discriminator: the UI ignores messages whose id doesn't match the
    /// search it last started. Prevents stale results from a previous
    /// keystroke clobbering the current set.
    pub search_id: u64,
    pub hits: Vec<HitRef>,
    /// Cumulative hit count delivered so far, including the hits in this
    /// message. The UI reads this for its "N hits" badge.
    pub total: u64,
    /// True only on the terminal message of a given search. After this
    /// the search task has exited.
    pub done: bool,
}

/// Request body for `start_search`. Bundled into a struct so the IPC
/// command stays under the `too_many_arguments` lint.
#[derive(Debug, serde::Deserialize)]
pub struct SearchRequest {
    mode: String,
    query: String,
    #[serde(default)]
    case_sensitive: bool,
    /// Bitmask of allowed levels. Bit ordering matches `clog_core::search::level_bit`.
    level_mask: u32,
    /// Bitmask of allowed thread groups. Bit ordering matches `clog_core::group_bit`.
    /// Defaults to ALL (0x3F) when absent so older session-restored payloads
    /// still decode.
    #[serde(default = "default_full_thread_group_mask")]
    thread_group_mask: u32,
}

fn default_full_thread_group_mask() -> u32 {
    0x3F
}

/// Start (or restart) a search across `file_id`. Any previous in-flight
/// search for this file is cancelled first. Hits stream back on
/// `on_hits` in batches; the terminal message has `done = true`.
#[tauri::command]
fn start_search(
    app: AppHandle,
    state: State<'_, AppState>,
    file_id: u64,
    request: SearchRequest,
    on_hits: Channel<SearchDelta>,
) -> Result<u64, IpcError> {
    let SearchRequest {
        mode,
        query,
        case_sensitive,
        level_mask,
        thread_group_mask,
    } = request;
    let search_mode = match mode.as_str() {
        "smart" => SearchMode::Smart,
        "regex" => SearchMode::Regex,
        _ => {
            return Err(IpcError::BadPattern {
                message: format!("unknown search mode {mode:?}"),
            })
        }
    };

    // Pre-flight: validate a regex compiles so the UI can red-underline
    // a broken pattern without ever spawning a task. Smart mode's empty
    // query is caught here too so the error surface is uniform.
    let opts = SearchOptions {
        case_sensitive,
        level_mask: LevelMask(u16::try_from(level_mask & 0xFFFF).unwrap_or(0xFFFF)),
        thread_group_mask: ThreadGroupMask(u8::try_from(thread_group_mask & 0xFF).unwrap_or(0x3F)),
    };
    if matches!(search_mode, SearchMode::Smart) && query.split_ascii_whitespace().next().is_none() {
        return Err(IpcError::EmptyQuery);
    }
    if matches!(search_mode, SearchMode::Regex) {
        // Bytes regex; same compile path the engine uses.
        let pattern = if case_sensitive {
            query.clone()
        } else {
            format!("(?i){query}")
        };
        regex::bytes::Regex::new(&pattern).map_err(|e| IpcError::BadRegex {
            message: e.to_string(),
        })?;
    }

    // Snapshot the records + bytes under the lock so the search task can
    // run on a clone. Cancel any previous search, allocate a fresh flag
    // + id, and remember them on the file so cancel_search/close_file
    // can reach them.
    let (records_snapshot, bytes_snapshot, search_id, cancel_flag, win_lo, win_hi) = {
        let mut guard = state.files.lock().expect("files mutex poisoned");
        let file = guard
            .get_mut(&file_id)
            .ok_or(IpcError::UnknownFile { file_id })?;
        file.cancel_search();
        file.current_search_id += 1;
        let id = file.current_search_id;
        let flag = Arc::new(AtomicBool::new(false));
        file.search_cancel = Some(flag.clone());
        let (lo, hi) = file.truncate_window();
        (file.records.clone(), file.bytes.clone(), id, flag, lo, hi)
    };

    let app_handle = app.clone();
    let join = tauri::async_runtime::spawn(async move {
        let emitter = SearchEmitter::new(on_hits, search_id);
        // Heavy lifting on the blocking pool so we don't park the tokio
        // worker for the duration of the rayon parallel walk.
        let cancel_for_task = cancel_flag.clone();
        let result = tokio::task::spawn_blocking(move || {
            // The rayon pass produces ALL hits at once. Cancellation
            // takes effect at the granularity of "between hot loop and
            // emission" rather than mid-walk, which is acceptable -- the
            // bound is sub-second on a 75k-record file.
            if cancel_for_task.load(std::sync::atomic::Ordering::Relaxed) {
                return Ok(Vec::new());
            }
            search_records(
                &records_snapshot,
                &bytes_snapshot,
                search_mode,
                &query,
                opts,
            )
        })
        .await;

        let hits = match result {
            Ok(Ok(hits)) => hits,
            // BadRegex was caught pre-flight; this can only be EmptyQuery
            // re-raised from the engine. Treat as "no hits" so the UI
            // still receives a terminal done message.
            Ok(Err(_)) | Err(_) => Vec::new(),
        };

        // Drop hits whose record sits outside the truncate window before
        // streaming, so the hit count and next/previous navigation only see
        // the kept region. record_idx stays absolute (the window filters
        // which hits stream, never renumbers them).
        let hits: Vec<HitRef> = hits
            .into_iter()
            .filter(|h| h.record_first_line >= win_lo && h.record_first_line < win_hi)
            .collect();

        // Stream them out in batched messages. The emitter ships
        // SEARCH_BATCH_SIZE per batch.
        let _cancelled = stream_hits(emitter, hits, &cancel_flag);

        // Clear the file's cancel handle iff we're still the current
        // search. A newer start_search has already replaced it -- don't
        // stomp.
        clear_search_state_if_current(&app_handle, file_id, search_id);
    });

    let mut guard = state.files.lock().expect("files mutex poisoned");
    if let Some(file) = guard.get_mut(&file_id) {
        file.search_join = Some(join);
    }
    Ok(search_id)
}

fn clear_search_state_if_current(app: &AppHandle, file_id: u64, search_id: u64) {
    let state = app.state::<AppState>();
    let mut guard = state.files.lock().expect("files mutex poisoned");
    if let Some(f) = guard.get_mut(&file_id) {
        if f.current_search_id == search_id {
            f.search_cancel = None;
            f.search_join = None;
        }
    }
}

fn stream_hits(mut emitter: SearchEmitter, hits: Vec<HitRef>, cancel: &Arc<AtomicBool>) -> bool {
    let mut cancelled = false;
    for hit in hits {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        if emitter.push(hit).is_err() {
            cancelled = true;
            break;
        }
    }
    let _ = if cancelled {
        emitter.abort()
    } else {
        emitter.finish()
    };
    cancelled
}

#[tauri::command]
fn cancel_search(state: State<'_, AppState>, file_id: u64) -> Result<(), IpcError> {
    let mut guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get_mut(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;
    file.cancel_search();
    Ok(())
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
    // User-applied patterns clear the auto-detected name, so the file
    // becomes "custom" and switches to loose-mode scanning (each line is
    // its own record).
    let (records, source_string, kind) = match (pattern, regex) {
        (Some(p), _) => {
            let scanner = CompiledPattern::compile(&p)?;
            let src = scanner.source.clone();
            (
                scan_records(&LooseScanner::new(&scanner), &line_index, &file.bytes),
                src.clone(),
                ScannerKind::Pattern(src),
            )
        }
        (_, Some(r)) => {
            let scanner = RegexScanner::compile(&r)?;
            (
                scan_records(&LooseScanner::new(&scanner), &line_index, &file.bytes),
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
    file.scanner_kind = kind.clone();
    file.loose = true;
    let line_count = line_index.line_count() as u64;
    file.rebuild_line_caches(line_count, line_index.line_offsets.clone());

    // Persist a per-file override so the next open uses this pattern
    // automatically, and refresh the on-disk index cache to match.
    let path_key = file.path.to_string_lossy().to_string();
    let path_buf = file.path.clone();
    let bytes_view = file.bytes.clone();
    let records_view = file.records.clone();
    drop(guard);
    let (override_kind, override_source) = match &kind {
        ScannerKind::Pattern(s) => ("pattern".to_string(), s.clone()),
        ScannerKind::Regex(s) => ("regex".to_string(), s.clone()),
    };
    let mut patterns = PatternsFile::load();
    patterns.overrides.insert(
        path_key,
        PatternOverride {
            kind: override_kind,
            source: override_source.clone(),
        },
    );
    if let Err(e) = patterns.save() {
        tracing::warn!(target: "clog::patterns", error = %e, "patterns.json write failed");
    }
    let fp_source = {
        let raw = match &kind {
            ScannerKind::Pattern(s) => s.clone(),
            ScannerKind::Regex(s) => format!("regex:{s}"),
        };
        // set_pattern always switches to loose mode, so mirror open_file's
        // fingerprint scheme.
        format!("loose:{raw}")
    };
    if let Ok(fp) = CacheFingerprint::for_path(&path_buf, &fp_source) {
        let li = clog_core::LineIndex {
            line_offsets: line_index.line_offsets,
            file_size: bytes_view.len() as u64,
        };
        let cache_path = paths::index_cache_path(&path_buf);
        if let Err(e) = clog_core::save_index_cache(&cache_path, &fp, &li, &records_view) {
            tracing::warn!(target: "clog::cache", error = %e, "index cache refresh after set_pattern failed");
        }
    }

    Ok(ApplyPatternPayload {
        record_count: count,
        pattern_source: source_string,
        loose: true,
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
    if file.loose {
        let loose = LooseScanner::new(&scanner);
        extend_with_appended(file, &loose, from_offset, appended);
    } else {
        extend_with_appended(file, &scanner, from_offset, appended);
    }
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
    let prev_was_partial = file.bytes.last().is_some_and(|&b| b != b'\n');
    file.bytes.extend_from_slice(appended);
    let new_total = file.bytes.len() as u64;
    let first_new_record_idx = file.records.len();

    // Partial-line continuation. If the file's previous tail byte was
    // not `\n`, the previous record's last physical line is incomplete
    // and the first bytes of `appended` belong to it. They extend its
    // text (and possibly close it with a `\n`); they MUST NOT push a
    // new line_offset or record. The byte_len fix-up at the bottom
    // brings the bookkeeping back into sync.
    let mut local = 0usize;
    if prev_was_partial && !appended.is_empty() {
        match appended.iter().position(|&b| b == b'\n') {
            Some(rel) => local = rel + 1,
            None => local = appended.len(),
        }
    }

    // Walk the rest as new physical lines. The buffer may end mid-line
    // (no trailing `\n`); that trailing chunk becomes a new partial
    // line with its own line_offset/record, and the next tick will
    // extend it via the partial-line continuation branch above.
    while local < appended.len() {
        let (nl_abs, has_newline) = match appended[local..].iter().position(|&b| b == b'\n') {
            Some(rel) => (local + rel, true),
            None => (appended.len(), false),
        };
        let abs_line_start = from_offset + local as u64;
        let mut clean_end = nl_abs;
        if has_newline && clean_end > local && appended[clean_end - 1] == b'\r' {
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

        local = if has_newline {
            nl_abs + 1
        } else {
            appended.len()
        };
    }

    // Fix up byte_len for any record whose end falls inside the appended
    // range. That's the last record before the append (it always gains
    // bytes -- either via partial-line continuation or because the
    // append closed its trailing `\n`) plus every new record we just
    // pushed.
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
    let (path, scanner_kind, loose) = {
        let guard = state.files.lock().expect("files mutex poisoned");
        let file = guard
            .get(&file_id)
            .ok_or(IpcError::UnknownFile { file_id })?;
        (file.path.clone(), file.scanner_kind.clone(), file.loose)
    };

    let scanner = scanner_kind.compile()?;
    // Re-read everything. The file may transiently be very small or even
    // empty between rotation steps; that's fine, we'll re-index again on
    // the next growth event.
    let (mut source, line_index, records) = if loose {
        index_file(&path, &LooseScanner::new(&scanner))?
    } else {
        index_file(&path, &scanner)?
    };
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

// --- Persistence IPC -------------------------------------------------------

#[derive(Debug, Serialize)]
struct DataDirPayload {
    path: String,
    portable: bool,
}

#[tauri::command]
fn get_data_dir() -> DataDirPayload {
    let dir = paths::data_dir();
    let portable = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.join("clog-data").is_dir()))
        .unwrap_or(false);
    DataDirPayload {
        path: dir.to_string_lossy().to_string(),
        portable,
    }
}

#[tauri::command]
fn open_data_dir() -> Result<(), IpcError> {
    let dir = paths::data_dir();
    #[cfg(target_os = "windows")]
    let res = std::process::Command::new("explorer.exe")
        .arg(&dir)
        .spawn()
        .map(|_| ());
    #[cfg(target_os = "macos")]
    let res = std::process::Command::new("open")
        .arg(&dir)
        .spawn()
        .map(|_| ());
    #[cfg(all(unix, not(target_os = "macos")))]
    let res = std::process::Command::new("xdg-open")
        .arg(&dir)
        .spawn()
        .map(|_| ());

    res.map_err(|e| IpcError::Io {
        message: e.to_string(),
        path: dir.display().to_string(),
    })
}

#[tauri::command]
fn get_settings() -> Settings {
    Settings::load()
}

#[derive(Debug, serde::Deserialize)]
pub struct SettingsPatch {
    pub theme: Option<String>,
    pub font_size: Option<u32>,
    pub follow_tail_default: Option<bool>,
    /// Set to `Some(Some(thresholds))` to update, `Some(None)` to clear,
    /// `None` to leave untouched.
    #[serde(default, deserialize_with = "deserialize_optional_optional")]
    pub slow_request_thresholds: Option<Option<clog_core::SlowRequestThresholds>>,
    pub colour_blind: Option<bool>,
    pub minimap_heatmap_blend: Option<f32>,
    pub minimap_background_opacity: Option<f32>,
    pub speed_rail_enabled: Option<bool>,
    /// Global default collapse mode for multi-line records:
    /// `"none"` | `"errors"` | `"all"`. `None` leaves it untouched.
    pub collapse_records_default: Option<String>,
    /// Tri-state: `None` = untouched, `Some(None)` = clear (revert to default
    /// stack), `Some(Some(name))` = set this family.
    #[serde(default, deserialize_with = "deserialize_optional_optional")]
    pub mono_font_family: Option<Option<String>>,
}

#[allow(clippy::option_option)] // tri-state patch: None=untouched, Some(None)=clear, Some(Some)=set
fn deserialize_optional_optional<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    <Option<T> as serde::Deserialize>::deserialize(deserializer).map(Some)
}

#[tauri::command]
fn update_settings(patch: SettingsPatch) -> Result<Settings, IpcError> {
    let mut s = Settings::load();
    if let Some(t) = patch.theme {
        s.theme = t;
    }
    if let Some(f) = patch.font_size {
        s.font_size = f.clamp(9, 24);
    }
    if let Some(b) = patch.follow_tail_default {
        s.follow_tail_default = b;
    }
    if let Some(opt) = patch.slow_request_thresholds {
        match opt {
            Some(t) => match clog_core::SlowRequestThresholds::new(t.fast_ms, t.slow_ms) {
                Some(valid) => s.slow_request_thresholds = Some(valid),
                None => {
                    return Err(IpcError::BadPattern {
                        message: format!(
                            "invalid slow_request_thresholds: fast={} slow={}",
                            t.fast_ms, t.slow_ms
                        ),
                    })
                }
            },
            None => s.slow_request_thresholds = None,
        }
    }
    if let Some(b) = patch.colour_blind {
        s.colour_blind = b;
    }
    if let Some(v) = patch.minimap_heatmap_blend {
        s.minimap_heatmap_blend = v.clamp(0.0, 1.0);
    }
    if let Some(v) = patch.minimap_background_opacity {
        s.minimap_background_opacity = v.clamp(0.0, 1.0);
    }
    if let Some(b) = patch.speed_rail_enabled {
        s.speed_rail_enabled = b;
    }
    if let Some(v) = patch.collapse_records_default {
        s.collapse_records_default = v;
    }
    if let Some(opt) = patch.mono_font_family {
        s.mono_font_family = opt.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    }
    s.save().map_err(|e| IpcError::Io {
        message: e.to_string(),
        path: paths::settings_path().display().to_string(),
    })?;
    Ok(s)
}

/// Enumerate the system's installed font families. Returned list is
/// sorted case-insensitively and de-duplicated. On failure an empty list
/// is returned rather than an error so the picker degrades to "no
/// suggestions" instead of breaking the modal.
#[tauri::command]
fn list_system_fonts() -> Vec<String> {
    use font_kit::source::SystemSource;
    let Ok(mut names) = SystemSource::new().all_families() else {
        return Vec::new();
    };
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

#[tauri::command]
fn forget_recent(path: String) -> Result<Settings, IpcError> {
    let mut s = Settings::load();
    s.forget_recent(&path);
    s.save().map_err(|e| IpcError::Io {
        message: e.to_string(),
        path: paths::settings_path().display().to_string(),
    })?;
    Ok(s)
}

#[tauri::command]
fn get_session() -> Session {
    Session::load()
}

#[tauri::command]
fn save_session(session: Session) -> Result<(), IpcError> {
    session.save().map_err(|e| IpcError::Io {
        message: e.to_string(),
        path: paths::session_path().display().to_string(),
    })
}

#[tauri::command]
fn get_pattern_override(path: String) -> Option<PatternOverride> {
    PatternsFile::load().overrides.get(&path).cloned()
}

#[tauri::command]
fn forget_pattern_override(path: String) -> Result<(), IpcError> {
    let mut p = PatternsFile::load();
    p.overrides.remove(&path);
    p.save().map_err(|e| IpcError::Io {
        message: e.to_string(),
        path: paths::patterns_path().display().to_string(),
    })
}

// --- Highlight rules IPC --------------------------------------------------

#[tauri::command]
fn get_highlight_rules() -> HighlightRulesFile {
    HighlightRulesFile::load()
}

#[tauri::command]
fn save_highlight_rules(rules: HighlightRulesFile) -> Result<(), IpcError> {
    rules.save().map_err(|e| IpcError::Io {
        message: e.to_string(),
        path: paths::highlight_rules_path().display().to_string(),
    })
}

#[tauri::command]
fn get_per_file_rules(path: String) -> PerFileRulesFile {
    PerFileRulesFile::load(Path::new(&path))
}

#[tauri::command]
fn save_per_file_rules(path: String, rules: PerFileRulesFile) -> Result<(), IpcError> {
    let source = PathBuf::from(&path);
    rules.save(&source).map_err(|e| IpcError::Io {
        message: e.to_string(),
        path: paths::per_file_rules_path(&source).display().to_string(),
    })
}

#[tauri::command]
fn forget_per_file_rules(path: String) -> Result<(), IpcError> {
    let source = PathBuf::from(&path);
    PerFileRulesFile::forget(&source).map_err(|e| IpcError::Io {
        message: e.to_string(),
        path: paths::per_file_rules_path(&source).display().to_string(),
    })
}

#[derive(Debug, serde::Deserialize)]
pub struct ResetRequest {
    /// `"settings"` | `"session"` | `"patterns"` | `"index"` | `"all"`.
    pub scope: String,
}

#[tauri::command]
fn reset_data(req: ResetRequest) -> Result<(), IpcError> {
    let dir = paths::data_dir();
    let candidates: Vec<PathBuf> = match req.scope.as_str() {
        "settings" => vec![paths::settings_path()],
        "session" => vec![paths::session_path()],
        "patterns" => vec![paths::patterns_path()],
        "index" => vec![paths::index_dir()],
        "highlight" => vec![paths::highlight_rules_path(), paths::per_file_rules_dir()],
        "all" => vec![
            paths::settings_path(),
            paths::session_path(),
            paths::patterns_path(),
            paths::index_dir(),
            paths::highlight_rules_path(),
            paths::per_file_rules_dir(),
        ],
        other => {
            return Err(IpcError::BadPattern {
                message: format!("unknown reset scope {other:?}"),
            })
        }
    };
    for p in candidates {
        if p.is_dir() {
            let _ = std::fs::remove_dir_all(&p);
        } else {
            let _ = std::fs::remove_file(&p);
        }
    }
    tracing::info!(target: "clog::reset", scope = %req.scope, root = %dir.display(), "reset data");
    Ok(())
}

// --- updater -------------------------------------------------------------

#[derive(Debug, Serialize)]
struct UpdateStatus {
    /// True when `latest.json` advertised a version higher than the running
    /// binary AND we are not currently snoozing that version.
    available: bool,
    /// Currently-running app version. Always populated.
    current_version: String,
    /// Advertised version (only present when `available` is true).
    available_version: Option<String>,
    /// One-line summary lifted from `latest.json`'s `notes` field. Trimmed
    /// of trailing whitespace; rendered verbatim by the UI.
    notes: Option<String>,
    /// `"installer"` for installed builds, `"portable"` for portable builds.
    /// The UI uses this to swap the `Update now` button for a `Download`
    /// link to the release page when running portably.
    mode: &'static str,
    /// True when the silent-cadence guard kept us from making an HTTP
    /// request this call. The UI uses this only to decide whether to log
    /// telemetry; the `available` flag is the actual signal.
    skipped_by_cadence: bool,
    /// True when the user has snoozed the advertised version. The IPC
    /// still returns the version + notes so callers can show "you're
    /// snoozing 1.2.3" if they want; `available` is forced to false.
    snoozed: bool,
}

#[tauri::command]
async fn check_for_update(force: bool, app: AppHandle) -> Result<UpdateStatus, IpcError> {
    use tauri_plugin_updater::UpdaterExt;

    let current_version = app.package_info().version.to_string();
    let mode = if paths::is_portable() {
        "portable"
    } else {
        "installer"
    };

    let mut state = update::UpdateState::load();
    let now = std::time::SystemTime::now();

    if !force && !state.should_silent_check(now) {
        return Ok(UpdateStatus {
            available: false,
            current_version,
            available_version: None,
            notes: None,
            mode,
            skipped_by_cadence: true,
            snoozed: false,
        });
    }

    let updater = app.updater().map_err(|e| IpcError::BadPattern {
        message: format!("updater unavailable: {e}"),
    })?;

    // mark_checked before the network call so a hung endpoint still
    // counts as "checked today" and we don't hammer a flaky server on
    // every launch.
    state.mark_checked(now);
    let _ = state.save();

    let check_result = updater.check().await;
    match check_result {
        Ok(Some(u)) => {
            let version = u.version.clone();
            let notes = u.body.as_ref().and_then(|b| {
                let trimmed = b.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.lines().next().unwrap_or(trimmed).to_string())
                }
            });
            let snoozed = !force && state.is_snoozed(&version, now);
            Ok(UpdateStatus {
                available: !snoozed,
                current_version,
                available_version: Some(version),
                notes,
                mode,
                skipped_by_cadence: false,
                snoozed,
            })
        }
        Ok(None) => Ok(UpdateStatus {
            available: false,
            current_version,
            available_version: None,
            notes: None,
            mode,
            skipped_by_cadence: false,
            snoozed: false,
        }),
        Err(e) => {
            tracing::warn!(target: "clog::update", error = %e, "update check failed");
            Err(IpcError::BadPattern {
                message: format!("update check failed: {e}"),
            })
        }
    }
}

#[tauri::command]
async fn install_update_now(app: AppHandle) -> Result<(), IpcError> {
    use tauri_plugin_updater::UpdaterExt;

    if paths::is_portable() {
        return Err(IpcError::BadPattern {
            message: "portable installs do not auto-update; open the release page".to_string(),
        });
    }

    let updater = app.updater().map_err(|e| IpcError::BadPattern {
        message: format!("updater unavailable: {e}"),
    })?;

    let update = updater
        .check()
        .await
        .map_err(|e| IpcError::BadPattern {
            message: format!("update check failed: {e}"),
        })?
        .ok_or(IpcError::BadPattern {
            message: "no update available".to_string(),
        })?;

    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|e| IpcError::BadPattern {
            message: format!("update install failed: {e}"),
        })?;
    Ok(())
}

#[tauri::command]
fn snooze_update(version: String) -> Result<(), IpcError> {
    let mut state = update::UpdateState::load();
    state.snooze(&version, std::time::SystemTime::now());
    state.save().map_err(|e| IpcError::Io {
        message: e.to_string(),
        path: paths::update_state_path().display().to_string(),
    })
}

fn init_tracing() -> tracing_appender::non_blocking::WorkerGuard {
    let logs = paths::logs_dir();
    let appender = tracing_appender::rolling::daily(&logs, "clog.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let env_filter = tracing_subscriber::EnvFilter::try_from_env("CLOG_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(writer)
        .with_ansi(false)
        .try_init();
    tracing::info!(target: "clog::boot", logs = %logs.display(), "tracing initialised");
    guard
}

fn main() {
    let log_guard = init_tracing();
    std::panic::set_hook(Box::new(|info| {
        tracing::error!(target: "clog::panic", "panic: {info}");
    }));

    // Seed startup_paths from this process's argv so the UI picks them up
    // on boot via `take_startup_paths`.
    let argv: Vec<String> = std::env::args().collect();
    let initial_paths = filter_paths(&argv);
    let state = AppState::default();
    if !initial_paths.is_empty() {
        *state.startup_paths.lock().expect("startup_paths mutex") = initial_paths;
    }

    tauri::Builder::default()
        // The single-instance plugin must be registered first so a second
        // launch is shut down before any heavy state allocates. The callback
        // runs in the *already-running* instance with the new process's argv
        // and cwd; we forward filtered paths to the webview via an event.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            let paths = filter_paths(&argv);
            tracing::info!(
                target: "clog::single_instance",
                count = paths.len(),
                "second instance forwarded paths"
            );
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_focus();
            }
            let _ = app.emit("single-instance-paths", paths);
        }))
        .manage(state)
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            open_file,
            get_records,
            get_lines,
            get_line_window,
            get_record_lines,
            close_file,
            test_pattern,
            set_pattern,
            start_tail,
            stop_tail,
            get_level_minimap,
            get_markers,
            get_slow_requests,
            get_slow_request_speeds,
            get_slow_request_thresholds,
            save_slow_request_thresholds,
            start_search,
            cancel_search,
            list_records_by_filters,
            set_truncate,
            get_data_dir,
            open_data_dir,
            get_settings,
            update_settings,
            list_system_fonts,
            forget_recent,
            get_session,
            save_session,
            get_pattern_override,
            forget_pattern_override,
            get_highlight_rules,
            save_highlight_rules,
            get_per_file_rules,
            save_per_file_rules,
            forget_per_file_rules,
            reset_data,
            take_startup_paths,
            check_for_update,
            install_update_now,
            snooze_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
    drop(log_guard);
}

#[cfg(test)]
mod tests {
    use super::*;
    use clog_core::builtin_pattern;

    #[test]
    fn windowed_speed_grid_buckets_relative_to_span() {
        use clog_core::{build_speed_grid, SlowRequestOccurrence};
        // One occurrence at absolute line 8, duration 500ms. With a window
        // [5, 10) (span 5) it relabels to line 3 -> bucket 3 of a 5-bucket grid.
        let occ = SlowRequestOccurrence {
            timestamp_ms: None,
            duration_ms: 500,
            line_index: 3, // already window-relative (8 - 5)
            record_idx: 0,
            dup_count: 1,
            class_method: "GET.index".to_string(),
            raw_path: "/x".to_string(),
        };
        let grid = build_speed_grid(&[occ], 5, 5);
        assert_eq!(grid.buckets[3].count, 1);
        assert_eq!(grid.buckets[3].max_ms, 500);
    }

    /// Build a fresh `OpenedFile` with no content, wired up with the
    /// wsl-dev pattern so a stream of header lines can be appended via
    /// `extend_with_appended`.
    fn fresh_file() -> (OpenedFile, CompiledScanner) {
        let pattern_src = builtin_pattern("wsl-dev").expect("wsl-dev builtin");
        let scanner_kind = ScannerKind::Pattern(pattern_src.to_string());
        let scanner = scanner_kind.compile().expect("compile wsl-dev");
        let file = OpenedFile {
            path: PathBuf::from("test.log"),
            records: Vec::new(),
            record_first_line: Vec::new(),
            line_count: 0,
            bytes: Vec::new(),
            line_offsets: Vec::new(),
            pattern_source: pattern_src.to_string(),
            pattern_name: Some("wsl-dev".to_string()),
            scanner_kind,
            loose: false,
            tail_shutdown: None,
            tail_join: None,
            current_search_id: 0,
            search_cancel: None,
            search_join: None,
            slow_request_cache: None,
            truncate_before: None,
            truncate_after: None,
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

    #[test]
    fn level_minimap_baseline_worst_severity_per_bucket() {
        let (mut file, scanner) = fresh_file();
        let body = concat!(
            "[INFO ] 2026-05-22 16:28:59.246 [main] play - one\n",
            "[ERROR] 2026-05-22 16:28:59.247 [main] play - two\n",
            "[INFO ] 2026-05-22 16:28:59.248 [main] play - three\n",
        );
        extend(&mut file, &scanner, body.as_bytes());
        assert_eq!(file.line_count, 3);

        let payload = build_level_minimap_payload(&file.records, file.line_count, 3);
        assert_eq!(payload.buckets.len(), 3);
        assert_eq!(payload.buckets[0].worst, Level::Info);
        assert_eq!(payload.buckets[1].worst, Level::Error);
        assert_eq!(payload.buckets[2].worst, Level::Info);
        assert_eq!(payload.line_count, 3);
    }

    #[test]
    fn level_minimap_counts_errors_and_warns_per_bucket() {
        let (mut file, scanner) = fresh_file();
        let body = concat!(
            "[WARN ] 2026-05-22 16:28:59.246 [main] play - w\n",
            "[ERROR] 2026-05-22 16:28:59.247 [main] play - e\n",
            "[FATAL] 2026-05-22 16:28:59.248 [main] play - f\n",
            "[INFO ] 2026-05-22 16:28:59.249 [main] play - i\n",
        );
        extend(&mut file, &scanner, body.as_bytes());
        assert_eq!(file.line_count, 4);

        let p = build_level_minimap_payload(&file.records, file.line_count, 1);
        assert_eq!(p.buckets.len(), 1);
        let b = &p.buckets[0];
        assert_eq!(b.worst, Level::Fatal);
        assert_eq!(b.error, 2, "ERROR + FATAL count");
        assert_eq!(b.warn, 1, "WARN count");
        assert_eq!(b.total, 4);
        assert_eq!(p.max_error_warn_sum, 3, "(error + warn) max");
        assert_eq!(p.max_total, 4);
    }

    #[test]
    fn apply_truncate_rejects_inverted_and_counts_window() {
        use std::fmt::Write as _;
        let (mut file, scanner) = fresh_file();
        // 100 single-line records, each its own header line -> line_offset 0..100.
        let mut body = String::new();
        for i in 0..100 {
            writeln!(
                body,
                "[INFO ] 2026-05-22 16:28:59.246 [main] play - line {i}"
            )
            .expect("write to String");
        }
        extend(&mut file, &scanner, body.as_bytes());
        assert_eq!(file.line_count, 100);
        assert_eq!(file.records.len(), 100);

        // Inverted window is rejected and leaves no partial state behind that
        // would let the bad bounds count.
        let err = file
            .apply_truncate(Some(50), Some(20))
            .expect_err("inverted window must be rejected");
        assert!(matches!(err, IpcError::BadPattern { .. }));

        // A valid window [10, 40) keeps 30 physical lines and the 30 records
        // whose first line falls in [10, 40).
        let payload = file
            .apply_truncate(Some(10), Some(40))
            .expect("valid window accepted");
        assert_eq!(payload.before, Some(10));
        assert_eq!(payload.after, Some(40));
        assert_eq!(payload.line_count, 30);
        assert_eq!(payload.record_count, 30);

        // Clearing the window restores the full file.
        let cleared = file.apply_truncate(None, None).expect("clear accepted");
        assert_eq!(cleared.line_count, 100);
        assert_eq!(cleared.record_count, 100);
    }

    #[test]
    fn get_lines_caps_long_line_and_reports_full_len() {
        let (mut file, scanner) = fresh_file();
        let prefix = "[INFO ] 2026-05-22 16:28:59.246 [main] play - ";
        let long_msg = "x".repeat(5000);
        let body = format!("{prefix}{long_msg}\n");
        extend(&mut file, &scanner, body.as_bytes());
        assert_eq!(file.line_count, 1);

        let payload = build_lines_payload(&file, 0, 1).expect("payload");
        let line = &payload.lines[0];
        let real_len = prefix.len() + long_msg.len();
        assert_eq!(
            line.full_len, real_len as u64,
            "full_len is the untruncated length"
        );
        assert_eq!(line.text.len(), LINE_TEXT_CAP, "text capped to the limit");
        assert!(line.text.len() < real_len, "line was actually truncated");
        assert!(line.truncated, "capped line is flagged truncated");
        assert!(
            body.starts_with(&line.text),
            "capped text is a prefix of the line"
        );

        // The uncapped path (get_record_lines / full-record modal) keeps it all.
        let full = build_lines_payload_capped(&file, 0, 1, None).expect("uncapped");
        assert_eq!(full.lines[0].text.len(), real_len, "uncapped is complete");
        assert_eq!(full.lines[0].full_len, real_len as u64);
        assert!(
            !full.lines[0].truncated,
            "uncapped line is not flagged truncated"
        );
    }

    #[test]
    fn get_lines_leaves_short_line_intact() {
        let (mut file, scanner) = fresh_file();
        let body = "[INFO ] 2026-05-22 16:28:59.246 [main] play - short\n";
        extend(&mut file, &scanner, body.as_bytes());
        let payload = build_lines_payload(&file, 0, 1).expect("payload");
        let line = &payload.lines[0];
        assert_eq!(
            line.text,
            "[INFO ] 2026-05-22 16:28:59.246 [main] play - short"
        );
        assert_eq!(line.full_len, line.text.len() as u64);
        assert!(!line.truncated, "short line is not flagged truncated");
    }

    #[test]
    fn multibyte_line_under_cap_is_not_flagged_truncated() {
        // A non-ASCII line whose byte length exceeds its char count must not be
        // mistaken for a truncated line (the modal-affordance regression).
        let (mut file, scanner) = fresh_file();
        let body =
            "[INFO ] 2026-05-22 16:28:59.246 [main] play - \u{00b5}m \u{00d7} 3 \u{2192} ok\n";
        extend(&mut file, &scanner, body.as_bytes());
        let payload = build_lines_payload(&file, 0, 1).expect("payload");
        let line = &payload.lines[0];
        assert!(
            line.full_len > line.text.chars().count() as u64,
            "precondition: more bytes than chars"
        );
        assert!(!line.truncated, "multi-byte line under the cap is intact");
    }

    #[test]
    fn line_window_centres_on_offset_and_clamps() {
        let (mut file, scanner) = fresh_file();
        let prefix = "[INFO ] 2026-05-22 16:28:59.246 [main] play - ";
        let long_msg = "x".repeat(5000);
        let body = format!("{prefix}{long_msg}\n");
        extend(&mut file, &scanner, body.as_bytes());
        let line_len = (prefix.len() + long_msg.len()) as u64;

        // A window centred deep in the message returns a slice around it.
        let w = build_line_window(&file, 0, 4000, 100).expect("window");
        assert_eq!(w.full_len, line_len);
        assert_eq!(w.start, 3900);
        assert_eq!(w.text.len(), 200);
        assert!(w.text.chars().all(|c| c == 'x'));

        // A window past the end clamps to the line length.
        let tail = build_line_window(&file, 0, line_len + 999, 50).expect("window");
        assert_eq!(tail.start, line_len - 50);
        assert_eq!(tail.text.len(), 50);

        // Out-of-range line index errors.
        assert!(matches!(
            build_line_window(&file, 9, 0, 10),
            Err(IpcError::OutOfRange)
        ));
    }

    #[test]
    fn level_minimap_empty_file_zeroes_counts() {
        let (file, _scanner) = fresh_file();
        let p = build_level_minimap_payload(&file.records, file.line_count, 8);
        assert_eq!(p.buckets.len(), 8);
        for b in &p.buckets {
            assert_eq!(b.worst, Level::Unknown);
            assert_eq!(b.error, 0);
            assert_eq!(b.warn, 0);
            assert_eq!(b.total, 0);
        }
        assert_eq!(p.max_error_warn_sum, 0);
        assert_eq!(p.max_total, 0);
    }

    #[test]
    fn level_minimap_multi_line_record_bumps_every_touched_bucket_once() {
        let (mut file, scanner) = fresh_file();
        let body = concat!(
            "[ERROR] 2026-05-22 16:28:59.246 [main] play - boom\n",
            "    at com.example.A.foo(A.java:12)\n",
            "    at com.example.B.bar(B.java:34)\n",
        );
        extend(&mut file, &scanner, body.as_bytes());
        assert_eq!(file.line_count, 3);
        assert_eq!(file.records.len(), 1);

        let p = build_level_minimap_payload(&file.records, file.line_count, 3);
        assert_eq!(p.buckets.len(), 3);
        for (i, b) in p.buckets.iter().enumerate() {
            assert_eq!(b.worst, Level::Error, "bucket {i}");
            assert_eq!(b.error, 1, "bucket {i} - counted once per touched bucket");
            assert_eq!(b.warn, 0, "bucket {i}");
            assert_eq!(b.total, 1, "bucket {i}");
        }
        assert_eq!(p.max_error_warn_sum, 1);
        assert_eq!(p.max_total, 1);
    }

    #[test]
    fn windowed_minimap_relabels_buckets_to_span() {
        // An Error record originally at line 5; window [5,10) relabels it to
        // line 0 over span 5, so a 1-bucket grid must report Error.
        let wb = RecordHeader {
            byte_offset: 0,
            byte_len: 0,
            line_offset: 0,
            line_count: 1,
            level: Level::Error,
            fields: HeaderFields::default(),
        };
        let payload = build_level_minimap_payload(&[wb], 5, 1);
        assert_eq!(payload.buckets[0].worst, Level::Error);
        assert_eq!(payload.buckets[0].error, 1);
    }

    /// Reproduces the symptom the user reported: each tail tick adds one
    /// new line and `get_lines(0, line_count)` must always report content
    /// matching exactly what's been appended -- no empty rows, no stale
    /// "previous tick's content lagging" effect.
    #[test]
    fn appending_lines_one_at_a_time_keeps_get_lines_consistent() {
        let (mut file, scanner) = fresh_file();
        let body = [
            "[INFO ] 2026-05-22 16:28:59.246 [main] play - Starting /var/play/sites/cheesecake\n",
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

    /// Exact-scenario regression: a five-line wsl-dev fixture mixing
    /// WARN/DEBUG/TRACE/ERROR (same shape the user reported a sticky-row
    /// bug against) is appended one line per tick. After every tick:
    ///
    /// * `line_count` and `records.len()` match the tick number.
    /// * Every record so far has `line_count == 1` -- no line is
    ///   silently absorbed as a continuation of the previous record.
    /// * Every line's text reads back exactly.
    /// * `byte_offset_in_record == 0` on every line (each is its own
    ///   record's first line).
    ///
    /// Then the same five lines are also tested when appended in a
    /// single tail batch -- the per-tick and batch paths must agree.
    #[test]
    fn five_line_mixed_levels_each_become_their_own_record() {
        let body = [
            "[WARN ] 2026-05-22 16:28:59.246 [main] play - Starting /var/play/sites/cheesecake\n",
            "[DEBUG] 2026-05-22 16:28:59.390 [main] play - Module crud is available\n",
            "[TRACE] 2026-05-22 16:28:59.391 [main] play - Module secure is available\n",
            "[ERROR] 2026-05-22 16:28:59.392 [main] play - Module secure is available\n",
            "[ERROR] 2026-05-22 16:28:59.393 [main] play - Module secure is available\n",
        ];
        let expected_text: Vec<String> = body.iter().map(|l| l.trim_end().to_string()).collect();
        let expected_levels = [
            Level::Warn,
            Level::Debug,
            Level::Trace,
            Level::Error,
            Level::Error,
        ];

        // --- Per-tick path ---
        let (mut file, scanner) = fresh_file();
        for (tick, payload) in body.iter().enumerate() {
            extend(&mut file, &scanner, payload.as_bytes());
            assert_eq!(
                file.line_count,
                (tick + 1) as u64,
                "tick {tick}: line_count"
            );
            assert_eq!(file.records.len(), tick + 1, "tick {tick}: records.len");
            for (ri, rec) in file.records.iter().enumerate() {
                assert_eq!(
                    rec.line_count, 1,
                    "tick {tick}: record {ri} should be one line, got {}",
                    rec.line_count
                );
                assert_eq!(
                    rec.level, expected_levels[ri],
                    "tick {tick}: record {ri} level"
                );
            }
            let lines =
                build_lines_payload(&file, 0, file.line_count).expect("build_lines_payload");
            assert_eq!(lines.lines.len(), tick + 1);
            for (i, got) in lines.lines.iter().enumerate() {
                assert_eq!(got.text, expected_text[i], "tick {tick} line {i} text");
                assert_eq!(got.line_within_record, 0, "tick {tick} line {i} l-w-r");
                assert_eq!(
                    got.byte_offset_in_record, 0,
                    "tick {tick} line {i} byte_offset_in_record"
                );
                assert!(got.fields.is_some(), "tick {tick} line {i} fields present");
            }
        }

        // --- Single-batch path ---
        let (mut file, scanner) = fresh_file();
        let combined: String = body.concat();
        extend(&mut file, &scanner, combined.as_bytes());
        assert_eq!(file.line_count, 5);
        assert_eq!(file.records.len(), 5);
        for (ri, rec) in file.records.iter().enumerate() {
            assert_eq!(rec.line_count, 1, "batch record {ri} should be one line");
            assert_eq!(rec.level, expected_levels[ri], "batch record {ri} level");
        }
        let lines = build_lines_payload(&file, 0, 5).expect("build_lines_payload");
        for (i, got) in lines.lines.iter().enumerate() {
            assert_eq!(got.text, expected_text[i], "batch line {i} text");
            assert_eq!(got.line_within_record, 0, "batch line {i} l-w-r");
            assert_eq!(got.byte_offset_in_record, 0, "batch line {i} boff");
        }
    }

    /// Mixed scenario: a real-world log has both new header records and
    /// stack-trace continuation lines interleaved. After several ticks:
    ///
    /// * Header lines produce new records.
    /// * Continuation lines extend the previous record's `line_count`
    ///   AND keep their `byte_offset_in_record` > 0.
    /// * `get_lines` walks every physical line and returns the right
    ///   (`record_idx`, `line_within_record`, `byte_offset_in_record`)
    ///   for each, regardless of whether neighbouring lines on the same
    ///   page were appended in different ticks.
    #[test]
    fn header_and_continuation_interleaved_stay_consistent_across_ticks() {
        let (mut file, scanner) = fresh_file();
        // Tick 1: a header, then a continuation. One record, two lines.
        extend(
            &mut file,
            &scanner,
            b"[ERROR] 2026-05-22 16:28:59.246 [main] play - boom\n\tat com.example.Foo.bar(Foo.java:42)\n",
        );
        assert_eq!(file.line_count, 2);
        assert_eq!(file.records.len(), 1);
        assert_eq!(file.records[0].line_count, 2);

        // Tick 2: another continuation, then a fresh header. The
        // continuation must attach to record 0; the header must
        // produce record 1.
        extend(
            &mut file,
            &scanner,
            b"\tat com.example.Foo.baz(Foo.java:17)\n[INFO ] 2026-05-22 16:28:59.300 [main] play - back to normal\n",
        );
        assert_eq!(file.line_count, 4);
        assert_eq!(file.records.len(), 2);
        assert_eq!(
            file.records[0].line_count, 3,
            "stack frames attach to ERROR"
        );
        assert_eq!(file.records[1].line_count, 1, "INFO is its own record");

        let lines = build_lines_payload(&file, 0, 4).expect("build_lines_payload");
        // Line 0: ERROR header.
        assert_eq!(lines.lines[0].line_within_record, 0);
        assert_eq!(lines.lines[0].byte_offset_in_record, 0);
        assert!(lines.lines[0].fields.is_some());
        // Lines 1 and 2: stack continuations of record 0. Their
        // byte_offset_in_record must be > 0 because they're past the
        // record's first line.
        assert_eq!(lines.lines[1].line_within_record, 1);
        assert!(lines.lines[1].byte_offset_in_record > 0);
        assert_eq!(lines.lines[2].line_within_record, 2);
        assert!(lines.lines[2].byte_offset_in_record > lines.lines[1].byte_offset_in_record);
        // Line 3: INFO header starts a new record -> back to 0.
        assert_eq!(lines.lines[3].line_within_record, 0);
        assert_eq!(lines.lines[3].byte_offset_in_record, 0);
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

    // --- Property tests --------------------------------------------------

    use proptest::prelude::*;

    fn level_word(i: u32) -> &'static str {
        // Five-char-padded (matches the wsl-dev pattern's `%-5level`).
        match i % 6 {
            0 => "INFO ",
            1 => "DEBUG",
            2 => "WARN ",
            3 => "ERROR",
            4 => "TRACE",
            _ => "FATAL",
        }
    }

    fn expected_level(i: u32) -> Level {
        match i % 6 {
            0 => Level::Info,
            1 => Level::Debug,
            2 => Level::Warn,
            3 => Level::Error,
            4 => Level::Trace,
            _ => Level::Fatal,
        }
    }

    /// Build a synthetic log buffer of `n_lines` complete wsl-dev records
    /// and return `(bytes, expected_levels)`. Each record is exactly one
    /// line and ends in `\n`.
    fn synth_lines(n_lines: u32) -> (Vec<u8>, Vec<Level>) {
        let mut bytes: Vec<u8> = Vec::new();
        let mut levels: Vec<Level> = Vec::new();
        for i in 0..n_lines {
            let level = level_word(i);
            let line = format!(
                "[{level}] 2026-05-22 16:28:59.{:03} [main] play - synthetic line {i}\n",
                i % 1000
            );
            bytes.extend_from_slice(line.as_bytes());
            levels.push(expected_level(i));
        }
        (bytes, levels)
    }

    /// Apply the tail trim (drop bytes after the last newline) the way
    /// `TailState::poll` does in clog-core. Returns the trimmed slice.
    fn tail_trim(buf: &[u8]) -> &[u8] {
        match buf.iter().rposition(|b| *b == b'\n') {
            Some(p) => &buf[..=p],
            None => &[],
        }
    }

    /// Simulate a tail pipeline: hand the bytes to `extend_with_appended`
    /// in N arbitrary chunks. After every chunk we trim to the last `\n`
    /// (just like `TailState`), so a chunk that ends mid-line holds
    /// the partial bytes until the next chunk completes it. Verifies the
    /// final state -- records, `line_count`, every line's text and level --
    /// is identical to the single-batch path.
    fn run_split_property(n_lines: u32, splits: Vec<u32>) {
        let (full_bytes, expected_levels) = synth_lines(n_lines);
        let total = full_bytes.len();

        // --- Reference: one shot. ---
        let (mut ref_file, ref_scanner) = fresh_file();
        extend(&mut ref_file, &ref_scanner, &full_bytes);

        // --- Streamed: split + tail-trim chunks. ---
        // Normalise + sort splits into a unique, in-range list, and tack
        // on `total` so the trailing bytes always get flushed.
        let mut points: Vec<usize> = splits
            .into_iter()
            .map(|s| (s as usize).min(total))
            .collect();
        points.push(total);
        points.sort_unstable();
        points.dedup();

        let (mut streamed_file, streamed_scanner) = fresh_file();
        // `shipped` tracks what we've handed to extend_with_appended.
        // Each split boundary is the current writer position; bytes
        // between shipped and the boundary that don't end in `\n` get
        // held back until the next chunk completes the line.
        let mut shipped = 0usize;
        for boundary in points {
            let pending = &full_bytes[shipped..boundary];
            let usable = tail_trim(pending);
            if !usable.is_empty() {
                extend(&mut streamed_file, &streamed_scanner, usable);
                shipped += usable.len();
            }
        }
        // After the final chunk we must have shipped everything (the
        // synthetic buffer ends in `\n`).
        assert_eq!(shipped, total, "tail must ship every complete line");

        // --- Invariants ---
        assert_eq!(
            streamed_file.line_count, ref_file.line_count,
            "line_count parity"
        );
        assert_eq!(
            streamed_file.records.len(),
            ref_file.records.len(),
            "records.len parity"
        );
        assert_eq!(
            streamed_file.records.len(),
            n_lines as usize,
            "each synthetic line is its own record"
        );
        for (i, rec) in streamed_file.records.iter().enumerate() {
            assert_eq!(rec.line_count, 1, "record {i} line_count");
            assert_eq!(rec.level, expected_levels[i], "record {i} level");
        }
        // Per-line text + boff parity across the two paths.
        let ref_lines = build_lines_payload(&ref_file, 0, ref_file.line_count).unwrap();
        let streamed_lines =
            build_lines_payload(&streamed_file, 0, streamed_file.line_count).unwrap();
        for i in 0..ref_lines.lines.len() {
            assert_eq!(
                streamed_lines.lines[i].text, ref_lines.lines[i].text,
                "line {i} text parity"
            );
            assert_eq!(
                streamed_lines.lines[i].line_within_record, 0,
                "line {i} l-w-r"
            );
            assert_eq!(
                streamed_lines.lines[i].byte_offset_in_record, 0,
                "line {i} byte_offset_in_record"
            );
        }
    }

    proptest! {
        // Deterministic-ish: a single random seed should hit a wide range of
        // split shapes. The shape is (n_lines, list of split points). We
        // bound n_lines so each case stays fast.
        #[test]
        fn tail_chunking_preserves_record_state(
            n_lines in 1u32..30,
            splits in proptest::collection::vec(0u32..2048, 0..40),
        ) {
            run_split_property(n_lines, splits);
        }
    }

    /// Exact reproduction of the user's bug report: open a file with
    /// four complete wsl-dev lines (all ending in `\n`), then append a
    /// fifth line WITHOUT a trailing `\n`, then later with the `\n`.
    /// This drives the FULL tail+extend pipeline end-to-end (real
    /// `TailState`, real file I/O) so we know whether the empty-row
    /// symptom can be reproduced server-side.
    #[test]
    fn append_without_newline_then_with_newline_matches_user_repro() {
        use clog_core::tail::{TailEvent, TailState};
        use std::fs::OpenOptions;
        use std::io::Write as _;
        use std::path::PathBuf;

        // Spin up a unique temp file.
        let pid = std::process::id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path: PathBuf = std::env::temp_dir().join(format!("clog-repro-{pid}-{ts}.log"));
        let _guard = scopeguard_like(&path);

        // Seed the four initial lines, each terminated with `\n`.
        let seed = "[WARN ] 2026-05-22 16:28:59.246 [main] play - Starting /var/play/sites/cheesecake\n\
                    [DEBUG] 2026-05-22 16:28:59.390 [main] play - Module crud is available (/usr/local/play-1.7.1/modules/crud)\n\
                    [TRACE] 2026-05-22 16:28:59.391 [main] play - Module secure is available (/usr/local/play-1.7.1/modules/secure)\n\
                    [ERROR] 2026-05-22 16:28:59.392 [main] play - Module secure is available (/usr/local/play-1.7.1/modules/secure)\n";
        std::fs::write(&path, seed.as_bytes()).expect("seed file");

        // Open: index + records.
        let (mut file, _scanner_unused) = fresh_file();
        let scanner_kind = file.scanner_kind.clone();
        let scanner = scanner_kind.compile().expect("compile");
        let (mut source, _line_index, records) =
            clog_core::index_file(&path, &scanner).expect("index_file");
        file.path = path.clone();
        file.records = records;
        let bytes = source.read_all().expect("read_all");
        let line_offsets = clog_core::LineIndex::build(std::io::Cursor::new(&bytes))
            .expect("LineIndex")
            .line_offsets;
        let lc = line_offsets.len() as u64;
        file.bytes = bytes;
        file.rebuild_line_caches(lc, line_offsets);

        assert_eq!(file.records.len(), 4, "four records at open");
        assert_eq!(file.line_count, 4, "four lines at open");

        // Start a tail state anchored at the file's current size.
        let mut tail = TailState::new(&path, file.bytes.len() as u64).expect("TailState::new");

        // --- Step 3: append the fifth line WITHOUT trailing `\n`. ---
        let line5_no_nl =
            "[INFO ] 2026-05-22 16:28:59.393 [main] play - Module secure is available (/usr/local/play-1.7.1/modules/secure)";
        OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open append")
            .write_all(line5_no_nl.as_bytes())
            .expect("append no-nl");

        // New contract: tail ships the partial line immediately. The
        // sink (extend) parses the header even from the partial bytes
        // -- the wsl-dev pattern's level/date/thread/logger fields all
        // sit ahead of the message, so a partial message tail still
        // allows a full header parse.
        match tail.poll().expect("poll after no-nl append") {
            TailEvent::Appended { from_offset, bytes } => {
                assert_eq!(from_offset, 413, "consumed cursor");
                let text = String::from_utf8(bytes.clone()).unwrap();
                assert!(
                    text.starts_with("[INFO ]"),
                    "shipped chunk starts with INFO header, got {text:?}"
                );
                assert!(
                    !text.ends_with('\n'),
                    "shipped chunk is partial (no trailing newline), got {text:?}"
                );
                extend(&mut file, &scanner, &bytes);
            }
            other => panic!("expected Appended (partial ships), got {other:?}"),
        }
        // The partial INFO line is now visible.
        assert_eq!(file.records.len(), 5, "INFO record visible while partial");
        assert_eq!(file.line_count, 5);
        assert_eq!(file.records[4].level, Level::Info);
        assert_eq!(file.records[4].line_count, 1);

        // --- Step 4: append the trailing newline. ---
        OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open append")
            .write_all(b"\n")
            .expect("append nl");

        match tail.poll().expect("poll after nl") {
            TailEvent::Appended { bytes, .. } => {
                // Just the `\n` byte -- the partial bytes were shipped
                // last tick and consumed already moved past them.
                assert_eq!(bytes, b"\n", "remainder is exactly the newline");
                extend(&mut file, &scanner, &bytes);
            }
            other => panic!("expected Appended (remainder), got {other:?}"),
        }

        // Final post-condition unchanged: 5 records, 5 lines. The `\n`
        // extends record 4's byte_len but does not push a new line.
        assert_eq!(file.records.len(), 5);
        assert_eq!(file.line_count, 5);
        assert_eq!(file.records[4].line_count, 1);

        let lines = build_lines_payload(&file, 0, file.line_count).expect("build_lines_payload");
        assert!(lines.lines[4].text.starts_with("[INFO ]"));
        assert_eq!(lines.lines[4].line_within_record, 0);
        assert!(lines.lines[4].fields.is_some());
    }

    /// Same flow but the user's editor "saves" by writing all 5 lines in
    /// one atomic step (truncate + write). Drives the rotation path AND
    /// the subsequent extend path.
    #[test]
    fn atomic_save_with_fifth_line_lands_as_five_records() {
        use clog_core::tail::{TailEvent, TailState};
        use std::path::PathBuf;

        let pid = std::process::id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path: PathBuf = std::env::temp_dir().join(format!("clog-atomic-{pid}-{ts}.log"));
        let _guard = scopeguard_like(&path);

        let four_lines = "[WARN ] 2026-05-22 16:28:59.246 [main] play - Starting /var/play/sites/cheesecake\n\
                          [DEBUG] 2026-05-22 16:28:59.390 [main] play - Module crud is available (/usr/local/play-1.7.1/modules/crud)\n\
                          [TRACE] 2026-05-22 16:28:59.391 [main] play - Module secure is available (/usr/local/play-1.7.1/modules/secure)\n\
                          [ERROR] 2026-05-22 16:28:59.392 [main] play - Module secure is available (/usr/local/play-1.7.1/modules/secure)\n";
        std::fs::write(&path, four_lines.as_bytes()).expect("seed");

        let (mut file, _scanner_unused) = fresh_file();
        let scanner_kind = file.scanner_kind.clone();
        let scanner = scanner_kind.compile().expect("compile");
        let (mut source, _li, records) = clog_core::index_file(&path, &scanner).expect("idx");
        file.path = path.clone();
        file.records = records;
        let bytes = source.read_all().expect("read");
        let line_offsets = clog_core::LineIndex::build(std::io::Cursor::new(&bytes))
            .expect("li")
            .line_offsets;
        let lc = line_offsets.len() as u64;
        file.bytes = bytes;
        file.rebuild_line_caches(lc, line_offsets);

        let mut tail = TailState::new(&path, file.bytes.len() as u64).expect("tail");

        // Atomic-style save: write the full new content in one shot,
        // truncating the old file. Same first-256-byte prefix so the
        // tail's head-hash check matches and this looks like a pure
        // append, not a rotation.
        let full_with_fifth = format!(
            "{four_lines}[INFO ] 2026-05-22 16:28:59.393 [main] play - Module secure is available (/usr/local/play-1.7.1/modules/secure)\n"
        );
        std::fs::write(&path, full_with_fifth.as_bytes()).expect("rewrite");

        match tail.poll().expect("poll after rewrite") {
            TailEvent::Appended { bytes, .. } => {
                extend(&mut file, &scanner, &bytes);
            }
            other => panic!("expected Appended (head-hash matches), got {other:?}"),
        }

        assert_eq!(file.records.len(), 5, "five records after atomic save");
        assert_eq!(file.line_count, 5);
        for (i, expected) in [
            Level::Warn,
            Level::Debug,
            Level::Trace,
            Level::Error,
            Level::Info,
        ]
        .iter()
        .enumerate()
        {
            assert_eq!(file.records[i].level, *expected, "record {i} level");
            assert_eq!(file.records[i].line_count, 1, "record {i} is one line");
        }
    }

    /// Hypothesis A for the user's "empty row 5 with the previous
    /// record's colour" report: the seed file has NO trailing `\n` on
    /// line 4, so when an editor saves with the new line 5 appended,
    /// it implicitly closes line 4 with a `\n` first. From the tail's
    /// point of view the appended buffer starts with `\n`, which
    /// `extend_with_appended` turns into an empty continuation of the
    /// previous record (same colour, no text). Asserts the exact
    /// resulting state shape so the bug -- if real -- is visible here.
    #[test]
    fn file_opened_without_trailing_newline_then_appended_produces_continuation_row() {
        use clog_core::tail::{TailEvent, TailState};
        use std::fs::OpenOptions;
        use std::io::Write as _;
        use std::path::PathBuf;

        let pid = std::process::id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path: PathBuf = std::env::temp_dir().join(format!("clog-hypa-{pid}-{ts}.log"));
        let _guard = scopeguard_like(&path);

        // Four lines BUT no trailing `\n` on line 4.
        let seed_no_trailing = "[WARN ] 2026-05-22 16:28:59.246 [main] play - Starting /var/play/sites/cheesecake\n\
                                [DEBUG] 2026-05-22 16:28:59.390 [main] play - Module crud is available (/usr/local/play-1.7.1/modules/crud)\n\
                                [TRACE] 2026-05-22 16:28:59.391 [main] play - Module secure is available (/usr/local/play-1.7.1/modules/secure)\n\
                                [ERROR] 2026-05-22 16:28:59.392 [main] play - Module secure is available (/usr/local/play-1.7.1/modules/secure)";
        std::fs::write(&path, seed_no_trailing.as_bytes()).expect("seed");

        // Open path.
        let (mut file, _) = fresh_file();
        let scanner_kind = file.scanner_kind.clone();
        let scanner = scanner_kind.compile().expect("compile");
        let (mut source, _li, records) = clog_core::index_file(&path, &scanner).expect("idx");
        file.path = path.clone();
        file.records = records;
        let bytes = source.read_all().expect("read");
        let line_offsets = clog_core::LineIndex::build(std::io::Cursor::new(&bytes))
            .expect("li")
            .line_offsets;
        let lc = line_offsets.len() as u64;
        file.bytes = bytes;
        file.rebuild_line_caches(lc, line_offsets);

        // Sanity: LineIndex still counts the partial 4th line.
        assert_eq!(file.line_count, 4, "four lines visible at open");
        assert_eq!(file.records.len(), 4);

        let mut tail = TailState::new(&path, file.bytes.len() as u64).expect("tail");

        // Editor's save: closes line 4 with `\n`, then appends line 5
        // (with trailing `\n` so the editor's "always add final newline"
        // setting matches the common case).
        OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open")
            .write_all(b"\n[INFO ] 2026-05-22 16:28:59.393 [main] play - Module secure is available (/usr/local/play-1.7.1/modules/secure)\n")
            .expect("append");

        match tail.poll().expect("poll") {
            TailEvent::Appended { bytes, .. } => {
                extend(&mut file, &scanner, &bytes);
            }
            other => panic!("expected Appended, got {other:?}"),
        }

        // What the USER intuitively wants: 5 records, last is INFO,
        // each is its own line. What the CODE currently does: line 4
        // ate the stray `\n` as a continuation, so we get 4 records
        // and a phantom empty row 5 with ERROR colour. Pin both
        // behaviours so the diff is plain.
        eprintln!(
            "DEBUG: records.len={} line_count={} record[3].line_count={}",
            file.records.len(),
            file.line_count,
            file.records[3].line_count,
        );
        let lines = build_lines_payload(&file, 0, file.line_count).expect("build_lines_payload");
        for (i, l) in lines.lines.iter().enumerate() {
            eprintln!(
                "  line {i}: record_idx={} l_w_r={} level={:?} text={:?}",
                l.record_idx, l.line_within_record, l.level, l.text
            );
        }

        // Fixed state. The stray `\n` that arrived at the head of the
        // tail buffer was consumed as completion of line 4's partial,
        // not as the start of a new empty line. So there is no phantom
        // row, no continuation on the ERROR record, and the INFO line
        // is record 4 sitting at virtual line 4.
        assert_eq!(file.line_count, 5, "no phantom row");
        assert_eq!(file.records.len(), 5);
        assert_eq!(
            file.records[3].line_count, 1,
            "ERROR record is one line (the stray `\\n` just closed it)"
        );
        assert_eq!(file.records[4].line_count, 1);
        let lines = build_lines_payload(&file, 0, file.line_count).expect("build_lines_payload");
        assert!(lines.lines[3].text.starts_with("[ERROR]"), "row 4 is ERROR");
        assert!(lines.lines[4].text.starts_with("[INFO ]"), "row 5 is INFO");
        assert_eq!(lines.lines[4].level, Level::Info);
        // The fix's invariant: the byte_len of record 3 must include
        // the closing `\n` even though that `\n` arrived in the tail
        // append, not in the initial index.
        let r3 = &file.records[3];
        let r3_last_byte =
            usize::try_from(r3.byte_offset + u64::from(r3.byte_len) - 1).unwrap_or(usize::MAX);
        assert_eq!(
            file.bytes[r3_last_byte], b'\n',
            "record 3 now ends in `\\n`"
        );
    }

    /// Latest user repro: file opened with NO trailing `\n` on line 4,
    /// then a fifth line is added with NO trailing `\n` either.
    /// Symptom: nothing at all updates in the UI.
    ///
    /// Walks the same pipeline as the live app: real file, real
    /// `TailState`, real `extend_with_appended`. Asserts the resulting
    /// state shape so the diff between "current" and "desired"
    /// behaviour is plain.
    #[test]
    fn no_trailing_newline_at_open_then_partial_append_is_invisible() {
        use clog_core::tail::{TailEvent, TailState};
        use std::fs::OpenOptions;
        use std::io::Write as _;
        use std::path::PathBuf;

        let pid = std::process::id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path: PathBuf = std::env::temp_dir().join(format!("clog-partial-{pid}-{ts}.log"));
        let _guard = scopeguard_like(&path);

        let seed_no_trailing = "[WARN ] 2026-05-22 16:28:59.246 [main] play - Starting /var/play/sites/cheesecake\n\
                                [DEBUG] 2026-05-22 16:28:59.390 [main] play - Module crud is available (/usr/local/play-1.7.1/modules/crud)\n\
                                [TRACE] 2026-05-22 16:28:59.391 [main] play - Module secure is available (/usr/local/play-1.7.1/modules/secure)\n\
                                [ERROR] 2026-05-22 16:28:59.392 [main] play - Module secure is available (/usr/local/play-1.7.1/modules/secure)";
        std::fs::write(&path, seed_no_trailing.as_bytes()).expect("seed");

        let (mut file, _) = fresh_file();
        let scanner_kind = file.scanner_kind.clone();
        let scanner = scanner_kind.compile().expect("compile");
        let (mut source, _li, records) = clog_core::index_file(&path, &scanner).expect("idx");
        file.path = path.clone();
        file.records = records;
        let bytes = source.read_all().expect("read");
        let line_offsets = clog_core::LineIndex::build(std::io::Cursor::new(&bytes))
            .expect("li")
            .line_offsets;
        let lc = line_offsets.len() as u64;
        file.bytes = bytes;
        file.rebuild_line_caches(lc, line_offsets);

        assert_eq!(file.line_count, 4, "four lines visible at open");
        assert_eq!(file.records.len(), 4);

        let mut tail = TailState::new(&path, file.bytes.len() as u64).expect("tail");

        // The editor's save: closes line 4's partial with `\n`, appends
        // line 5 content WITHOUT trailing `\n`.
        OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open")
            .write_all(b"\n[INFO ] 2026-05-22 16:28:59.393 [main] play - Module secure is available (/usr/local/play-1.7.1/modules/secure)")
            .expect("append");

        // The tail loop fires. Apply whatever it ships.
        match tail.poll().expect("poll") {
            TailEvent::Appended { bytes, .. } => {
                extend(&mut file, &scanner, &bytes);
            }
            TailEvent::NoChange => {
                // tail held everything back -- no UI update.
            }
            TailEvent::Rotated => panic!("not a rotation"),
        }

        eprintln!(
            "DEBUG: records.len={} line_count={} record[3].line_count={}",
            file.records.len(),
            file.line_count,
            file.records[3].line_count,
        );
        let lines = build_lines_payload(&file, 0, file.line_count).expect("build_lines_payload");
        for (i, l) in lines.lines.iter().enumerate() {
            eprintln!(
                "  line {i}: record_idx={} l_w_r={} level={:?} text={:?}",
                l.record_idx, l.line_within_record, l.level, l.text
            );
        }

        // Fixed state: the partial INFO line is visible immediately,
        // even without a trailing `\n`. Tail ships it, extend parses
        // the header from the partial bytes and pushes a record.
        assert_eq!(file.line_count, 5);
        assert_eq!(file.records.len(), 5);
        assert_eq!(file.records[4].level, Level::Info);
        assert_eq!(file.records[4].line_count, 1);
        assert!(lines.lines[4].text.starts_with("[INFO ]"));
        assert!(lines.lines[4].text.ends_with("/modules/secure)"));
    }

    /// Hypothesis B for the user's report: the editor or some other tool
    /// inserts a literal blank line between line 4 and line 5. The
    /// appended buffer is `\n[INFO ]...\n`, which tail ships in one
    /// shot. We pin the resulting state so the differing semantics
    /// (continuation vs new record) are visible.
    #[test]
    fn blank_line_then_real_line_appended_in_one_shot() {
        use clog_core::tail::{TailEvent, TailState};
        use std::fs::OpenOptions;
        use std::io::Write as _;
        use std::path::PathBuf;

        let pid = std::process::id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path: PathBuf = std::env::temp_dir().join(format!("clog-hypb-{pid}-{ts}.log"));
        let _guard = scopeguard_like(&path);

        let four_lines = "[WARN ] 2026-05-22 16:28:59.246 [main] play - Starting /var/play/sites/cheesecake\n\
                          [DEBUG] 2026-05-22 16:28:59.390 [main] play - Module crud is available (/usr/local/play-1.7.1/modules/crud)\n\
                          [TRACE] 2026-05-22 16:28:59.391 [main] play - Module secure is available (/usr/local/play-1.7.1/modules/secure)\n\
                          [ERROR] 2026-05-22 16:28:59.392 [main] play - Module secure is available (/usr/local/play-1.7.1/modules/secure)\n";
        std::fs::write(&path, four_lines.as_bytes()).expect("seed");

        let (mut file, _) = fresh_file();
        let scanner_kind = file.scanner_kind.clone();
        let scanner = scanner_kind.compile().expect("compile");
        let (mut source, _li, records) = clog_core::index_file(&path, &scanner).expect("idx");
        file.path = path.clone();
        file.records = records;
        let bytes = source.read_all().expect("read");
        let line_offsets = clog_core::LineIndex::build(std::io::Cursor::new(&bytes))
            .expect("li")
            .line_offsets;
        let lc = line_offsets.len() as u64;
        file.bytes = bytes;
        file.rebuild_line_caches(lc, line_offsets);

        let mut tail = TailState::new(&path, file.bytes.len() as u64).expect("tail");

        // Append a STRAY blank line first (just `\n`), then the real
        // line 5. Done in one append so tail ships them together.
        OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open")
            .write_all(b"\n[INFO ] 2026-05-22 16:28:59.393 [main] play - Module secure is available (/usr/local/play-1.7.1/modules/secure)\n")
            .expect("append");

        match tail.poll().expect("poll") {
            TailEvent::Appended { bytes, .. } => {
                extend(&mut file, &scanner, &bytes);
            }
            other => panic!("expected Appended, got {other:?}"),
        }

        eprintln!(
            "DEBUG: records.len={} line_count={}",
            file.records.len(),
            file.line_count
        );
        let lines = build_lines_payload(&file, 0, file.line_count).expect("build_lines_payload");
        for (i, l) in lines.lines.iter().enumerate() {
            eprintln!(
                "  line {i}: record_idx={} l_w_r={} level={:?} text={:?}",
                l.record_idx, l.line_within_record, l.level, l.text
            );
        }

        // line_count grows by 2 (the empty line + the INFO line). The
        // empty line is currently absorbed as a continuation of record
        // 3 (the previous ERROR), and the INFO line becomes record 4.
        // That's exactly the user's "row 5 empty/red, row 6 INFO"
        // symptom.
        assert_eq!(file.line_count, 6, "two new lines visible");
        assert_eq!(
            file.records.len(),
            5,
            "the blank line is a continuation, so only one new record"
        );
        assert_eq!(
            file.records[3].line_count, 2,
            "ERROR record has a phantom blank continuation"
        );
        assert_eq!(file.records[4].level, Level::Info);
    }

    /// Tiny RAII helper to clean up the temp file even when the test
    /// panics. `scopeguard` would do this; rolling our own keeps the
    /// dep list tight.
    fn scopeguard_like(path: &std::path::Path) -> impl Drop {
        struct G(std::path::PathBuf);
        impl Drop for G {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        G(path.to_path_buf())
    }

    /// Explicit corner cases the property test could in principle hit but
    /// is hard to guarantee:
    ///
    /// * A chunk boundary that lands EXACTLY on a `\n` (whole-line ship).
    /// * A chunk boundary in the middle of a line (partial-line hold).
    /// * A chunk boundary right after a `\n` (zero new bytes after).
    /// * A series of single-byte chunks (degenerate split case).
    #[test]
    fn tail_chunking_explicit_corner_cases() {
        // Whole-line chunks.
        run_split_property(5, vec![]);
        // Mid-line splits: chosen to fall inside a level field, inside
        // a date, inside the message body.
        run_split_property(5, vec![3, 12, 40, 70]);
        // Splits right at the `\n` boundary of each line (84 chars/line
        // approx, but the exact value isn't sensitive).
        run_split_property(3, vec![84, 168]);
        // Byte-by-byte streaming: every single offset is a split.
        let (bytes, _) = synth_lines(4);
        let single_byte_splits: Vec<u32> =
            (1..u32::try_from(bytes.len()).unwrap_or(u32::MAX)).collect();
        run_split_property(4, single_byte_splits);
    }

    #[test]
    fn markers_flag_core_plugin_load_records() {
        let (mut file, scanner) = fresh_file();
        let body = concat!(
            "[INFO ] 2026-05-22 16:28:59.246 [main] play - boot\n",
            "[INFO ] 2026-05-22 16:28:59.247 [main] play - Core Plugin Load: starting\n",
            "[INFO ] 2026-05-22 16:28:59.248 [main] play - serving\n",
            "[INFO ] 2026-05-22 16:30:01.000 [main] play - Core Plugin Load (round 2)\n",
        );
        extend(&mut file, &scanner, body.as_bytes());
        assert_eq!(file.line_count, 4);

        let markers = scan_markers(
            &file.records,
            &file.bytes,
            &file.line_offsets,
            BUILTIN_MARKER_RULES,
        );
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].kind, MarkerKind::Restart);
        assert_eq!(markers[0].line_index, 1);
        assert_eq!(markers[0].record_idx, 1);
        assert_eq!(markers[1].kind, MarkerKind::Restart);
        assert_eq!(markers[1].line_index, 3);
        assert_eq!(markers[1].record_idx, 3);
    }

    #[test]
    fn markers_ignore_continuation_lines_carrying_the_needle() {
        // A stack trace continuation mentioning the needle must NOT be
        // counted - markers attach to the record's first physical line only.
        let (mut file, scanner) = fresh_file();
        let body = concat!(
            "[ERROR] 2026-05-22 16:28:59.246 [main] play - boom\n",
            "    at com.example.Core Plugin Load.foo(X.java:1)\n",
        );
        extend(&mut file, &scanner, body.as_bytes());
        assert_eq!(file.records.len(), 1);
        let markers = scan_markers(
            &file.records,
            &file.bytes,
            &file.line_offsets,
            BUILTIN_MARKER_RULES,
        );
        assert!(markers.is_empty(), "continuation match must not flag");
    }

    #[test]
    fn markers_handle_empty_inputs() {
        let (file, _scanner) = fresh_file();
        let markers = scan_markers(
            &file.records,
            &file.bytes,
            &file.line_offsets,
            BUILTIN_MARKER_RULES,
        );
        assert!(markers.is_empty());
    }

    #[test]
    fn markers_one_marker_per_record_even_with_multiple_matching_rules() {
        // Synthetic rule set where two rules match the same line. The
        // record should still produce exactly one MarkerRef (the first
        // rule wins).
        let (mut file, scanner) = fresh_file();
        let body = "[INFO ] 2026-05-22 16:28:59.246 [main] play - Core Plugin Load and friends\n";
        extend(&mut file, &scanner, body.as_bytes());
        let rules = &[
            MarkerRule {
                kind: MarkerKind::Restart,
                needle: "Core Plugin Load",
            },
            MarkerRule {
                kind: MarkerKind::Restart,
                needle: "Plugin Load",
            },
        ];
        let markers = scan_markers(&file.records, &file.bytes, &file.line_offsets, rules);
        assert_eq!(markers.len(), 1);
    }

    #[test]
    fn slow_request_smoke_against_prod_fixture() {
        use std::path::Path;
        let path = Path::new("..")
            .join("..")
            .join("research")
            .join("cheesecake-prod.log");
        if !path.exists() {
            return;
        }
        let pattern_src = clog_core::builtin_pattern("prod").expect("prod builtin");
        let scanner = clog_core::CompiledPattern::compile(pattern_src).expect("compiles");
        let (mut source, line_index, records) =
            clog_core::index_file(&path, &scanner).expect("indexes");
        let bytes = source.read_all().expect("read_all");
        let line_offsets = line_index.line_offsets;
        let summary = clog_core::extract_slow_requests(
            &records,
            &bytes,
            &line_offsets,
            clog_core::PathMode::Normalised,
            extract_record_timestamp_ms,
        );
        assert!(
            summary.total_hits > 0,
            "prod fixture must contain at least one slow request"
        );
        assert!(summary.entries.iter().any(|e| e.path.contains("preflight")));
    }

    #[test]
    fn update_settings_rejects_invalid_thresholds() {
        let result = (|| -> Result<(), IpcError> {
            let mut s = persistence::Settings::default();
            let patch_pair = clog_core::SlowRequestThresholds {
                fast_ms: 1000,
                slow_ms: 1000,
            };
            match clog_core::SlowRequestThresholds::new(patch_pair.fast_ms, patch_pair.slow_ms) {
                Some(valid) => s.slow_request_thresholds = Some(valid),
                None => {
                    return Err(IpcError::BadPattern {
                        message: "rejected".into(),
                    })
                }
            }
            Ok(())
        })();
        assert!(result.is_err());
    }

    #[test]
    fn settings_patch_captures_collapse_records_default() {
        // Regression: the patch struct used to omit this field, so serde
        // silently dropped it and the global default never changed.
        let patch: SettingsPatch = serde_json::from_str(r#"{"collapse_records_default":"errors"}"#)
            .expect("patch with collapse_records_default decodes");
        assert_eq!(patch.collapse_records_default.as_deref(), Some("errors"));
    }
}
