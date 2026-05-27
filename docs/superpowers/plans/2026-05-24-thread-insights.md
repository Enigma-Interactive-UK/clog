# Thread insights + consolidated filter flyout implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a thread-group filter axis (Requests / Jobs / Scheduler / System / Infra / Other) that runs alongside the existing log-level mask, and collapse both filter axes behind a single "Filters" button + popover in the search bar.

**Architecture:** A new `clog_core::thread_groups` module owns a `ThreadGroup` enum with a stable bitmask and a `classify(&[u8])` function. The existing `LevelMask`/`SearchOptions` gain a parallel `ThreadGroupMask`. The IPC `list_records_by_level` is renamed to `list_records_by_filters` taking both masks, and `SearchRequest` grows a `thread_group_mask` field. Frontend mirrors with `THREAD_GROUP_KEYS` / `threadGroupAllow` and a new `FiltersPopover.vue` component, with persistence round-tripping `thread_group_mask` via `RestoredFile`. All field additions use `#[serde(default ...)]` / optional TS fields so older `session.json` files keep loading.

**Tech Stack:** Rust (clog-core, clog-app via Tauri v2), TypeScript + Vue 3 (Composition API, `<script setup>`), Vitest, `cargo test`.

**Spec:** [docs/superpowers/specs/2026-05-24-thread-insights-design.md](../specs/2026-05-24-thread-insights-design.md)

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `crates/clog-core/src/thread_groups.rs` | Create | `ThreadGroup` enum, `ThreadGroupMask`, `classify`, `group_bit` |
| `crates/clog-core/src/lib.rs` | Modify | `mod thread_groups; pub use thread_groups::{...}` |
| `crates/clog-core/src/search.rs` | Modify | Add `thread_group_mask` to `SearchOptions`; gate records on both masks |
| `crates/clog-app/src/main.rs` | Modify | Rename `list_records_by_level` -> `list_records_by_filters`; extend `SearchRequest` |
| `crates/clog-app/src/persistence.rs` | Modify | Add `thread_group_mask: u32` to `RestoredFile` with serde default |
| `ui/src/types.ts` | Modify | Add `ThreadGroupKey`, `THREAD_GROUP_KEYS`, `THREAD_GROUP_BIT`; extend `RestoredFile` |
| `ui/src/tab.ts` | Modify | `threadGroupAllow`, helpers, `toggleThreadGroup`, plumb mask through `refreshAllowedRecords` + `runSearch` + restore/snapshot |
| `ui/src/composables/useSession.ts` | Modify | Fold `threadGroupAllow` into autosave fingerprint |
| `ui/src/components/FiltersPopover.vue` | Create | Popover hosting Levels + Threads sections + Reset |
| `ui/src/components/SearchBar.vue` | Modify | Remove inline `.level-mask`; add Filters button + popover wiring |
| `docs/future-ideas.md` | Modify | Note thread groups landed; add "Custom user-defined thread groups" entry |

---

## Task 1: ThreadGroup enum + classifier (core)

**Files:**
- Create: `crates/clog-core/src/thread_groups.rs`
- Modify: `crates/clog-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/clog-core/src/thread_groups.rs`:

