//! Clog engine. No Tauri deps.

use std::io;
use std::path::PathBuf;

use serde::Serialize;
use thiserror::Error;

pub mod index;
pub mod pattern;
pub mod record;
pub mod regex_scanner;
pub mod source;
pub mod tail;

pub use index::LineIndex;
pub use pattern::{
    auto_detect, builtin_pattern, CompiledPattern, DateAtom, DateFormat, HeaderFields,
    ParsedHeader, PatternError, PatternWarning, Token, BUILTIN_PATTERNS,
};
pub use record::{scan_records, Level, RecordHeader, RecordScanner};
pub use regex_scanner::{RegexScanner, RegexScannerError};
pub use source::{LineSource, StreamedFile};
pub use tail::{TailEvent, TailState, DEFAULT_POLL_INTERVAL_MS, HEAD_HASH_BYTES};

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

/// Read up to `cap_bytes` of `path`, split into physical lines (without
/// trailing EOL), and return them. Used by auto-detect and the UI's
/// pattern-test readout.
///
/// # Errors
///
/// Returns `CoreError::Io` if the file cannot be opened or read.
pub fn sample_lines(path: impl Into<PathBuf>, cap_bytes: usize) -> Result<Vec<Vec<u8>>, CoreError> {
    use std::io::Read;
    let path: PathBuf = path.into();
    let mut file = std::fs::File::open(&path).map_err(|source| CoreError::Io {
        path: path.clone(),
        source,
    })?;
    let mut buf = vec![0u8; cap_bytes];
    let n = file.read(&mut buf).map_err(|source| CoreError::Io {
        path: path.clone(),
        source,
    })?;
    buf.truncate(n);
    // Drop a trailing partial line so we never feed a half-line to the matcher.
    let last_nl = buf.iter().rposition(|b| *b == b'\n');
    let usable = match last_nl {
        Some(p) => &buf[..=p],
        None => &buf[..],
    };
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut start = 0;
    for (i, b) in usable.iter().enumerate() {
        if *b == b'\n' {
            let mut end = i;
            if end > start && usable[end - 1] == b'\r' {
                end -= 1;
            }
            out.push(usable[start..end].to_vec());
            start = i + 1;
        }
    }
    Ok(out)
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
        let scanner = CompiledPattern::compile(BUILTIN_PATTERNS[0].1).expect("compile");
        let (source, line_index, records) = index_file(path, &scanner).expect("index_file");
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
        assert!(records.len() <= line_index.line_count());
    }

    /// P3: auto-detect must choose wsl-oink for `solopress-wsl-oink.out`.
    #[test]
    fn auto_detect_chooses_wsl_oink_for_sample() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../research/solopress-wsl-oink.out"
        );
        if !Path::new(path).exists() {
            eprintln!("skipping: fixture {path} not present");
            return;
        }
        let lines = sample_lines(path, 64 * 1024).expect("sample");
        let refs: Vec<&[u8]> = lines.iter().map(Vec::as_slice).collect();
        let (name, _, score) = auto_detect(refs).expect("detects something");
        assert_eq!(name, "wsl-oink");
        // The fixture is a startup log peppered with stack-trace
        // continuation lines, so the absolute score is well under 1. The
        // important assertion is that wsl-oink wins; we just sanity-check
        // it dominates noise.
        assert!(score > 0.4, "score {score} too low to be a confident pick");
    }

    /// P3: auto-detect must choose prod for the prod sample.
    #[test]
    fn auto_detect_chooses_prod_for_sample() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../research/solopress-prod.log"
        );
        if !Path::new(path).exists() {
            eprintln!("skipping: fixture {path} not present");
            return;
        }
        let lines = sample_lines(path, 64 * 1024).expect("sample");
        let refs: Vec<&[u8]> = lines.iter().map(Vec::as_slice).collect();
        let (name, _, score) = auto_detect(refs).expect("detects something");
        assert_eq!(name, "prod");
        assert!(score > 0.9, "score {score} should dominate");
    }
}
