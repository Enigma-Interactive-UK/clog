//! In-memory line offset index.
//!
//! P2 surface: walk the file once, record the byte offset of every physical
//! line start. Persistent on-disk caching of the index is deferred to P7.

use std::io::{self, Read};

#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Byte offset of every physical line start. `line_offsets[i]` points at
    /// the first byte of the i-th line.
    pub line_offsets: Vec<u64>,
    /// Total file size in bytes (so `line_offsets[i+1] - line_offsets[i]`
    /// extends naturally; for the last line the end is `file_size`).
    pub file_size: u64,
}

impl LineIndex {
    /// Build the index by streaming `reader` to EOF.
    ///
    /// Empty files produce an empty `line_offsets`. A file ending without a
    /// trailing newline still records its last line. A trailing `\n` does
    /// not invent a phantom empty line past EOF.
    ///
    /// # Errors
    ///
    /// Bubbles up the first I/O error from `reader`.
    pub fn build(mut reader: impl Read) -> io::Result<Self> {
        let mut line_offsets: Vec<u64> = Vec::new();
        let mut pos: u64 = 0;
        let mut start_of_line = true;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            for (i, &b) in buf[..n].iter().enumerate() {
                if start_of_line {
                    line_offsets.push(pos + i as u64);
                    start_of_line = false;
                }
                if b == b'\n' {
                    start_of_line = true;
                }
            }
            pos += n as u64;
        }
        Ok(Self {
            line_offsets,
            file_size: pos,
        })
    }

    #[must_use]
    pub fn line_count(&self) -> usize {
        self.line_offsets.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn empty_file_has_no_lines() {
        let idx = LineIndex::build(Cursor::new(b"")).unwrap();
        assert_eq!(idx.line_count(), 0);
        assert_eq!(idx.file_size, 0);
    }

    #[test]
    fn three_lines_with_trailing_newline() {
        let idx = LineIndex::build(Cursor::new(b"a\nbb\nccc\n")).unwrap();
        assert_eq!(idx.line_offsets, vec![0, 2, 5]);
        assert_eq!(idx.file_size, 9);
    }

    #[test]
    fn no_trailing_newline_keeps_last_line() {
        let idx = LineIndex::build(Cursor::new(b"a\nbb")).unwrap();
        assert_eq!(idx.line_offsets, vec![0, 2]);
        assert_eq!(idx.file_size, 4);
    }

    #[test]
    fn blank_line_in_middle() {
        let idx = LineIndex::build(Cursor::new(b"a\n\nb\n")).unwrap();
        assert_eq!(idx.line_offsets, vec![0, 2, 3]);
    }
}
