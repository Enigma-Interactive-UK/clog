//! Regex escape hatch.
//!
//! For exotic patterns the `PatternLayout` compiler does not handle, the user
//! can supply a raw regex with named captures (`level`, `timestamp`,
//! `thread`, `logger`, `msg`). Anything matched at the start of a physical
//! line counts as a record header.

use regex::bytes::Regex;
use thiserror::Error;

use crate::pattern::{HeaderFields, ParsedHeader};
use crate::record::{Level, RecordScanner};

#[derive(Debug, Error)]
pub enum RegexScannerError {
    #[error("regex compile failed: {0}")]
    Compile(#[from] regex::Error),
}

pub struct RegexScanner {
    re: Regex,
    /// Capture group indices for the named fields, looked up once at build
    /// time. `None` if the user did not include that group.
    g_level: Option<usize>,
    g_timestamp: Option<usize>,
    g_thread: Option<usize>,
    g_logger: Option<usize>,
    g_msg: Option<usize>,
}

impl RegexScanner {
    /// Compile `src` as a header regex. Anchored at start-of-line at parse
    /// time (a leading `^` is implicit). Named groups `level`, `timestamp`,
    /// `thread`, `logger`, `msg` populate the header fields.
    ///
    /// # Errors
    ///
    /// Returns `RegexScannerError::Compile` if the pattern is not a valid
    /// regex.
    pub fn compile(src: &str) -> Result<Self, RegexScannerError> {
        let anchored = if src.starts_with('^') {
            src.to_string()
        } else {
            format!("^{src}")
        };
        let re = Regex::new(&anchored)?;
        let g_level = re.capture_names().position(|n| n == Some("level"));
        let g_timestamp = re.capture_names().position(|n| n == Some("timestamp"));
        let g_thread = re.capture_names().position(|n| n == Some("thread"));
        let g_logger = re.capture_names().position(|n| n == Some("logger"));
        let g_msg = re.capture_names().position(|n| n == Some("msg"));
        Ok(Self {
            re,
            g_level,
            g_timestamp,
            g_thread,
            g_logger,
            g_msg,
        })
    }
}

fn classify_word(buf: &[u8]) -> Level {
    let trimmed = trim_ascii(buf);
    match trimmed {
        b"TRACE" => Level::Trace,
        b"DEBUG" => Level::Debug,
        b"INFO" => Level::Info,
        b"WARN" => Level::Warn,
        b"ERROR" => Level::Error,
        b"FATAL" => Level::Fatal,
        b"OFF" => Level::Off,
        b"ALL" => Level::All,
        _ => Level::Unknown,
    }
}

fn trim_ascii(buf: &[u8]) -> &[u8] {
    let mut s = 0;
    let mut e = buf.len();
    while s < e && buf[s].is_ascii_whitespace() {
        s += 1;
    }
    while e > s && buf[e - 1].is_ascii_whitespace() {
        e -= 1;
    }
    &buf[s..e]
}

fn span_of(caps: &regex::bytes::Captures<'_>, idx: Option<usize>) -> Option<(u32, u32)> {
    let i = idx?;
    let m = caps.get(i)?;
    Some((
        u32::try_from(m.start()).unwrap_or(u32::MAX),
        u32::try_from(m.end()).unwrap_or(u32::MAX),
    ))
}

impl RecordScanner for RegexScanner {
    fn try_parse_header(&self, line: &[u8]) -> Option<ParsedHeader> {
        let caps = self.re.captures(line)?;
        let level = self
            .g_level
            .and_then(|i| caps.get(i))
            .map_or(Level::Unknown, |m| classify_word(m.as_bytes()));
        let fields = HeaderFields {
            level: span_of(&caps, self.g_level),
            timestamp: span_of(&caps, self.g_timestamp),
            thread: span_of(&caps, self.g_thread),
            logger: span_of(&caps, self.g_logger),
            message: span_of(&caps, self.g_msg),
        };
        Some(ParsedHeader { level, fields })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_captures() {
        let r = RegexScanner::compile(
            r"(?P<timestamp>\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3}) (?P<level>INFO|WARN|ERROR) \[(?P<thread>[^\]]+)\] - (?P<msg>.*)",
        )
        .expect("compile");
        let line = b"2026-05-21 00:00:04.401 INFO [play-thread-1] - hello";
        let h = r.try_parse_header(line).expect("matches");
        assert_eq!(h.level, Level::Info);
        assert!(h.fields.timestamp.is_some());
        assert!(h.fields.thread.is_some());
        assert!(h.fields.message.is_some());
    }

    #[test]
    fn rejects_non_matching() {
        let r = RegexScanner::compile(r"^\[(?P<level>INFO)\]").expect("compile");
        assert!(r.try_parse_header(b"  at Foo.bar(Foo.java:1)").is_none());
    }
}
