# Slow request insights implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the slow-request insights drawer + speed-rail heatmap end to end - detection, dedup, aggregation, per-file auto / global / per-file threshold layering, right-side drawer UI, and a 4px continuous-gradient rail painted next to the existing minimap.

**Architecture:** Pure engine work lands in a new `clog-core::slow_requests` module (detector, aggregator, speed-grid builder, threshold struct). The Tauri app exposes four new IPC commands sharing a short-lived per-file `SlowRequestCache`, and extends the existing schema-versioned persistence layer for global + per-file thresholds. The UI grows a 4px `.speed-rail` `<canvas>` next to the minimap (continuous-gradient paint, green default) plus a new collapsible `InsightsDrawer.vue` with a sortable entry table and an inline threshold editor.

**Tech Stack:** Rust (clog-core, clog-app, serde, regex, blake3-keyed JSON persistence), Tauri v2 IPC, Vue 3 + TypeScript + `<script setup>`, HTML5 canvas.

**Spec:** [docs/superpowers/specs/2026-05-23-slow-request-insights-design.md](../specs/2026-05-23-slow-request-insights-design.md)

---

## File map

**Create**
- `crates/clog-core/src/slow_requests.rs` - detector + aggregator + speed grid + threshold struct + tests.
- `ui/src/components/InsightsDrawer.vue` - drawer (table + filter/sort + threshold editor + status chip).

**Modify**
- `crates/clog-core/src/lib.rs` - re-export the new public surface.
- `crates/clog-core/Cargo.toml` - already depends on `regex`; no change expected. Confirm during Task 1.
- `crates/clog-app/src/main.rs` - `SlowRequestCache` on `OpenedFile`, four new IPC commands, `EffectiveThresholds` struct, `SettingsPatch.slow_request_thresholds`, registrations in `invoke_handler!`, new unit tests.
- `crates/clog-app/src/persistence.rs` - `slow_request_thresholds` field on `Settings` and `PerFileRulesFile` with `#[serde(default)]`, empty-file deletion helper, new unit tests.
- `ui/src/types.ts` - `SlowRequest*`, `Speed*`, `SlowRequestThresholds`, `EffectiveThresholds`, `SlowRequestPathMode`.
- `ui/src/tab.ts` - per-tab insights state refs.
- `ui/src/components/LogViewport.vue` - 4px `.speed-rail` `<canvas>`, gradient paint, tooltip integration, drawer slot in the shell, expose `jumpToLine`.
- `ui/src/components/AppHeader.vue` - insights toggle button next to the settings cog.
- `ui/src/components/SettingsModal.vue` - new "Slow requests" section with global threshold inputs + validation.
- `ui/src/App.vue` - thread the insights toggle to the active tab.
- `ui/src/style.css` - three speed-rail palette tokens for dark + light themes, drawer width/transition tokens.
- `.wolf/anatomy.md` - updates for the new IPC surface, modules, components.
- `.wolf/memory.md` - append a one-line entry per OpenWolf protocol.

---

## Task 1: Path normalisation helper (pure, isolated)

Path normalisation is the smallest pure unit; getting it in first gives every later task a stable function to reference. TDD all the way.

**Files:**
- Create: `crates/clog-core/src/slow_requests.rs`
- Modify: `crates/clog-core/src/lib.rs`

- [ ] **Step 1: Create the module file with `PathMode` and a stub `normalise_path`**

Create `crates/clog-core/src/slow_requests.rs`:

```rust
//! Slow-request detection, aggregation, and speed-grid builder.
//!
//! Parses `SLOW REQUEST` lines emitted by Play 1.x in either of two
//! observed formats, dedupes records that report the same hit twice, and
//! groups them by (optionally normalised) URL path. A separate helper
//! buckets parsed occurrences across a fixed-count grid so the UI can
//! paint a file-wide speed heatmap.

use serde::{Deserialize, Serialize};

/// How paths are grouped in `SlowRequestSummary.entries`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathMode {
    /// Numeric / UUID / long-hex segments collapse to `{id}`, query
    /// strings are stripped, trailing slash preserved.
    Normalised,
    /// Each raw observed path is its own group.
    Raw,
}

/// Normalise a raw URL path for aggregation. See [`PathMode::Normalised`].
#[must_use]
pub fn normalise_path(raw: &str) -> String {
    raw.to_string()
}
```

Add the module to `crates/clog-core/src/lib.rs` - find the existing `pub mod` lines (e.g. `pub mod record;`) and add:

```rust
pub mod slow_requests;
pub use slow_requests::{normalise_path, PathMode};
```

- [ ] **Step 2: Write failing tests for normalisation**

In `crates/clog-core/src/slow_requests.rs`, append:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_strips_query_string() {
        assert_eq!(normalise_path("/foo?bar=1&baz=2"), "/foo");
    }

    #[test]
    fn normalise_collapses_numeric_segments() {
        assert_eq!(normalise_path("/order/12345/edit"), "/order/{id}/edit");
        assert_eq!(normalise_path("/12/34/56"), "/{id}/{id}/{id}");
    }

    #[test]
    fn normalise_collapses_uuid_segments() {
        assert_eq!(
            normalise_path("/job/3f3c8b58-0d2d-4f12-9f8f-1a0bb6e5e1aa/status"),
            "/job/{id}/status"
        );
    }

    #[test]
    fn normalise_collapses_long_hex_runs() {
        assert_eq!(normalise_path("/asset/abcdef0123456789/preview"), "/asset/{id}/preview");
    }

    #[test]
    fn normalise_preserves_trailing_slash() {
        assert_eq!(normalise_path("/foo/"), "/foo/");
        assert_eq!(normalise_path("/foo"), "/foo");
        assert_ne!(normalise_path("/foo/"), normalise_path("/foo"));
    }

    #[test]
    fn normalise_root_path_is_root() {
        assert_eq!(normalise_path("/"), "/");
    }

    #[test]
    fn normalise_leaves_human_segments_alone() {
        assert_eq!(
            normalise_path("/checkout/setdeliveryaddress.json"),
            "/checkout/setdeliveryaddress.json"
        );
    }
}
```

- [ ] **Step 3: Run the tests and confirm they fail**

```powershell
cargo test -p clog-core slow_requests::tests::normalise -- --nocapture
```

Expected: most tests FAIL because the stub just returns the input verbatim.

- [ ] **Step 4: Implement `normalise_path`**

Replace the stub in `crates/clog-core/src/slow_requests.rs` with:

```rust
#[must_use]
pub fn normalise_path(raw: &str) -> String {
    // Strip the query string at the first `?`.
    let path = match raw.find('?') {
        Some(i) => &raw[..i],
        None => raw,
    };
    if path.is_empty() {
        return String::new();
    }
    // Preserve the leading and trailing slash. `split('/')` on
    // `/a/b/` yields ["", "a", "b", ""] - we walk indices 1..len-1
    // for actual segments when the path starts with `/`.
    let leading = path.starts_with('/');
    let trailing = path.ends_with('/') && path.len() > 1;
    let mut out = String::with_capacity(path.len());
    if leading {
        out.push('/');
    }
    let mut first = true;
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        if !first {
            out.push('/');
        }
        first = false;
        out.push_str(&normalise_segment(seg));
    }
    if trailing {
        out.push('/');
    }
    out
}

fn normalise_segment(seg: &str) -> String {
    if seg.chars().all(|c| c.is_ascii_digit()) {
        return "{id}".to_string();
    }
    if is_uuid(seg) || is_long_hex(seg) {
        return "{id}".to_string();
    }
    seg.to_string()
}

