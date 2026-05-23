//! Clog engine. No Tauri deps.
//!
//! P1 surface: just enough to summarise a file (size + line count) so the app
//! shell can prove its IPC plumbing works end-to-end.

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, Serialize)]
pub struct FileSummary {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub line_count: u64,
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("io error opening {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Open the file at `path` and return a summary.
///
/// Counts physical lines by scanning the byte stream once. This is the P1
/// path; later phases replace this with the persistent line index.
///
/// # Errors
///
/// Returns `CoreError::Io` if the file cannot be opened or read.
pub fn summarise_file(path: impl AsRef<Path>) -> Result<FileSummary, CoreError> {
    let path = path.as_ref();
    let file = File::open(path).map_err(|source| CoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let size_bytes = file
        .metadata()
        .map_err(|source| CoreError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();

    let mut reader = BufReader::new(file);
    let mut line_count: u64 = 0;
    let mut buf = Vec::with_capacity(64 * 1024);
    loop {
        buf.clear();
        let n = reader
            .read_until(b'\n', &mut buf)
            .map_err(|source| CoreError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        if n == 0 {
            break;
        }
        line_count += 1;
    }

    Ok(FileSummary {
        path: path.to_path_buf(),
        size_bytes,
        line_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: the prod sample's line count is stable. If it changes,
    /// either the fixture moved or the line counter regressed.
    #[test]
    fn solopress_prod_line_count_is_stable() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../research/solopress-prod.log"
        );
        if !std::path::Path::new(path).exists() {
            eprintln!("skipping: fixture {path} not present");
            return;
        }
        let summary = summarise_file(path).expect("summarise");
        assert_eq!(summary.line_count, 74_921);
    }
}