```rust
//! Thread-name classification into a fixed five-group taxonomy + Other.
//!
//! Rules are tried in order; first match wins. Patterns are hand-rolled
//! byte matchers (no regex dependency in the hot path) because the
//! shapes are simple and `classify` runs once per record on the
//! filter/search hot path.
//!
//! Group set is locked for v1; see
//! docs/superpowers/specs/2026-05-24-thread-insights-design.md.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadGroup {
    Requests,
    Jobs,
    Scheduler,
    System,
    Infra,
    Other,
}

#[must_use]
pub fn group_bit(group: ThreadGroup) -> u8 {
    match group {
        ThreadGroup::Requests  => 1 << 0,
        ThreadGroup::Jobs      => 1 << 1,
        ThreadGroup::Scheduler => 1 << 2,
        ThreadGroup::System    => 1 << 3,
        ThreadGroup::Infra     => 1 << 4,
        ThreadGroup::Other     => 1 << 5,
    }
}

/// Bitmask of thread groups the filter is allowed to include. A bit set =
/// include. Layout matches `group_bit`. `ALL` = 0x3F.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadGroupMask(pub u8);

impl ThreadGroupMask {
    pub const ALL: Self = Self(0x3F);

    #[must_use]
    pub fn allows(self, group: ThreadGroup) -> bool {
        self.0 & group_bit(group) != 0
    }

    #[must_use]
    pub fn with(self, group: ThreadGroup, allow: bool) -> Self {
        if allow {
            Self(self.0 | group_bit(group))
        } else {
            Self(self.0 & !group_bit(group))
        }
    }
}

impl Default for ThreadGroupMask {
    fn default() -> Self {
        Self::ALL
    }
}

/// Classify a thread byte slice into one of the five named groups, or
/// `Other` as the fallthrough.
#[must_use]
pub fn classify(thread: &[u8]) -> ThreadGroup {
    // 1. Requests: ^play-thread-\d+$
    if has_prefix_then_digits(thread, b"play-thread-") {
        return ThreadGroup::Requests;
    }
    // 2. Jobs: ^jobs-thread-\d+$
    if has_prefix_then_digits(thread, b"jobs-thread-") {
        return ThreadGroup::Jobs;
    }
    // 3. Scheduler: case-insensitive substring "quartz"
    if contains_ascii_ci(thread, b"quartz") {
        return ThreadGroup::Scheduler;
    }
    // 4. System: ^main$  |  ^Thread-\d+$
    if thread == b"main" {
        return ThreadGroup::System;
    }
    if has_prefix_then_digits(thread, b"Thread-") {
        return ThreadGroup::System;
    }
    // 5. Infra: well-known framework plumbing names.
    if matches_infra(thread) {
        return ThreadGroup::Infra;
    }
    ThreadGroup::Other
}

/// True iff `s` starts with `prefix` and the remainder is a non-empty
/// ASCII-digit-only tail.
fn has_prefix_then_digits(s: &[u8], prefix: &[u8]) -> bool {
    if !s.starts_with(prefix) {
        return false;
    }
    let tail = &s[prefix.len()..];
    !tail.is_empty() && tail.iter().all(u8::is_ascii_digit)
}

/// Case-insensitive ASCII substring search. Non-ASCII bytes in either
/// side compare exactly. Fine for our needs - "quartz" is pure ASCII.
fn contains_ascii_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    let nl = needle.len();
    let limit = haystack.len() - nl + 1;
    'outer: for i in 0..limit {
        for j in 0..nl {
            if !haystack[i + j].eq_ignore_ascii_case(&needle[j]) {
                continue 'outer;
            }
        }
        return true;
    }
    false
}

fn matches_infra(s: &[u8]) -> bool {
    // pool-\d+-thread-\d+
    if s.starts_with(b"pool-") {
        let rest = &s[5..];
        if let Some(dash) = rest.iter().position(|&b| b == b'-') {
            let pool_id = &rest[..dash];
            let after = &rest[dash + 1..];
            if !pool_id.is_empty()
                && pool_id.iter().all(u8::is_ascii_digit)
                && after.starts_with(b"thread-")
                && has_digits_only(&after[7..])
            {
                return true;
            }
        }
    }
    // New I/O worker #\d+   |   New I/O boss #\d+
    if let Some(rest) = strip_prefix(s, b"New I/O worker #") {
        return has_digits_only(rest);
    }
    if let Some(rest) = strip_prefix(s, b"New I/O boss #") {
        return has_digits_only(rest);
    }
    // I/O dispatcher \d+
    if let Some(rest) = strip_prefix(s, b"I/O dispatcher ") {
        return has_digits_only(rest);
    }
    // jgroups-...   (any tail; the suffix carries cluster/node and varies)
    if s.starts_with(b"jgroups-") {
        return true;
    }
    // Memcached IO over ...
    if s.starts_with(b"Memcached IO ") {
        return true;
    }
    false
}

fn strip_prefix<'a>(s: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    if s.starts_with(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

fn has_digits_only(s: &[u8]) -> bool {
    !s.is_empty() && s.iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_play_request_workers() {
        assert_eq!(classify(b"play-thread-1"),  ThreadGroup::Requests);
        assert_eq!(classify(b"play-thread-20"), ThreadGroup::Requests);
    }

    #[test]
    fn classifies_play_job_workers() {
        assert_eq!(classify(b"jobs-thread-1"), ThreadGroup::Jobs);
        assert_eq!(classify(b"jobs-thread-8"), ThreadGroup::Jobs);
    }

    #[test]
    fn classifies_quartz_workers() {
        assert_eq!(classify(b"DefaultQuartzScheduler_Worker-10"), ThreadGroup::Scheduler);
        assert_eq!(classify(b"quartz-scheduler-1"),               ThreadGroup::Scheduler);
        assert_eq!(classify(b"MyQuartzScheduler_Worker-3"),       ThreadGroup::Scheduler);
    }

    #[test]
    fn classifies_system_threads() {
        assert_eq!(classify(b"main"),       ThreadGroup::System);
        assert_eq!(classify(b"Thread-16"),  ThreadGroup::System);
        assert_eq!(classify(b"Thread-1"),   ThreadGroup::System);
    }

    #[test]
    fn classifies_infra_threads() {
        assert_eq!(classify(b"pool-3-thread-1"),       ThreadGroup::Infra);
        assert_eq!(classify(b"pool-6-thread-1"),       ThreadGroup::Infra);
        assert_eq!(classify(b"New I/O worker #1"),     ThreadGroup::Infra);
        assert_eq!(classify(b"New I/O worker #63"),    ThreadGroup::Infra);
        assert_eq!(classify(b"New I/O boss #132"),     ThreadGroup::Infra);
        assert_eq!(classify(b"I/O dispatcher 1"),      ThreadGroup::Infra);
        assert_eq!(classify(b"jgroups-12,solo.prod,solo-webapp-001-27322"), ThreadGroup::Infra);
        assert_eq!(classify(b"Memcached IO over {MemcachedConnection to /127.0.0.1:11211} - SHUTTING DOWN"), ThreadGroup::Infra);
    }

    #[test]
    fn classifies_unknown_as_other() {
        assert_eq!(classify(b""),                      ThreadGroup::Other);
        assert_eq!(classify(b"some-other-thread"),     ThreadGroup::Other);
        assert_eq!(classify(b"play-thread-"),          ThreadGroup::Other); // empty digit tail
        assert_eq!(classify(b"play-thread-abc"),       ThreadGroup::Other); // non-digit tail
        assert_eq!(classify(b"jobs-thread-1a"),        ThreadGroup::Other);
        assert_eq!(classify(b"\xff\xfe\x00"),          ThreadGroup::Other); // garbage bytes
    }

    #[test]
    fn first_match_wins_does_not_mis_route_main_substring() {
        // A thread that contains "main" but isn't exactly "main" must not
        // be classified as System.
        assert_eq!(classify(b"main-pool-worker-2"), ThreadGroup::Other);
    }

    #[test]
    fn mask_round_trip() {
        let m = ThreadGroupMask::ALL.with(ThreadGroup::Requests, false);
        assert!(!m.allows(ThreadGroup::Requests));
        assert!(m.allows(ThreadGroup::Jobs));
        let m2 = m.with(ThreadGroup::Requests, true);
        assert_eq!(m2, ThreadGroupMask::ALL);
    }

    #[test]
    fn mask_all_is_0x3f() {
        assert_eq!(ThreadGroupMask::ALL.0, 0x3F);
    }
}
```