fn is_uuid(seg: &str) -> bool {
    if seg.len() != 36 {
        return false;
    }
    let bytes = seg.as_bytes();
    // 8-4-4-4-12
    let dash_positions = [8usize, 13, 18, 23];
    for (i, &b) in bytes.iter().enumerate() {
        if dash_positions.contains(&i) {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

fn is_long_hex(seg: &str) -> bool {
    seg.len() >= 12 && seg.bytes().all(|b| b.is_ascii_hexdigit())
}
```

- [ ] **Step 5: Run the tests and confirm they pass**

```powershell
cargo test -p clog-core slow_requests::tests::normalise
```

Expected: all 7 normalisation tests PASS.

- [ ] **Step 6: Lint sweep**

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 7: Commit**

```powershell
git add crates/clog-core/src/slow_requests.rs crates/clog-core/src/lib.rs
git commit -m "Added the clog-core slow_requests module skeleton with the URL path normaliser. Strips query strings, collapses numeric / UUID / long-hex segments to {id}, preserves leading and trailing slashes, leaves human-readable segments alone. Seven unit tests pin the behaviour."
```

---

## Task 2: `SlowRequestThresholds` struct + clamp helper

Add the threshold pair early so persistence and IPC can reference it without circular dependencies.

**Files:**
- Modify: `crates/clog-core/src/slow_requests.rs`
- Modify: `crates/clog-core/src/lib.rs`

- [ ] **Step 1: Write failing tests for `SlowRequestThresholds::new`**

Append to the `tests` module in `crates/clog-core/src/slow_requests.rs`:

```rust
#[test]
fn thresholds_new_accepts_valid_range() {
    assert!(SlowRequestThresholds::new(1000, 5000).is_some());
    assert!(SlowRequestThresholds::new(0, 600_000).is_some());
}

#[test]
fn thresholds_new_rejects_fast_ge_slow() {
    assert!(SlowRequestThresholds::new(5000, 5000).is_none());
    assert!(SlowRequestThresholds::new(5000, 1000).is_none());
}

#[test]
fn thresholds_new_rejects_out_of_bounds() {
    assert!(SlowRequestThresholds::new(0, 600_001).is_none());
}
```

- [ ] **Step 2: Run the tests and confirm they fail to compile**

```powershell
cargo test -p clog-core slow_requests::tests::thresholds
```

Expected: compile error - `SlowRequestThresholds` undefined.

- [ ] **Step 3: Add the struct and constructor**

In `crates/clog-core/src/slow_requests.rs`, insert near the top after the `PathMode` enum:

```rust
/// Configurable speed-rail gradient anchors. When `None` at every
/// persistence tier, the rail falls back to per-file auto-normalisation.
/// When `Some`, both fields are present and `fast_ms < slow_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlowRequestThresholds {
    pub fast_ms: u32,
    pub slow_ms: u32,
}

impl SlowRequestThresholds {
    /// Maximum permitted anchor value (10 minutes in ms).
    pub const MAX_MS: u32 = 600_000;

    /// Build a validated pair. Returns `None` when `fast_ms >= slow_ms`
    /// or when either field exceeds `MAX_MS`. Stored on disk as
    /// `Option<Self>` so "unset" and "set but invalid" never collide.
    #[must_use]
    pub fn new(fast_ms: u32, slow_ms: u32) -> Option<Self> {
        if fast_ms >= slow_ms {
            return None;
        }
        if slow_ms > Self::MAX_MS {
            return None;
        }
        Some(Self { fast_ms, slow_ms })
    }
}
```

Re-export from `crates/clog-core/src/lib.rs`:

```rust
pub use slow_requests::{normalise_path, PathMode, SlowRequestThresholds};
```

- [ ] **Step 4: Run the tests and confirm they pass**

```powershell
cargo test -p clog-core slow_requests::tests::thresholds
```

Expected: all 3 pass.

- [ ] **Step 5: Lint sweep + commit**

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings

git add crates/clog-core/src/slow_requests.rs crates/clog-core/src/lib.rs
git commit -m "Added SlowRequestThresholds with a validating constructor. Both anchors must be set together, fast must be strictly less than slow, and neither may exceed ten minutes."
```

---

## Task 3: Detection regex + `RawSlowRequest` extraction

Per-record parsing of either format into a `RawSlowRequest`, message-bytes-only so continuation lines cannot mismatch.

**Files:**
- Modify: `crates/clog-core/src/slow_requests.rs`

- [ ] **Step 1: Write failing tests for the per-record extractor**

Append to the `tests` module:

```rust
fn make_message(msg: &str) -> Vec<u8> {
    msg.as_bytes().to_vec()
}

#[test]
fn extract_parses_format_a() {
    let msg = make_message(
        "SLOW REQUEST: 5064ms - /preflight/killpreflightrequest.json (SoloPreflightFront.killPreflightRequest_JSON)",
    );
    let r = extract_raw(&msg).expect("format A parses");
    assert_eq!(r.duration_ms, 5064);
    assert_eq!(r.raw_path, "/preflight/killpreflightrequest.json");
    assert_eq!(r.class_method, "SoloPreflightFront.killPreflightRequest_JSON");
}

#[test]
fn extract_parses_format_b_with_suggestion_tail() {
    let msg = make_message(
        "SLOW REQUEST (5064ms) - /preflight/killpreflightrequest.json [SoloPreflightFront.killPreflightRequest_JSON] - consider using an asynchronous call to ease the load on the threadpool.",
    );
    let r = extract_raw(&msg).expect("format B parses");
    assert_eq!(r.duration_ms, 5064);
    assert_eq!(r.raw_path, "/preflight/killpreflightrequest.json");
    assert_eq!(r.class_method, "SoloPreflightFront.killPreflightRequest_JSON");
}

#[test]
fn extract_rejects_unrelated_lines() {
    assert!(extract_raw(b"Google 360 identifier /event-tickets/").is_none());
    assert!(extract_raw(b"finalisePreflightRequest designId 14861895").is_none());
    assert!(extract_raw(b"").is_none());
}

#[test]
fn extract_rejects_anchored_substring_only() {
    // Match must start at the message head, not anywhere inside.
    assert!(extract_raw(b"prefix SLOW REQUEST: 1000ms - /x (Y.Z)").is_none());
}
```

- [ ] **Step 2: Run the tests and confirm they fail to compile**

```powershell
cargo test -p clog-core slow_requests::tests::extract
```

Expected: compile error - `RawSlowRequest` and `extract_raw` undefined.

- [ ] **Step 3: Implement the regex + extractor**

In `crates/clog-core/src/slow_requests.rs`, add at the top:

```rust
use std::sync::OnceLock;

use regex::bytes::Regex;
```

Then insert (after the `SlowRequestThresholds` block, before `tests`):

```rust
/// Per-occurrence record, pre-dedup, pre-aggregation. Internal to the
/// crate; the public output is `SlowRequestOccurrence`.
#[derive(Debug, Clone)]
pub struct RawSlowRequest {
    pub duration_ms: u32,
    pub raw_path: String,
    pub class_method: String,
}

fn slow_request_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Anchored to message head. Two duration delimiters (`:` or
        // `()`), two class.method delimiters (`()` or `[]`). The
        // double-alternation captures land in groups 1+3 (format A) or
        // 2+4 (format B). Path is whitespace-bounded between them.
        Regex::new(
            r"(?-u)^SLOW REQUEST\s*(?:: \s*(\d+)ms|\((\d+)ms\))\s*-\s*(\S+)\s+(?:\(([^)]+)\)|\[([^\]]+)\])",
        )
        .expect("slow-request regex compiles")
    })
}

/// Try to parse a single message-byte slice into a `RawSlowRequest`.
/// `None` when the bytes do not start with `SLOW REQUEST` or do not
/// match either supported phrasing.
#[must_use]
pub fn extract_raw(message: &[u8]) -> Option<RawSlowRequest> {
    let caps = slow_request_re().captures(message)?;
    let duration_bytes = caps.get(1).or_else(|| caps.get(2))?.as_bytes();
    let duration_ms: u32 = std::str::from_utf8(duration_bytes).ok()?.parse().ok()?;
    let raw_path =
        std::str::from_utf8(caps.get(3)?.as_bytes()).ok()?.to_string();
    let class_method_bytes = caps.get(4).or_else(|| caps.get(5))?.as_bytes();
    let class_method = std::str::from_utf8(class_method_bytes).ok()?.to_string();
    Some(RawSlowRequest {
        duration_ms,
        raw_path,
        class_method,
    })
}
```

Note the regex uses `(?-u)` to disable Unicode mode on `\s` / `\S` / `\d` so it walks bytes consistently. The pattern requires `:\s*N` or `(N)` for the duration and `(C.M)` or `[C.M]` for the class.method; the anchoring `^` ensures matches must start at the message head.

Also note: the pattern uses a space before `\s*` in `:\s*` and `: \s*` is intentional to handle either `: 5064ms` (space after colon) or `:5064ms`. The verbose form catches both — `(?:: \s*(\d+)ms|\((\d+)ms\))` actually requires at least one space after the colon; we accept that since both Play formats include the space.

Actually, simplify: drop the extra space so any amount of whitespace works:

Replace the regex source with:

```rust
r"(?-u)^SLOW REQUEST\s*(?::\s*(\d+)ms|\((\d+)ms\))\s*-\s*(\S+)\s+(?:\(([^)]+)\)|\[([^\]]+)\])",
```

- [ ] **Step 4: Run the extractor tests**

```powershell
cargo test -p clog-core slow_requests::tests::extract
```

Expected: all 4 pass.

- [ ] **Step 5: Lint sweep**

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean. If clippy fires on `OnceLock` or import ordering, fix inline.

- [ ] **Step 6: Confirm the `regex` crate is already a clog-core dep**

```powershell
cargo tree -p clog-core --depth 1 | findstr regex
```

Expected: `regex` listed. (It is per `crates/clog-core/Cargo.toml`.) If not, add `regex = "1"` to `[dependencies]` and re-run the tests.

- [ ] **Step 7: Commit**

```powershell
git add crates/clog-core/src/slow_requests.rs
git commit -m "Added per-record SLOW REQUEST extraction. One regex with alternation handles both observed Play formats - colon vs parenthesised duration, parens vs brackets around the class.method. Anchored to the message head so trailing copy after the closing bracket is ignored and substring matches inside other text never fire. Returns a RawSlowRequest with parsed duration, raw path and class.method."
```

---

## Task 4: Walk records into `RawSlowRequest`s (sourced from real headers)

Wrap `extract_raw` with the record-walker that slices each record's first-line message bytes via `RecordHeader.fields.message`. This is the first time the module touches `RecordHeader`.

**Files:**
- Modify: `crates/clog-core/src/slow_requests.rs`

- [ ] **Step 1: Write failing tests for `scan_raw`**

Append to the `tests` module:

```rust
use crate::pattern::{builtin_pattern, CompiledPattern, HeaderFields, ParsedHeader};
use crate::record::{Level, RecordHeader, RecordScanner};

fn wsl_dev_scanner() -> CompiledPattern {
    let src = builtin_pattern("wsl-dev").expect("wsl-dev builtin exists");
    CompiledPattern::compile(src).expect("wsl-dev compiles")
}

/// Build a (bytes, line_offsets, records) triple from a body, using the
/// wsl-dev scanner. Each `\n` ends a line.
fn make_file(body: &str) -> (Vec<u8>, Vec<u64>, Vec<RecordHeader>) {
    let scanner = wsl_dev_scanner();
    let bytes = body.as_bytes().to_vec();
    let mut line_offsets = vec![0u64];
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' && i + 1 < bytes.len() {
            line_offsets.push((i + 1) as u64);
        }
    }
    // Synthesise headers by re-parsing each line. This duplicates the
    // scan_records walk but lives in test code; using the real
    // scan_records here would pull in StreamedFile.
    let mut records: Vec<RecordHeader> = Vec::new();
    for (idx, &start) in line_offsets.iter().enumerate() {
        let end = line_offsets.get(idx + 1).copied().unwrap_or(bytes.len() as u64);
        let end_no_nl =
            if end > start && bytes[(end as usize) - 1] == b'\n' { end - 1 } else { end };
        let line = &bytes[(start as usize)..(end_no_nl as usize)];
        match scanner.try_parse_header(line) {
            Some(ParsedHeader { level, fields }) => records.push(RecordHeader {
                byte_offset: start,
                byte_len: (end - start) as u32,
                line_offset: idx as u32,
                line_count: 1,
                level,
                fields,
            }),
            None => {
                if let Some(last) = records.last_mut() {
                    last.line_count = last.line_count.saturating_add(1);
                    last.byte_len = (end - last.byte_offset) as u32;
                }
            }
        }
    }
    (bytes, line_offsets, records)
}

#[test]
fn scan_raw_extracts_one_per_matching_record() {
    let body = concat!(
        "[INFO ] 2026-05-21 00:00:04.401 [play-thread-1] play - SLOW REQUEST: 2826ms - / (CoreRender.renderPublishedPage)\n",
        "[INFO ] 2026-05-21 00:00:30.409 [play-thread-20] play - Google 360 identifier /event-tickets/\n",
        "[INFO ] 2026-05-21 00:00:44.830 [play-thread-11] play - SLOW REQUEST (5064ms) - /preflight/killpreflightrequest.json [SoloPreflightFront.killPreflightRequest_JSON] - consider using an asynchronous call to ease the load on the threadpool.\n",
    );
    let (bytes, line_offsets, records) = make_file(body);
    let raws = scan_raw(&records, &bytes, &line_offsets);
    assert_eq!(raws.len(), 2);
    assert_eq!(raws[0].raw.duration_ms, 2826);
    assert_eq!(raws[0].raw.raw_path, "/");
    assert_eq!(raws[1].raw.duration_ms, 5064);
}

#[test]
fn scan_raw_skips_continuation_lines() {
    let body = concat!(
        "[ERROR] 2026-05-21 00:00:00.000 [play-thread-1] play - boom\n",
        "    at SLOW REQUEST: 9999ms - /x (Y.Z)\n",
    );
    let (bytes, line_offsets, records) = make_file(body);
    let raws = scan_raw(&records, &bytes, &line_offsets);
    assert!(raws.is_empty(), "continuation line must not flag");
}
```

- [ ] **Step 2: Run the tests and confirm they fail to compile**

```powershell
cargo test -p clog-core slow_requests::tests::scan_raw
```

Expected: compile error - `scan_raw` undefined.

- [ ] **Step 3: Implement `scan_raw`**

Above the `tests` module in `crates/clog-core/src/slow_requests.rs`:

```rust
use crate::pattern::HeaderFields;
use crate::record::RecordHeader;

/// A `RawSlowRequest` plus the source location (line index + record
/// index) needed for the UI to scroll back to the occurrence.
#[derive(Debug, Clone)]
pub struct LocatedRaw {
    pub raw: RawSlowRequest,
    pub line_index: u64,
    pub record_idx: u32,
    pub timestamp_span: Option<(u32, u32)>,
}

/// Walk every record's first physical line message bytes and emit one
/// `LocatedRaw` per match. Continuation lines are skipped because they
/// don't have their own `RecordHeader.fields.message` slot.
#[must_use]
pub fn scan_raw(
    records: &[RecordHeader],
    bytes: &[u8],
    line_offsets: &[u64],
) -> Vec<LocatedRaw> {
    let mut out = Vec::new();
    let total_lines = line_offsets.len();
    for (rec_idx, rec) in records.iter().enumerate() {
        let message = match record_message_bytes(rec, bytes, line_offsets, total_lines) {
            Some(m) => m,
            None => continue,
        };
        let Some(raw) = extract_raw(message) else { continue };
        out.push(LocatedRaw {
            raw,
            line_index: u64::from(rec.line_offset),
            record_idx: u32::try_from(rec_idx).unwrap_or(u32::MAX),
            timestamp_span: rec.fields.timestamp,
        });
    }
    out
}

fn record_message_bytes<'a>(
    rec: &RecordHeader,
    bytes: &'a [u8],
    line_offsets: &[u64],
    total_lines: usize,
) -> Option<&'a [u8]> {
    let line_idx = rec.line_offset as usize;
    if line_idx >= total_lines {
        return None;
    }
    let line_start = usize::try_from(line_offsets[line_idx]).unwrap_or(usize::MAX);
    let line_end = if line_idx + 1 < total_lines {
        usize::try_from(line_offsets[line_idx + 1])
            .unwrap_or(usize::MAX)
            .saturating_sub(1)
    } else {
        usize::try_from(rec.byte_offset + u64::from(rec.byte_len)).unwrap_or(usize::MAX)
    };
    let line_end = line_end.min(bytes.len()).max(line_start);
    let line = &bytes[line_start..line_end];
    match rec.fields.message {
        Some((s, e)) => {
            let s = s as usize;
            let e = (e as usize).min(line.len());
            if s <= e && e <= line.len() {
                Some(&line[s..e])
            } else {
                Some(line)
            }
        }
        None => {
            // Pattern has no %msg token - degrade to whole-line match.
            Some(line)
        }
    }
}
```

Also delete the now-unused `use ... HeaderFields` if rustc flags it; the only consumer is the `fields.message` field access which is reached through `rec.fields`.

- [ ] **Step 4: Run the tests and confirm they pass**

```powershell
cargo test -p clog-core slow_requests::tests::scan_raw
```

Expected: both pass.

- [ ] **Step 5: Lint sweep + commit**

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings

git add crates/clog-core/src/slow_requests.rs
git commit -m "Added scan_raw which walks records and slices each record's first-line message bytes via RecordHeader.fields.message before running the extractor. Continuation lines are skipped because they have no header fields of their own, so a stack-trace mentioning SLOW REQUEST cannot mis-flag. Falls back to whole-line bytes when the active pattern has no %msg token."
```

---

## Task 5: Dedup, aggregation, `SlowRequestSummary`

Group `LocatedRaw`s by aggregation key, dedupe colocated duplicates, compute count / total / min / max / avg / p95 / longest_line / top-50 occurrences.

**Files:**
- Modify: `crates/clog-core/src/slow_requests.rs`

- [ ] **Step 1: Write failing tests for `extract_slow_requests`**

Append to the `tests` module:

```rust
fn parse_ts_ms(rec: &RecordHeader, bytes: &[u8]) -> Option<i64> {
    // The test fixtures use the wsl-dev pattern which renders
    // timestamps as `YYYY-MM-DD HH:MM:SS.SSS`. Stand-alone parse so
    // tests don't need to plumb the full date-parser here.
    let (s, e) = rec.fields.timestamp?;
    let slice = &bytes[(rec.byte_offset as usize + s as usize)
        ..(rec.byte_offset as usize + e as usize)];
    let s = std::str::from_utf8(slice).ok()?;
    // Cheap parse - 4-2-2 2:2:2.3
    if s.len() != 23 {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: i64 = s[5..7].parse().ok()?;
    let day: i64 = s[8..10].parse().ok()?;
    let hour: i64 = s[11..13].parse().ok()?;
    let min: i64 = s[14..16].parse().ok()?;
    let sec: i64 = s[17..19].parse().ok()?;
    let ms: i64 = s[20..23].parse().ok()?;
    // We only need a stable ordinal, not a true epoch - use a
    // monotonic encoding suitable for dedup-equality only.
    Some(
        ((year * 372 + (month - 1) * 31 + (day - 1)) * 86_400 + hour * 3600 + min * 60 + sec)
            * 1000
            + ms,
    )
}

#[test]
fn extract_aggregates_and_dedupes_format_a_and_b_at_same_ms() {
    let body = concat!(
        "[INFO ] 2026-05-21 00:00:44.830 [play-thread-11] play - SLOW REQUEST: 5064ms - /preflight/killpreflightrequest.json (SoloPreflightFront.killPreflightRequest_JSON)\n",
        "[INFO ] 2026-05-21 00:00:44.830 [play-thread-11] play - SLOW REQUEST (5064ms) - /preflight/killpreflightrequest.json [SoloPreflightFront.killPreflightRequest_JSON] - consider...\n",
    );
    let (bytes, line_offsets, records) = make_file(body);
    let summary = extract_slow_requests(
        &records,
        &bytes,
        &line_offsets,
        PathMode::Normalised,
        parse_ts_ms,
    );
    assert_eq!(summary.entries.len(), 1, "one endpoint");
    let entry = &summary.entries[0];
    assert_eq!(entry.count, 1, "dedup collapses A+B at same ts");
    assert_eq!(entry.occurrences.len(), 1);
    assert_eq!(entry.occurrences[0].dup_count, 2);
    assert_eq!(summary.total_hits, 1);
    assert_eq!(summary.deduped, 1);
}

#[test]
fn extract_does_not_dedupe_when_timestamps_differ_by_one_ms() {
    let body = concat!(
        "[INFO ] 2026-05-21 00:00:44.830 [play-thread-11] play - SLOW REQUEST: 5064ms - /x (Y.Z)\n",
        "[INFO ] 2026-05-21 00:00:44.831 [play-thread-11] play - SLOW REQUEST: 5064ms - /x (Y.Z)\n",
    );
    let (bytes, line_offsets, records) = make_file(body);
    let summary = extract_slow_requests(
        &records,
        &bytes,
        &line_offsets,
        PathMode::Normalised,
        parse_ts_ms,
    );
    assert_eq!(summary.entries.len(), 1);
    assert_eq!(summary.entries[0].count, 2);
    assert_eq!(summary.deduped, 0);
}

#[test]
fn extract_normalised_mode_merges_numeric_paths() {
    let body = concat!(
        "[INFO ] 2026-05-21 00:00:01.000 [t1] play - SLOW REQUEST: 1000ms - /order/12345/edit (X.x)\n",
        "[INFO ] 2026-05-21 00:00:02.000 [t1] play - SLOW REQUEST: 3000ms - /order/67890/edit (X.x)\n",
    );
    let (bytes, line_offsets, records) = make_file(body);
    let summary = extract_slow_requests(
        &records,
        &bytes,
        &line_offsets,
        PathMode::Normalised,
        parse_ts_ms,
    );
    assert_eq!(summary.entries.len(), 1);
    let entry = &summary.entries[0];
    assert_eq!(entry.path, "/order/{id}/edit");
    assert_eq!(entry.count, 2);
    assert_eq!(entry.total_ms, 4000);
    assert_eq!(entry.min_ms, 1000);
    assert_eq!(entry.max_ms, 3000);
    assert_eq!(entry.avg_ms, 2000);
    assert_eq!(entry.p95_ms, 3000, "nearest-rank p95 on N=2 picks the larger");
}

#[test]
fn extract_raw_mode_keeps_paths_distinct() {
    let body = concat!(
        "[INFO ] 2026-05-21 00:00:01.000 [t1] play - SLOW REQUEST: 1000ms - /order/12345/edit (X.x)\n",
        "[INFO ] 2026-05-21 00:00:02.000 [t1] play - SLOW REQUEST: 3000ms - /order/67890/edit (X.x)\n",
    );
    let (bytes, line_offsets, records) = make_file(body);
    let summary = extract_slow_requests(
        &records,
        &bytes,
        &line_offsets,
        PathMode::Raw,
        parse_ts_ms,
    );
    assert_eq!(summary.entries.len(), 2);
}

#[test]
fn extract_returns_empty_for_no_matches() {
    let body = "[INFO ] 2026-05-21 00:00:01.000 [t1] play - hello\n";
    let (bytes, line_offsets, records) = make_file(body);
    let summary = extract_slow_requests(
        &records,
        &bytes,
        &line_offsets,
        PathMode::Normalised,
        parse_ts_ms,
    );
    assert!(summary.entries.is_empty());
    assert_eq!(summary.total_hits, 0);
    assert_eq!(summary.deduped, 0);
    assert_eq!(summary.total_ms, 0);
}

#[test]
fn extract_caps_occurrences_at_50_per_entry_keeping_top_durations() {
    // 60 hits at varied durations should yield count=60 and exactly
    // 50 occurrences ordered by duration desc.
    let mut body = String::new();
    for i in 1..=60u32 {
        body.push_str(&format!(
            "[INFO ] 2026-05-21 00:{:02}:{:02}.000 [t1] play - SLOW REQUEST: {}ms - /x (Y.Z)\n",
            i / 60,
            i % 60,
            i * 100
        ));
    }
    let (bytes, line_offsets, records) = make_file(&body);
    let summary = extract_slow_requests(
        &records,
        &bytes,
        &line_offsets,
        PathMode::Normalised,
        parse_ts_ms,
    );
    assert_eq!(summary.entries.len(), 1);
    let entry = &summary.entries[0];
    assert_eq!(entry.count, 60);
    assert_eq!(entry.occurrences.len(), 50);
    // Slowest first
    assert_eq!(entry.occurrences[0].duration_ms, 6000);
    assert_eq!(entry.occurrences[49].duration_ms, 1100);
}

#[test]
fn extract_longest_line_points_at_slowest_hit() {
    let body = concat!(
        "[INFO ] 2026-05-21 00:00:01.000 [t1] play - SLOW REQUEST: 1000ms - /x (Y.Z)\n",
        "[INFO ] 2026-05-21 00:00:02.000 [t1] play - SLOW REQUEST: 9000ms - /x (Y.Z)\n",
        "[INFO ] 2026-05-21 00:00:03.000 [t1] play - SLOW REQUEST: 3000ms - /x (Y.Z)\n",
    );
    let (bytes, line_offsets, records) = make_file(body);
    let summary = extract_slow_requests(
        &records,
        &bytes,
        &line_offsets,
        PathMode::Normalised,
        parse_ts_ms,
    );
    assert_eq!(summary.entries[0].longest_line, 1, "second line is the slowest");
}
```

- [ ] **Step 2: Run the tests and confirm they fail to compile**

```powershell
cargo test -p clog-core slow_requests::tests::extract_
```

Expected: compile error - `extract_slow_requests`, `SlowRequestSummary`, etc. undefined.

- [ ] **Step 3: Implement the aggregator + structs**

Insert above the `tests` module:

```rust
use std::collections::HashMap;

/// One occurrence in the final summary, after dedup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlowRequestOccurrence {
    pub timestamp_ms: Option<i64>,
    pub duration_ms: u32,
    pub line_index: u64,
    pub record_idx: u32,
    pub dup_count: u32,
    pub class_method: String,
    pub raw_path: String,
}

/// One aggregated endpoint group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlowRequestEntry {
    pub path: String,
    pub raw_paths: Vec<String>,
    pub count: u32,
    pub total_ms: u64,
    pub min_ms: u32,
    pub max_ms: u32,
    pub avg_ms: u32,
    pub p95_ms: u32,
    pub longest_line: u64,
    pub occurrences: Vec<SlowRequestOccurrence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlowRequestSummary {
    pub entries: Vec<SlowRequestEntry>,
    pub total_hits: u32,
    pub deduped: u32,
    pub total_ms: u64,
}

/// Per-entry occurrence cap. A group with 10 000 hits keeps the
/// slowest 50 in `occurrences`; `count` still tracks the true total.
pub const OCCURRENCE_CAP: usize = 50;

/// Aggregate slow requests for an opened file. `timestamp_extractor`
/// pulls the timestamp_ms from a record so callers can choose between
/// a fast bespoke parser and a full date-format engine.
#[must_use]
pub fn extract_slow_requests<F>(
    records: &[RecordHeader],
    bytes: &[u8],
    line_offsets: &[u64],
    mode: PathMode,
    timestamp_extractor: F,
) -> SlowRequestSummary
where
    F: Fn(&RecordHeader, &[u8]) -> Option<i64>,
{
    let raws = scan_raw(records, bytes, line_offsets);
    aggregate(&raws, records, bytes, mode, &timestamp_extractor)
}

fn aggregate<F>(
    raws: &[LocatedRaw],
    records: &[RecordHeader],
    bytes: &[u8],
    mode: PathMode,
    ts_extractor: &F,
) -> SlowRequestSummary
where
    F: Fn(&RecordHeader, &[u8]) -> Option<i64>,
{
    // Pass 1: dedup. Key on (bucket, normalised_path, class_method)
    // where `bucket` is the parsed timestamp_ms or, in its absence, the
    // record's line_offset (which effectively disables dedup for that
    // record).
    #[derive(Debug)]
    struct Acc {
        kept_idx: usize,
        dup_count: u32,
    }
    let mut occurrences: Vec<SlowRequestOccurrence> = Vec::with_capacity(raws.len());
    let mut dedup_index: HashMap<(i64, String, String), Acc> = HashMap::new();
    let mut deduped = 0u32;
    for r in raws {
        let rec = &records[r.record_idx as usize];
        let timestamp_ms = ts_extractor(rec, bytes);
        let bucket = timestamp_ms.unwrap_or(i64::from(r.line_index as i32));
        let normalised = match mode {
            PathMode::Normalised => normalise_path(&r.raw.raw_path),
            PathMode::Raw => r.raw.raw_path.clone(),
        };
        let key = (bucket, normalised.clone(), r.raw.class_method.clone());
        if let Some(acc) = dedup_index.get_mut(&key) {
            acc.dup_count = acc.dup_count.saturating_add(1);
            // Earlier-line-index record wins; later one folds in.
            if r.line_index < occurrences[acc.kept_idx].line_index {
                let existing = &mut occurrences[acc.kept_idx];
                existing.line_index = r.line_index;
                existing.record_idx = r.record_idx;
            }
            occurrences[acc.kept_idx].dup_count = acc.dup_count;
            deduped = deduped.saturating_add(1);
        } else {
            dedup_index.insert(
                key,
                Acc {
                    kept_idx: occurrences.len(),
                    dup_count: 1,
                },
            );
            occurrences.push(SlowRequestOccurrence {
                timestamp_ms,
                duration_ms: r.raw.duration_ms,
                line_index: r.line_index,
                record_idx: r.record_idx,
                dup_count: 1,
                class_method: r.raw.class_method.clone(),
                raw_path: r.raw.raw_path.clone(),
            });
        }
    }

    // Pass 2: group by aggregation key.
    #[derive(Debug)]
    struct GroupAcc {
        path: String,
        raw_paths: Vec<String>,
        occs: Vec<SlowRequestOccurrence>,
    }
    let mut groups: HashMap<String, GroupAcc> = HashMap::new();
    for occ in occurrences {
        let key = match mode {
            PathMode::Normalised => normalise_path(&occ.raw_path),
            PathMode::Raw => occ.raw_path.clone(),
        };
        let g = groups.entry(key.clone()).or_insert_with(|| GroupAcc {
            path: key.clone(),
            raw_paths: Vec::new(),
            occs: Vec::new(),
        });
        if !g.raw_paths.contains(&occ.raw_path) {
            g.raw_paths.push(occ.raw_path.clone());
        }
        g.occs.push(occ);
    }

    let mut entries: Vec<SlowRequestEntry> = groups
        .into_values()
        .map(|mut g| {
            let count = u32::try_from(g.occs.len()).unwrap_or(u32::MAX);
            let mut durations: Vec<u32> = g.occs.iter().map(|o| o.duration_ms).collect();
            let total_ms: u64 = durations.iter().copied().map(u64::from).sum();
            let min_ms = *durations.iter().min().unwrap_or(&0);
            let max_ms = *durations.iter().max().unwrap_or(&0);
            let avg_ms = if count == 0 {
                0
            } else {
                u32::try_from(total_ms / u64::from(count)).unwrap_or(u32::MAX)
            };
            // Nearest-rank p95: sort ascending, pick index ceil(0.95*n)-1.
            durations.sort_unstable();
            let p95_idx = if durations.is_empty() {
                0
            } else {
                let n = durations.len();
                (((n as f64) * 0.95).ceil() as usize).saturating_sub(1).min(n - 1)
            };
            let p95_ms = durations.get(p95_idx).copied().unwrap_or(0);
            // Top-N by duration desc.
            g.occs.sort_unstable_by(|a, b| b.duration_ms.cmp(&a.duration_ms));
            let longest_line = g.occs.first().map_or(0, |o| o.line_index);
            g.occs.truncate(OCCURRENCE_CAP);
            SlowRequestEntry {
                path: g.path,
                raw_paths: g.raw_paths,
                count,
                total_ms,
                min_ms,
                max_ms,
                avg_ms,
                p95_ms,
                longest_line,
                occurrences: g.occs,
            }
        })
        .collect();
    entries.sort_unstable_by(|a, b| b.total_ms.cmp(&a.total_ms));

    let total_hits = entries.iter().map(|e| e.count).sum();
    let total_ms = entries.iter().map(|e| e.total_ms).sum();
    SlowRequestSummary {
        entries,
        total_hits,
        deduped,
        total_ms,
    }
}
```

Re-export the new types from `crates/clog-core/src/lib.rs`:

```rust
pub use slow_requests::{
    extract_slow_requests, normalise_path, PathMode, SlowRequestEntry,
    SlowRequestOccurrence, SlowRequestSummary, SlowRequestThresholds,
};
```

- [ ] **Step 4: Run the tests and confirm they pass**

```powershell
cargo test -p clog-core slow_requests::tests::extract_
```

Expected: all 7 aggregation tests pass.

- [ ] **Step 5: Run the entire slow_requests test set**

```powershell
cargo test -p clog-core slow_requests
```

Expected: all green.

- [ ] **Step 6: Lint sweep + commit**

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings

git add crates/clog-core/src/slow_requests.rs crates/clog-core/src/lib.rs
git commit -m "Added the slow-request aggregator. Dedups records on (timestamp_ms, normalised_path, class_method) with earlier-line-index winning and later occurrences folding into a dup_count. Groups by normalised or raw path, computes count / total / min / max / avg / nearest-rank p95, caps per-entry occurrences at fifty by duration desc, returns groups sorted by total time desc.

Eight new unit tests cover format-A-and-B dedup at the same ms, no-dedup with a one-ms gap, normalised vs raw merging, the empty-input case, the occurrence cap with sixty hits, and longest_line pointing at the slowest hit."
```

---

## Task 6: Speed grid builder

Bucket dedupe-passed occurrences across a fixed grid, computing avg / max per bucket plus file-wide min / max.

**Files:**
- Modify: `crates/clog-core/src/slow_requests.rs`
- Modify: `crates/clog-core/src/lib.rs`

- [ ] **Step 1: Write failing tests for `build_speed_grid`**

Append to the `tests` module:

```rust
fn occs(line_ms: &[(u64, u32)]) -> Vec<SlowRequestOccurrence> {
    line_ms
        .iter()
        .map(|&(line, ms)| SlowRequestOccurrence {
            timestamp_ms: None,
            duration_ms: ms,
            line_index: line,
            record_idx: 0,
            dup_count: 1,
            class_method: "X.x".into(),
            raw_path: "/x".into(),
        })
        .collect()
}

#[test]
fn speed_grid_buckets_occurrences_across_grid() {
    // 10 lines, 2 buckets. Lines 0-4 -> bucket 0, lines 5-9 -> bucket 1.
    let o = occs(&[(0, 1000), (4, 3000), (5, 2000), (9, 8000)]);
    let g = build_speed_grid(&o, 10, 2);
    assert_eq!(g.buckets.len(), 2);
    assert_eq!(g.buckets[0].count, 2);
    assert_eq!(g.buckets[0].avg_ms, 2000);
    assert_eq!(g.buckets[0].max_ms, 3000);
    assert_eq!(g.buckets[1].count, 2);
    assert_eq!(g.buckets[1].avg_ms, 5000);
    assert_eq!(g.buckets[1].max_ms, 8000);
    assert_eq!(g.min_avg_ms, 2000);
    assert_eq!(g.max_avg_ms, 5000);
}

#[test]
fn speed_grid_empty_input_yields_zeroed_buckets() {
    let g = build_speed_grid(&[], 100, 4);
    assert_eq!(g.buckets.len(), 4);
    for b in &g.buckets {
        assert_eq!(b.count, 0);
        assert_eq!(b.avg_ms, 0);
        assert_eq!(b.max_ms, 0);
    }
    assert_eq!(g.min_avg_ms, 0);
    assert_eq!(g.max_avg_ms, 0);
}

#[test]
fn speed_grid_degenerate_spread_collapses_min_eq_max() {
    let o = occs(&[(0, 1000), (5, 1000)]);
    let g = build_speed_grid(&o, 10, 2);
    assert_eq!(g.min_avg_ms, 1000);
    assert_eq!(g.max_avg_ms, 1000);
}

#[test]
fn speed_grid_clamps_overflow_bucket_to_last() {
    // Edge: line_index equal to line_count should still land in the last bucket.
    let o = occs(&[(10, 5000)]);
    let g = build_speed_grid(&o, 10, 2);
    assert_eq!(g.buckets[1].count, 1);
}
```

- [ ] **Step 2: Run the tests and confirm they fail to compile**

```powershell
cargo test -p clog-core slow_requests::tests::speed_grid
```

Expected: compile error - `build_speed_grid`, `SpeedBucket`, `SpeedGrid` undefined.

- [ ] **Step 3: Implement the speed grid**

Insert before the `tests` module:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SpeedBucket {
    pub count: u32,
    pub avg_ms: u32,
    pub max_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeedGrid {
    pub buckets: Vec<SpeedBucket>,
    pub min_avg_ms: u32,
    pub max_avg_ms: u32,
}

#[must_use]
pub fn build_speed_grid(
    occurrences: &[SlowRequestOccurrence],
    line_count: u64,
    bucket_count: usize,
) -> SpeedGrid {
    let bucket_count = bucket_count.max(1);
    let empty = SpeedBucket {
        count: 0,
        avg_ms: 0,
        max_ms: 0,
    };
    if line_count == 0 || occurrences.is_empty() {
        return SpeedGrid {
            buckets: vec![empty; bucket_count],
            min_avg_ms: 0,
            max_avg_ms: 0,
        };
    }
    let mut sums = vec![0u64; bucket_count];
    let mut counts = vec![0u32; bucket_count];
    let mut maxes = vec![0u32; bucket_count];
    let bc = bucket_count as u64;
    for occ in occurrences {
        let mut b = (occ.line_index.saturating_mul(bc) / line_count) as usize;
        if b >= bucket_count {
            b = bucket_count - 1;
        }
        sums[b] = sums[b].saturating_add(u64::from(occ.duration_ms));
        counts[b] = counts[b].saturating_add(1);
        if occ.duration_ms > maxes[b] {
            maxes[b] = occ.duration_ms;
        }
    }
    let mut buckets = Vec::with_capacity(bucket_count);
    let mut min_avg = u32::MAX;
    let mut max_avg = 0u32;
    let mut any = false;
    for i in 0..bucket_count {
        let count = counts[i];
        let avg = if count == 0 {
            0
        } else {
            u32::try_from(sums[i] / u64::from(count)).unwrap_or(u32::MAX)
        };
        if count > 0 {
            any = true;
            if avg < min_avg {
                min_avg = avg;
            }
            if avg > max_avg {
                max_avg = avg;
            }
        }
        buckets.push(SpeedBucket {
            count,
            avg_ms: avg,
            max_ms: maxes[i],
        });
    }
    SpeedGrid {
        buckets,
        min_avg_ms: if any { min_avg } else { 0 },
        max_avg_ms: max_avg,
    }
}
```

Re-export in `crates/clog-core/src/lib.rs`:

```rust
pub use slow_requests::{
    build_speed_grid, extract_slow_requests, normalise_path, PathMode,
    SlowRequestEntry, SlowRequestOccurrence, SlowRequestSummary,
    SlowRequestThresholds, SpeedBucket, SpeedGrid,
};
```

- [ ] **Step 4: Run the tests and confirm they pass**

```powershell
cargo test -p clog-core slow_requests::tests::speed_grid
```

Expected: all 4 pass.

- [ ] **Step 5: Full workspace test sweep + commit**

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

git add crates/clog-core/src/slow_requests.rs crates/clog-core/src/lib.rs
git commit -m "Added build_speed_grid which buckets deduped occurrences across a fixed grid and reports per-bucket count / avg / max plus the file-wide min and max bucket averages. Mirrors the level minimap's bucket geometry so the speed rail aligns row-for-row. Four unit tests cover happy-path bucketing, empty input, degenerate single-value spread, and the line-equals-line-count edge clamping into the last bucket."
```

---

## Task 7: Persistence - add `slow_request_thresholds` to `Settings` and `PerFileRulesFile`

Extend the two existing JSON schemas with optional threshold fields, back-compat covered by `#[serde(default)]`. Add an "auto-delete when empty" helper to `PerFileRulesFile`.

**Files:**
- Modify: `crates/clog-app/src/persistence.rs`

- [ ] **Step 1: Write failing tests for back-compat + round-trip + auto-delete**

Locate the existing `#[cfg(test)] mod tests` block at the bottom of `crates/clog-app/src/persistence.rs`. Append (or create the block if it doesn't already exist):

```rust
#[cfg(test)]
mod thresholds_tests {
    use super::*;
    use clog_core::SlowRequestThresholds;

    #[test]
    fn settings_loads_old_file_without_threshold_field() {
        let raw = r#"{"schema":1,"theme":"dark","font_size":13,"recent_files":[],"follow_tail_default":true}"#;
        let s: Settings = serde_json::from_str(raw).expect("v1 settings decodes");
        assert!(s.slow_request_thresholds.is_none());
    }

    #[test]
    fn settings_round_trips_thresholds() {
        let mut s = Settings::default();
        s.slow_request_thresholds = SlowRequestThresholds::new(1000, 5000);
        let json = serde_json::to_string(&s).expect("serialises");
        let back: Settings = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(back.slow_request_thresholds, s.slow_request_thresholds);
    }

    #[test]
    fn per_file_rules_loads_old_file_without_threshold_field() {
        let raw = r#"{"schema":1,"path":"/x","rules":[]}"#;
        let f: PerFileRulesFile =
            serde_json::from_str(raw).expect("v1 per-file decodes");
        assert!(f.slow_request_thresholds.is_none());
    }

    #[test]
    fn per_file_rules_is_empty_when_no_rules_and_no_thresholds() {
        let f = PerFileRulesFile::default();
        assert!(f.is_effectively_empty());
        let f2 = PerFileRulesFile {
            slow_request_thresholds: SlowRequestThresholds::new(100, 200),
            ..PerFileRulesFile::default()
        };
        assert!(!f2.is_effectively_empty());
    }
}
```

- [ ] **Step 2: Run the tests and confirm they fail to compile**

```powershell
cargo test -p clog-app persistence::thresholds_tests
```

Expected: compile error - `slow_request_thresholds` field missing, `is_effectively_empty` undefined.

- [ ] **Step 3: Add the field to `Settings`**

In `crates/clog-app/src/persistence.rs`, find the `pub struct Settings { ... }` block and add the field:

```rust
#[serde(default)]
pub slow_request_thresholds: Option<SlowRequestThresholds>,
```

Add the import near the top of the file:

```rust
use clog_core::SlowRequestThresholds;
```

Update `Settings::default()` (around line 44) to include:

```rust
slow_request_thresholds: None,
```

- [ ] **Step 4: Add the field to `PerFileRulesFile` and the `is_effectively_empty` helper**

Find the `pub struct PerFileRulesFile { ... }` block and add:

```rust
#[serde(default)]
pub slow_request_thresholds: Option<SlowRequestThresholds>,
```

In the `impl PerFileRulesFile` block, append:

```rust
/// True when this file holds no highlight rules and no slow-request
/// thresholds. Callers use this to decide whether `save` should
/// instead delete the file - leaving an empty stub on disk is wasted
/// I/O and confuses readers grepping the per-file-rules folder.
#[must_use]
pub fn is_effectively_empty(&self) -> bool {
    self.rules.is_empty() && self.slow_request_thresholds.is_none()
}
```

- [ ] **Step 5: Run the tests and confirm they pass**

```powershell
cargo test -p clog-app persistence::thresholds_tests
```

Expected: all 4 pass.

- [ ] **Step 6: Workspace lint + test sweep**

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: all green. Existing persistence tests must still pass.

- [ ] **Step 7: Commit**

```powershell
git add crates/clog-app/src/persistence.rs
git commit -m "Extended the persistence layer with an optional slow_request_thresholds field on both Settings (global default) and PerFileRulesFile (per-file override), with #[serde(default)] so existing v1 files load cleanly with None. Added PerFileRulesFile::is_effectively_empty which the upcoming save IPC uses to delete the on-disk file rather than leave an empty stub when the user clears both their rules and their thresholds. Four new unit tests pin the back-compat and round-trip behaviour."
```

---

## Task 8: clog-app IPC - `SlowRequestCache` + `get_slow_requests` + `get_slow_request_speeds`

Add the per-file cache, both read IPCs, and a timestamp parser that reuses the file's compiled pattern.

**Files:**
- Modify: `crates/clog-app/src/main.rs`

- [ ] **Step 1: Add the cache field to `OpenedFile`**

In `crates/clog-app/src/main.rs`, find the `struct OpenedFile { ... }` block (around line 127). Add at the end of the struct, before the closing brace:

```rust
/// Cached slow-request occurrences. Rebuilt lazily on first call
/// after any change to (records, bytes, pattern). Both
/// `get_slow_requests` and `get_slow_request_speeds` read this.
slow_request_cache: Option<SlowRequestCache>,
```

Above the struct definition, add:

```rust
#[derive(Debug, Clone)]
struct SlowRequestCache {
    /// Snapshot signature: `(records.len(), bytes.len(), pattern_hash)`.
    /// Invalidated automatically when any of the three changes.
    signature: (u64, u64, u64),
    occurrences: Vec<clog_core::SlowRequestOccurrence>,
}
```

Find every site that constructs an `OpenedFile { ... }` literal (use `grep -n "OpenedFile {" crates/clog-app/src/main.rs`). Add `slow_request_cache: None,` to each.

- [ ] **Step 2: Add a `pattern_hash` helper and the timestamp extractor**

Above `extend_with_appended`, add:

```rust
fn pattern_hash(pattern_source: &str) -> u64 {
    let h = blake3::hash(pattern_source.as_bytes());
    let bytes = h.as_bytes();
    u64::from_le_bytes(bytes[..8].try_into().unwrap_or([0; 8]))
}

/// Pull a timestamp out of a record by re-rendering the slice covered
/// by `RecordHeader.fields.timestamp` and feeding it through a cheap
/// `YYYY-MM-DD HH:MM:SS.sss` parser. Returns `None` when the pattern
/// produced no timestamp field or the bytes don't match the expected
/// shape - dedup callers fall back to the line index in that case.
fn extract_record_timestamp_ms(rec: &RecordHeader, bytes: &[u8]) -> Option<i64> {
    let (s, e) = rec.fields.timestamp?;
    let start = (rec.byte_offset as usize).saturating_add(s as usize);
    let end = (rec.byte_offset as usize).saturating_add(e as usize);
    let slice = bytes.get(start..end)?;
    let text = std::str::from_utf8(slice).ok()?;
    parse_yyyy_mm_dd_hh_mm_ss_sss(text)
}

fn parse_yyyy_mm_dd_hh_mm_ss_sss(s: &str) -> Option<i64> {
    if s.len() != 23 {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: i64 = s[5..7].parse().ok()?;
    let day: i64 = s[8..10].parse().ok()?;
    let hour: i64 = s[11..13].parse().ok()?;
    let min: i64 = s[14..16].parse().ok()?;
    let sec: i64 = s[17..19].parse().ok()?;
    let ms: i64 = s[20..23].parse().ok()?;
    Some(
        ((year * 372 + (month - 1) * 31 + (day - 1)) * 86_400
            + hour * 3600
            + min * 60
            + sec)
            * 1000
            + ms,
    )
}
```

The `year * 372 + ...` encoding is a stable ordinal - it does not produce a true epoch but is monotonic and unique per `(year, month, day)` triple, which is all dedup-equality needs.

- [ ] **Step 3: Add the cache builder + both IPC commands**

Insert above the existing `get_level_minimap` command (around line 741):

```rust
fn rebuild_slow_request_cache(file: &mut OpenedFile) -> &[clog_core::SlowRequestOccurrence] {
    let signature = (
        file.records.len() as u64,
        file.bytes.len() as u64,
        pattern_hash(&file.pattern_source),
    );
    let needs_rebuild = file
        .slow_request_cache
        .as_ref()
        .is_none_or(|c| c.signature != signature);
    if needs_rebuild {
        let summary = clog_core::extract_slow_requests(
            &file.records,
            &file.bytes,
            &file.line_offsets,
            clog_core::PathMode::Raw, // dedup uses raw paths only; later aggregator re-normalises
            extract_record_timestamp_ms,
        );
        // We only need the deduped occurrences here, not the grouped
        // entries; the entries IPC reconstitutes them per call so a
        // mode flip doesn't re-scan the file.
        let mut occurrences: Vec<clog_core::SlowRequestOccurrence> = Vec::new();
        for entry in summary.entries {
            occurrences.extend(entry.occurrences);
        }
        file.slow_request_cache = Some(SlowRequestCache {
            signature,
            occurrences,
        });
    }
    &file
        .slow_request_cache
        .as_ref()
        .expect("cache built above")
        .occurrences
}

#[tauri::command]
fn get_slow_requests(
    state: State<'_, AppState>,
    file_id: u64,
    mode: clog_core::PathMode,
) -> Result<clog_core::SlowRequestSummary, IpcError> {
    let mut guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get_mut(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;
    let _ = rebuild_slow_request_cache(file);
    // Re-aggregate from the cached occurrences. The cache was built
    // with `PathMode::Raw` so every observed path is preserved; we now
    // re-bucket per the caller's chosen mode without re-scanning the
    // file.
    let occs = file
        .slow_request_cache
        .as_ref()
        .expect("rebuild leaves cache populated")
        .occurrences
        .clone();
    Ok(reaggregate_from_cache(&occs, mode))
}

fn reaggregate_from_cache(
    occs: &[clog_core::SlowRequestOccurrence],
    mode: clog_core::PathMode,
) -> clog_core::SlowRequestSummary {
    // The aggregator wants raw inputs; we already have deduped
    // occurrences. Rebuild groups directly here to skip the dedup pass.
    use std::collections::HashMap;
    struct G {
        path: String,
        raw_paths: Vec<String>,
        occs: Vec<clog_core::SlowRequestOccurrence>,
    }
    let mut groups: HashMap<String, G> = HashMap::new();
    for occ in occs {
        let key = match mode {
            clog_core::PathMode::Normalised => clog_core::normalise_path(&occ.raw_path),
            clog_core::PathMode::Raw => occ.raw_path.clone(),
        };
        let g = groups.entry(key.clone()).or_insert_with(|| G {
            path: key.clone(),
            raw_paths: Vec::new(),
            occs: Vec::new(),
        });
        if !g.raw_paths.contains(&occ.raw_path) {
            g.raw_paths.push(occ.raw_path.clone());
        }
        g.occs.push(occ.clone());
    }
    let mut entries: Vec<clog_core::SlowRequestEntry> = groups
        .into_values()
        .map(|mut g| {
            let count = u32::try_from(g.occs.len()).unwrap_or(u32::MAX);
            let mut durations: Vec<u32> = g.occs.iter().map(|o| o.duration_ms).collect();
            let total_ms: u64 = durations.iter().copied().map(u64::from).sum();
            let min_ms = *durations.iter().min().unwrap_or(&0);
            let max_ms = *durations.iter().max().unwrap_or(&0);
            let avg_ms = if count == 0 {
                0
            } else {
                u32::try_from(total_ms / u64::from(count)).unwrap_or(u32::MAX)
            };
            durations.sort_unstable();
            let n = durations.len();
            let p95_ms = if n == 0 {
                0
            } else {
                let idx = (((n as f64) * 0.95).ceil() as usize).saturating_sub(1).min(n - 1);
                durations[idx]
            };
            g.occs.sort_unstable_by(|a, b| b.duration_ms.cmp(&a.duration_ms));
            let longest_line = g.occs.first().map_or(0, |o| o.line_index);
            g.occs.truncate(clog_core::slow_requests::OCCURRENCE_CAP);
            clog_core::SlowRequestEntry {
                path: g.path,
                raw_paths: g.raw_paths,
                count,
                total_ms,
                min_ms,
                max_ms,
                avg_ms,
                p95_ms,
                longest_line,
                occurrences: g.occs,
            }
        })
        .collect();
    entries.sort_unstable_by(|a, b| b.total_ms.cmp(&a.total_ms));
    let total_hits = entries.iter().map(|e| e.count).sum();
    let total_ms = entries.iter().map(|e| e.total_ms).sum();
    let deduped = occs.iter().map(|o| o.dup_count.saturating_sub(1)).sum();
    clog_core::SlowRequestSummary {
        entries,
        total_hits,
        deduped,
        total_ms,
    }
}

#[tauri::command]
fn get_slow_request_speeds(
    state: State<'_, AppState>,
    file_id: u64,
    bucket_count: u32,
) -> Result<clog_core::SpeedGrid, IpcError> {
    let mut guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get_mut(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;
    let line_count = file.line_count;
    let _ = rebuild_slow_request_cache(file);
    let occs = &file
        .slow_request_cache
        .as_ref()
        .expect("rebuild leaves cache populated")
        .occurrences;
    Ok(clog_core::build_speed_grid(
        occs,
        line_count,
        bucket_count as usize,
    ))
}
```

`OCCURRENCE_CAP` must be public; add `pub const OCCURRENCE_CAP: usize = 50;` to `crates/clog-core/src/slow_requests.rs` (replace the existing private const). Then re-export it in `clog-core/src/lib.rs` via `pub use slow_requests::OCCURRENCE_CAP;` if not already.

- [ ] **Step 4: Register both commands in `invoke_handler!`**

In `crates/clog-app/src/main.rs`, find the `tauri::generate_handler![...]` list (around line 1651). Add the two commands after `get_markers`:

```rust
            get_markers,
            get_slow_requests,
            get_slow_request_speeds,
            start_search,
```

- [ ] **Step 5: Add a smoke test against the prod fixture**

In the existing `#[cfg(test)] mod tests` block at the bottom of `crates/clog-app/src/main.rs`, append:

```rust
#[test]
fn slow_request_smoke_against_prod_fixture() {
    use std::path::Path;
    let path = Path::new("..").join("..").join("research").join("solopress-prod.log");
    if !path.exists() {
        // Fixture is gitignored; skip silently when absent.
        return;
    }
    let pattern_src = clog_core::builtin_pattern("prod").expect("prod builtin");
    let scanner = clog_core::CompiledPattern::compile(pattern_src).expect("compiles");
    let (mut source, line_index, records) =
        clog_core::index_file(&path, &scanner).expect("indexes");
    let bytes = source.read_all().expect("read_all");
    let line_offsets = line_index.line_offsets;
    let summary = clog_core::extract_slow_requests(
        &records,
        &bytes,
        &line_offsets,
        clog_core::PathMode::Normalised,
        extract_record_timestamp_ms,
    );
    assert!(
        summary.total_hits > 0,
        "prod fixture must contain at least one slow request"
    );
    assert!(summary.entries.iter().any(|e| e.path.contains("preflight")));
}
```

- [ ] **Step 6: Run the new tests**

```powershell
cargo test -p clog-app slow_request
```

Expected: all green. The smoke test prints a count and returns early if the gitignored fixture is missing.

- [ ] **Step 7: Workspace sweep + commit**

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

git add crates/clog-app/src/main.rs crates/clog-core/src/slow_requests.rs crates/clog-core/src/lib.rs
git commit -m "Added the two read IPCs - get_slow_requests and get_slow_request_speeds - backed by a short-lived SlowRequestCache on OpenedFile keyed by (records.len, bytes.len, pattern_hash). The cache stores deduped occurrences once per signature; flipping path mode or fetching the speed grid never re-scans the file, only re-aggregates the cached occurrences. Added a YYYY-MM-DD HH:MM:SS.sss timestamp parser as the dedup-equality key extractor. Made OCCURRENCE_CAP public so the IPC re-aggregator uses the same cap as the core helper. Registered both new commands in the invoke_handler and added a prod-fixture smoke test."
```

---

## Task 9: clog-app IPC - threshold commands + `update_settings` extension

`get_slow_request_thresholds`, `save_slow_request_thresholds`, and a one-line addition to `SettingsPatch`.

**Files:**
- Modify: `crates/clog-app/src/main.rs`

- [ ] **Step 1: Extend `SettingsPatch` with the optional global field**

Find the `SettingsPatch` definition (around line 1557) and add:

```rust
#[derive(Debug, serde::Deserialize)]
pub struct SettingsPatch {
    pub theme: Option<String>,
    pub font_size: Option<u32>,
    pub follow_tail_default: Option<bool>,
    /// Set to `Some(Some(thresholds))` to update, `Some(None)` to clear,
    /// `None` to leave untouched. Validation happens here, not in the
    /// UI - invalid values are rejected with `BadInput`.
    #[serde(default, deserialize_with = "deserialize_optional_optional")]
    pub slow_request_thresholds: Option<Option<clog_core::SlowRequestThresholds>>,
}

/// Custom deserialiser that distinguishes "field absent" (decodes to
/// `None`) from "field present and null" (decodes to `Some(None)`).
/// Serde's default `Option<Option<T>>` decode collapses both into
/// `None`, which would make "clear the global default" impossible to
/// express on the wire.
fn deserialize_optional_optional<'de, D, T>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}
```

Update the `update_settings` body to handle the new patch field:

```rust
    if let Some(opt) = patch.slow_request_thresholds {
        // Validate when Some(Some(...)); reject the whole IPC if the
        // anchors fail SlowRequestThresholds::new's invariants. The
        // wire type carries the un-validated pair so the UI's
        // disabled-save-on-invalid is belt-and-braces.
        match opt {
            Some(t) => match clog_core::SlowRequestThresholds::new(t.fast_ms, t.slow_ms) {
                Some(valid) => s.slow_request_thresholds = Some(valid),
                None => {
                    return Err(IpcError::BadPattern {
                        message: format!(
                            "invalid slow_request_thresholds: fast={} slow={}",
                            t.fast_ms, t.slow_ms
                        ),
                    })
                }
            },
            None => s.slow_request_thresholds = None,
        }
    }
```

Reusing `IpcError::BadPattern` is a slight semantic stretch; if a `BadInput` variant feels cleaner, add it to the enum and the error variants in one go. The first cut here uses `BadPattern` to avoid a wider edit; refactor the error taxonomy in a separate change if it becomes a pattern.

- [ ] **Step 2: Add `EffectiveThresholds` and the two new commands**

Insert above the existing settings-related commands (or near the bottom of the file before the `tauri::Builder` block):

```rust
#[derive(Debug, Clone, Serialize)]
struct EffectiveThresholds {
    source: &'static str,
    effective: clog_core::SlowRequestThresholds,
    per_file: Option<clog_core::SlowRequestThresholds>,
    global: Option<clog_core::SlowRequestThresholds>,
}

#[tauri::command]
fn get_slow_request_thresholds(
    state: State<'_, AppState>,
    file_id: u64,
) -> Result<EffectiveThresholds, IpcError> {
    let path = {
        let guard = state.files.lock().expect("files mutex poisoned");
        let file = guard
            .get(&file_id)
            .ok_or(IpcError::UnknownFile { file_id })?;
        file.path.clone()
    };
    let per_file = persistence::PerFileRulesFile::load(&path).slow_request_thresholds;
    let global = persistence::Settings::load().slow_request_thresholds;
    let (effective, source) = if let Some(t) = per_file {
        (t, "per_file")
    } else if let Some(t) = global {
        (t, "global")
    } else {
        // Auto: use the current speed grid extremes.
        let mut guard = state.files.lock().expect("files mutex poisoned");
        let file = guard
            .get_mut(&file_id)
            .ok_or(IpcError::UnknownFile { file_id })?;
        let line_count = file.line_count;
        let _ = rebuild_slow_request_cache(file);
        let occs = &file
            .slow_request_cache
            .as_ref()
            .expect("rebuild leaves cache populated")
            .occurrences;
        let g = clog_core::build_speed_grid(occs, line_count, 256);
        let fast = g.min_avg_ms;
        let slow = g.max_avg_ms.max(fast.saturating_add(1));
        (
            clog_core::SlowRequestThresholds::new(fast, slow)
                .unwrap_or_else(|| clog_core::SlowRequestThresholds::new(0, 1).expect("valid")),
            "auto",
        )
    };
    Ok(EffectiveThresholds {
        source,
        effective,
        per_file,
        global,
    })
}

#[tauri::command]
fn save_slow_request_thresholds(
    state: State<'_, AppState>,
    file_id: u64,
    thresholds: Option<clog_core::SlowRequestThresholds>,
) -> Result<(), IpcError> {
    let path = {
        let guard = state.files.lock().expect("files mutex poisoned");
        let file = guard
            .get(&file_id)
            .ok_or(IpcError::UnknownFile { file_id })?;
        file.path.clone()
    };
    // Validate. Pass-through clears (None) are always accepted; set
    // values must round-trip through SlowRequestThresholds::new.
    let validated = match thresholds {
        Some(t) => match clog_core::SlowRequestThresholds::new(t.fast_ms, t.slow_ms) {
            Some(v) => Some(v),
            None => {
                return Err(IpcError::BadPattern {
                    message: format!(
                        "invalid thresholds: fast={} slow={}",
                        t.fast_ms, t.slow_ms
                    ),
                })
            }
        },
        None => None,
    };
    let mut f = persistence::PerFileRulesFile::load(&path);
    f.slow_request_thresholds = validated;
    if f.is_effectively_empty() {
        persistence::PerFileRulesFile::forget(&path).map_err(|e| IpcError::Io {
            message: e.to_string(),
            path: path.display().to_string(),
        })?;
    } else {
        f.save(&path).map_err(|e| IpcError::Io {
            message: e.to_string(),
            path: path.display().to_string(),
        })?;
    }
    Ok(())
}
```

- [ ] **Step 3: Register both new commands in `invoke_handler!`**

```rust
            get_slow_requests,
            get_slow_request_speeds,
            get_slow_request_thresholds,
            save_slow_request_thresholds,
            start_search,
```

- [ ] **Step 4: Write a unit test for `update_settings` rejecting invalid thresholds**

Append to the `#[cfg(test)] mod tests` block (the same one with the level-minimap tests):

```rust
#[test]
fn update_settings_rejects_invalid_thresholds() {
    // fast == slow is rejected at the IPC validation layer.
    let result = (|| -> Result<(), IpcError> {
        let mut s = persistence::Settings::default();
        let patch_pair = clog_core::SlowRequestThresholds {
            fast_ms: 1000,
            slow_ms: 1000,
        };
        match clog_core::SlowRequestThresholds::new(patch_pair.fast_ms, patch_pair.slow_ms) {
            Some(valid) => s.slow_request_thresholds = Some(valid),
            None => {
                return Err(IpcError::BadPattern {
                    message: "rejected".into(),
                })
            }
        }
        Ok(())
    })();
    assert!(result.is_err());
}
```

This test mirrors the validation branch in `update_settings` without spinning up a Tauri runtime; for the full IPC plumbing the smoke test in Task 16 covers it.

- [ ] **Step 5: Workspace sweep + commit**

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

git add crates/clog-app/src/main.rs
git commit -m "Added the two threshold IPCs. get_slow_request_thresholds resolves the effective anchors through per-file then global then auto, returning the active value alongside the source tag and the two raw tiers so the UI can render its Auto / Global / Per-file chip and populate the editor with what the user is inheriting. save_slow_request_thresholds validates, writes the per-file file, and deletes it when the resulting file holds no rules and no thresholds. Extended SettingsPatch with an optional slow_request_thresholds field for the global tier and routed it through the existing update_settings command."
```

---

## Task 10: UI wire shapes + per-tab state

TypeScript mirrors of every new Rust wire shape, plus the per-tab refs that drive the drawer.

**Files:**
- Modify: `ui/src/types.ts`
- Modify: `ui/src/tab.ts`

- [ ] **Step 1: Add types to `ui/src/types.ts`**

After the existing `TailDelta` interface, append:

```ts
// --- Slow request insights --------------------------------------------------

export type SlowRequestPathMode = 'normalised' | 'raw'

export interface SlowRequestThresholds {
  fast_ms: number
  slow_ms: number
}

export type ThresholdSource = 'auto' | 'global' | 'per_file'

export interface EffectiveThresholds {
  source: ThresholdSource
  effective: SlowRequestThresholds
  per_file: SlowRequestThresholds | null
  global: SlowRequestThresholds | null
}

export interface SpeedBucket {
  count: number
  avg_ms: number
  max_ms: number
}

export interface SpeedGrid {
  buckets: SpeedBucket[]
  min_avg_ms: number
  max_avg_ms: number
}

export interface SlowRequestOccurrence {
  timestamp_ms: number | null
  duration_ms: number
  line_index: number
  record_idx: number
  dup_count: number
  class_method: string
  raw_path: string
}

export interface SlowRequestEntry {
  path: string
  raw_paths: string[]
  count: number
  total_ms: number
  min_ms: number
  max_ms: number
  avg_ms: number
  p95_ms: number
  longest_line: number
  occurrences: SlowRequestOccurrence[]
}

export interface SlowRequestSummary {
  entries: SlowRequestEntry[]
  total_hits: number
  deduped: number
  total_ms: number
}
```

- [ ] **Step 2: Add per-tab refs to `ui/src/tab.ts`**

Find the `createTab` factory. Locate the section that builds the returned object (where `searchQuery`, `bookmarks`, etc. are assembled). Add:

```ts
const insightsOpen = ref<boolean>(false)
const slowRequestMode = ref<SlowRequestPathMode>('normalised')
const slowRequestSort = ref<{
  field: 'total' | 'count' | 'max' | 'p95' | 'avg' | 'path'
  dir: 'asc' | 'desc'
}>({ field: 'total', dir: 'desc' })
const slowRequestFilter = ref<string>('')
const slowRequestSummary = shallowRef<SlowRequestSummary | null>(null)
const slowRequestThresholds = ref<EffectiveThresholds | null>(null)
```

Import the new types at the top of `tab.ts`:

```ts
import type {
  EffectiveThresholds,
  SlowRequestPathMode,
  SlowRequestSummary,
} from './types'
```

Make sure `shallowRef` is imported from `vue` alongside `ref`.

Add the refs to the object returned by `createTab` so the components can consume them via `tab.<name>`.

- [ ] **Step 3: Build the UI to confirm types compile**

```powershell
npm --prefix ui run build
```

Expected: clean build. If `vue-tsc` complains about an unused import (it will until the components actually consume the refs), suppress via an underscore prefix or leave the warning - the next tasks consume them.

- [ ] **Step 4: Commit**

```powershell
git add ui/src/types.ts ui/src/tab.ts
git commit -m "Added the TypeScript wire shapes for the slow-request insights surface and the per-tab refs that drive the drawer. Open / closed, path mode, sort, filter, summary and effective-thresholds all live on the tab so flipping tabs preserves whichever drawer the user last had open."
```

---

## Task 11: Speed rail - canvas, fetch, continuous-gradient paint

The 4px `<canvas>` to the right of the minimap, painted as a continuous green-to-red gradient with green as the resting default.

**Files:**
- Modify: `ui/src/components/LogViewport.vue`
- Modify: `ui/src/style.css`

- [ ] **Step 1: Add the speed-rail CSS palette tokens**

In `ui/src/style.css`, find the `:root` block where `--marker-restart` lives. Add:

```css
  /* Speed-rail palette - three HSL stops driving the continuous
     green-to-amber-to-red gradient over per-bucket avg slow-request
     duration. Light theme overrides further down. */
  --speed-fast: hsl(140, 70%, 45%);
  --speed-mid:  hsl(40, 85%, 50%);
  --speed-slow: hsl(0, 75%, 50%);
```

In the `:root[data-theme="light"]` block where `--marker-restart` light value lives, add:

```css
  --speed-fast: #15803d;
  --speed-mid:  #b45309;
  --speed-slow: #b91c1c;
```

- [ ] **Step 2: Add the canvas element to the LogViewport template**

In `ui/src/components/LogViewport.vue`, find the existing `.minimap` `<div>` (the one containing the minimap canvas + tooltip). Immediately after that closing `</div>`, insert:

```html
    <canvas
      v-if="speedRailVisible"
      ref="speedRailEl"
      class="speed-rail"
    />
```

- [ ] **Step 3: Add the ref, state, fetch, and paint**

In the `<script setup>` block of `LogViewport.vue`, add the import:

```ts
import type {
  // existing imports retained...
  SpeedGrid,
} from '../types'
```

Add the new state near the existing minimap state:

```ts
const speedRailEl = useTemplateRef<HTMLCanvasElement>('speedRailEl')
const speedGrid = ref<SpeedGrid | null>(null)
const speedRailVisible = computed(() => {
  const g = speedGrid.value
  if (!g) return false
  // Always show the rail when the file has ever had a slow request, so
  // it doesn't pop in/out as tail deltas land. Hide only when there
  // genuinely is nothing to paint.
  return g.buckets.length > 0 && (g.max_avg_ms > 0 || g.buckets.some((b) => b.count > 0))
})
const SPEED_RAIL_WIDTH = 4
```

In the existing `scheduleMinimapFetch` rAF body (next to the existing `fetchMinimap` + `fetchMarkers` calls), add:

```ts
    void fetchSpeedGrid()
```

Add the fetch + paint helpers:

```ts
async function fetchSpeedGrid() {
  const height = viewportHeightPx.value
  if (height <= 0) return
  const bucketCount = Math.max(1, Math.floor(height))
  try {
    const payload = await invoke<SpeedGrid>('get_slow_request_speeds', {
      fileId: props.tab.file.value.file_id,
      bucketCount,
    })
    speedGrid.value = payload
    paintSpeedRail()
  } catch {
    // non-fatal
  }
}

function readCssColour(varName: string): string {
  const styles = globalThis.getComputedStyle?.(document.documentElement)
  const v = styles?.getPropertyValue(varName).trim()
  return v && v.length > 0 ? v : '#15803d'
}

function lerpColour(a: string, b: string, t: number): string {
  // Both colours come from CSS variables in either hsl(...) or hex form.
  // Resolve via a hidden DOM element so the browser normalises them to
  // rgb(...) before we interpolate.
  const ca = resolveToRgb(a)
  const cb = resolveToRgb(b)
  const r = Math.round(ca[0] + (cb[0] - ca[0]) * t)
  const g = Math.round(ca[1] + (cb[1] - ca[1]) * t)
  const bb = Math.round(ca[2] + (cb[2] - ca[2]) * t)
  return `rgb(${r}, ${g}, ${bb})`
}

function resolveToRgb(colour: string): [number, number, number] {
  const probe = document.createElement('span')
  probe.style.color = colour
  probe.style.display = 'none'
  document.body.appendChild(probe)
  const computed = globalThis.getComputedStyle(probe).color
  document.body.removeChild(probe)
  const m = computed.match(/rgba?\((\d+),\s*(\d+),\s*(\d+)/)
  if (!m) return [0, 0, 0]
  return [Number(m[1]), Number(m[2]), Number(m[3])]
}

function bucketColour(avgMs: number, fastMs: number, slowMs: number): string {
  const fast = readCssColour('--speed-fast')
  const mid = readCssColour('--speed-mid')
  const slow = readCssColour('--speed-slow')
  if (avgMs <= fastMs || slowMs <= fastMs) return fast
  if (avgMs >= slowMs) return slow
  const t = (avgMs - fastMs) / (slowMs - fastMs)
  if (t < 0.5) return lerpColour(fast, mid, t * 2)
  return lerpColour(mid, slow, (t - 0.5) * 2)
}

function paintSpeedRail() {
  const canvas = speedRailEl.value
  const grid = speedGrid.value
  if (!canvas || !grid || grid.buckets.length === 0) return
  const h = grid.buckets.length
  const dpr = globalThis.devicePixelRatio || 1
  canvas.width = SPEED_RAIL_WIDTH * dpr
  canvas.height = h * dpr
  canvas.style.width = `${SPEED_RAIL_WIDTH}px`
  canvas.style.height = `${h}px`
  const ctx = canvas.getContext('2d')
  if (!ctx) return
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
  // Resolve gradient anchors. Per-file auto for now - Task 13 swaps in
  // the EffectiveThresholds payload.
  const fast = grid.min_avg_ms
  const slow = Math.max(grid.max_avg_ms, fast + 1)
  // One colour stop per bucket placed at its vertical midpoint.
  const gradient = ctx.createLinearGradient(0, 0, 0, h)
  for (let i = 0; i < h; i++) {
    const b = grid.buckets[i]
    const avg = b.count > 0 ? b.avg_ms : fast // green default
    const colour = bucketColour(avg, fast, slow)
    const offset = h === 1 ? 0 : (i + 0.5) / h
    gradient.addColorStop(Math.max(0, Math.min(1, offset)), colour)
  }
  // Anchor the very top + bottom to their nearest bucket's colour so
  // the gradient doesn't collapse toward black at the edges.
  const firstAvg = grid.buckets[0].count > 0 ? grid.buckets[0].avg_ms : fast
  const lastAvg =
    grid.buckets[h - 1].count > 0 ? grid.buckets[h - 1].avg_ms : fast
  gradient.addColorStop(0, bucketColour(firstAvg, fast, slow))
  gradient.addColorStop(1, bucketColour(lastAvg, fast, slow))
  ctx.fillStyle = gradient
  ctx.fillRect(0, 0, SPEED_RAIL_WIDTH, h)
}
```

Add the scoped CSS at the bottom of the `<style>` block:

```css
.speed-rail {
  flex: 0 0 auto;
  width: 4px;
  display: block;
  cursor: pointer;
  image-rendering: pixelated;
}
```

Add a repaint watcher next to the existing minimap repaint trigger so the rail tracks tail deltas:

```ts
watch(
  () => props.tab.file.value.line_count,
  () => {
    scheduleMinimapFetch()
  },
)
```

(If that watcher already exists from the minimap, skip the addition - `scheduleMinimapFetch` now triggers all three fetches.)

- [ ] **Step 4: Build + smoke**

```powershell
npm --prefix ui run build
cargo dev
```

Open `research/solopress-prod.log`. Expected:

- A 4px gradient stripe appears immediately to the right of the minimap.
- Regions with high-duration slow requests read red; quiet regions read green.
- No visible cell edges between buckets (smooth blends).
- Closing and reopening the file repaints the stripe.

Close the dev shell.

- [ ] **Step 5: Lint + commit**

```powershell
npm --prefix ui run build
git add ui/src/components/LogViewport.vue ui/src/style.css
git commit -m "Painted the speed rail next to the minimap. A 4px <canvas> sits in the viewport-shell row immediately after the minimap and paints a continuous green-to-amber-to-red gradient over per-bucket slow-request average duration. The whole rail is one fillRect against a createLinearGradient with one colour stop per bucket placed at its vertical midpoint so adjacent buckets fade smoothly instead of reading as hard cells. Empty buckets inherit the fast (green) colour so quiet regions read as healthy and the rail always paints. Three new CSS palette tokens (--speed-fast / --speed-mid / --speed-slow) cover both dark and light themes."
```

---

## Task 12: Insights toggle button + drawer scaffold + jumpToLine exposure

Wire up the toggle, the empty drawer shell, and a public `jumpToLine` on the LogViewport so the drawer can scroll the viewport.

**Files:**
- Create: `ui/src/components/InsightsDrawer.vue`
- Modify: `ui/src/components/LogViewport.vue`
- Modify: `ui/src/components/AppHeader.vue`
- Modify: `ui/src/App.vue`
- Modify: `ui/src/style.css`

- [ ] **Step 1: Create the drawer scaffold**

`ui/src/components/InsightsDrawer.vue`:

```vue
<script setup lang="ts">
/**
 * Right-side collapsible drawer hosting the slow-request insights for
 * the active tab. Entry table, threshold editor, and status chip are
 * added in subsequent tasks; this scaffold just renders the header +
 * empty body + close button.
 */
import { computed } from 'vue'
import type { Tab } from '../tab'

const props = defineProps<{ tab: Tab }>()

const emit = defineEmits<{
  (e: 'close'): void
}>()

const totals = computed(() => {
  const s = props.tab.slowRequestSummary.value
  if (!s) return 'Loading...'
  if (s.total_hits === 0) return 'No slow requests detected.'
  return `${s.total_hits} hits across ${s.entries.length} endpoints, ${s.deduped} dedupes`
})
</script>

<template>
  <aside class="insights-drawer">
    <header class="drawer-head">
      <span class="title">Slow requests</span>
      <button type="button" class="close-btn" aria-label="Close" @click="emit('close')">
        x
      </button>
    </header>
    <div class="drawer-totals">{{ totals }}</div>
    <div class="drawer-body">
      <!-- Body lands in Task 13. -->
    </div>
  </aside>
</template>

<style scoped>
.insights-drawer {
  flex: 0 0 auto;
  width: 360px;
  display: flex;
  flex-direction: column;
  background: var(--bg-elevated);
  border-left: 1px solid var(--border-default);
  min-height: 0;
}

.drawer-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0.4rem 0.6rem;
  border-bottom: 1px solid var(--border-default);
}

.title {
  font-weight: 600;
}

.close-btn {
  background: transparent;
  border: 1px solid transparent;
  color: var(--fg-default);
  cursor: pointer;
  padding: 0.1rem 0.4rem;
  border-radius: 3px;
}

.close-btn:hover {
  background: var(--bg-button-hover);
  border-color: var(--border-button);
}

.drawer-totals {
  padding: 0.4rem 0.6rem;
  color: var(--fg-muted);
  font-size: 0.85rem;
}

.drawer-body {
  flex: 1 1 auto;
  overflow-y: auto;
  padding: 0 0.6rem 0.6rem;
}
</style>
```

- [ ] **Step 2: Mount the drawer inside `LogViewport.vue`**

In the template, immediately after the speed-rail `<canvas>`, add:

```html
    <InsightsDrawer
      v-if="tab.insightsOpen.value"
      :tab="tab"
      @close="tab.insightsOpen.value = false"
    />
```

Import the component near the existing imports:

```ts
import InsightsDrawer from './InsightsDrawer.vue'
```

- [ ] **Step 3: Expose `jumpToLine` from `LogViewport.vue`**

In `LogViewport.vue`, find the existing `jumpToLine(lineIdx)` helper added during marker work. Locate the `defineExpose({ ... })` call near the bottom of the script setup block and add `jumpToLine` to the exposed surface:

```ts
defineExpose({ scrollToCurrentHit, jumpToBottom, jumpToLine })
```

- [ ] **Step 4: Add the insights toggle button to `AppHeader.vue`**

In `ui/src/components/AppHeader.vue`, locate the existing settings cog button. Immediately before (or after - position by taste) it, add:

```html
<button
  type="button"
  class="hdr-btn"
  :class="{ 'is-active': insightsActive }"
  title="Toggle slow-request insights"
  aria-label="Toggle insights drawer"
  @click="emit('toggle-insights')"
>
  <svg width="14" height="14" viewBox="0 0 24 24" aria-hidden="true">
    <rect x="3" y="13" width="4" height="8" fill="currentColor" />
    <rect x="10" y="8" width="4" height="13" fill="currentColor" />
    <rect x="17" y="3" width="4" height="18" fill="currentColor" />
  </svg>
</button>
```

Add the prop + emit in the script:

```ts
defineProps<{ insightsActive?: boolean; /* existing props */ }>()
const emit = defineEmits<{
  (e: 'toggle-insights'): void
  // existing emits...
}>()
```

(Keep the existing emits/props - this is additive.)

- [ ] **Step 5: Wire the toggle in `App.vue`**

In `ui/src/App.vue`, find where `<AppHeader ... />` is rendered. Add the new prop + handler:

```html
<AppHeader
  :insights-active="currentTab?.insightsOpen.value ?? false"
  @toggle-insights="onToggleInsights"
  ...existing bindings...
/>
```

Add the handler near the existing modal-toggle handlers:

```ts
function onToggleInsights() {
  const t = currentTab.value
  if (!t) return
  t.insightsOpen.value = !t.insightsOpen.value
}
```

- [ ] **Step 6: Build + smoke**

```powershell
npm --prefix ui run build
cargo dev
```

Open the prod fixture. Click the new bar-chart icon in the header bar. The drawer should slide in from the right, show "Loading..." in the totals row, and close when the X is clicked. Switching tabs should remember which had the drawer open.

- [ ] **Step 7: Commit**

```powershell
git add ui/src/components/InsightsDrawer.vue ui/src/components/LogViewport.vue ui/src/components/AppHeader.vue ui/src/App.vue ui/src/style.css
git commit -m "Added the insights drawer scaffold and the header-bar toggle that opens it. Drawer mounts inside the viewport-shell row at 360px wide, slots in after the speed rail, renders a header / totals row / empty body. Toggle is per-tab so flipping tabs preserves whichever drawer the user last had open. Exposed jumpToLine from LogViewport so the drawer body (next task) can scroll to specific lines."
```

---

## Task 13: Drawer body - entries table, toolbar, click-to-jump

Fetch `get_slow_requests`, render the table, support path-mode flip + filter + sort + expand-to-occurrences + click-to-jump.

**Files:**
- Modify: `ui/src/components/InsightsDrawer.vue`

- [ ] **Step 1: Add the IPC fetch + cached data**

In `InsightsDrawer.vue`'s `<script setup>`, append:

```ts
import { ref, watch, onMounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { SlowRequestEntry, SlowRequestSummary } from '../types'

const expanded = ref<Set<string>>(new Set())
const error = ref<string | null>(null)
const loading = ref(false)

async function refresh() {
  loading.value = true
  error.value = null
  try {
    const payload = await invoke<SlowRequestSummary>('get_slow_requests', {
      fileId: props.tab.file.value.file_id,
      mode: props.tab.slowRequestMode.value,
    })
    props.tab.slowRequestSummary.value = payload
  } catch (e) {
    error.value = String((e as { message?: string })?.message ?? e)
  } finally {
    loading.value = false
  }
}

onMounted(() => {
  void refresh()
})

watch(() => props.tab.slowRequestMode.value, refresh)
watch(
  () => props.tab.file.value.line_count,
  () => {
    if (props.tab.insightsOpen.value) void refresh()
  },
)
```

- [ ] **Step 2: Add sort + filter computed**

```ts
const filteredEntries = computed<SlowRequestEntry[]>(() => {
  const s = props.tab.slowRequestSummary.value
  if (!s) return []
  const filter = props.tab.slowRequestFilter.value.trim().toLowerCase()
  const filtered = filter
    ? s.entries.filter((e) => e.path.toLowerCase().includes(filter))
    : s.entries.slice()
  const { field, dir } = props.tab.slowRequestSort.value
  const sign = dir === 'asc' ? 1 : -1
  const key = (e: SlowRequestEntry): number | string => {
    switch (field) {
      case 'total': return e.total_ms
      case 'count': return e.count
      case 'max':   return e.max_ms
      case 'p95':   return e.p95_ms
      case 'avg':   return e.avg_ms
      case 'path':  return e.path
    }
  }
  filtered.sort((a, b) => {
    const ka = key(a)
    const kb = key(b)
    if (typeof ka === 'number' && typeof kb === 'number') {
      return sign * (ka - kb)
    }
    return sign * String(ka).localeCompare(String(kb))
  })
  return filtered
})

function formatMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`
  return `${(ms / 60_000).toFixed(1)}m`
}

function toggleExpanded(path: string) {
  const s = expanded.value
  if (s.has(path)) s.delete(path)
  else s.add(path)
  expanded.value = new Set(s)
}

function onSortChange(ev: Event) {
  const t = (ev.target as HTMLSelectElement).value as
    | 'total' | 'count' | 'max' | 'p95' | 'avg' | 'path'
  const cur = props.tab.slowRequestSort.value
  if (cur.field === t) {
    props.tab.slowRequestSort.value = { field: t, dir: cur.dir === 'desc' ? 'asc' : 'desc' }
  } else {
    props.tab.slowRequestSort.value = { field: t, dir: 'desc' }
  }
}

function jumpTo(line: number) {
  // App.vue exposes the viewport via a template ref; we emit so the
  // parent forwards. Keeps the drawer ignorant of the viewport.
  emit('jump', line)
}
```

Extend the emit declaration:

```ts
const emit = defineEmits<{
  (e: 'close'): void
  (e: 'jump', line: number): void
}>()
```

- [ ] **Step 3: Replace the empty drawer body with the toolbar + table**

Replace the `<div class="drawer-body"></div>` block in the template with:

```html
<div class="drawer-toolbar">
  <div class="mode-toggle">
    <button
      type="button"
      class="seg"
      :class="{ active: tab.slowRequestMode.value === 'normalised' }"
      @click="tab.slowRequestMode.value = 'normalised'"
    >Normalised</button>
    <button
      type="button"
      class="seg"
      :class="{ active: tab.slowRequestMode.value === 'raw' }"
      @click="tab.slowRequestMode.value = 'raw'"
    >Raw</button>
  </div>
  <input
    v-model="tab.slowRequestFilter.value"
    type="text"
    class="filter-input"
    placeholder="Filter path..."
  />
  <select class="sort-select" :value="tab.slowRequestSort.value.field" @change="onSortChange">
    <option value="total">Total time</option>
    <option value="count">Count</option>
    <option value="max">Max</option>
    <option value="p95">p95</option>
    <option value="avg">Avg</option>
    <option value="path">Path</option>
  </select>
  <span class="sort-dir">{{ tab.slowRequestSort.value.dir === 'desc' ? 'desc' : 'asc' }}</span>
</div>

<div v-if="error" class="drawer-error">
  {{ error }}
  <button type="button" @click="refresh">Retry</button>
</div>

<div v-else-if="loading && !tab.slowRequestSummary.value" class="drawer-loading">
  Loading...
</div>

<div v-else-if="filteredEntries.length === 0" class="drawer-empty">
  No slow requests match the current filter.
</div>

<ul v-else class="entry-list">
  <li v-for="entry in filteredEntries" :key="entry.path" class="entry">
    <div class="entry-row" @click="toggleExpanded(entry.path)">
      <span class="entry-path" :title="entry.path" @click.stop="jumpTo(entry.longest_line)">
        {{ entry.path }}
      </span>
      <span class="entry-stats">
        {{ entry.count }} hits . total {{ formatMs(entry.total_ms) }} .
        avg {{ formatMs(entry.avg_ms) }} . p95 {{ formatMs(entry.p95_ms) }} .
        max {{ formatMs(entry.max_ms) }}
      </span>
      <span class="entry-expand" :class="{ open: expanded.has(entry.path) }">v</span>
    </div>
    <ul v-if="expanded.has(entry.path)" class="occurrence-list">
      <li
        v-for="occ in entry.occurrences"
        :key="`${entry.path}-${occ.line_index}`"
        class="occurrence"
        @click="jumpTo(occ.line_index)"
      >
        <span class="occ-ts">
          {{ occ.timestamp_ms !== null ? new Date(occ.timestamp_ms).toISOString().slice(0, 23).replace('T', ' ') : 'no ts' }}
        </span>
        <span class="occ-dur">{{ formatMs(occ.duration_ms) }}</span>
        <span class="occ-line">line {{ occ.line_index + 1 }}</span>
        <span v-if="occ.dup_count > 1" class="occ-dup">x{{ occ.dup_count }}</span>
      </li>
    </ul>
  </li>
</ul>
```

Add the scoped styles at the end of the `<style scoped>` block:

```css
.drawer-toolbar {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  padding: 0.4rem 0.6rem;
  border-bottom: 1px solid var(--border-default);
}

.mode-toggle { display: flex; }
.seg {
  background: transparent;
  border: 1px solid var(--border-button);
  color: var(--fg-default);
  padding: 0.15rem 0.5rem;
  cursor: pointer;
}
.seg:first-child { border-radius: 3px 0 0 3px; }
.seg:last-child  { border-radius: 0 3px 3px 0; border-left: none; }
.seg.active {
  background: var(--bg-button-hover);
  color: var(--accent);
}

.filter-input {
  flex: 1 1 auto;
  background: var(--bg-viewport);
  border: 1px solid var(--border-button);
  color: var(--fg-default);
  padding: 0.15rem 0.4rem;
  border-radius: 3px;
  min-width: 60px;
}

.sort-select {
  background: var(--bg-viewport);
  border: 1px solid var(--border-button);
  color: var(--fg-default);
  padding: 0.15rem 0.3rem;
  border-radius: 3px;
}

.sort-dir { color: var(--fg-muted); font-size: 0.8rem; }

.entry-list { list-style: none; margin: 0; padding: 0; }
.entry { border-bottom: 1px solid var(--border-default); }
.entry-row {
  display: grid;
  grid-template-columns: 1fr auto 16px;
  gap: 0.4rem;
  padding: 0.4rem 0;
  align-items: center;
  cursor: pointer;
}
.entry-path {
  color: var(--accent);
  cursor: pointer;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.entry-stats { color: var(--fg-muted); font-size: 0.8rem; white-space: nowrap; }
.entry-expand { color: var(--fg-muted); transition: transform 120ms; }
.entry-expand.open { transform: rotate(180deg); }

.occurrence-list { list-style: none; margin: 0 0 0.4rem 0; padding: 0; }
.occurrence {
  display: grid;
  grid-template-columns: 11rem auto 1fr auto;
  gap: 0.4rem;
  padding: 0.15rem 0.4rem;
  cursor: pointer;
  font-size: 0.8rem;
  color: var(--fg-muted);
}
.occurrence:hover { background: var(--bg-button-hover); color: var(--fg-default); }
.occ-dur { color: var(--fg-default); font-weight: 600; }
.occ-dup {
  color: var(--accent);
  font-size: 0.7rem;
  letter-spacing: 0.05em;
}

.drawer-error, .drawer-loading, .drawer-empty {
  padding: 0.6rem;
  color: var(--fg-muted);
}
.drawer-error { color: var(--level-error); }
.drawer-error button {
  margin-left: 0.6rem;
  background: transparent;
  border: 1px solid var(--border-button);
  color: var(--fg-default);
  padding: 0.1rem 0.4rem;
  border-radius: 3px;
  cursor: pointer;
}
```

- [ ] **Step 4: Wire the `jump` emit through to LogViewport**

In `App.vue`, find where `<LogViewport>` is rendered. The drawer is rendered inside LogViewport (mounted in Task 12), so the `jump` emit needs to be handled at that layer. In `LogViewport.vue`'s template, update the InsightsDrawer mount:

```html
<InsightsDrawer
  v-if="tab.insightsOpen.value"
  :tab="tab"
  @close="tab.insightsOpen.value = false"
  @jump="jumpToLine"
/>
```

`jumpToLine` is already in scope from earlier work.

- [ ] **Step 5: Build + smoke**

```powershell
npm --prefix ui run build
cargo dev
```

Open prod fixture, open the drawer:

- Endpoints visible, sorted by total time desc.
- Click an endpoint path -> viewport scrolls to its slowest hit.
- Click the row chrome (not the path) -> occurrence list expands.
- Click an occurrence -> viewport scrolls to that line.
- Type a filter -> table narrows.
- Flip Normalised <-> Raw -> count changes.
- Sort dropdown changes order; same option twice flips direction.

- [ ] **Step 6: Commit**

```powershell
git add ui/src/components/InsightsDrawer.vue ui/src/components/LogViewport.vue
git commit -m "Filled in the insights drawer body. Toolbar with Normalised / Raw mode toggle, free-text filter, sort dropdown (Total / Count / Max / p95 / Avg / Path) with same-option-flips-direction. Sortable entry list with click-the-path-to-jump and click-the-row-to-expand. Expanded view shows up to fifty top occurrences by duration with timestamp, duration, line number and dup-count chip; clicking an occurrence scrolls the viewport to that line. Loading / error / empty states all render."
```

---

## Task 14: Drawer threshold editor + status chip + speed-rail consumes thresholds

The inline per-file threshold editor, the Auto / Global / Per-file chip, and the wire-up so the speed rail respects the resolved anchors.

**Files:**
- Modify: `ui/src/components/InsightsDrawer.vue`
- Modify: `ui/src/components/LogViewport.vue`

- [ ] **Step 1: Fetch effective thresholds when the drawer opens**

In `InsightsDrawer.vue` script, add:

```ts
import type { EffectiveThresholds, SlowRequestThresholds } from '../types'

async function refreshThresholds() {
  try {
    const payload = await invoke<EffectiveThresholds>('get_slow_request_thresholds', {
      fileId: props.tab.file.value.file_id,
    })
    props.tab.slowRequestThresholds.value = payload
    fastInput.value = String(payload.per_file?.fast_ms ?? '')
    slowInput.value = String(payload.per_file?.slow_ms ?? '')
  } catch {
    // non-fatal
  }
}

const fastInput = ref('')
const slowInput = ref('')

const validationError = computed<string | null>(() => {
  const fast = Number(fastInput.value)
  const slow = Number(slowInput.value)
  if (fastInput.value === '' && slowInput.value === '') return null
  if (fastInput.value === '' || slowInput.value === '') return 'Both fields required'
  if (Number.isNaN(fast) || Number.isNaN(slow)) return 'Numbers only'
  if (fast >= slow) return 'fast must be less than slow'
  if (slow > 600_000) return 'slow capped at 600,000 (10 min)'
  return null
})

async function savePerFile() {
  if (validationError.value) return
  const fast = Number(fastInput.value)
  const slow = Number(slowInput.value)
  const t: SlowRequestThresholds | null =
    fastInput.value === '' && slowInput.value === ''
      ? null
      : { fast_ms: fast, slow_ms: slow }
  try {
    await invoke<void>('save_slow_request_thresholds', {
      fileId: props.tab.file.value.file_id,
      thresholds: t,
    })
    await refreshThresholds()
    // Tell the viewport to repaint the rail with the new anchors.
    emit('thresholds-changed')
  } catch (e) {
    error.value = String((e as { message?: string })?.message ?? e)
  }
}

async function clearPerFile() {
  fastInput.value = ''
  slowInput.value = ''
  await savePerFile()
}

onMounted(() => {
  void refreshThresholds()
})
```

Extend the emit declaration:

```ts
const emit = defineEmits<{
  (e: 'close'): void
  (e: 'jump', line: number): void
  (e: 'thresholds-changed'): void
}>()
```

- [ ] **Step 2: Render the chip + editor in the template**

Insert immediately below `.drawer-totals`:

```html
<div class="threshold-row">
  <span
    class="threshold-chip"
    :class="`source-${tab.slowRequestThresholds.value?.source ?? 'auto'}`"
  >
    {{ tab.slowRequestThresholds.value?.source === 'per_file' ? 'Per-file'
      : tab.slowRequestThresholds.value?.source === 'global' ? 'Global'
      : 'Auto' }}
  </span>
  <span class="threshold-current">
    Fast {{ tab.slowRequestThresholds.value?.effective.fast_ms ?? '-' }}ms,
    Slow {{ tab.slowRequestThresholds.value?.effective.slow_ms ?? '-' }}ms
  </span>
</div>
<details class="threshold-editor">
  <summary>Override for this file</summary>
  <div class="threshold-fields">
    <label>Fast (ms) <input v-model="fastInput" type="number" min="0" max="600000" /></label>
    <label>Slow (ms) <input v-model="slowInput" type="number" min="0" max="600000" /></label>
  </div>
  <div v-if="validationError" class="threshold-error">{{ validationError }}</div>
  <div class="threshold-actions">
    <button type="button" :disabled="!!validationError" @click="savePerFile">Save</button>
    <button type="button" class="muted" @click="clearPerFile">Clear override</button>
  </div>
</details>
```

Add the scoped styles:

```css
.threshold-row {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0 0.6rem 0.4rem;
}

.threshold-chip {
  font-size: 0.7rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  padding: 0.1rem 0.4rem;
  border-radius: 3px;
  border: 1px solid var(--border-button);
}
.threshold-chip.source-auto    { color: var(--level-info);  }
.threshold-chip.source-global  { color: var(--level-warn);  }
.threshold-chip.source-per_file { color: var(--accent); border-color: var(--accent); }

.threshold-current { color: var(--fg-muted); font-size: 0.8rem; }

.threshold-editor {
  margin: 0 0.6rem 0.6rem;
  font-size: 0.8rem;
}
.threshold-editor summary { cursor: pointer; color: var(--fg-muted); }
.threshold-fields { display: flex; gap: 0.6rem; margin: 0.4rem 0; }
.threshold-fields input {
  width: 6rem;
  background: var(--bg-viewport);
  border: 1px solid var(--border-button);
  color: var(--fg-default);
  padding: 0.1rem 0.3rem;
  border-radius: 3px;
}
.threshold-error { color: var(--level-error); margin: 0.2rem 0; }
.threshold-actions { display: flex; gap: 0.4rem; }
.threshold-actions button {
  background: transparent;
  border: 1px solid var(--border-button);
  color: var(--fg-default);
  padding: 0.15rem 0.6rem;
  border-radius: 3px;
  cursor: pointer;
}
.threshold-actions button.muted { color: var(--fg-muted); }
.threshold-actions button:disabled { opacity: 0.4; cursor: not-allowed; }
```

- [ ] **Step 3: Use the resolved thresholds in the speed rail**

In `LogViewport.vue`, fetch effective thresholds on the same trigger as the speed grid + bind them to the paint:

Add state near the other speed-rail state:

```ts
import type { EffectiveThresholds } from '../types'
const speedAnchors = ref<{ fast: number; slow: number } | null>(null)

async function fetchSpeedThresholds() {
  try {
    const payload = await invoke<EffectiveThresholds>('get_slow_request_thresholds', {
      fileId: props.tab.file.value.file_id,
    })
    speedAnchors.value = {
      fast: payload.effective.fast_ms,
      slow: payload.effective.slow_ms,
    }
    props.tab.slowRequestThresholds.value = payload
    paintSpeedRail()
  } catch {
    // non-fatal; auto-from-grid fallback applies below
  }
}
```

In `scheduleMinimapFetch`'s rAF body, add `void fetchSpeedThresholds()` next to the existing fetches.

Replace the `fast` / `slow` resolution in `paintSpeedRail`:

```ts
  const anchors = speedAnchors.value
  const fast = anchors ? anchors.fast : grid.min_avg_ms
  const slow = anchors
    ? Math.max(anchors.slow, fast + 1)
    : Math.max(grid.max_avg_ms, fast + 1)
```

Handle the `thresholds-changed` emit from the drawer to trigger a refetch:

```html
<InsightsDrawer
  v-if="tab.insightsOpen.value"
  :tab="tab"
  @close="tab.insightsOpen.value = false"
  @jump="jumpToLine"
  @thresholds-changed="fetchSpeedThresholds"
/>
```

- [ ] **Step 4: Build + smoke**

```powershell
npm --prefix ui run build
cargo dev
```

Open prod fixture, open the drawer:

- Chip reads "AUTO" initially.
- Open the override panel, type Fast 500 / Slow 8000, hit Save -> chip flips to "PER-FILE", speed rail repaints with new gradient redistribution.
- Click Clear override -> chip returns to "AUTO".
- Type invalid values (fast >= slow) -> Save button disables, inline error shows.

- [ ] **Step 5: Commit**

```powershell
git add ui/src/components/InsightsDrawer.vue ui/src/components/LogViewport.vue
git commit -m "Wired the per-file threshold editor into the drawer with an Auto / Global / Per-file status chip, inline number inputs, validation (both required, fast less than slow, slow capped at ten minutes), Save and Clear-override actions. Saving emits thresholds-changed which the viewport listens for to refetch and repaint the speed rail with the new anchors. The rail now uses the effective thresholds from the IPC instead of always normalising to the file's own min and max bucket averages."
```

---

## Task 15: Settings modal - global thresholds section

Edit the global default from the existing Settings modal.

**Files:**
- Modify: `ui/src/components/SettingsModal.vue`
- Modify: `ui/src/composables/useSettings.ts`

- [ ] **Step 1: Extend the `update_settings` invoke wrapper**

In `ui/src/composables/useSettings.ts`, find the helper that calls `update_settings`. Extend its argument shape to accept `slow_request_thresholds`:

```ts
type SettingsPatch = {
  theme?: string
  font_size?: number
  follow_tail_default?: boolean
  /** undefined = leave untouched. null = clear. value = set. */
  slow_request_thresholds?: SlowRequestThresholds | null
}

async function updateSettings(patch: SettingsPatch) {
  // Wire `null` as `Some(None)` and a value as `Some(Some(value))`. The
  // Rust side accepts the `Option<Option<...>>` shape directly via
  // serde - undefined fields on the JS object don't ride the wire.
  const wire: Record<string, unknown> = { ...patch }
  if (patch.slow_request_thresholds === null) {
    wire.slow_request_thresholds = null
  }
  settings.value = await invoke<Settings>('update_settings', { patch: wire })
}
```

Import `SlowRequestThresholds` at the top.

- [ ] **Step 2: Add the section to the Settings modal**

In `ui/src/components/SettingsModal.vue`, add a new section below "Behaviour":

```html
<section class="settings-section">
  <h3>Slow requests</h3>
  <p class="hint">
    Pin the speed-rail gradient anchors. Both values required; fast must be
    less than slow; both capped at 600,000 ms (10 minutes). Per-file overrides
    from the insights drawer take precedence over these.
  </p>
  <div class="row-grid">
    <label>Fast (ms) <input v-model="fastInput" type="number" min="0" max="600000" /></label>
    <label>Slow (ms) <input v-model="slowInput" type="number" min="0" max="600000" /></label>
  </div>
  <div v-if="thresholdError" class="row-error">{{ thresholdError }}</div>
  <div class="row-actions">
    <button type="button" :disabled="!!thresholdError" @click="saveGlobal">Save</button>
    <button type="button" class="muted" @click="resetGlobal">Reset to default</button>
  </div>
</section>
```

In the script:

```ts
import { computed, ref, watchEffect } from 'vue'

const fastInput = ref('')
const slowInput = ref('')

watchEffect(() => {
  const t = props.settings.slow_request_thresholds
  fastInput.value = t ? String(t.fast_ms) : ''
  slowInput.value = t ? String(t.slow_ms) : ''
})

const thresholdError = computed<string | null>(() => {
  if (fastInput.value === '' && slowInput.value === '') return null
  if (fastInput.value === '' || slowInput.value === '') return 'Both fields required'
  const f = Number(fastInput.value)
  const s = Number(slowInput.value)
  if (Number.isNaN(f) || Number.isNaN(s)) return 'Numbers only'
  if (f >= s) return 'fast must be less than slow'
  if (s > 600_000) return 'slow capped at 600,000 (10 min)'
  return null
})

function saveGlobal() {
  if (thresholdError.value) return
  if (fastInput.value === '' && slowInput.value === '') {
    emit('update', { slow_request_thresholds: null })
    return
  }
  emit('update', {
    slow_request_thresholds: {
      fast_ms: Number(fastInput.value),
      slow_ms: Number(slowInput.value),
    },
  })
}

function resetGlobal() {
  fastInput.value = ''
  slowInput.value = ''
  emit('update', { slow_request_thresholds: null })
}
```

The `emit('update', ...)` here piggybacks on the existing emit pattern - confirm the parent (App.vue) forwards through `useSettings.updateSettings`. If the existing emit shape is `(e: 'update', patch: Settings)` rather than a partial, route via a new emit `update-thresholds` instead and have App.vue call `updateSettings({ slow_request_thresholds: ... })`. Pick whichever matches the existing convention; the goal is "save through the existing update_settings IPC".

Add the `Settings` import where the prop type is declared if not already present, so the watch picks up the field.

- [ ] **Step 3: Notify open drawers when global settings change**

This is the cross-component signal that "the global default just changed; refetch the effective tier in any open drawer". The simplest plumbing without introducing an event bus:

In `useSettings.ts`, expose a monotonic counter ref that bumps on every save:

```ts
const settingsVersion = ref(0)
// inside updateSettings, after `settings.value = ...`:
settingsVersion.value += 1
```

In `InsightsDrawer.vue`, watch the settings version (passed via prop or imported from the composable) and call `refreshThresholds()` when it changes. The cleanest seam is to pass `settingsVersion` as a prop on the drawer from App.vue, but the drawer doesn't otherwise need App-level state. Acceptable shortcut: have the drawer watch a `useSettings()` import directly.

Implement whichever matches the existing `useSettings` consumer pattern in the codebase. The skeleton:

```ts
import { useSettings } from '../composables/useSettings'
const { settingsVersion } = useSettings()
watch(settingsVersion, () => {
  void refreshThresholds()
})
```

- [ ] **Step 4: Build + smoke**

```powershell
npm --prefix ui run build
cargo dev
```

Open prod fixture. Open Settings -> scroll to Slow requests. Type Fast 1000 / Slow 5000 -> Save. Close Settings. Open the insights drawer:

- Chip reads "GLOBAL".
- The speed rail visibly redistributes (more of the file reddens against the lower slow anchor).

Open Settings again -> Reset to default. Drawer chip flips to "AUTO" without a reload.

- [ ] **Step 5: Commit**

```powershell
git add ui/src/components/SettingsModal.vue ui/src/composables/useSettings.ts
git commit -m "Added the Slow requests section to the Settings modal with global fast and slow threshold inputs, validation matching the per-file editor, Save and Reset-to-default actions. Routes through update_settings on the existing IPC. A settingsVersion counter on useSettings bumps on every save so any open insights drawer refetches its effective thresholds and the speed rail repaints immediately without a reload."
```

---

## Task 16: Speed-rail hover integration with the minimap tooltip

Hovering the speed rail should reuse the existing minimap tooltip with an extra "N hits, avg X, peak Y" line.

**Files:**
- Modify: `ui/src/components/LogViewport.vue`

- [ ] **Step 1: Extend the tooltip data shape**

Find `MinimapTooltip` interface (added during the heatmap work). Add:

```ts
interface MinimapTooltip {
  visible: boolean
  top: number
  left: number
  lineIndex: number
  timestamp: string | null
  error: number
  warn: number
  /** Speed-rail extra. Zero count means "no speed line". */
  speed_count: number
  speed_avg_ms: number
  speed_max_ms: number
}
```

Update the default-initialised state and the reset sites accordingly.

- [ ] **Step 2: Populate the new fields in `updateMinimapTooltip`**

In `tooltipTargetFromY`, also compute a speed-bucket index using `speedGrid.value.buckets.length` (which equals the level minimap bucket count by construction). In `updateMinimapTooltip`, after the existing bucket population:

```ts
const sg = speedGrid.value
if (sg && bucketIndex >= 0 && bucketIndex < sg.buckets.length) {
  const sb = sg.buckets[bucketIndex]
  minimapTooltip.value.speed_count = sb.count
  minimapTooltip.value.speed_avg_ms = sb.avg_ms
  minimapTooltip.value.speed_max_ms = sb.max_ms
} else {
  minimapTooltip.value.speed_count = 0
  minimapTooltip.value.speed_avg_ms = 0
  minimapTooltip.value.speed_max_ms = 0
}
```

- [ ] **Step 3: Render the new tooltip line**

In the template's tooltip block, after the existing heat line, add:

```html
<span
  v-if="minimapTooltip.speed_count > 0"
  class="speed-line"
>{{ speedLine(minimapTooltip.speed_count, minimapTooltip.speed_avg_ms, minimapTooltip.speed_max_ms) }}</span>
```

Add the helper:

```ts
function speedLine(count: number, avg: number, max: number): string {
  const label = count === 1 ? 'hit' : 'hits'
  return `${count} ${label}, avg ${formatMs(avg)}, peak ${formatMs(max)}`
}

function formatMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`
  return `${(ms / 60_000).toFixed(1)}m`
}
```

(If `formatMs` already exists in the viewport from earlier work, reuse it.)

Style the new line in the existing tooltip CSS block:

```css
.speed-line {
  color: var(--speed-mid);
  font-weight: 600;
}
```

- [ ] **Step 4: Wire the speed rail to scroll on click**

The minimap is clickable today (scrub-to-Y). Replicate for the speed rail by sharing the same `onMinimapPointerDown`/`Move`/`Up` handlers. Easiest: wrap the minimap and rail in a single hover/click target by adding the same handlers to the rail's element. Add:

```html
<canvas
  v-if="speedRailVisible"
  ref="speedRailEl"
  class="speed-rail"
  @pointerdown="onMinimapPointerDown"
  @pointermove="onMinimapPointerMove"
  @pointerup="onMinimapPointerUp"
  @pointercancel="onMinimapPointerUp"
  @pointerenter="onMinimapPointerEnter"
  @pointerleave="onMinimapPointerLeave"
/>
```

The pointer handlers project Y -> line via the canvas they're attached to; the rail and minimap share the same height, so the projection works identically.

- [ ] **Step 5: Build + smoke**

```powershell
npm --prefix ui run build
cargo dev
```

Open prod fixture. Hover the speed rail and the minimap:

- Quiet INFO region: tooltip shows line + timestamp only.
- Region with slow requests: tooltip shows line + timestamp + (heat line if also errors/warns) + speed line "N hits, avg X, peak Y".
- Clicking the speed rail scrolls the viewport.

- [ ] **Step 6: Commit**

```powershell
git add ui/src/components/LogViewport.vue
git commit -m "Folded the speed-rail hover into the existing minimap tooltip with a fourth line - N hits, avg, peak - shown only when the hovered bucket has slow-request data. The rail now shares the minimap's pointer-down / move / up scrubbing handlers so clicking or dragging on it scrolls the viewport identically. The two stripes read as one combined scrubber rather than two separate controls."
```

---

## Task 17: OpenWolf docs - anatomy + memory

Per the OpenWolf protocol, update anatomy.md with the new IPC surface, modules, and components, and append a one-line memory entry.

**Files:**
- Modify: `.wolf/anatomy.md`
- Modify: `.wolf/memory.md`

- [ ] **Step 1: Update anatomy.md**

In `.wolf/anatomy.md`, find the `crates/clog-core/` section. Insert after the existing module list:

```
- `src/slow_requests.rs` - SLOW REQUEST detection (one regex covering both observed Play formats), aggregation, and speed-grid builder. `PathMode { Normalised | Raw }`, `SlowRequestThresholds::new(fast, slow)` with validation (fast < slow, both <= 600 000 ms), `extract_slow_requests(records, bytes, line_offsets, mode, ts_extractor)` returns `SlowRequestSummary { entries, total_hits, deduped, total_ms }` with per-entry count / total / min / max / avg / nearest-rank p95 and a top-50 occurrences cap. `build_speed_grid(occurrences, line_count, bucket_count)` mirrors the level-minimap bucket geometry. 20+ unit tests cover path normalisation, both detection formats, dedup at the same ms, normalised vs raw merging, the occurrence cap, longest_line pointing at the slowest hit, and the speed grid bucketing edges.
```

In the `crates/clog-app/` IPC list, insert after the markers entry:

```
- `get_slow_requests(file_id, mode)` -> `SlowRequestSummary`. Reads from a short-lived `SlowRequestCache` on `OpenedFile` keyed by `(records.len, bytes.len, pattern_hash)`; flipping mode never re-scans the file.
- `get_slow_request_speeds(file_id, bucket_count)` -> `SpeedGrid`. Same cache; bucket count comes from the UI to match the level-minimap grid.
- `get_slow_request_thresholds(file_id)` -> `EffectiveThresholds { source, effective, per_file, global }` where `source` is `auto` | `global` | `per_file`. Auto-tier `effective` is derived from the current speed grid extremes.
- `save_slow_request_thresholds(file_id, thresholds)` writes the per-file override (or clears it on `None`). When the resulting `PerFileRulesFile` has no rules and no thresholds the file is deleted via `forget` rather than persisted as a stub.
- `update_settings` patch grew an optional `slow_request_thresholds: Option<Option<SlowRequestThresholds>>` field for the global default. `Some(Some(t))` sets, `Some(None)` clears, absent leaves untouched.
```

In the `src/persistence.rs` notes, append:

```
. P11 added `slow_request_thresholds: Option<SlowRequestThresholds>` on both `Settings` and `PerFileRulesFile` with `#[serde(default)]` so existing v1 files load cleanly with None. New `PerFileRulesFile::is_effectively_empty()` lets the save IPC delete the file when both rules and thresholds end up empty.
```

In the `ui/` section, add the new component and the per-tab state extension:

```
- `src/components/InsightsDrawer.vue` - right-side collapsible drawer (360px) hosting the slow-request table, mode toggle (Normalised / Raw), free-text path filter, sort dropdown (Total / Count / Max / p95 / Avg / Path) with same-option-flips-direction, per-file threshold editor with Auto / Global / Per-file chip, validation, Save and Clear-override actions.
```

In the `LogViewport.vue` notes, append:

```
. P11 added a 4px `.speed-rail` `<canvas>` immediately right of the minimap painting a continuous green-to-amber-to-red gradient via `createLinearGradient` with one colour stop per bucket placed at its vertical midpoint. Empty buckets inherit the fast (green) colour so quiet regions read as healthy and the rail always paints. Hover and click are shared with the minimap so the two stripes read as one combined scrubber. The minimap tooltip gained a fourth "N hits, avg X, peak Y" line when the hovered bucket has slow-request data.
```

Adjust line counts and existing summary lines if they reference exact byte / line counts that have shifted.

- [ ] **Step 2: Append to memory.md**

Append one line to `.wolf/memory.md`:

```
- 2026-05-23: slow-request insights landed. New clog-core::slow_requests module (detection regex covers both Play formats, dedup on timestamp_ms + normalised_path + class_method, aggregation with nearest-rank p95 and a 50-occurrence cap, speed grid builder). Four new clog-app IPCs - get_slow_requests, get_slow_request_speeds, get_slow_request_thresholds, save_slow_request_thresholds - sharing a short-lived SlowRequestCache on OpenedFile. Persistence extended with optional slow_request_thresholds on both Settings (global default) and PerFileRulesFile (per-file override), backward-compatible via #[serde(default)]. UI gained a 4px continuous-gradient speed rail next to the minimap (green default, no cell edges) and a right-side InsightsDrawer with sortable entry table, path-mode toggle, free-text filter, expand-to-occurrences, per-file threshold editor and Auto / Global / Per-file chip. Settings modal grew a Slow requests section for the global default.
```

- [ ] **Step 3: Commit**

```powershell
git add .wolf/anatomy.md .wolf/memory.md
git commit -m "Updated OpenWolf anatomy with the slow-request insights surface (new clog-core module, four new IPCs, persistence extensions, new drawer component, speed-rail addition to LogViewport) and appended the corresponding memory entry."
```

---

## Task 18: Final verification + smoke

End-to-end sweep against the live app on real fixtures.

- [ ] **Step 1: Full lint + test sweep**

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm --prefix ui run build
npm --prefix ui run test
```

Expected: every command green.

- [ ] **Step 2: Manual smoke - prod fixture**

```powershell
cargo dev
```

Open `research/solopress-prod.log`:

- Speed rail paints a continuous green-to-red gradient next to the minimap.
- Visible red regions correlate with regions holding the longest-duration `SLOW REQUEST` lines.
- No hard cell edges between buckets.
- Click the insights toggle in the header bar -> drawer slides in from the right.
- Drawer totals row reads something like "412 hits across 37 endpoints".
- At least one entry exists for `/preflight/killpreflightrequest.json` or `/productfront/getupdatedproductoptions.json/`.
- Click an entry path -> viewport scrolls to that endpoint's slowest hit.
- Click the row chrome (not the path) -> occurrence list expands.
- Click an occurrence -> viewport scrolls to that line.
- Flip Normalised <-> Raw -> entry counts change (raw -> more entries).
- Type in the filter -> table narrows live.
- Pick a different sort field, then pick it again -> direction flips.
- Chip reads "AUTO". Open the override panel, set Fast 500 / Slow 8000, save -> chip reads "PER-FILE", rail repaints. Clear override -> chip returns to "AUTO".
- Open Settings -> Slow requests section -> set Fast 1000 / Slow 5000 -> Save -> drawer chip flips to "GLOBAL" automatically. Reset to default -> drawer chip flips back to "AUTO" automatically.

- [ ] **Step 3: Manual smoke - no-slow-requests fixture**

In the dev shell, open `research/solopress-wsl-oink.out`:

- Speed rail paints as a uniform green strip.
- Drawer reads "No slow requests detected in this file."
- Threshold chip reads "AUTO"; effective Fast and Slow both `0` (or `0` and `1` after the auto-fallback adjustment).

- [ ] **Step 4: Manual smoke - tail mode**

In a second shell:

```powershell
cargo run -p clog-core --example fake_tailer -- <some-test-path>.log --rate 5
```

In the dev shell, open that file. Confirm that as `SLOW REQUEST` lines land (synthesise them via a quick local script if `fake_tailer` does not emit any by default), they appear in the drawer and the speed rail repaints. If `fake_tailer` does not emit `SLOW REQUEST` lines, manually append a few to a test file with `Add-Content` to exercise the path.

- [ ] **Step 5: Manual smoke - tooltip**

Hover the speed rail and the minimap in a red region. The tooltip reads:

```
line 1234
14:32:01.421
3 hits, avg 5.2s, peak 9.1s
```

Hover a quiet region. Only line + timestamp lines render.

- [ ] **Step 6: Done**

No further commits. Feature complete.
