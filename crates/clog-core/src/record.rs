//! Record header type and scanner.
//!
//! P2 ships exactly one hardcoded scanner: the wsl-oink pattern
//! `[%-5level] %d{...} [%t] %c{1} - %msg%n`. Pattern generalisation lands
//! in P3.

use serde::Serialize;

use crate::index::LineIndex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
    Off,
    All,
    /// Records that did not parse as a known header (e.g. files whose pattern
    /// does not match the active scanner). Treated as a single record per
    /// orphan run for now; P3 replaces this with proper auto-detect.
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordHeader {
    /// Byte offset of the first byte of this record in the source file.
    pub byte_offset: u64,
    /// Byte length of the whole record (header + continuations + trailing
    /// `\n`). `header[i].byte_offset + header[i].byte_len ==
    /// header[i+1].byte_offset` for all adjacent pairs.
    pub byte_len: u32,
    /// Index into `LineIndex::line_offsets` of the first physical line of
    /// this record.
    pub line_offset: u32,
    /// Number of physical lines this record spans (`>= 1`).
    pub line_count: u32,
    pub level: Level,
}

pub trait RecordScanner {
    /// If `line` (without trailing `\n`/`\r\n`) is a record header, return
    /// its parsed level. Otherwise `None`, meaning it is a continuation of
    /// the previous record.
    fn classify_header(&self, line: &[u8]) -> Option<Level>;
}

/// Hardcoded scanner for the wsl-oink production pattern:
/// `[%-5level] %d{yyyy-MM-dd HH:mm:ss.SSS} [%t] %c{1} - %msg%n`.
///
/// A line is a header iff its first 8 bytes are `[`, a 5-char padded level
/// (`INFO `, `WARN `, `ERROR`, ...), `]`, ` `.
#[derive(Debug, Default, Clone, Copy)]
pub struct WslOinkScanner;

impl RecordScanner for WslOinkScanner {
    fn classify_header(&self, line: &[u8]) -> Option<Level> {
        if line.len() < 8 || line[0] != b'[' || line[6] != b']' || line[7] != b' ' {
            return None;
        }
        match &line[1..6] {
            b"TRACE" => Some(Level::Trace),
            b"DEBUG" => Some(Level::Debug),
            b"INFO " => Some(Level::Info),
            b"WARN " => Some(Level::Warn),
            b"ERROR" => Some(Level::Error),
            b"FATAL" => Some(Level::Fatal),
            b"OFF  " => Some(Level::Off),
            b"ALL  " => Some(Level::All),
            _ => None,
        }
    }
}

fn strip_eol(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    if end > 0 && line[end - 1] == b'\n' {
        end -= 1;
    }
    if end > 0 && line[end - 1] == b'\r' {
        end -= 1;
    }
    &line[..end]
}

