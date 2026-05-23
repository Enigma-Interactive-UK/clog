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

use clog_core::{index_file, CoreError, LineSource, RecordHeader, WslOinkScanner};
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

struct OpenedFile {
    path: PathBuf,
    records: Vec<RecordHeader>,
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

#[tauri::command]
fn open_file(state: State<'_, AppState>, path: String) -> Result<OpenedFilePayload, IpcError> {
    let (source, line_index, records) = index_file(PathBuf::from(&path), &WslOinkScanner)?;
    let file_id = state.next_id.fetch_add(1, Ordering::Relaxed);
    let payload = OpenedFilePayload {
        file_id,
        path: source.path().to_path_buf(),
        size_bytes: source.file_size(),
        line_count: line_index.line_count() as u64,
        record_count: records.len() as u64,
    };
    let opened = OpenedFile {
        path: source.path().to_path_buf(),
        records,
    };
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

    let bytes = read_range(&file.path, base_offset, len).map_err(|source| IpcError::Io {
        message: source.to_string(),
        path: file.path.display().to_string(),
    })?;

    Ok(RecordsPayload {
        start,
        base_offset,
        headers: slice.to_vec(),
        text: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

fn read_range(path: &Path, start: u64, len: usize) -> std::io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

#[tauri::command]
fn close_file(state: State<'_, AppState>, file_id: u64) {
    state
        .files
        .lock()
        .expect("files mutex poisoned")
        .remove(&file_id);
}

fn main() {
    tauri::Builder::default()
        .manage(AppState::default())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![open_file, get_records, close_file])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