Then add to `crates/clog-core/src/lib.rs` in the module list and re-exports. Find the line `pub mod search;` and add `pub mod thread_groups;` next to it. Find the `pub use search::{...}` block and add below it:

```rust
pub use thread_groups::{classify as classify_thread, group_bit, ThreadGroup, ThreadGroupMask};
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p clog-core thread_groups`
Expected: all eight tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/clog-core/src/thread_groups.rs crates/clog-core/src/lib.rs
git commit -m "Added clog_core::thread_groups with the v1 thread-group taxonomy (Requests/Jobs/Scheduler/System/Infra/Other), a u8 ThreadGroupMask, and a byte-level classify() that hand-matches each rule without a regex dependency."
```

---

## Task 2: Wire thread_group_mask through SearchOptions

**Files:**
- Modify: `crates/clog-core/src/search.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/clog-core/src/search.rs`, after `level_mask_filters_records`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p clog-core thread_group_mask_filters_records`
Expected: compile error - `SearchOptions` has no field `thread_group_mask`.

- [ ] **Step 3: Extend SearchOptions and predicate**

Edit `crates/clog-core/src/search.rs`. Update the imports near the top:

```rust
use crate::record::{Level, RecordHeader};
use crate::thread_groups::{classify, ThreadGroupMask};
```

Update `SearchOptions`:

```rust
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
```

Add a tiny predicate helper just above `search_records`:

```rust
fn record_passes(rec: &RecordHeader, bytes: &[u8], opts: SearchOptions) -> bool {
    if !opts.level_mask.allows(rec.level) {
        return false;
    }
    if opts.thread_group_mask == ThreadGroupMask::ALL {
        return true;
    }
    let group = match rec.fields.thread {
        Some((s, e)) => {
            let start = usize::try_from(rec.byte_offset).unwrap_or(usize::MAX)
                .saturating_add(s as usize);
            let end = usize::try_from(rec.byte_offset).unwrap_or(usize::MAX)
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
```

Then in both `SearchMode::Smart` and `SearchMode::Regex` arms inside `search_records`, replace the `if !opts.level_mask.allows(rec.level) { return None; }` early-out with:

```rust
if !record_passes(rec, bytes, opts) {
    return None;
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p clog-core search`
Expected: all search tests pass, including the new `thread_group_mask_filters_records`.

- [ ] **Step 5: Commit**

```bash
git add crates/clog-core/src/search.rs
git commit -m "Extended SearchOptions with a thread_group_mask. search_records now rejects records whose classified thread group is masked out, alongside the existing level filter."
```

---

## Task 3: Add list_records_by_filters IPC, drop list_records_by_level

**Files:**
- Modify: `crates/clog-app/src/main.rs`

- [ ] **Step 1: Read the existing IPC**

