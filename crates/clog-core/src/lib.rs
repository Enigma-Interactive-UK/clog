//! Clog engine. No Tauri deps.

use std::io;
use std::path::PathBuf;

use serde::Serialize;
use thiserror::Error;

pub mod index;
pub mod record;
pub mod source;

pub use index::LineIndex;
pub use record::{scan_records, Level, RecordHeader, RecordScanner, WslOinkScanner};
pub use source::{LineSource, StreamedFile};

#[derive(Debug, Clone, Serialize)]
pub struct FileSummary {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub line_count: u64,
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("io error on {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Open `path`, build a `LineIndex` and a `RecordHeader` array using the
/// supplied scanner, and return the assembled state.
///
/// # Errors
///
/// Returns `CoreError::Io` if the file cannot be opened or read.
pub fn index_file<S: RecordScanner>(
    path: impl Into<PathBuf>,
    scanner: &S,
) -> Result<(StreamedFile, LineIndex, Vec<RecordHeader>), CoreError> {
    let mut source = StreamedFile::open(path)?;
    let bytes = source.read_all()?;
    let line_index =
        LineIndex::build(std::io::Cursor::new(&bytes)).map_err(|source_err| CoreError::Io {
            path: source.path().to_path_buf(),
            source: source_err,
        })?;
    let records = scan_records(scanner, &line_index, &bytes);
    Ok((source, line_index, records))
}

/// Lightweight summary still used by the P1 smoke test and the open-file
/// IPC's quick-look payload.
///
/// # Errors
///
/// Returns `CoreError::Io` if the file cannot be opened or read.
pub fn summarise_file(path: impl Into<PathBuf>) -> Result<FileSummary, CoreError> {
    let path = path.into();
    let mut source = StreamedFile::open(&path)?;
    let line_index = source.build_line_index()?;
    Ok(FileSummary {
        path,
        size_bytes: source.file_size(),
        line_count: line_index.line_count() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Smoke test: the prod sample's line count is stable. If it changes,
    /// either the fixture moved or the line counter regressed.
    #[test]
    fn solopress_prod_line_count_is_stable() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../research/solopress-prod.log"
        );
        if !Path::new(path).exists() {
            eprintln!("skipping: fixture {path} not present");
            return;
        }
        let summary = summarise_file(path).expect("summarise");
        assert_eq!(summary.line_count, 74_921);
    }

    /// Integration test for P2: scan the wsl-oink sample and assert that
    /// adjacent records meet exactly, the last record runs to EOF, and the
    /// first/last record byte offsets match the file shape.
    #[test]
    fn solopress_wsl_oink_record_coverage_is_watertight() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../research/solopress-wsl-oink.out"
        );
        if !Path::new(path).exists() {
            eprintln!("skipping: fixture {path} not present");
            return;
        }
        let (source, line_index, records) = index_file(path, &WslOinkScanner).expect("index_file");
        assert!(!records.is_empty(), "expected at least one record");
        assert_eq!(records[0].byte_offset, 0, "first record starts at 0");

        for pair in records.windows(2) {
            assert_eq!(
                pair[0].byte_offset + u64::from(pair[0].byte_len),
                pair[1].byte_offset,
                "records {} and next must meet exactly",
                pair[0].byte_offset
            );
        }

        let last = records.last().unwrap();
        assert_eq!(
            last.byte_offset + u64::from(last.byte_len),
            source.file_size(),
            "last record runs to EOF"
        );
        assert_eq!(line_index.file_size, source.file_size());

        // Sanity: the file is 386 lines and most records are single-line
        // headers, so there should be fewer records than lines.
        assert!(records.len() <= line_index.line_count());
    }
}
