#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use clog_core::{summarise_file, CoreError, FileSummary};
use serde::Serialize;

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum IpcError {
    #[error("{message}")]
    Io { message: String, path: String },
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

#[tauri::command]
fn open_file(path: String) -> Result<FileSummary, IpcError> {
    summarise_file(PathBuf::from(path)).map_err(Into::into)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![open_file])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
