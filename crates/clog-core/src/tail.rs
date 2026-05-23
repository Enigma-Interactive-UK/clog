//! Polling tail loop + rotation detection.
//!
//! v1 design (`docs/design.md` s6, s10): `notify` is unreliable over SMB so
//! we poll. Each tick stats the file size and hashes the first 256 bytes.
//! Rotation is detected when the size shrinks OR the head hash changes;
//! both `OnStartupTriggeringPolicy` (truncate) and `TimeBasedTriggeringPolicy`
//! (rename + recreate) are covered by that pair.
//!
//! Network access I/O is exposed as a state machine so tests can drive
//! `poll()` by hand. The app crate wraps this in a tokio task that ticks
//! every 250 ms (`docs/design.md` s6).

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Number of leading bytes hashed for rotation detection. Big enough that
/// "different file content" is virtually certain to diverge, small enough
/// to read in a single sector.
pub const HEAD_HASH_BYTES: usize = 256;

/// Default polling interval. Used by the app crate's tail task.
pub const DEFAULT_POLL_INTERVAL_MS: u64 = 250;

/// Outcome of a single `TailState::poll()` tick.
#[derive(Debug)]
pub enum TailEvent {
    /// File hasn't changed since last poll.
    NoChange,
    /// File grew. `bytes` is the entire appended payload, starting at
    /// `from_offset`. The buffer may end with a partial line (no trailing
    /// `\n`) -- the caller's `extend_with_appended`-style sink is
    /// expected to handle partial-line continuation and completion in a
    /// future tick.
    Appended { from_offset: u64, bytes: Vec<u8> },
    /// Rotation detected. Caller must re-index from offset 0. The new size
    /// and head hash have already been adopted into `TailState`.
    Rotated,
}

/// Per-file tail tracker. Cheap to construct; owns no file handle between
/// polls so the OS is free to handle rename/recreate underneath.
#[derive(Debug, Clone)]
pub struct TailState {
    path: PathBuf,
    /// Highest byte offset already delivered to the caller. Sits between
    /// 0 and the on-disk size; a trailing partial line keeps it short of
    /// the size until a newline arrives.
    consumed: u64,
    /// FNV-1a hash of the file's first `head_prefix_len` bytes. `None` only
    /// while the file is empty.
    head_hash: Option<u64>,
    /// Byte length the head hash was computed over. Pinned to the smaller
    /// of `HEAD_HASH_BYTES` and the file's current size; only grows, never
    /// shrinks (a shrink is caught by size comparison first).
    head_prefix_len: usize,
}

