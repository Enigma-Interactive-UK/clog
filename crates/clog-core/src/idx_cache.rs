//! Persistent on-disk cache of the `(LineIndex, Vec<RecordHeader>)` produced
//! by `index_file`. P7.
//!
//! Format (little-endian, bincode-encoded body):
//!
//! ```text
//! magic    [u8;6]  = b"CLOGIX"
//! schema   u16     = 1
//! _pad     u16
//! file_size u64               // file_size of the indexed source
//! mtime_ns i128               // last-modified nanos since epoch (best-effort)
//! pattern_hash [u8;32]        // blake3 of the pattern source string
//! body     bincode<Body>
//! ```
//!
//! Invalidation is "shape parity": when the cached `file_size` and mtime match
//! the current `file_size` and mtime, AND the pattern hash matches the current
//! scanner's source, the cache hits. Anything else is a miss; the caller
//! re-indexes and overwrites.
//!
//! Path keying lives at the call site (`clog-app` derives a blake3 hash of
//! the absolute path and stores the cache under `<data>/index/<hex>.idx`).
//! Keeping the keying out of `clog-core` means tests can use a raw scratch
//! path.

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::index::LineIndex;
use crate::record::RecordHeader;

const MAGIC: &[u8; 6] = b"CLOGIX";
/// Schema version. Bump when the on-disk format changes.
pub const SCHEMA_VERSION: u16 = 1;

/// Identifying metadata the cache stores in its header. A cache hit requires
/// every field to match the live file + scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheFingerprint {
    pub file_size: u64,
    pub mtime_ns: i128,
    pub pattern_hash: [u8; 32],
}

impl CacheFingerprint {
    /// Build a fingerprint for `path` against a scanner source string.
    /// Returns `Io` if `path` cannot be stat'd.
    ///
    /// # Errors
    /// Bubbles up filesystem errors from `metadata()`.
    pub fn for_path(path: &Path, pattern_source: &str) -> io::Result<Self> {
        let meta = fs::metadata(path)?;
        Ok(Self {
            file_size: meta.len(),
            mtime_ns: mtime_ns(&meta),
            pattern_hash: blake3::hash(pattern_source.as_bytes()).into(),
        })
    }
}

fn mtime_ns(meta: &fs::Metadata) -> i128 {
    meta.modified()
        .ok()
        .and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_nanos().cast_signed())
                .or_else(|| {
                    std::time::UNIX_EPOCH
                        .duration_since(t)
                        .ok()
                        .map(|d| -d.as_nanos().cast_signed())
                })
        })
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Body {
    line_offsets: Vec<u64>,
    file_size: u64,
    records: Vec<RecordHeader>,
}

/// Result of a load attempt.
#[derive(Debug)]
pub enum LoadOutcome {
    /// Cache file existed and matched the fingerprint. The decoded payload
    /// can be used in place of a fresh `index_file` call.
    Hit {
        line_index: LineIndex,
        records: Vec<RecordHeader>,
    },
    /// Cache was absent, unreadable, schema-mismatched, or fingerprint-stale.
    /// Caller should re-index and (optionally) call `save`.
    Miss,
}

/// Try to load `path` as an index cache. Any I/O error or shape mismatch is
/// downgraded to `Miss` so an unparseable cache never blocks a file open.
#[must_use]
pub fn load(path: &Path, expect: &CacheFingerprint) -> LoadOutcome {
    let Ok(mut f) = fs::File::open(path) else {
        return LoadOutcome::Miss;
    };
    let mut header = [0u8; 6 + 2 + 2 + 8 + 16 + 32];
    if f.read_exact(&mut header).is_err() {
        return LoadOutcome::Miss;
    }
    if &header[..6] != MAGIC {
        return LoadOutcome::Miss;
    }
    let schema = u16::from_le_bytes([header[6], header[7]]);
    if schema != SCHEMA_VERSION {
        return LoadOutcome::Miss;
    }
    let file_size = u64::from_le_bytes(header[10..18].try_into().unwrap_or([0; 8]));
    let mtime_ns = i128::from_le_bytes(header[18..34].try_into().unwrap_or([0; 16]));
    let mut pattern_hash = [0u8; 32];
    pattern_hash.copy_from_slice(&header[34..66]);

    if file_size != expect.file_size
        || mtime_ns != expect.mtime_ns
        || pattern_hash != expect.pattern_hash
    {
        return LoadOutcome::Miss;
    }

    let mut body_bytes: Vec<u8> = Vec::new();
    if f.read_to_end(&mut body_bytes).is_err() {
        return LoadOutcome::Miss;
    }
    let Ok(body) = bincode::deserialize::<Body>(&body_bytes) else {
        return LoadOutcome::Miss;
    };
    LoadOutcome::Hit {
        line_index: LineIndex {
            line_offsets: body.line_offsets,
            file_size: body.file_size,
        },
        records: body.records,
    }
}

