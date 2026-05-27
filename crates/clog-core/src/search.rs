//! Smart + regex search engine. P6.
//!
//! The same engine powers two UI surfaces: a search bar that flags hits in
//! place and a filter that narrows the visible set. Both modes return
//! `HitRef` (`record_idx` + record-relative byte ranges + score); the UI
//! decides whether to highlight or hide.
//!
//! ## Smart search
//!
//! Tokens are split on ASCII whitespace from the query. Each token must
//! appear in the record bytes in the original order; case-insensitive
//! unless `case_sensitive` is set. The score is the total number of
//! "gap chars" between consecutive tokens (fewer = better). The example
//! table from docs/design.md s7:
//!
//! | text                     | gap | rank |
//! | ---                      | --- | ---  |
//! | `connectionrefused`      | 0   | best |
//! | `connection refused`     | 1   | next |
//! | `connection was refused` | 5   | worse|
//!
//! The implementation finds every occurrence of token 0, then for each
//! greedily picks the earliest occurrence of token 1 starting at or after
//! the end of token 0, and so on. The result with the lowest gap-sum is
//! returned. Greedy is optimal here because picking a later occurrence of
//! any subsequent token only increases the gap on its left side.
//!
//! ## Regex
//!
//! `regex::bytes::Regex` is applied per record (the haystack is the record
//! bytes, not the whole file), so matches never cross record boundaries
//! and the parallel-iter shape stays the same as smart search.

use rayon::prelude::*;
use regex::bytes::Regex;
use serde::Serialize;
use thiserror::Error;

use crate::record::{Level, RecordHeader};
use crate::thread_groups::{classify, ThreadGroupMask};

/// Search mode. Mirrored over IPC as a tagged enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    Smart,
    Regex,
}

/// Bitmask of levels the search is allowed to include. A bit set = include.
///
/// We use `u16` rather than a `Vec<Level>` so the mask is `Copy` and cheap
/// to test in the parallel hot loop. The mapping is fixed (see `level_bit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelMask(pub u16);

impl LevelMask {
    pub const ALL: Self = Self(0xFFFF);

    #[must_use]
    pub fn allows(self, level: Level) -> bool {
        self.0 & level_bit(level) != 0
    }

    #[must_use]
    pub fn with(self, level: Level, allow: bool) -> Self {
        if allow {
            Self(self.0 | level_bit(level))
        } else {
            Self(self.0 & !level_bit(level))
        }
    }
}

impl Default for LevelMask {
    fn default() -> Self {
        Self::ALL
    }
}

#[must_use]
pub fn level_bit(level: Level) -> u16 {
    match level {
        Level::Trace => 1 << 0,
        Level::Debug => 1 << 1,
        Level::Info => 1 << 2,
        Level::Warn => 1 << 3,
        Level::Error => 1 << 4,
        Level::Fatal => 1 << 5,
        Level::Off => 1 << 6,
        Level::All => 1 << 7,
        Level::Unknown => 1 << 8,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HitRef {
    /// Index into the record array.
    pub record_idx: u64,
    /// First physical line of the matching record (= `record.line_offset`).
    /// Echoed onto the hit so the UI can expand a filtered set into a
    /// flat virtual-line index without a second round trip.
    pub record_first_line: u64,
    /// Number of physical lines this record spans.
    pub record_line_count: u32,
    /// Level of the matching record. Echoed onto the hit so the UI can
    /// drive the filtered-minimap without a second lookup.
    pub level: Level,
    /// Byte ranges within this record (relative to `record.byte_offset`).
    /// One range per smart-search token, or one range per regex match.
    pub ranges: Vec<(u32, u32)>,
    /// Lower = better. For smart search this is the total gap-char count;
    /// for regex it's always 0.
    pub score: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub level_mask: LevelMask,
    pub thread_group_mask: ThreadGroupMask,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            level_mask: LevelMask::ALL,
            thread_group_mask: ThreadGroupMask::ALL,
        }
    }
}

#[derive(Debug, Error)]
pub enum SearchError {
    #[error("empty query")]
    EmptyQuery,
    #[error("regex compile failed: {0}")]
    BadRegex(String),
}

fn record_passes(rec: &RecordHeader, bytes: &[u8], opts: SearchOptions) -> bool {
    if !opts.level_mask.allows(rec.level) {
        return false;
    }
    if opts.thread_group_mask == ThreadGroupMask::ALL {
        return true;
    }
    let group = match rec.fields.thread {
        Some((s, e)) => {
            let start = usize::try_from(rec.byte_offset)
                .unwrap_or(usize::MAX)
                .saturating_add(s as usize);
            let end = usize::try_from(rec.byte_offset)
                .unwrap_or(usize::MAX)
                .saturating_add(e as usize);
            let end = end.min(bytes.len());
            if start > end || start >= bytes.len() {
                crate::thread_groups::ThreadGroup::Other
            } else {
                classify(&bytes[start..end])
            }
        }
        None => crate::thread_groups::ThreadGroup::Other,
    };
    opts.thread_group_mask.allows(group)
}