/// Walk every physical line, classify it against `scanner`, and produce one
/// `RecordHeader` per logical record. Continuation lines extend the
/// preceding record's `line_count` and `byte_len`.
///
/// `bytes` must be the full file content and `line_index` must have been
/// built from the same bytes.
#[must_use]
pub fn scan_records<S: RecordScanner>(
    scanner: &S,
    line_index: &LineIndex,
    bytes: &[u8],
) -> Vec<RecordHeader> {
    let line_count = line_index.line_offsets.len();
    let mut headers: Vec<RecordHeader> = Vec::new();
    if line_count == 0 {
        return headers;
    }

    for i in 0..line_count {
        let start_u64 = line_index.line_offsets[i];
        let start = usize::try_from(start_u64).unwrap_or(usize::MAX);
        let end = if i + 1 < line_count {
            usize::try_from(line_index.line_offsets[i + 1]).unwrap_or(usize::MAX)
        } else {
            bytes.len()
        };
        let line = strip_eol(&bytes[start..end]);

        if let Some(level) = scanner.classify_header(line) {
            headers.push(RecordHeader {
                byte_offset: start_u64,
                byte_len: 0,
                line_offset: u32::try_from(i).unwrap_or(u32::MAX),
                line_count: 1,
                level,
            });
        } else if let Some(last) = headers.last_mut() {
            last.line_count = last.line_count.saturating_add(1);
        } else {
            // Orphan continuation before any header: synthesize an Unknown
            // record so byte coverage stays watertight.
            headers.push(RecordHeader {
                byte_offset: start_u64,
                byte_len: 0,
                line_offset: u32::try_from(i).unwrap_or(u32::MAX),
                line_count: 1,
                level: Level::Unknown,
            });
        }
    }

    // Fill byte_len so adjacent records meet exactly and the last record
    // runs to file_size.
    let total = line_index.file_size;
    let n = headers.len();
    for i in 0..n {
        let end = if i + 1 < n {
            headers[i + 1].byte_offset
        } else {
            total
        };
        headers[i].byte_len = u32::try_from(end - headers[i].byte_offset).unwrap_or(u32::MAX);
    }

    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx_for(bytes: &[u8]) -> LineIndex {
        LineIndex::build(std::io::Cursor::new(bytes)).unwrap()
    }

    #[test]
    fn classifies_padded_levels() {
        let s = WslOinkScanner;
        assert_eq!(s.classify_header(b"[INFO ] 2026..."), Some(Level::Info));
        assert_eq!(s.classify_header(b"[WARN ] 2026..."), Some(Level::Warn));
        assert_eq!(s.classify_header(b"[ERROR] 2026..."), Some(Level::Error));
        assert_eq!(s.classify_header(b"[DEBUG] 2026..."), Some(Level::Debug));
        assert_eq!(s.classify_header(b"[TRACE] 2026..."), Some(Level::Trace));
    }

    #[test]
    fn rejects_non_headers() {
        let s = WslOinkScanner;
        assert_eq!(s.classify_header(b"  at com.foo.Bar(Bar.java:42)"), None);
        assert_eq!(s.classify_header(b"[NOPE ] x"), None);
        assert_eq!(s.classify_header(b""), None);
        assert_eq!(s.classify_header(b"[INFO]x"), None);
    }

    #[test]
    fn single_record_no_continuation() {
        let bytes = b"[INFO ] 2026-01-01 00:00:00.000 [t] play - hi\n";
        let li = idx_for(bytes);
        let recs = scan_records(&WslOinkScanner, &li, bytes);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].byte_offset, 0);
        assert_eq!(u64::from(recs[0].byte_len), bytes.len() as u64);
        assert_eq!(recs[0].line_count, 1);
        assert_eq!(recs[0].level, Level::Info);
    }

    #[test]
    fn record_with_continuation_lines() {
        let bytes = b"[ERROR] 2026-01-01 00:00:00.000 [t] play - boom\n  at A.b(A.java:1)\n  at C.d(C.java:2)\n[INFO ] 2026-01-01 00:00:01.000 [t] play - ok\n";
        let li = idx_for(bytes);
        let recs = scan_records(&WslOinkScanner, &li, bytes);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].level, Level::Error);
        assert_eq!(recs[0].line_count, 3);
        assert_eq!(recs[1].level, Level::Info);
        assert_eq!(recs[1].line_count, 1);
        // Byte coverage is watertight.
        assert_eq!(
            recs[0].byte_offset + u64::from(recs[0].byte_len),
            recs[1].byte_offset
        );
        assert_eq!(
            recs[1].byte_offset + u64::from(recs[1].byte_len),
            li.file_size
        );
    }

    #[test]
    fn orphan_continuation_becomes_unknown_record() {
        let bytes = b"   leading garbage\n[INFO ] 2026-01-01 00:00:00.000 [t] play - hi\n";
        let li = idx_for(bytes);
        let recs = scan_records(&WslOinkScanner, &li, bytes);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].level, Level::Unknown);
        assert_eq!(recs[1].level, Level::Info);
    }
}