impl TailState {
    /// Build a tail tracker for `path` already at `initial_size`.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error if the head bytes cannot be read.
    pub fn new(path: impl Into<PathBuf>, initial_size: u64) -> io::Result<Self> {
        let path = path.into();
        let prefix_len = head_prefix_for(initial_size);
        let head_hash = read_head_hash(&path, prefix_len)?;
        Ok(Self {
            path,
            consumed: initial_size,
            head_hash,
            head_prefix_len: prefix_len,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn consumed(&self) -> u64 {
        self.consumed
    }

    /// Stat the file and emit at most one event. Returns `NoChange`,
    /// `Appended` with the freshly-read bytes, or `Rotated`. After a
    /// `Rotated` event the caller is expected to re-index; the state is
    /// already re-anchored.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error if the file cannot be stat'd or
    /// read. A "not found" error is treated as a transient between-rotation
    /// state and surfaced as `NoChange` so the caller can retry next tick.
    pub fn poll(&mut self) -> io::Result<TailEvent> {
        let meta = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            // The file may briefly disappear between rotation steps on some
            // filesystems. Treat that as "wait for next tick" rather than
            // an error that tears down the loop.
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(TailEvent::NoChange),
            Err(e) => return Err(e),
        };
        let size = meta.len();

        // Size shrink -> rotation, no need to rehash. Most common case
        // (truncate-in-place from OnStartupTriggeringPolicy).
        if size < self.consumed {
            let prefix_len = head_prefix_for(size);
            self.head_hash = read_head_hash(&self.path, prefix_len)?;
            self.head_prefix_len = prefix_len;
            self.consumed = 0;
            return Ok(TailEvent::Rotated);
        }

        // Hash-change rotation: rehash the SAME prefix length we anchored on.
        // Any growth keeps the prefix bytes unchanged, so the hash should
        // only flip when the writer recreated the file underneath us. For a
        // zero-prefix anchor (the file was empty at construction) we adopt
        // any non-empty hash as a fresh anchor rather than calling it a
        // rotation.
        if self.head_prefix_len > 0 {
            let new_head = read_head_hash(&self.path, self.head_prefix_len)?;
            if new_head != self.head_hash {
                let prefix_len = head_prefix_for(size);
                self.head_hash = read_head_hash(&self.path, prefix_len)?;
                self.head_prefix_len = prefix_len;
                self.consumed = 0;
                return Ok(TailEvent::Rotated);
            }
        } else if size > 0 {
            // First time we have content: adopt it as the new anchor.
            let prefix_len = head_prefix_for(size);
            self.head_hash = read_head_hash(&self.path, prefix_len)?;
            self.head_prefix_len = prefix_len;
        }

        if size == self.consumed {
            return Ok(TailEvent::NoChange);
        }

        // Pure append. Read the gap and ship it whole, partial trailing
        // line and all. The sink (`extend_with_appended`) is responsible
        // for partial-line state: a buffer that ends mid-line just
        // means the next tick will deliver more bytes that either
        // extend the same line or close it with a `\n`.
        //
        // Why we don't hold partials back: doing so makes a genuinely
        // partial last line on disk invisible to the viewer until the
        // writer flushes a newline, which can never come for files
        // authored by hand or via editors that omit the trailing
        // newline.
        let from = self.consumed;
        let len = size - from;
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(from))?;
        let len_usz = usize::try_from(len).unwrap_or(usize::MAX);
        let mut buf = vec![0u8; len_usz];
        file.read_exact(&mut buf)?;
        self.consumed = size;
        Ok(TailEvent::Appended {
            from_offset: from,
            bytes: buf,
        })
    }

    /// Re-anchor after the caller has re-indexed a rotated file. Sets the
    /// consumed cursor to the supplied size and refreshes the head hash.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error if the head bytes cannot be read.
    pub fn reset_to(&mut self, size: u64) -> io::Result<()> {
        let prefix_len = head_prefix_for(size);
        self.consumed = size;
        self.head_hash = read_head_hash(&self.path, prefix_len)?;
        self.head_prefix_len = prefix_len;
        Ok(())
    }
}

fn head_prefix_for(size: u64) -> usize {
    usize::min(
        HEAD_HASH_BYTES,
        usize::try_from(size).unwrap_or(HEAD_HASH_BYTES),
    )
}

fn read_head_hash(path: &Path, prefix_len: usize) -> io::Result<Option<u64>> {
    if prefix_len == 0 {
        return Ok(None);
    }
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut buf = vec![0u8; prefix_len];
    let n = file.read(&mut buf)?;
    buf.truncate(n);
    if buf.len() < prefix_len {
        // File got truncated between metadata stat and read. Treat as a
        // rotation by returning a sentinel that won't match the prior hash.
        return Ok(Some(fnv1a_64(&buf) ^ 0xDEAD_BEEF_DEAD_BEEF));
    }
    Ok(Some(fnv1a_64(&buf)))
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    // Inline FNV-1a to avoid pulling in a hashing dep just for rotation
    // detection. Quality is fine for a "did the prefix change" predicate.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::Write;

    struct TempLog {
        path: PathBuf,
    }

    impl TempLog {
        fn new(name: &str) -> Self {
            let mut p = std::env::temp_dir();
            // Make the name unique enough to avoid stomping parallel tests.
            let pid = std::process::id();
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            p.push(format!("clog-tail-{name}-{pid}-{ts}.log"));
            // Start with an empty file.
            File::create(&p).expect("create temp log");
            Self { path: p }
        }

        fn append(&self, bytes: &[u8]) {
            let mut f = OpenOptions::new()
                .append(true)
                .open(&self.path)
                .expect("open append");
            f.write_all(bytes).expect("append write");
            f.flush().expect("flush");
        }

        fn rewrite(&self, bytes: &[u8]) {
            // Truncate-and-write in one shot; mirrors OnStartupTriggeringPolicy.
            let mut f = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&self.path)
                .expect("open truncate");
            f.write_all(bytes).expect("rewrite");
            f.flush().expect("flush");
        }

        fn size(&self) -> u64 {
            std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0)
        }
    }

    impl Drop for TempLog {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[test]
    fn no_change_when_file_idle() {
        let log = TempLog::new("idle");
        log.append(b"hello\n");
        let mut tail = TailState::new(&log.path, log.size()).expect("new");
        match tail.poll().expect("poll") {
            TailEvent::NoChange => {}
            other => panic!("expected NoChange, got {other:?}"),
        }
    }

    #[test]
    fn append_only_growth_is_reported_once() {
        let log = TempLog::new("append");
        log.append(b"first\n");
        let mut tail = TailState::new(&log.path, log.size()).expect("new");
        log.append(b"second\nthird\n");
        let from;
        let bytes;
        match tail.poll().expect("poll") {
            TailEvent::Appended {
                from_offset,
                bytes: b,
            } => {
                from = from_offset;
                bytes = b;
            }
            other => panic!("expected Appended, got {other:?}"),
        }
        assert_eq!(from, 6);
        assert_eq!(bytes, b"second\nthird\n");
        // Second poll without writes -> NoChange.
        match tail.poll().expect("poll") {
            TailEvent::NoChange => {}
            other => panic!("second poll should be NoChange, got {other:?}"),
        }
    }

    #[test]
    fn partial_trailing_line_ships_and_then_completes() {
        // New contract: a partial line ships immediately. A follow-up
        // tick delivers the remainder (including the closing `\n`),
        // never re-ships the same bytes. The sink is responsible for
        // splicing the two halves into a single line.
        let log = TempLog::new("partial");
        log.append(b"complete\n");
        let mut tail = TailState::new(&log.path, log.size()).expect("new");
        log.append(b"in-progress without newline");
        match tail.poll().expect("poll") {
            TailEvent::Appended { from_offset, bytes } => {
                assert_eq!(from_offset, 9);
                assert_eq!(bytes, b"in-progress without newline");
            }
            other => panic!("expected Appended (partial ships), got {other:?}"),
        }
        log.append(b" and done\n");
        match tail.poll().expect("poll") {
            TailEvent::Appended { from_offset, bytes } => {
                // consumed advanced through the partial bytes already.
                assert_eq!(from_offset, 9 + 27);
                assert_eq!(bytes, b" and done\n");
            }
            other => panic!("expected Appended (remainder), got {other:?}"),
        }
    }

    #[test]
    fn size_shrink_is_treated_as_rotation() {
        let log = TempLog::new("shrink");
        log.append(b"this is the original content of the log file, long enough to be over 256 bytes so the head hash is meaningful. ");
        log.append(&[b'x'; 200]);
        log.append(b"\n");
        let mut tail = TailState::new(&log.path, log.size()).expect("new");
        // Truncate to a smaller payload that starts the same way as before.
        log.rewrite(b"short\n");
        match tail.poll().expect("poll") {
            TailEvent::Rotated => {}
            other => panic!("expected Rotated, got {other:?}"),
        }
        // Caller would now re-index; reset_to mimics that.
        tail.reset_to(log.size()).expect("reset_to");
        log.append(b"after\n");
        match tail.poll().expect("poll") {
            TailEvent::Appended { bytes, .. } => assert_eq!(bytes, b"after\n"),
            other => panic!("expected Appended after reset, got {other:?}"),
        }
    }

    #[test]
    fn head_hash_change_is_treated_as_rotation() {
        let log = TempLog::new("hash");
        // Make the file longer than HEAD_HASH_BYTES so the head prefix is
        // anchored to the full 256-byte window.
        let mut body = b"AAA the original content. ".to_vec();
        while body.len() < HEAD_HASH_BYTES + 64 {
            body.push(b'a');
        }
        body.push(b'\n');
        log.append(&body);
        let mut tail = TailState::new(&log.path, log.size()).expect("new");
        // Different head bytes, growing the file rather than shrinking it.
        let mut replacement = b"BBB the rewritten content. ".to_vec();
        while replacement.len() < body.len() {
            replacement.push(b'b');
        }
        replacement.push(b'\n');
        log.rewrite(&replacement);
        match tail.poll().expect("poll") {
            TailEvent::Rotated => {}
            other => panic!("expected Rotated on head-hash change, got {other:?}"),
        }
    }

    #[test]
    fn detection_latency_is_one_poll() {
        // Asserting "within 1 polling interval" reduces in a synchronous
        // test to "the very next poll after the change sees it". The other
        // tests already prove that; this is a focused affirmation.
        let log = TempLog::new("latency");
        log.append(b"line-a\n");
        let mut tail = TailState::new(&log.path, log.size()).expect("new");
        log.append(b"line-b\n");
        assert!(matches!(
            tail.poll().expect("poll"),
            TailEvent::Appended { .. }
        ));
    }
}