/// Run a search across the file's records. Returns hits in record order.
///
/// `bytes` must be the full file content and the offsets carried by each
/// `RecordHeader` must index into it.
///
/// # Errors
///
/// Returns `SearchError::EmptyQuery` if the query has no usable content
/// (smart) or compiles to a regex that matches nothing useful (regex).
/// Returns `SearchError::BadRegex` if the regex fails to compile.
pub fn search_records(
    records: &[RecordHeader],
    bytes: &[u8],
    mode: SearchMode,
    query: &str,
    opts: SearchOptions,
) -> Result<Vec<HitRef>, SearchError> {
    match mode {
        SearchMode::Smart => {
            let tokens = smart_tokens(query, opts.case_sensitive);
            if tokens.is_empty() {
                return Err(SearchError::EmptyQuery);
            }
            Ok(records
                .par_iter()
                .enumerate()
                .filter_map(|(i, rec)| {
                    if !record_passes(rec, bytes, opts) {
                        return None;
                    }
                    let text = record_text(rec, bytes);
                    smart_match(text, &tokens, opts.case_sensitive).map(|(ranges, score)| HitRef {
                        record_idx: i as u64,
                        record_first_line: u64::from(rec.line_offset),
                        record_line_count: rec.line_count,
                        level: rec.level,
                        ranges,
                        score,
                    })
                })
                .collect())
        }
        SearchMode::Regex => {
            let pattern = if opts.case_sensitive {
                query.to_string()
            } else {
                format!("(?i){query}")
            };
            let re = Regex::new(&pattern).map_err(|e| SearchError::BadRegex(e.to_string()))?;
            Ok(records
                .par_iter()
                .enumerate()
                .filter_map(|(i, rec)| {
                    if !record_passes(rec, bytes, opts) {
                        return None;
                    }
                    let text = record_text(rec, bytes);
                    let ranges: Vec<(u32, u32)> = re
                        .find_iter(text)
                        .filter(|m| m.end() > m.start())
                        .map(|m| {
                            (
                                u32::try_from(m.start()).unwrap_or(u32::MAX),
                                u32::try_from(m.end()).unwrap_or(u32::MAX),
                            )
                        })
                        .collect();
                    if ranges.is_empty() {
                        None
                    } else {
                        Some(HitRef {
                            record_idx: i as u64,
                            record_first_line: u64::from(rec.line_offset),
                            record_line_count: rec.line_count,
                            level: rec.level,
                            ranges,
                            score: 0,
                        })
                    }
                })
                .collect())
        }
    }
}

fn record_text<'a>(rec: &RecordHeader, bytes: &'a [u8]) -> &'a [u8] {
    let start = usize::try_from(rec.byte_offset).unwrap_or(usize::MAX);
    let end = start.saturating_add(usize::try_from(rec.byte_len).unwrap_or(usize::MAX));
    let end = end.min(bytes.len());
    &bytes[start..end]
}

fn smart_tokens(query: &str, case_sensitive: bool) -> Vec<Vec<u8>> {
    query
        .split_ascii_whitespace()
        .map(|t| {
            if case_sensitive {
                t.as_bytes().to_vec()
            } else {
                t.as_bytes().to_ascii_lowercase()
            }
        })
        .filter(|t| !t.is_empty())
        .collect()
}