/// Serialise `(line_index, records)` to `path`, overwriting any prior cache.
/// Best-effort: errors are returned so the caller can log + ignore (a cache
/// write failure should never abort the file open).
///
/// # Errors
/// Bubbles up filesystem errors.
pub fn save(
    path: &Path,
    fp: &CacheFingerprint,
    line_index: &LineIndex,
    records: &[RecordHeader],
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = Body {
        line_offsets: line_index.line_offsets.clone(),
        file_size: line_index.file_size,
        records: records.to_vec(),
    };
    let encoded = bincode::serialize(&body).map_err(io::Error::other)?;

    let tmp = path.with_extension("idx.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(MAGIC)?;
        f.write_all(&SCHEMA_VERSION.to_le_bytes())?;
        f.write_all(&[0u8, 0u8])?; // pad
        f.write_all(&fp.file_size.to_le_bytes())?;
        f.write_all(&fp.mtime_ns.to_le_bytes())?;
        f.write_all(&fp.pattern_hash)?;
        f.write_all(&encoded)?;
        f.sync_data().ok();
    }
    // Atomic replace.
    fs::rename(&tmp, path).or_else(|_| {
        // On Windows fs::rename can fail if target exists pre-rename; remove
        // and retry.
        let _ = fs::remove_file(path);
        fs::rename(&tmp, path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::LineIndex;
    use crate::pattern::HeaderFields;
    use crate::record::{Level, RecordHeader};

    fn sample() -> (LineIndex, Vec<RecordHeader>) {
        let li = LineIndex {
            line_offsets: vec![0, 12, 47, 60],
            file_size: 200,
        };
        let recs = vec![
            RecordHeader {
                byte_offset: 0,
                byte_len: 12,
                line_offset: 0,
                line_count: 1,
                level: Level::Info,
                fields: HeaderFields::default(),
            },
            RecordHeader {
                byte_offset: 12,
                byte_len: 35,
                line_offset: 1,
                line_count: 1,
                level: Level::Warn,
                fields: HeaderFields {
                    level: Some((1, 5)),
                    timestamp: Some((7, 30)),
                    thread: None,
                    logger: Some((32, 40)),
                    message: Some((42, 60)),
                },
            },
            RecordHeader {
                byte_offset: 47,
                byte_len: 13,
                line_offset: 2,
                line_count: 1,
                level: Level::Error,
                fields: HeaderFields::default(),
            },
            RecordHeader {
                byte_offset: 60,
                byte_len: 140,
                line_offset: 3,
                line_count: 5,
                level: Level::Debug,
                fields: HeaderFields::default(),
            },
        ];
        (li, recs)
    }

    fn fp() -> CacheFingerprint {
        CacheFingerprint {
            file_size: 200,
            mtime_ns: 1_234_567_890_000_000,
            pattern_hash: [7u8; 32],
        }
    }

    #[test]
    fn roundtrip_preserves_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.idx");
        let (li, recs) = sample();
        save(&path, &fp(), &li, &recs).unwrap();

        match load(&path, &fp()) {
            LoadOutcome::Hit {
                line_index,
                records,
            } => {
                assert_eq!(line_index.line_offsets, li.line_offsets);
                assert_eq!(line_index.file_size, li.file_size);
                assert_eq!(records, recs);
            }
            LoadOutcome::Miss => panic!("expected cache hit"),
        }
    }

    #[test]
    fn fingerprint_mismatch_is_miss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.idx");
        let (li, recs) = sample();
        save(&path, &fp(), &li, &recs).unwrap();

        let mut other = fp();
        other.file_size += 1;
        assert!(matches!(load(&path, &other), LoadOutcome::Miss));

        let mut other = fp();
        other.mtime_ns += 1;
        assert!(matches!(load(&path, &other), LoadOutcome::Miss));

        let mut other = fp();
        other.pattern_hash[0] ^= 1;
        assert!(matches!(load(&path, &other), LoadOutcome::Miss));
    }

    #[test]
    fn absent_file_is_miss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.idx");
        assert!(matches!(load(&path, &fp()), LoadOutcome::Miss));
    }

    #[test]
    fn corrupted_magic_is_miss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.idx");
        fs::write(&path, b"NOPENOPENOPE").unwrap();
        assert!(matches!(load(&path, &fp()), LoadOutcome::Miss));
    }
}