Re-read [crates/clog-app/src/main.rs:1166-1199](../../../crates/clog-app/src/main.rs#L1166-L1199) (the `list_records_by_level` command) and [main.rs:2095-2120](../../../crates/clog-app/src/main.rs#L2095-L2120) area to find the `invoke_handler` macro list.

- [ ] **Step 2: Rename and extend the command**

Edit `crates/clog-app/src/main.rs`. Update the imports near the top of the file - find:

```rust
use clog_core::{
    ...CompiledPattern, CoreError, HeaderFields, HitRef, Level, LevelMask, LineSource...
```

Add `ThreadGroup, ThreadGroupMask, classify_thread` to the same `use clog_core::{...}` block (alphabetical position).

Replace the entire `list_records_by_level` function (lines 1171-1199) with:

```rust
/// Return every record's `(record_idx, first_line, line_count)` whose
/// level passes `level_mask` AND whose thread group passes
/// `thread_group_mask`. The UI uses this to build `filteredLineIndices`
/// when the search query is empty -- the masks alone narrow the view
/// without needing a fake "match all" search.
#[tauri::command]
fn list_records_by_filters(
    state: State<'_, AppState>,
    file_id: u64,
    level_mask: u32,
    thread_group_mask: u32,
) -> Result<RecordRefsPayload, IpcError> {
    let guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;
    let lmask = LevelMask(u16::try_from(level_mask & 0xFFFF).unwrap_or(0xFFFF));
    let tmask = ThreadGroupMask(u8::try_from(thread_group_mask & 0xFF).unwrap_or(0x3F));
    let bytes = &file.bytes;
    let refs = file
        .records
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            if !lmask.allows(r.level) {
                return false;
            }
            if tmask == ThreadGroupMask::ALL {
                return true;
            }
            let group = thread_group_of(r, bytes);
            tmask.allows(group)
        })
        .map(|(i, r)| RecordRef {
            record_idx: i as u64,
            record_first_line: u64::from(r.line_offset),
            record_line_count: r.line_count,
            level: r.level,
        })
        .collect();
    Ok(RecordRefsPayload { refs })
}

fn thread_group_of(rec: &clog_core::RecordHeader, bytes: &[u8]) -> ThreadGroup {
    match rec.fields.thread {
        Some((s, e)) => {
            let base = usize::try_from(rec.byte_offset).unwrap_or(usize::MAX);
            let start = base.saturating_add(s as usize);
            let end = base.saturating_add(e as usize).min(bytes.len());
            if start > end || start >= bytes.len() {
                ThreadGroup::Other
            } else {
                classify_thread(&bytes[start..end])
            }
        }
        None => ThreadGroup::Other,
    }
}
```

Note: the existing command reads `file.records` but does not need raw bytes. Confirm the `FileEntry` struct exposes `bytes` (or whatever the field is called) - search for `struct FileEntry` / `struct OpenFile` near the top of main.rs. If the field is named differently (e.g. `raw`, `data`, `mmap`), substitute that name into `&file.bytes` above.

- [ ] **Step 3: Update SearchRequest to carry thread_group_mask**

Find the `pub struct SearchRequest` (around line 1232) and update it:

```rust
#[derive(Debug, serde::Deserialize)]
pub struct SearchRequest {
    mode: String,
    query: String,
    #[serde(default)]
    case_sensitive: bool,
    /// Bitmask of allowed levels. Bit ordering matches `clog_core::search::level_bit`.
    level_mask: u32,
    /// Bitmask of allowed thread groups. Bit ordering matches `clog_core::group_bit`.
    /// Defaults to ALL (0x3F) when absent so older session-restored payloads
    /// still decode.
    #[serde(default = "default_full_thread_group_mask")]
    thread_group_mask: u32,
}

fn default_full_thread_group_mask() -> u32 {
    0x3F
}
```

In `start_search` (around line 1244), update the destructure and `SearchOptions` construction:

```rust
let SearchRequest {
    mode,
    query,
    case_sensitive,
    level_mask,
    thread_group_mask,
} = request;
```

```rust
let opts = SearchOptions {
    case_sensitive,
    level_mask: LevelMask(u16::try_from(level_mask & 0xFFFF).unwrap_or(0xFFFF)),
    thread_group_mask: ThreadGroupMask(u8::try_from(thread_group_mask & 0xFF).unwrap_or(0x3F)),
};
```

- [ ] **Step 4: Update the invoke_handler macro list**

Find the `tauri::generate_handler![...]` block (around line 2095-2120). Replace `list_records_by_level` with `list_records_by_filters` in the list.

- [ ] **Step 5: Build to surface any remaining references**

Run: `cargo build -p clog-app`
Expected: builds clean. If a stray `list_records_by_level` reference remains, fix it and re-run.

- [ ] **Step 6: Run tests**

Run: `cargo test -p clog-app`
Expected: existing tests pass; no new tests added in this task.

- [ ] **Step 7: Commit**

```bash
git add crates/clog-app/src/main.rs
git commit -m "Renamed list_records_by_level to list_records_by_filters and added a thread_group_mask parameter alongside level_mask. SearchRequest also carries the new mask with a serde default of 0x3F so older session payloads keep decoding."
```

---

## Task 4: Persist thread_group_mask in RestoredFile

**Files:**
- Modify: `crates/clog-app/src/persistence.rs`

- [ ] **Step 1: Write the failing test**

Add to the `thresholds_tests` module (or a new module at the bottom) of `crates/clog-app/src/persistence.rs`:

```rust
#[test]
fn restored_file_loads_old_payload_without_thread_group_mask() {
    let raw = r#"{"path":"/x","scroll_top":0,"follow_tail":true,"level_mask":63,"filter_text":"","search_mode":"smart","search_case_sensitive":false,"filter_mode":false}"#;
    let r: RestoredFile = serde_json::from_str(raw).expect("v1 RestoredFile decodes");
    assert_eq!(r.thread_group_mask, 0x3F);
}

#[test]
fn restored_file_round_trips_thread_group_mask() {
    let r = RestoredFile {
        path: "/x".into(),
        scroll_top: 0.0,
        follow_tail: true,
        level_mask: 63,
        thread_group_mask: 0x0B,
        filter_text: String::new(),
        search_mode: "smart".into(),
        search_case_sensitive: false,
        filter_mode: false,
        bookmarks: vec![],
    };
    let json = serde_json::to_string(&r).expect("serialises");
    let back: RestoredFile = serde_json::from_str(&json).expect("round-trips");
    assert_eq!(back.thread_group_mask, 0x0B);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p clog-app restored_file_loads_old_payload_without_thread_group_mask restored_file_round_trips_thread_group_mask`
Expected: compile error - `RestoredFile` has no field `thread_group_mask`.

- [ ] **Step 3: Add the field**

Edit `crates/clog-app/src/persistence.rs`. In `RestoredFile` (around line 177-199), add after `level_mask`:

```rust
#[serde(default = "default_full_thread_group_mask")]
pub thread_group_mask: u32,
```

Add the helper next to `default_full_mask`:

```rust
fn default_full_thread_group_mask() -> u32 {
    0x3F
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p clog-app persistence`
Expected: all persistence tests pass, including the two new ones.

- [ ] **Step 5: Commit**

```bash
git add crates/clog-app/src/persistence.rs
git commit -m "Added thread_group_mask to RestoredFile with a serde default of 0x3F so older session.json payloads still load with all thread groups enabled."
```

---

## Task 5: TypeScript types

**Files:**
- Modify: `ui/src/types.ts`

- [ ] **Step 1: Extend types**

Edit `ui/src/types.ts`. After the `LEVEL_KEYS` declaration (line 86), add:

```typescript
export type ThreadGroupKey =
  | 'requests'
  | 'jobs'
  | 'scheduler'
  | 'system'
  | 'infra'
  | 'other'

// Bit positions must match clog_core::thread_groups::group_bit.
export const THREAD_GROUP_BIT: Record<ThreadGroupKey, number> = {
  requests:  1 << 0,
  jobs:      1 << 1,
  scheduler: 1 << 2,
  system:    1 << 3,
  infra:     1 << 4,
  other:     1 << 5,
}
export const THREAD_GROUP_KEYS: ThreadGroupKey[] = [
  'requests', 'jobs', 'scheduler', 'system', 'infra', 'other',
]
export const THREAD_GROUP_LABEL: Record<ThreadGroupKey, string> = {
  requests:  'Requests',
  jobs:      'Jobs',
  scheduler: 'Scheduler',
  system:    'System',
  infra:     'Infra',
  other:     'Other',
}
export const FULL_THREAD_GROUP_MASK = 0x3F
```

In `RestoredFile` (line 109-119), add `thread_group_mask: number` next to `level_mask: number`:

```typescript
export interface RestoredFile {
  path: string
  scroll_top: number
  follow_tail: boolean
  level_mask: number
  thread_group_mask: number
  filter_text: string
  search_mode: SearchMode
  search_case_sensitive: boolean
  filter_mode: boolean
  bookmarks?: number[]
}
```

- [ ] **Step 2: Verify it compiles**

Run: `npm --prefix ui run build`
Expected: build succeeds (type errors in `tab.ts` are fine for now - we'll fix in the next task; if any do appear, note them and proceed - they'll resolve in Task 6).

Actually a cleaner approach: `npm --prefix ui run build` may fail because Task 6 hasn't supplied `thread_group_mask` in the `snapshot()`/`applyRestored()` flow yet. If it does fail with a `thread_group_mask` missing error, that's expected - move to Task 6 and the chain resolves.

- [ ] **Step 3: Commit**

```bash
git add ui/src/types.ts
git commit -m "Added ThreadGroupKey, THREAD_GROUP_BIT/KEYS/LABEL constants and FULL_THREAD_GROUP_MASK. Bit layout mirrors clog_core::thread_groups::group_bit. RestoredFile gains a required thread_group_mask field."
```

---

## Task 6: Tab state for thread groups

**Files:**
- Modify: `ui/src/tab.ts`

- [ ] **Step 1: Read existing helpers**

Re-read [ui/src/tab.ts:62-105](../../../ui/src/tab.ts#L62-L105) for the level-mask helper pattern. Mirror that pattern exactly.

- [ ] **Step 2: Update imports**

In `ui/src/tab.ts`, update the imports from `./types` to add the new constants:

```typescript
import {
  FULL_THREAD_GROUP_MASK,
  LEVEL_BIT,
  LEVEL_KEYS,
  PAGE_SIZE,
  THREAD_GROUP_BIT,
  THREAD_GROUP_KEYS,
  type ApplyPatternPayload,
  type EffectiveThresholds,
  type HitRef,
  type IpcError,
  type LineRow,
  type LinesPayload,
  type LevelKey,
  type OpenedFile,
  type PatternTestPayload,
  type RecordRef,
  type RecordRefsPayload,
  type RestoredFile,
  type SearchDelta,
  type SearchMode,
  type SlowRequestPathMode,
  type SlowRequestSummary,
  type TailDelta,
  type ThreadGroupKey,
} from './types'
```

- [ ] **Step 3: Add helpers next to the level-mask helpers**

After `applyMaskToAllow` (around line 105), add:

```typescript
export function buildThreadGroupMaskFromAllow(allow: Record<string, boolean>): number {
  let mask = 0
  for (const [k, v] of Object.entries(allow)) {
    if (v) mask |= THREAD_GROUP_BIT[k as ThreadGroupKey] ?? 0
  }
  return mask
}

export function isFullThreadGroupMask(allow: Record<string, boolean>): boolean {
  for (const k of THREAD_GROUP_KEYS) {
    if (!allow[k]) return false
  }
  return true
}

export function defaultThreadGroupAllow(): Record<ThreadGroupKey, boolean> {
  return {
    requests: true,
    jobs: true,
    scheduler: true,
    system: true,
    infra: true,
    other: true,
  }
}

export function applyThreadGroupMaskToAllow(mask: number): Record<ThreadGroupKey, boolean> {
  const allow = defaultThreadGroupAllow()
  for (const k of THREAD_GROUP_KEYS) {
    allow[k] = (mask & THREAD_GROUP_BIT[k]) !== 0
  }
  return allow
}
```

- [ ] **Step 4: Add the ref and toggle method**

Inside `createTab`, after the existing `const levelAllow = ref<Record<string, boolean>>(defaultLevelAllow())` (around line 134), add:

```typescript
  const threadGroupAllow = ref<Record<ThreadGroupKey, boolean>>(defaultThreadGroupAllow())
```

After `toggleLevel` (around line 419-423), add:

```typescript
  function toggleThreadGroup(group: ThreadGroupKey) {
    threadGroupAllow.value = { ...threadGroupAllow.value, [group]: !threadGroupAllow.value[group] }
    void refreshAllowedRecords()
    if (searchQuery.value.trim().length > 0) scheduleSearch()
  }
```

- [ ] **Step 5: Update refreshAllowedRecords to pass both masks and call the renamed IPC**

Replace `refreshAllowedRecords` (around line 425-439) with:

```typescript
  async function refreshAllowedRecords(): Promise<void> {
    if (isFullLevelMask(levelAllow.value) && isFullThreadGroupMask(threadGroupAllow.value)) {
      allowedRecords.value = null
      return
    }
    try {
      const payload = await invoke<RecordRefsPayload>('list_records_by_filters', {
        fileId: file.value.file_id,
        levelMask: buildLevelMaskFromAllow(levelAllow.value),
        threadGroupMask: buildThreadGroupMaskFromAllow(threadGroupAllow.value),
      })
      allowedRecords.value = payload.refs
    } catch {
      // non-fatal -- keep previous list
    }
  }
```

- [ ] **Step 6: Update runSearch to send thread_group_mask**

In `runSearch` (around line 327), update the mask construction and the `invoke('start_search', ...)` call:

```typescript
    const mask = buildLevelMaskFromAllow(levelAllow.value)
    const tgMask = buildThreadGroupMaskFromAllow(threadGroupAllow.value)
```

```typescript
    try {
      await invoke('start_search', {
        fileId,
        request: {
          mode: searchMode.value,
          query,
          case_sensitive: searchCaseSensitive.value,
          level_mask: mask,
          thread_group_mask: tgMask,
        },
        onHits: channel,
      })
```

- [ ] **Step 7: Update tail-driven refreshes**

In `handleTailDelta` (around line 285 and 307), the existing two lines that read `if (!isFullLevelMask(levelAllow.value)) void refreshAllowedRecords()` must become:

```typescript
      if (!isFullLevelMask(levelAllow.value) || !isFullThreadGroupMask(threadGroupAllow.value)) void refreshAllowedRecords()
```

Both occurrences.

- [ ] **Step 8: Round-trip thread_group_mask in applyRestored + snapshot**

In `applyRestored` (around line 483), after `levelAllow.value = applyMaskToAllow(r.level_mask)`, add:

```typescript
    threadGroupAllow.value = applyThreadGroupMaskToAllow(r.thread_group_mask ?? FULL_THREAD_GROUP_MASK)
```

In `snapshot` (around line 503), after `level_mask: buildLevelMaskFromAllow(levelAllow.value),`, add:

```typescript
      thread_group_mask: buildThreadGroupMaskFromAllow(threadGroupAllow.value),
```

- [ ] **Step 9: Expose threadGroupAllow + toggleThreadGroup on the api object**

In the `const api = { ... }` block (around line 538), add `threadGroupAllow` next to `levelAllow`, and `toggleThreadGroup` next to `toggleLevel`.

- [ ] **Step 10: Build to verify**

Run: `npm --prefix ui run build`
Expected: builds clean.

- [ ] **Step 11: Commit**

```bash
git add ui/src/tab.ts
git commit -m "Added per-tab threadGroupAllow state, build/apply mask helpers, toggleThreadGroup, and plumbed the new mask through refreshAllowedRecords (now calling list_records_by_filters), runSearch, applyRestored, and snapshot. Tail-delta refresh also considers the new mask."
```

---

## Task 7: Session autosave fingerprint

**Files:**
- Modify: `ui/src/composables/useSession.ts`

- [ ] **Step 1: Extend the fingerprint**

Edit `ui/src/composables/useSession.ts`. In the `watch(...)` source function (around line 83), append the thread-group allow set to the fingerprint string. Replace the watch source with:

```typescript
    () => tabs.value.map((t) => `${t.file.value.path}|${t.followTail.value}|${t.searchMode.value}|${t.searchQuery.value}|${t.searchCaseSensitive.value}|${t.filterMode.value}|${Object.entries(t.levelAllow.value).filter(([, v]) => v).map(([k]) => k).join(',')}|tg:${Object.entries(t.threadGroupAllow.value).filter(([, v]) => v).map(([k]) => k).join(',')}|${t.scrollTop.value}|bm:${t.bookmarks.value.size}`).join('||') + '#' + String(activeTabId.value),
```

- [ ] **Step 2: Build to verify**

Run: `npm --prefix ui run build`
Expected: builds clean.

- [ ] **Step 3: Commit**

```bash
git add ui/src/composables/useSession.ts
git commit -m "Folded the per-tab threadGroupAllow set into the session autosave fingerprint so toggling a thread group triggers a debounced session save."
```

---

## Task 8: FiltersPopover.vue

**Files:**
- Create: `ui/src/components/FiltersPopover.vue`

- [ ] **Step 1: Create the component**

Create `ui/src/components/FiltersPopover.vue`:

```vue
<script setup lang="ts">
/**
 * Popover hosting the level mask and thread-group mask toggles. Anchored
 * by the parent, which positions us absolutely. We do not own the
 * trigger -- only the menu surface and its outside-click/Esc dismissal.
 */
import { onBeforeUnmount, onMounted, ref } from 'vue'
import {
  LEVEL_KEYS,
  THREAD_GROUP_KEYS,
  THREAD_GROUP_LABEL,
  type LevelKey,
  type ThreadGroupKey,
} from '../types'
import { defaultLevelAllow, defaultThreadGroupAllow } from '../tab'
import type { Tab } from '../tab'

const props = defineProps<{ tab: Tab }>()
const emit = defineEmits<{ (e: 'close'): void }>()

const rootEl = ref<HTMLElement | null>(null)

function toggleLevel(level: LevelKey) {
  props.tab.toggleLevel(level)
}

function toggleThreadGroup(group: ThreadGroupKey) {
  props.tab.toggleThreadGroup(group)
}

function resetAll() {
  // Reset levels.
  const allLevels = defaultLevelAllow()
  for (const k of LEVEL_KEYS) {
    if (props.tab.levelAllow.value[k] !== allLevels[k]) toggleLevel(k)
  }
  // Reset thread groups.
  const allGroups = defaultThreadGroupAllow()
  for (const k of THREAD_GROUP_KEYS) {
    if (props.tab.threadGroupAllow.value[k] !== allGroups[k]) toggleThreadGroup(k)
  }
}

function onDocClick(ev: MouseEvent) {
  const root = rootEl.value
  if (!root) return
  if (root.contains(ev.target as Node)) return
  // Click landed outside the popover surface -- but if it landed on the
  // trigger button, the parent will toggle us off; otherwise we dismiss.
  emit('close')
}

function onKey(ev: KeyboardEvent) {
  if (ev.key === 'Escape') {
    ev.preventDefault()
    emit('close')
  }
}

onMounted(() => {
  // setTimeout so the originating click that opened us doesn't immediately
  // re-fire this handler and close the popover.
  setTimeout(() => document.addEventListener('mousedown', onDocClick), 0)
  document.addEventListener('keydown', onKey)
})

onBeforeUnmount(() => {
  document.removeEventListener('mousedown', onDocClick)
  document.removeEventListener('keydown', onKey)
})
</script>

<template>
  <div ref="rootEl" class="filters-popover" role="menu">
    <section class="filters-section">
      <h4 class="filters-heading">Levels</h4>
      <div class="filters-row">
        <button
          v-for="lvl in LEVEL_KEYS"
          :key="lvl"
          type="button"
          class="filter-pill"
          :class="['lvl-' + lvl, { 'is-off': !tab.levelAllow.value[lvl] }]"
          :title="`Toggle ${lvl.toUpperCase()} records`"
          @click="toggleLevel(lvl)"
        >{{ lvl.toUpperCase() }}</button>
      </div>
    </section>
    <section class="filters-section">
      <h4 class="filters-heading">Threads</h4>
      <div class="filters-row">
        <button
          v-for="g in THREAD_GROUP_KEYS"
          :key="g"
          type="button"
          class="filter-pill thread-pill"
          :class="{ 'is-off': !tab.threadGroupAllow.value[g] }"
          :title="`Toggle ${THREAD_GROUP_LABEL[g]} thread records`"
          @click="toggleThreadGroup(g)"
        >{{ THREAD_GROUP_LABEL[g] }}</button>
      </div>
    </section>
    <footer class="filters-footer">
      <button type="button" class="reset-link" @click="resetAll">Reset all filters</button>
    </footer>
  </div>
</template>

<style scoped>
.filters-popover {
  position: absolute;
  top: calc(100% + 4px);
  right: 0;
  z-index: 50;
  min-width: 18rem;
  background: var(--bg-elevated);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  padding: 0.5rem 0.6rem 0.4rem;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.35);
  font-size: 0.85rem;
  color: var(--fg-default);
}

.filters-section + .filters-section { margin-top: 0.5rem; }

.filters-heading {
  margin: 0 0 0.25rem;
  font-size: 0.7rem;
  font-weight: 600;
  letter-spacing: 0.05em;
  text-transform: uppercase;
  color: var(--fg-muted);
}

.filters-row {
  display: flex;
  flex-wrap: wrap;
  gap: 0.2rem;
}

.filter-pill {
  background: var(--bg-button);
  color: var(--fg-default);
  border: 1px solid var(--border-button);
  border-radius: var(--radius-sm);
  padding: 0.2rem 0.55rem;
  font-size: 0.75rem;
  font-family: var(--font-mono);
  letter-spacing: 0.04em;
  cursor: pointer;

  &:hover:not(.is-off) { background: var(--bg-button-hover); }

  &.is-off {
    opacity: 0.35;
    text-decoration: line-through;
  }
}

.lvl-trace { color: var(--level-trace); }
.lvl-debug { color: var(--level-debug); }
.lvl-info  { color: var(--level-info); }
.lvl-warn  { color: var(--level-warn); }
.lvl-error { color: var(--level-error); }
.lvl-fatal { color: var(--level-fatal); }

.thread-pill { color: var(--fg-default); }

.filters-footer {
  display: flex;
  justify-content: flex-end;
  margin-top: 0.5rem;
  padding-top: 0.4rem;
  border-top: 1px solid var(--border-default);
}

.reset-link {
  background: transparent;
  border: 0;
  color: var(--accent);
  font-size: 0.75rem;
  cursor: pointer;
  padding: 0.1rem 0.2rem;

  &:hover { text-decoration: underline; }
}
</style>
```

- [ ] **Step 2: Build to verify**

Run: `npm --prefix ui run build`
Expected: builds clean.

- [ ] **Step 3: Commit**

```bash
git add ui/src/components/FiltersPopover.vue
git commit -m "Added FiltersPopover.vue, a self-dismissing popover hosting the level mask and thread-group mask toggles plus a reset-all-filters footer link. Outside-click and Esc dismiss; pill colour for levels is preserved, thread pills use the default foreground."
```

---

## Task 9: SearchBar - replace level pills with Filters button

**Files:**
- Modify: `ui/src/components/SearchBar.vue`

- [ ] **Step 1: Add imports + popover state**

Edit `ui/src/components/SearchBar.vue`. Replace the script block with:

```vue
<script setup lang="ts">
/**
 * Search + filter + level-mask control bar for a single tab. All state
 * lives on the tab object; this component is mostly markup + the small
 * methods that translate v-model writes into the right tab mutations.
 *
 * The level + thread-group mask toggles live in a Filters popover anchored
 * to a single button on the bar -- see FiltersPopover.vue.
 *
 * `next-hit` and `prev-hit` are emitted to the parent so it can call into
 * the LogViewport's exposed `scrollToCurrentHit()` -- this component does
 * not touch the DOM.
 */
import { computed, ref, useTemplateRef } from 'vue'
import { LEVEL_KEYS, THREAD_GROUP_KEYS, THREAD_GROUP_LABEL, type SearchMode } from '../types'
import { isFullLevelMask, isFullThreadGroupMask } from '../tab'
import type { Tab } from '../tab'
import FiltersPopover from './FiltersPopover.vue'

const props = defineProps<{
  tab: Tab
}>()

const emit = defineEmits<{
  (e: 'next-hit'): void
  (e: 'prev-hit'): void
}>()

const searchInputEl = useTemplateRef<HTMLInputElement>('searchInputEl')
const filtersOpen = ref(false)

function setSearchMode(mode: SearchMode) {
  props.tab.setSearchMode(mode)
}

function toggleFilterMode() {
  props.tab.filterMode.value = !props.tab.filterMode.value
}

function clearSearch() {
  if (props.tab.searchQuery.value.length === 0) return
  props.tab.searchQuery.value = ''
  props.tab.searchError.value = null
  props.tab.clearSearchState()
  searchInputEl.value?.focus()
}

function onNextHit() {
  if (props.tab.nextHitIdx() !== null) emit('next-hit')
}
function onPrevHit() {
  if (props.tab.prevHitIdx() !== null) emit('prev-hit')
}

const hasNonDefaultFilters = computed(() =>
  !isFullLevelMask(props.tab.levelAllow.value) ||
  !isFullThreadGroupMask(props.tab.threadGroupAllow.value))

const filtersSummary = computed(() => {
  const parts: string[] = []
  const offLevels = LEVEL_KEYS.filter((k) => !props.tab.levelAllow.value[k])
  if (offLevels.length > 0) {
    parts.push(`Hiding ${offLevels.map((k) => k.toUpperCase()).join(', ')}`)
  }
  const offGroups = THREAD_GROUP_KEYS.filter((k) => !props.tab.threadGroupAllow.value[k])
  if (offGroups.length > 0) {
    parts.push(`Hiding ${offGroups.map((k) => THREAD_GROUP_LABEL[k]).join(', ')} threads`)
  }
  return parts.length > 0 ? parts.join('; ') : 'No filters active'
})

defineExpose({
  focus: () => searchInputEl.value?.focus(),
})
</script>
```

- [ ] **Step 2: Replace the level-mask block in the template**

In the template, find the `<span class="level-mask">...</span>` block (lines 128-138 in the current file) and replace it with:

```vue
    <span class="filters-anchor">
      <button
        type="button"
        class="filters-toggle"
        :class="{ 'is-on': filtersOpen, 'has-active': hasNonDefaultFilters }"
        :title="filtersSummary"
        :aria-pressed="filtersOpen"
        @click="filtersOpen = !filtersOpen"
      >
        Filters<span v-if="hasNonDefaultFilters" class="filters-badge" aria-hidden="true" />
      </button>
      <FiltersPopover
        v-if="filtersOpen"
        :tab="tab"
        @close="filtersOpen = false"
      />
    </span>
```

- [ ] **Step 3: Replace the level-mask CSS**

In the `<style scoped>` block, find the `.level-mask { ... }` block (around lines 292-313) and replace it with:

```css
  .filters-anchor {
    position: relative;
    display: inline-flex;
  }

  .filters-toggle {
    position: relative;

    &.has-active {
      border-color: var(--accent);
      color: var(--accent);
    }
  }

  .filters-badge {
    position: absolute;
    top: 2px;
    right: 2px;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
  }
```

- [ ] **Step 4: Build to verify**

Run: `npm --prefix ui run build`
Expected: builds clean.

- [ ] **Step 5: Commit**

```bash
git add ui/src/components/SearchBar.vue
git commit -m "Removed the inline level-mask pill row from the search bar in favour of a single Filters button that opens FiltersPopover. Button title summarises the active filters and shows a small accent dot when anything is filtered."
```

---

## Task 10: Manual smoke test

**Files:**
- (None modified)

- [ ] **Step 1: Build dev**

Run: `cargo dev`
Expected: app window opens.

- [ ] **Step 2: Smoke test the prod fixture**

1. Open `research/cheesecake-prod.log`.
2. Click the Filters button. Confirm the popover opens with two sections: Levels (6 pills) and Threads (6 pills: Requests, Jobs, Scheduler, System, Infra, Other).
3. Toggle Requests off. Confirm the viewport collapses from ~75k records to a much smaller set dominated by Jobs and main lines.
4. Toggle everything off in the Threads section except Other. Confirm the residual records are the rare Memcached / Netty / JGroups header lines (if any are present in the fixture).
5. Combine: level mask = ERROR only, thread group = Jobs only. Confirm only ERROR-level records from `jobs-thread-*` appear, or that the viewport is empty if no such records exist in the fixture.
6. Hit "Reset all filters". Both sections return to all-on. The badge on the Filters button disappears.
7. Open Filters again, toggle off Jobs, then close the app and reopen. Confirm the same file reopens with Jobs still toggled off (round-tripped through `session.json`).
8. Press Esc while the popover is open. Confirm it closes.
9. Click outside the popover. Confirm it closes.

- [ ] **Step 3: If all steps pass, no commit needed.**

If any step fails, file findings and iterate.

---

## Task 11: Lint + tests + docs

**Files:**
- Modify: `docs/future-ideas.md`

- [ ] **Step 1: Full lint pass**

Run: `cargo fmt --check`
Expected: clean. If not, run `cargo fmt` and re-stage.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

Run: `cargo test --workspace`
Expected: all tests pass.

Run: `npm --prefix ui run test`
Expected: all UI tests pass.

If any failures arise, fix and add a tiny commit per fix.

- [ ] **Step 2: Update docs/future-ideas.md**

Edit `docs/future-ideas.md`. Under the "Analysis / insights" section, add a new bullet:

```markdown
- **Custom user-defined thread groups** - v1 ships a fixed taxonomy
  (Requests / Jobs / Scheduler / System / Infra / Other). Power users on
  unusual stacks may want to define their own regex-based groups in
  Settings. Plumbing is in place: the classifier is one swap away from
  being driven by user rules.
```

Under the "Search beyond v1" section, the existing "Field-scoped operators (`level:ERROR thread:akka msg:\"connection refused\"`)" bullet can stay as-is - thread-group filtering implements a chunk of that idea but the operator-syntax form is still a separate piece of work.

- [ ] **Step 3: Commit**

```bash
git add docs/future-ideas.md
git commit -m "Logged user-defined thread groups as a v1.1 candidate now that the fixed taxonomy has landed."
```

---

## Done

After all eleven tasks, the branch is ready for the standard `cargo build --workspace`, `npm --prefix ui run build`, `cargo tauri build` cycle if a release artefact is needed. Otherwise merge to `master` per the project's normal flow.