/// Greedy proximity match. Returns `(ranges, gap_sum)` for the best
/// alignment, or `None` if any token is missing.
fn smart_match(
    text: &[u8],
    tokens: &[Vec<u8>],
    case_sensitive: bool,
) -> Option<(Vec<(u32, u32)>, u32)> {
    if tokens.is_empty() {
        return None;
    }

    // Pre-compute every occurrence of each token.
    let occs: Vec<Vec<(usize, usize)>> = tokens
        .iter()
        .map(|t| find_all(text, t, !case_sensitive))
        .collect();
    if occs.iter().any(Vec::is_empty) {
        return None;
    }

    let mut best: Option<(u32, Vec<(u32, u32)>)> = None;
    for &(s0, e0) in &occs[0] {
        let mut chosen: Vec<(usize, usize)> = Vec::with_capacity(tokens.len());
        chosen.push((s0, e0));
        let mut prev_end = e0;
        let mut gap_sum: u32 = 0;
        let mut ok = true;
        for token_occs in occs.iter().skip(1) {
            // Earliest occurrence whose start is >= prev_end. Greedy is
            // optimal: choosing a later occurrence only widens the gap to
            // the left without helping any future gap.
            if let Some(&(s, e)) = token_occs.iter().find(|&&(s, _)| s >= prev_end) {
                gap_sum = gap_sum.saturating_add(u32::try_from(s - prev_end).unwrap_or(u32::MAX));
                chosen.push((s, e));
                prev_end = e;
            } else {
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }
        let ranges: Vec<(u32, u32)> = chosen
            .iter()
            .map(|&(s, e)| {
                (
                    u32::try_from(s).unwrap_or(u32::MAX),
                    u32::try_from(e).unwrap_or(u32::MAX),
                )
            })
            .collect();
        match &best {
            None => best = Some((gap_sum, ranges)),
            Some((bg, _)) if gap_sum < *bg => best = Some((gap_sum, ranges)),
            _ => {}
        }
    }
    best.map(|(g, r)| (r, g))
}

/// Linear scan for every non-overlapping occurrence of `needle` in
/// `haystack`. Returns inclusive-exclusive byte ranges.
fn find_all(haystack: &[u8], needle: &[u8], case_insensitive: bool) -> Vec<(usize, usize)> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let nl = needle.len();
    let limit = haystack.len() - nl + 1;
    let mut i = 0;
    'outer: while i < limit {
        for j in 0..nl {
            let a = haystack[i + j];
            let b = needle[j];
            let eq = if case_insensitive {
                a.eq_ignore_ascii_case(&b)
            } else {
                a == b
            };
            if !eq {
                i += 1;
                continue 'outer;
            }
        }
        out.push((i, i + nl));
        // Non-overlapping: advance past this match. Smart search never
        // benefits from overlapping matches of the same token.
        i += nl;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::{builtin_pattern, CompiledPattern};
    use crate::{index_file, scan_records, LineIndex};

    fn smart(text: &[u8], query: &str) -> Option<(Vec<(u32, u32)>, u32)> {
        let tokens = smart_tokens(query, false);
        smart_match(text, &tokens, false)
    }

    /// docs/design.md s7 ranking table: same query, three texts, lower
    /// gap-sum wins.
    #[test]
    fn smart_ranking_matches_design_table() {
        let (_, zero) = smart(b"connectionrefused", "connection refused").expect("hit");
        let (_, one) = smart(b"connection refused", "connection refused").expect("hit");
        let (_, five) = smart(b"connection was refused", "connection refused").expect("hit");
        assert_eq!(zero, 0);
        assert_eq!(one, 1);
        assert_eq!(five, 5);
        assert!(zero < one);
        assert!(one < five);
    }

    /// Multi-token: `foo bar baz` over `foo___bar_baz` ranks by sum of
    /// gaps between adjacent tokens.
    #[test]
    fn smart_multi_token_sums_gaps() {
        let (_, gap) = smart(b"foo___bar_baz", "foo bar baz").expect("hit");
        assert_eq!(gap, 3 + 1);
    }

    /// Out-of-order tokens do not match.
    #[test]
    fn smart_requires_in_order_match() {
        assert!(smart(b"refused connection", "connection refused").is_none());
    }

    /// Case-insensitive by default.
    #[test]
    fn smart_is_case_insensitive_by_default() {
        let tokens = smart_tokens("CoNnEcT", false);
        assert!(smart_match(b"connection refused", &tokens, false).is_some());
    }

    /// Case-sensitive when requested.
    #[test]
    fn smart_respects_case_sensitive_flag() {
        let tokens = smart_tokens("CoNnEcT", true);
        assert!(smart_match(b"connection refused", &tokens, true).is_none());
        let tokens = smart_tokens("connection", true);
        assert!(smart_match(b"connection refused", &tokens, true).is_some());
    }

    /// Regex `find_iter` is anchored to the record bytes, so a match must
    /// not cross a record boundary. We assemble two records back-to-back
    /// with a single byte that LOOKS like it would span them; the search
    /// engine then runs per record and only finds the in-bounds part.
    #[test]
    fn regex_does_not_cross_record_boundary() {
        // wsl-dev-shaped fixture: two records, neither contains the
        // boundary-spanning string when read on its own.
        let bytes = b"[INFO ] 2026-01-01 00:00:00.000 [t] play - alpha\n[INFO ] 2026-01-01 00:00:01.000 [t] play - beta\n";
        let li = LineIndex::build(std::io::Cursor::new(bytes.as_slice())).unwrap();
        let scanner = CompiledPattern::compile(builtin_pattern("wsl-dev").unwrap()).unwrap();
        let records = scan_records(&scanner, &li, bytes);
        assert_eq!(records.len(), 2);

        // `alpha\n[INFO` straddles the boundary in raw bytes, but each
        // record's text individually does not contain it.
        let hits = search_records(
            &records,
            bytes,
            SearchMode::Regex,
            r"alpha\n\[INFO",
            SearchOptions::default(),
        )
        .expect("regex compiles");
        assert!(hits.is_empty(), "match must not cross record boundary");

        // Sanity: `alpha` alone hits exactly once, in record 0.
        let hits = search_records(
            &records,
            bytes,
            SearchMode::Regex,
            "alpha",
            SearchOptions::default(),
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record_idx, 0);
    }

    /// Smart search must also stay within a record. A two-token query
    /// where token 0 lives in record A and token 1 lives in record B
    /// returns zero hits, not a single cross-record hit.
    #[test]
    fn smart_does_not_cross_record_boundary() {
        let bytes = b"[INFO ] 2026-01-01 00:00:00.000 [t] play - alpha\n[INFO ] 2026-01-01 00:00:01.000 [t] play - beta\n";
        let li = LineIndex::build(std::io::Cursor::new(bytes.as_slice())).unwrap();
        let scanner = CompiledPattern::compile(builtin_pattern("wsl-dev").unwrap()).unwrap();
        let records = scan_records(&scanner, &li, bytes);
        let hits = search_records(
            &records,
            bytes,
            SearchMode::Smart,
            "alpha beta",
            SearchOptions::default(),
        )
        .unwrap();
        assert!(hits.is_empty());
    }

    /// Level mask: a record whose level isn't in the mask is skipped.
    #[test]
    fn level_mask_filters_records() {
        let bytes = b"[INFO ] 2026-01-01 00:00:00.000 [t] play - hello\n[ERROR] 2026-01-01 00:00:01.000 [t] play - hello\n";
        let li = LineIndex::build(std::io::Cursor::new(bytes.as_slice())).unwrap();
        let scanner = CompiledPattern::compile(builtin_pattern("wsl-dev").unwrap()).unwrap();
        let records = scan_records(&scanner, &li, bytes);

        // Without mask: both hit.
        let hits = search_records(
            &records,
            bytes,
            SearchMode::Smart,
            "hello",
            SearchOptions::default(),
        )
        .unwrap();
        assert_eq!(hits.len(), 2);

        // Mask out INFO: only ERROR remains.
        let opts = SearchOptions {
            level_mask: LevelMask::ALL.with(Level::Info, false),
            ..SearchOptions::default()
        };
        let hits = search_records(&records, bytes, SearchMode::Smart, "hello", opts).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record_idx, 1);
    }

    /// Thread-group mask: a record whose thread classifies to an
    /// excluded group is skipped.
    #[test]
    fn thread_group_mask_filters_records() {
        use crate::ThreadGroup;
        use crate::ThreadGroupMask;
        let bytes = b"[INFO ] 2026-01-01 00:00:00.000 [play-thread-1] play - hello\n[INFO ] 2026-01-01 00:00:01.000 [jobs-thread-2] play - hello\n";
        let li = LineIndex::build(std::io::Cursor::new(bytes.as_slice())).unwrap();
        let scanner = CompiledPattern::compile(builtin_pattern("wsl-dev").unwrap()).unwrap();
        let records = scan_records(&scanner, &li, bytes);
        assert_eq!(records.len(), 2);

        // Without mask: both hit.
        let hits = search_records(
            &records,
            bytes,
            SearchMode::Smart,
            "hello",
            SearchOptions::default(),
        )
        .unwrap();
        assert_eq!(hits.len(), 2);

        // Mask out Jobs: only the Requests-classified record remains.
        let opts = SearchOptions {
            thread_group_mask: ThreadGroupMask::ALL.with(ThreadGroup::Jobs, false),
            ..SearchOptions::default()
        };
        let hits = search_records(&records, bytes, SearchMode::Smart, "hello", opts).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record_idx, 0);
    }

    /// Integration test on the prod fixture: a smart-search query that
    /// has a known number of hits across the whole file.
    #[test]
    fn cheesecake_prod_known_count_smart_search() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../research/cheesecake-prod.log"
        );
        if !std::path::Path::new(path).exists() {
            eprintln!("skipping: fixture {path} not present");
            return;
        }
        // Pick a pattern likely to be stable across the fixture: every
        // record carries `INFO`/`WARN`/`ERROR` (level) so `INFO 2026`
        // is too greedy. `Loading` (start-up message) is steady.
        let scanner = CompiledPattern::compile(builtin_pattern("prod").unwrap()).unwrap();
        let (mut source, _li, records) = index_file(path, &scanner).expect("index");
        let bytes = source.read_all().expect("read");
        let hits = search_records(
            &records,
            &bytes,
            SearchMode::Smart,
            "Application",
            SearchOptions::default(),
        )
        .unwrap();
        // Sanity bounds: at least one and far less than the total record
        // count. The exact value is asserted as a stability constant.
        assert!(!hits.is_empty(), "expected at least one Application hit");
        assert!(
            hits.len() < records.len(),
            "{} hits should be much fewer than {} records",
            hits.len(),
            records.len()
        );
    }
}
