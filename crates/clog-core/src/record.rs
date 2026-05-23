//! Record header type and scanner.
//!
//! P3: scanners are now produced from a compiled `PatternLayout` (or a regex
//! escape hatch). The hardcoded `WslOinkScanner` from P2 has been replaced
//! with `CompiledPattern` impl of `RecordScanner`.

use serde::{Deserialize, Serialize};

use crate::index::LineIndex;
use crate::pattern::{CompiledPattern, HeaderFields, ParsedHeader};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    /// does not match the active scanner) or lines that precede the first
    /// real header.
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecordHeader {
    /// Byte offset of the first byte of this record in the source file.
    pub byte_offset: u64,
    /// Byte length of the whole record (header + continuations + trailing
    /// `\n`).
    pub byte_len: u32,
    /// Index into `LineIndex::line_offsets` of the first physical line of
    /// this record.
    pub line_offset: u32,
    /// Number of physical lines this record spans (`>= 1`).
    pub line_count: u32,
    pub level: Level,
    /// Byte ranges *within the first line of this record* (relative to the
    /// line's first byte, not the file). Axis-1 styling references these.
    pub fields: HeaderFields,
}

pub trait RecordScanner {
    /// Try to parse `line` (no trailing newline) as a record header. Returns
    /// the parsed level + field spans on success, or `None` if `line` is a
    /// continuation of the previous record.
    fn try_parse_header(&self, line: &[u8]) -> Option<ParsedHeader>;
}

impl RecordScanner for CompiledPattern {
    fn try_parse_header(&self, line: &[u8]) -> Option<ParsedHeader> {
        Self::try_parse_header(self, line)
    }
}

/// Wraps any `RecordScanner` so that `try_parse_header` always returns
/// `Some(...)`. When the inner scanner says "this isn't a header line",
/// `LooseScanner` synthesises an `Unknown`-level header with empty fields
/// instead. The effect is that `scan_records` never merges lines into a
/// preceding record's `line_count` -- every physical line becomes its own
/// `RecordHeader`. Used when the active pattern is not a confidently
/// detected builtin, so the assumption "lines that don't match are
/// continuations of the previous record" is unsafe.
pub struct LooseScanner<'a, S: ?Sized> {
    pub inner: &'a S,
}

impl<'a, S: RecordScanner + ?Sized> LooseScanner<'a, S> {
    #[must_use]
    pub fn new(inner: &'a S) -> Self {
        Self { inner }
    }
}

impl<S: RecordScanner + ?Sized> RecordScanner for LooseScanner<'_, S> {
    fn try_parse_header(&self, line: &[u8]) -> Option<ParsedHeader> {
        Some(self.inner.try_parse_header(line).unwrap_or(ParsedHeader {
            level: Level::Unknown,
            fields: HeaderFields::default(),
        }))
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

        if let Some(parsed) = scanner.try_parse_header(line) {
            headers.push(RecordHeader {
                byte_offset: start_u64,
                byte_len: 0,
                line_offset: u32::try_from(i).unwrap_or(u32::MAX),
                line_count: 1,
                level: parsed.level,
                fields: parsed.fields,
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
                fields: HeaderFields::default(),
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
    use crate::pattern::BUILTIN_PATTERNS;

    fn idx_for(bytes: &[u8]) -> LineIndex {
        LineIndex::build(std::io::Cursor::new(bytes)).unwrap()
    }

    fn wsl_oink() -> CompiledPattern {
        CompiledPattern::compile(BUILTIN_PATTERNS[0].1).expect("compile")
    }

    #[test]
    fn single_record_no_continuation() {
        let bytes = b"[INFO ] 2026-01-01 00:00:00.000 [t] play - hi\n";
        let li = idx_for(bytes);
        let recs = scan_records(&wsl_oink(), &li, bytes);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].byte_offset, 0);
        assert_eq!(u64::from(recs[0].byte_len), bytes.len() as u64);
        assert_eq!(recs[0].line_count, 1);
        assert_eq!(recs[0].level, Level::Info);
        assert!(recs[0].fields.level.is_some());
        assert!(recs[0].fields.timestamp.is_some());
        assert!(recs[0].fields.thread.is_some());
        assert!(recs[0].fields.logger.is_some());
        assert!(recs[0].fields.message.is_some());
    }

    #[test]
    fn record_with_continuation_lines() {
        let bytes = b"[ERROR] 2026-01-01 00:00:00.000 [t] play - boom\n  at A.b(A.java:1)\n  at C.d(C.java:2)\n[INFO ] 2026-01-01 00:00:01.000 [t] play - ok\n";
        let li = idx_for(bytes);
        let recs = scan_records(&wsl_oink(), &li, bytes);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].level, Level::Error);
        assert_eq!(recs[0].line_count, 3);
        assert_eq!(recs[1].level, Level::Info);
        assert_eq!(recs[1].line_count, 1);
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
        let recs = scan_records(&wsl_oink(), &li, bytes);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].level, Level::Unknown);
        assert_eq!(recs[1].level, Level::Info);
    }
}
