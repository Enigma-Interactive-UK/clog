# Truncate logs (collapse above/below) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user hide every log line above and/or below a chosen point so the rest of the app treats the kept region as the whole file.

**Architecture:** A record-boundary-snapped physical-line window `[before, after)` is authoritative state on the backend `OpenedFile`. One new `set_truncate` command stores it; six read commands compute over the windowed slice so the minimap, slow-request insights, speed rail, markers, search and record map all behave as if the hidden lines do not exist. The frontend narrows its viewport projection off the window, renders dashed "+N lines before/after" banners, and adds context-menu actions. The window persists per file in `session.json`. Line numbering stays absolute throughout.

**Tech Stack:** Rust (clog-app, Tauri v2 commands), Vue 3 + TypeScript (ui), vitest, cargo test.

**Design spec:** [docs/superpowers/specs/2026-05-30-truncate-logs-design.md](../specs/2026-05-30-truncate-logs-design.md)

**Conventions for every commit in this plan:**
- ASCII only in code, comments and commit messages.
- No Conventional Commit prefixes; plain British English, one sentence per change.
- No `Co-Authored-By` trailer.
- After each task, append a one-line entry to `.wolf/memory.md`.
- CI-equivalent gates (must stay green): `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm --prefix ui run build`, `npm --prefix ui run test`.

---

## File map

**Modify:**
- `crates/clog-app/src/main.rs` - `OpenedFile` struct + window helpers; new `set_truncate` command + `TruncatePayload`; window `list_records_by_filters`, `get_level_minimap`, `get_markers`, `get_slow_requests`, `get_slow_request_speeds`, `start_search`; register the new command.
- `crates/clog-app/src/persistence.rs` - `RestoredFile` gains `truncate_before` / `truncate_after`.
- `ui/src/types.ts` - `RestoredFile` gains the two fields; new `SetTruncatePayload`.
- `ui/src/tab.ts` - `truncateBefore` / `truncateAfter` refs, `setTruncate`, clear-on-rotation/pattern, snapshot/restore, api export.
- `ui/src/composables/useSession.ts` - add truncate to the autosave fingerprint.
- `ui/src/components/LogViewport.vue` - projection gate, minimap refresh watch, context-menu items, dashed banners, CSS.
- `ui/src/style.css` - banner tokens/classes (if component-scoped CSS is not used; this repo uses `style.css` global palette + component `<style>`; banner colours via existing tokens).

**Test:**
- `crates/clog-app/src/main.rs` - `#[cfg(test)]` unit tests (the file already has a `tests` module with command-level tests, e.g. `get_lines_caps_long_line_and_reports_full_len` near line 2593).
- `ui/src/truncate.test.ts` (create) - pure helper + projection tests.

---

## Task 1: Backend truncate state, helpers and `set_truncate`

**Files:**
- Modify: `crates/clog-app/src/main.rs` (`OpenedFile` struct around line 135; command region; handler registration in the `tauri::generate_handler!` macro)
- Test: `crates/clog-app/src/main.rs` `#[cfg(test)]` module

- [ ] **Step 1: Add the window fields to `OpenedFile`**

In `crates/clog-app/src/main.rs`, find the `OpenedFile` struct (line 135). Add two fields after `slow_request_cache: Option<SlowRequestCache>,` (line 175):

```rust
    /// Inclusive lower bound (first visible physical line) of the truncate
    /// window. `None` = no "above" cut. Snapped to a record's first line.
    truncate_before: Option<u64>,
    /// Exclusive upper bound (one past the last visible physical line) of the
    /// truncate window. `None` = no "below" cut. Snapped to a record boundary.
    truncate_after: Option<u64>,
```

Find where `OpenedFile` is constructed (the literal with `slow_request_cache: None,` around line 456) and add:

```rust
        truncate_before: None,
        truncate_after: None,
```

- [ ] **Step 2: Add window helper methods**

Add to the `impl OpenedFile` block (the one containing `rebuild_line_caches` near line 179):

```rust
    /// Resolve the visible physical-line window, defaulting unset bounds to the
    /// full file. Returns `(lo_inclusive, hi_exclusive)`.
    fn truncate_window(&self) -> (u64, u64) {
        let lo = self.truncate_before.unwrap_or(0);
        let hi = self.truncate_after.unwrap_or(self.line_count);
        (lo, hi)
    }

    /// Number of physical lines hidden above and below the window. Used only
    /// for the windowed line/record counts returned to the UI.
    fn windowed_line_count(&self) -> u64 {
        let (lo, hi) = self.truncate_window();
        hi.saturating_sub(lo).min(self.line_count)
    }
```

- [ ] **Step 3: Add `TruncatePayload` and the `set_truncate` command**

Add near the other small payload structs (e.g. after `RecordRefsPayload` around line 1318):

```rust
#[derive(Debug, Serialize)]
struct TruncatePayload {
    before: Option<u64>,
    after: Option<u64>,
    /// Number of physical lines inside the window.
    line_count: u64,
    /// Number of records whose first line falls inside the window.
    record_count: u64,
}
```

Add the command next to the other file commands (e.g. just before `close_file` at line 1382):

```rust
/// Set (or clear) the truncate window for `file_id`. `before` is the first
/// visible physical line; `after` is one past the last visible physical line.
/// Both are expected pre-snapped to record boundaries by the caller. Rejects an
/// empty or inverted window. `(None, None)` clears the window.
#[tauri::command]
fn set_truncate(
    state: State<'_, AppState>,
    file_id: u64,
    before: Option<u64>,
    after: Option<u64>,
) -> Result<TruncatePayload, IpcError> {
    let mut guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get_mut(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;
    if let (Some(b), Some(a)) = (before, after) {
        if b >= a {
            return Err(IpcError::BadPattern {
                message: format!("invalid truncate window: before={b} >= after={a}"),
            });
        }
    }
    file.truncate_before = before;
    file.truncate_after = after;
    let (lo, hi) = file.truncate_window();
    let record_count = file
        .records
        .iter()
        .filter(|r| {
            let f = u64::from(r.line_offset);
            f >= lo && f < hi
        })
        .count() as u64;
    Ok(TruncatePayload {
        before,
        after,
        line_count: file.windowed_line_count(),
        record_count,
    })
}
```

- [ ] **Step 4: Register the command**

Find the `tauri::generate_handler!` macro invocation (search for `get_markers,` and `list_records_by_filters,` in the handler list). Add `set_truncate,` to that list.

- [ ] **Step 5: Write the failing test**

Add to the `#[cfg(test)]` module in `main.rs`. The module already constructs `OpenedFile` for command tests; mirror the existing setup helper. If a helper like `open_fixture(...)` or a direct `OpenedFile { .. }` builder exists in tests, reuse it; otherwise add this focused unit test of the helper logic that does not need a live file:

```rust
    #[test]
    fn truncate_window_defaults_and_count() {
        // A minimal OpenedFile-like check via the helper math: with no bounds
        // the window is the whole file; with bounds it narrows.
        // (If the test module has an `OpenedFile` builder, prefer it. This
        // asserts the pure (lo,hi) math the command relies on.)
        let line_count: u64 = 100;
        let lo = None::<u64>.unwrap_or(0);
        let hi = Some(40u64).unwrap_or(line_count);
        assert_eq!((lo, hi), (0, 40));
        let lo2 = Some(10u64).unwrap_or(0);
        let hi2 = None::<u64>.unwrap_or(line_count);
        assert_eq!((lo2, hi2), (10, 100));
    }
```

> Note: the substantive validation (rejecting `before >= after`) is covered by an integration-style test if the module has an `OpenedFile` builder. If it does, add instead:
>
> ```rust
>     #[test]
>     fn set_truncate_rejects_inverted_window() {
>         // build an OpenedFile with line_count = 100 via the test helper,
>         // insert into an AppState, then:
>         // assert!(set_truncate(state, id, Some(50), Some(20)).is_err());
>     }
> ```
>
> Use whichever matches the existing test harness in this module. Inspect the tests near line 2593 first.

- [ ] **Step 6: Run tests to verify they fail (compile error until the helpers exist), then pass**

Run: `cargo test --workspace -- truncate`
Expected: PASS after Steps 1-3 compile.

- [ ] **Step 7: Verify lints**

Run: `cargo fmt --check; cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add crates/clog-app/src/main.rs
git commit -m "Added a truncate window to OpenedFile and a set_truncate command. The window is a record-boundary-snapped physical-line range [before, after) held in backend state; the command validates before < after and returns the windowed line and record counts. Relates to #7"
```

---

## Task 2: Window the record-list, minimap and marker commands

**Files:**
- Modify: `crates/clog-app/src/main.rs` (`list_records_by_filters` line 1329, `get_level_minimap` line 1191, `get_markers` line 1293)
- Test: `main.rs` `#[cfg(test)]`

- [ ] **Step 1: Window `list_records_by_filters`**

In `list_records_by_filters` (line 1329), capture the window before the iterator and add a `first_line` membership test. Replace the body's filter/map chain so it reads:

```rust
    let lmask = LevelMask(u16::try_from(level_mask & 0xFFFF).unwrap_or(0xFFFF));
    let tmask = ThreadGroupMask(u8::try_from(thread_group_mask & 0xFF).unwrap_or(0x3F));
    let (lo, hi) = file.truncate_window();
    let bytes = &file.bytes;
    let refs = file
        .records
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            let f = u64::from(r.line_offset);
            if f < lo || f >= hi {
                return false;
            }
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
```

> `record_idx` stays absolute because `.enumerate()` runs before `.filter()`. This keeps the frontend's `record_idx -> record` lookup valid.

- [ ] **Step 2: Window `get_level_minimap`**

Replace the body of `get_level_minimap` (line 1191) so it builds window-relative record copies:

```rust
#[tauri::command]
fn get_level_minimap(
    state: State<'_, AppState>,
    file_id: u64,
    bucket_count: u32,
) -> Result<LevelMinimapPayload, IpcError> {
    let guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;
    let (lo, hi) = file.truncate_window();
    if file.truncate_before.is_none() && file.truncate_after.is_none() {
        return Ok(build_level_minimap_payload(
            &file.records,
            file.line_count,
            bucket_count as usize,
        ));
    }
    let span = hi.saturating_sub(lo);
    let windowed: Vec<RecordHeader> = file
        .records
        .iter()
        .filter(|r| {
            let f = u64::from(r.line_offset);
            f >= lo && f < hi
        })
        .map(|r| {
            let mut c = r.clone();
            c.line_offset = u32::try_from(u64::from(r.line_offset).saturating_sub(lo))
                .unwrap_or(u32::MAX);
            c
        })
        .collect();
    Ok(build_level_minimap_payload(&windowed, span, bucket_count as usize))
}
```

- [ ] **Step 3: Window `get_markers`**

Replace the body of `get_markers` (line 1293) so it drops out-of-window markers:

```rust
#[tauri::command]
fn get_markers(state: State<'_, AppState>, file_id: u64) -> Result<Vec<MarkerRef>, IpcError> {
    let guard = state.files.lock().expect("files mutex poisoned");
    let file = guard
        .get(&file_id)
        .ok_or(IpcError::UnknownFile { file_id })?;
    let (lo, hi) = file.truncate_window();
    let mut markers = scan_markers(
        &file.records,
        &file.bytes,
        &file.line_offsets,
        BUILTIN_MARKER_RULES,
    );
    markers.retain(|m| m.line_index >= lo && m.line_index < hi);
    Ok(markers)
}
```

- [ ] **Step 4: Write a failing test for minimap windowing**

`build_level_minimap_payload` is a free function callable from tests, and the `#[cfg(test)] mod tests` block uses `use super::*;`, so `RecordHeader`, `HeaderFields`, `Level` and `build_level_minimap_payload` are all already in scope (main.rs imports them at lines 19-22). `RecordHeader` has NO `Default`, so build an explicit literal (its six fields: `byte_offset: u64`, `byte_len: u32`, `line_offset: u32`, `line_count: u32`, `level: Level`, `fields: HeaderFields` - and `HeaderFields::default()` exists):

```rust
    #[test]
    fn windowed_minimap_relabels_buckets_to_span() {
        // An Error record originally at line 5; window [5,10) relabels it to
        // line 0 over span 5, so a 1-bucket grid must report Error.
        let wb = RecordHeader {
            byte_offset: 0,
            byte_len: 0,
            line_offset: 0,
            line_count: 1,
            level: Level::Error,
            fields: HeaderFields::default(),
        };
        let payload = build_level_minimap_payload(&[wb], 5, 1);
        assert_eq!(payload.buckets[0].worst, Level::Error);
        assert_eq!(payload.buckets[0].error, 1);
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test --workspace -- minimap`
Expected: PASS.

- [ ] **Step 6: Verify lints**

Run: `cargo fmt --check; cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/clog-app/src/main.rs
git commit -m "Windowed the record-list, level-minimap and marker commands against the active truncate window. list_records_by_filters now drops records outside the window (keeping absolute record indices), get_level_minimap buckets window-relative copies over the window span, and get_markers filters emitted markers by line. Relates to #7"
```

---

## Task 3: Window the slow-request commands

**Files:**
- Modify: `crates/clog-app/src/main.rs` (`get_slow_requests` line 978, `get_slow_request_speeds` line 1082)
- Test: `main.rs` `#[cfg(test)]`

- [ ] **Step 1: Window `get_slow_requests`**

Replace the occurrence-cloning tail of `get_slow_requests` (line 978) so it filters by line:

```rust
    let _ = rebuild_slow_request_cache(file);
    let (lo, hi) = file.truncate_window();
    let occs: Vec<clog_core::SlowRequestOccurrence> = file
        .slow_request_cache
        .as_ref()
        .expect("rebuild leaves cache populated")
        .occurrences
        .iter()
        .filter(|o| o.line_index >= lo && o.line_index < hi)
        .cloned()
        .collect();
    Ok(reaggregate_from_cache(&occs, mode))
```

- [ ] **Step 2: Window `get_slow_request_speeds`**

Replace the body of `get_slow_request_speeds` (line 1082) so it builds window-relative occurrences over the span:

```rust
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
    let (lo, hi) = file.truncate_window();
    let span = hi.saturating_sub(lo);
    let _ = rebuild_slow_request_cache(file);
    let occs: Vec<clog_core::SlowRequestOccurrence> = file
        .slow_request_cache
        .as_ref()
        .expect("rebuild leaves cache populated")
        .occurrences
        .iter()
        .filter(|o| o.line_index >= lo && o.line_index < hi)
        .map(|o| {
            let mut c = o.clone();
            c.line_index = o.line_index.saturating_sub(lo);
            c
        })
        .collect();
    Ok(clog_core::build_speed_grid(&occs, span, bucket_count as usize))
}
```

> `line_count` for the grid becomes `span` (window width), and occurrence line indices are shifted to window-relative so existing `build_speed_grid` buckets them correctly without a core change.

- [ ] **Step 3: Write a failing test**

`build_speed_grid` and `SlowRequestOccurrence` are public in `clog_core`. `SlowRequestOccurrence` has exactly these seven fields (slow_requests.rs:233): `timestamp_ms: Option<i64>`, `duration_ms: u32`, `line_index: u64`, `record_idx: u32`, `dup_count: u32`, `class_method: String`, `raw_path: String`. Add to the `main.rs` `#[cfg(test)]` module (`use super::*;` is in scope; `build_speed_grid`/`SlowRequestOccurrence` are reachable via `clog_core::`):

```rust
    #[test]
    fn windowed_speed_grid_buckets_relative_to_span() {
        use clog_core::{build_speed_grid, SlowRequestOccurrence};
        // One occurrence at absolute line 8, duration 500ms. With a window
        // [5, 10) (span 5) it relabels to line 3 -> bucket 3 of a 5-bucket grid.
        let occ = SlowRequestOccurrence {
            timestamp_ms: None,
            duration_ms: 500,
            line_index: 3, // already window-relative (8 - 5)
            record_idx: 0,
            dup_count: 1,
            class_method: "GET.index".to_string(),
            raw_path: "/x".to_string(),
        };
        let grid = build_speed_grid(&[occ], 5, 5);
        assert_eq!(grid.buckets[3].count, 1);
        assert_eq!(grid.buckets[3].max_ms, 500);
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace -- speed`
Expected: PASS.

- [ ] **Step 5: Verify lints**

Run: `cargo fmt --check; cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/clog-app/src/main.rs
git commit -m "Windowed the slow-request summary and speed-grid commands against the truncate window. get_slow_requests aggregates only in-window occurrences, and get_slow_request_speeds buckets window-relative occurrence copies over the window span so the speed rail reflects the kept region. Relates to #7"
```

---

## Task 4: Window search hits

**Files:**
- Modify: `crates/clog-app/src/main.rs` (`start_search` line 1434)

- [ ] **Step 1: Capture the window in the snapshot block**

In `start_search`, the snapshot block (lines 1486-1497) binds `(records_snapshot, bytes_snapshot, search_id, cancel_flag)`. Extend it to also capture the window:

```rust
    let (records_snapshot, bytes_snapshot, search_id, cancel_flag, win_lo, win_hi) = {
        let mut guard = state.files.lock().expect("files mutex poisoned");
        let file = guard
            .get_mut(&file_id)
            .ok_or(IpcError::UnknownFile { file_id })?;
        file.cancel_search();
        file.current_search_id += 1;
        let id = file.current_search_id;
        let flag = Arc::new(AtomicBool::new(false));
        file.search_cancel = Some(flag.clone());
        let (lo, hi) = file.truncate_window();
        (file.records.clone(), file.bytes.clone(), id, flag, lo, hi)
    };
```

- [ ] **Step 2: Filter hits by window before streaming**

In the spawned task, after `let hits = match result { ... };` (line 1523-1529) and before `stream_hits(...)` (line 1533), insert:

```rust
        let hits: Vec<HitRef> = hits
            .into_iter()
            .filter(|h| h.record_first_line >= win_lo && h.record_first_line < win_hi)
            .collect();
```

> Filtering before `stream_hits` means the emitter's cumulative `total` (the count badge) reflects only in-window hits. `record_idx` on each surviving `HitRef` is untouched, so navigation stays correct.

- [ ] **Step 3: Verify build and lints**

Run: `cargo test --workspace; cargo fmt --check; cargo clippy --workspace --all-targets -- -D warnings`
Expected: all existing tests pass, lints clean.

- [ ] **Step 4: Commit**

```bash
git add crates/clog-app/src/main.rs
git commit -m "Windowed search hits against the truncate window. start_search drops hits whose record sits outside the window before streaming, so the hit count and next/previous navigation only see the kept region while record indices stay absolute. Relates to #7"
```

---

## Task 5: Persist the window in `RestoredFile` (Rust)

**Files:**
- Modify: `crates/clog-app/src/persistence.rs` (`RestoredFile` line 193)
- Test: `persistence.rs` `#[cfg(test)]` (round-trip tests near line 510)

- [ ] **Step 1: Add the fields**

In `RestoredFile` (line 193), after `manually_collapsed` (line 227), add:

```rust
    /// First visible physical line (inclusive) of the truncate window, or
    /// `None` for no "above" cut. Out-of-range values are dropped UI-side.
    #[serde(default)]
    pub truncate_before: Option<u64>,
    /// One past the last visible physical line of the truncate window, or
    /// `None` for no "below" cut.
    #[serde(default)]
    pub truncate_after: Option<u64>,
```

- [ ] **Step 2: Update the round-trip test**

Find the round-trip test (near line 526-539 it builds a `RestoredFile` literal). Add the two fields to that literal:

```rust
            truncate_before: Some(12),
            truncate_after: Some(900),
```

And add assertions after the existing ones:

```rust
        assert_eq!(back.truncate_before, Some(12));
        assert_eq!(back.truncate_after, Some(900));
```

Also update the default-load test (near line 511-515, raw JSON without the fields) by adding:

```rust
        assert_eq!(r.truncate_before, None);
        assert_eq!(r.truncate_after, None);
```

- [ ] **Step 3: Run tests**

Run: `cargo test --workspace -- persistence`
Expected: PASS (the raw-JSON test confirms `#[serde(default)]` keeps old sessions loadable).

- [ ] **Step 4: Verify lints**

Run: `cargo fmt --check; cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/clog-app/src/persistence.rs
git commit -m "Added truncate_before and truncate_after to the persisted RestoredFile. Both are Option<u64> with serde(default) so existing session files load unchanged. Relates to #7"
```

---

## Task 6: Frontend types

**Files:**
- Modify: `ui/src/types.ts` (`RestoredFile` line 174)

- [ ] **Step 1: Extend `RestoredFile`**

After `manually_collapsed?: number[]` (line 190), add:

```ts
  /** First visible physical line of the truncate window. Absent/null = no cut. */
  truncate_before?: number | null
  /** One past the last visible physical line of the truncate window. */
  truncate_after?: number | null
```

- [ ] **Step 2: Add the command payload type**

Add near the other payload interfaces (search the file for `RecordRefsPayload` or `LinesPayload` and add beside them):

```ts
export interface SetTruncatePayload {
  before: number | null
  after: number | null
  line_count: number
  record_count: number
}
```

- [ ] **Step 3: Verify the UI typechecks**

Run: `npm --prefix ui run build`
Expected: build succeeds (vue-tsc clean).

- [ ] **Step 4: Commit**

```bash
git add ui/src/types.ts
git commit -m "Added truncate fields to the RestoredFile wire type and a SetTruncatePayload type for the set_truncate command. Relates to #7"
```

---

## Task 7: Frontend tab state and behaviour

**Files:**
- Modify: `ui/src/tab.ts`
- Test: `ui/src/truncate.test.ts` (create)

- [ ] **Step 1: Import the payload type**

In `ui/src/tab.ts`, add `SetTruncatePayload` to the `import { ... } from './types'` block (around line 27-49):

```ts
  type SetTruncatePayload,
```

- [ ] **Step 2: Add the refs**

After the `recordIndex` ref block (line 232), add:

```ts
  // --- Truncate window (physical line bounds; record-boundary snapped) ---
  // Persisted per file. null = no cut on that side.
  const truncateBefore = ref<number | null>(null)
  const truncateAfter = ref<number | null>(null)
```

- [ ] **Step 3: Add `setTruncate` and a local clear**

Add near `refreshRecordIndex` (after line 310):

```ts
  // Push a new truncate window to the backend, then refresh the windowed
  // views. Setting an "after" cut disengages follow-tail (lines below the cut
  // are not visible, so following them is meaningless). Non-fatal on error.
  async function setTruncate(before: number | null, after: number | null): Promise<void> {
    try {
      const payload = await invoke<SetTruncatePayload>('set_truncate', {
        fileId: file.value.file_id,
        before,
        after,
      })
      truncateBefore.value = payload.before
      truncateAfter.value = payload.after
      if (after !== null) followTail.value = false
      await refreshRecordIndex()
      if (!isFullLevelMask(levelAllow.value) || !isFullThreadGroupMask(threadGroupAllow.value)) {
        void refreshAllowedRecords()
      }
      if (searchQuery.value.trim().length > 0) scheduleSearch()
    } catch (e) {
      const err = e as IpcError | string
      hooks.onError?.(typeof err === 'string' ? err : err.message)
    }
  }

  // Clear both cuts locally and on the backend, without the refresh cascade
  // (callers that already re-index, e.g. rotation/pattern-apply, use this).
  function resetTruncateState() {
    truncateBefore.value = null
    truncateAfter.value = null
    void invoke('set_truncate', {
      fileId: file.value.file_id,
      before: null,
      after: null,
    }).catch(() => {})
  }
```

- [ ] **Step 4: Clear the window on rotation**

In `handleTailDelta`, inside the `if (delta.rotated)` branch (line 409-432), add alongside `clearBookmarks()` / `clearCollapseOverrides()` (line 422-423):

```ts
      resetTruncateState()
```

- [ ] **Step 5: Clear the window on pattern apply**

In `applyPattern`, in the success path after `void refreshRecordIndex()` (line 637), add:

```ts
      resetTruncateState()
```

- [ ] **Step 6: Persist in `snapshot()`**

In `snapshot()` (line 681), add to the returned object after `manually_collapsed: prunedManualSet(manuallyCollapsed.value),` (line 695):

```ts
      truncate_before:
        truncateBefore.value !== null && truncateBefore.value < file.value.line_count
          ? truncateBefore.value
          : null,
      truncate_after:
        truncateAfter.value !== null && truncateAfter.value <= file.value.line_count
          ? truncateAfter.value
          : null,
```

- [ ] **Step 7: Restore in `applyRestored()`**

In `applyRestored()` (line 646), after the `transientlyExpanded.value = new Set()` line (line 678), add:

```ts
    const tb = r.truncate_before
    const ta = r.truncate_after
    truncateBefore.value =
      typeof tb === 'number' && tb >= 0 && tb < limit ? tb : null
    truncateAfter.value =
      typeof ta === 'number' && ta > 0 && ta <= limit ? ta : null
    if (truncateBefore.value !== null || truncateAfter.value !== null) {
      void invoke('set_truncate', {
        fileId: file.value.file_id,
        before: truncateBefore.value,
        after: truncateAfter.value,
      })
        .then(() => refreshRecordIndex())
        .catch(() => {})
    }
```

> `limit` is already declared in `applyRestored` (line 666: `const limit = file.value.line_count`). Reuse it; do not redeclare.

- [ ] **Step 8: Export on the api object**

In the `const api = { ... }` block (line 720), add to the state group (near `recordIndex,` line 754):

```ts
    truncateBefore,
    truncateAfter,
```

and to the methods group (near `refreshRecordIndex,` line 784):

```ts
    setTruncate,
    resetTruncateState,
```

- [ ] **Step 9: Write the failing test (pure snapshot/restore pruning)**

Create `ui/src/truncate.test.ts`:

```ts
import { describe, it, expect } from 'vitest'

// Mirror of the snapshot/restore pruning rules in tab.ts so the boundary
// behaviour is locked without standing up a full Tab + Tauri invoke mock.
function pruneBefore(value: number | null, lineCount: number): number | null {
  return value !== null && value >= 0 && value < lineCount ? value : null
}
function pruneAfter(value: number | null, lineCount: number): number | null {
  return value !== null && value > 0 && value <= lineCount ? value : null
}

describe('truncate snapshot/restore pruning', () => {
  it('keeps in-range bounds', () => {
    expect(pruneBefore(10, 100)).toBe(10)
    expect(pruneAfter(90, 100)).toBe(90)
  })
  it('drops a before-cut at or past line_count', () => {
    expect(pruneBefore(100, 100)).toBeNull()
    expect(pruneBefore(150, 100)).toBeNull()
  })
  it('keeps an after-cut exactly at line_count', () => {
    expect(pruneAfter(100, 100)).toBe(100)
  })
  it('drops an after-cut past line_count', () => {
    expect(pruneAfter(101, 100)).toBeNull()
  })
  it('drops null', () => {
    expect(pruneBefore(null, 100)).toBeNull()
    expect(pruneAfter(null, 100)).toBeNull()
  })
})
```

- [ ] **Step 10: Run the test**

Run: `npm --prefix ui run test -- truncate`
Expected: PASS.

- [ ] **Step 11: Verify build**

Run: `npm --prefix ui run build`
Expected: clean.

- [ ] **Step 12: Commit**

```bash
git add ui/src/tab.ts ui/src/truncate.test.ts
git commit -m "Added truncate window state and behaviour to the tab. setTruncate pushes the window to the backend, disengages follow-tail on an after-cut and refreshes the windowed views; the window is cleared on rotation and pattern-apply and round-trips through snapshot/restore with out-of-range pruning. Relates to #7"
```

---

## Task 8: Session autosave fingerprint

**Files:**
- Modify: `ui/src/composables/useSession.ts` (fingerprint watch, line 82-83)

- [ ] **Step 1: Add truncate to the fingerprint**

In the `watch(() => tabs.value.map((t) => \`...\`)` template string (line 83), append before the closing backtick of the per-tab template (right after `mc:${t.manuallyCollapsed.value.size}`):

```ts
|tr:${t.truncateBefore.value}:${t.truncateAfter.value}
```

So the per-tab segment ends `...|mc:${t.manuallyCollapsed.value.size}|tr:${t.truncateBefore.value}:${t.truncateAfter.value}`.

- [ ] **Step 2: Verify build**

Run: `npm --prefix ui run build`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add ui/src/composables/useSession.ts
git commit -m "Extended the session autosave fingerprint with the truncate window so setting or lifting a cut schedules a debounced save. Relates to #7"
```

---

## Task 9: Viewport projection, context menu, banners

**Files:**
- Modify: `ui/src/components/LogViewport.vue`
- Test: `ui/src/truncate.test.ts` (extend)

- [ ] **Step 1: Add a window-active flag and gate the projection fast-path**

In `LogViewport.vue`, near `collapseHidesSomething` (line 190) add:

```ts
const hasTruncate = computed<boolean>(
  () => props.tab.truncateBefore.value !== null || props.tab.truncateAfter.value !== null,
)
```

In `projectionRecords` (line 204-211), change the identity-fast-path guard so a window forces a non-identity projection. Replace:

```ts
  if (!collapseHidesSomething.value) return null
  return props.tab.recordIndex.value
```

with:

```ts
  if (!collapseHidesSomething.value && !hasTruncate.value) return null
  return props.tab.recordIndex.value
```

> `recordIndex` is already windowed by the backend (Task 2), so returning it yields exactly the in-window rows and `effectiveCount` tracks the window.

- [ ] **Step 2: Refetch the minimap when the window changes**

Find the existing `watch(filteredSourceRecords, ...)` (line 1471). Add a sibling watch right after it:

```ts
watch(
  () => [props.tab.truncateBefore.value, props.tab.truncateAfter.value],
  () => {
    lastMinimapLineCount = -1
    lastMarkerLineCount = -1
    scheduleMinimapFetch(true)
  },
)
```

- [ ] **Step 3: Add hidden-line count computeds and banner handlers**

Near the other computeds (after `hasTruncate`), add:

```ts
const hiddenBefore = computed<number>(() => props.tab.truncateBefore.value ?? 0)
const hiddenAfter = computed<number>(() => {
  const after = props.tab.truncateAfter.value
  return after === null ? 0 : Math.max(0, props.tab.file.value.line_count - after)
})

function liftTruncateBefore() {
  void props.tab.setTruncate(null, props.tab.truncateAfter.value)
}
function liftTruncateAfter() {
  void props.tab.setTruncate(props.tab.truncateBefore.value, null)
}
```

- [ ] **Step 4: Add the context-menu items**

In `onRowContextMenu` (line 1714), after the "Show full record" item is pushed (line 1738) and before the `universal` block (line 1739), insert:

```ts
  const rec = props.tab.recordIndex.value.find((r) => r.record_idx === row.record_idx)
  if (rec) {
    const before = rec.record_first_line
    const after = rec.record_first_line + rec.record_line_count
    const tb = props.tab.truncateBefore.value
    const ta = props.tab.truncateAfter.value
    const truncItems: MenuItem[] = []
    if (tb === null) {
      truncItems.push({
        kind: 'action',
        label: 'Truncate before',
        disabled: ta !== null && before >= ta,
        onSelect: () => { void props.tab.setTruncate(before, ta) },
      })
    }
    if (ta === null) {
      truncItems.push({
        kind: 'action',
        label: 'Truncate after',
        disabled: tb !== null && after <= tb,
        onSelect: () => { void props.tab.setTruncate(tb, after) },
      })
    }
    if (truncItems.length > 0) {
      items.push({ kind: 'separator' }, ...truncItems)
    }
  }
```

> `row.record_idx` is the absolute record index from `get_lines`; `recordIndex` entries carry the same absolute `record_idx` (Task 2 keeps it absolute), so the lookup is valid even while windowed. The lookup is O(n) but only runs on right-click.

- [ ] **Step 5: Render the banners**

In the template, inside the `<div class="total" ...>` element (opens line 1813, closes line 1880), add as the first and last children (just inside the open tag, and just before the close tag):

```html
        <button
          v-if="tab.truncateBefore.value !== null"
          type="button"
          class="truncate-banner truncate-banner-top"
          :title="`Show the ${hiddenBefore} hidden lines above`"
          @click="liftTruncateBefore"
        >+{{ hiddenBefore }} lines before</button>
```

and before `</div>` at line 1880:

```html
        <button
          v-if="tab.truncateAfter.value !== null"
          type="button"
          class="truncate-banner truncate-banner-bottom"
          :title="`Show the ${hiddenAfter} hidden lines below`"
          @click="liftTruncateAfter"
        >+{{ hiddenAfter }} lines after</button>
```

- [ ] **Step 6: Style the banners**

In the component's `<style>` block (the `.total` rule lives there; search for `.total`), add. If `.total` has no `position`, add `position: relative;` to it. Then add:

```css
.truncate-banner {
  position: absolute;
  left: 0;
  right: 0;
  z-index: 3;
  height: 22px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 0.72rem;
  letter-spacing: 0.04em;
  color: var(--fg-muted);
  background: color-mix(in srgb, var(--bg-viewport) 86%, transparent);
  border: 0;
  cursor: pointer;
}
.truncate-banner:hover {
  color: var(--fg-default);
}
.truncate-banner-top {
  top: 0;
  border-bottom: 1px dashed var(--border-default);
}
.truncate-banner-bottom {
  bottom: 0;
  border-top: 1px dashed var(--border-default);
}
```

> These tokens are confirmed present in `ui/src/style.css` (`--bg-viewport`, `--fg-muted`, `--fg-default`, `--border-default`) for both dark (`:root`) and light (`:root[data-theme="light"]`) themes. Do not hardcode colours (cerebrum CSS rule).

- [ ] **Step 7: Extend the vitest with context-menu enable/disable logic**

Append to `ui/src/truncate.test.ts`:

```ts
// Mirror of the context-menu enable/disable rules in onRowContextMenu.
function truncateMenu(
  recBefore: number,
  recAfter: number,
  tb: number | null,
  ta: number | null,
) {
  const out: Array<{ label: string; disabled: boolean }> = []
  if (tb === null) out.push({ label: 'Truncate before', disabled: ta !== null && recBefore >= ta })
  if (ta === null) out.push({ label: 'Truncate after', disabled: tb !== null && recAfter <= tb })
  return out
}

describe('truncate context menu', () => {
  it('offers both when no cuts exist', () => {
    const m = truncateMenu(10, 14, null, null)
    expect(m.map((i) => i.label)).toEqual(['Truncate before', 'Truncate after'])
    expect(m.every((i) => !i.disabled)).toBe(true)
  })
  it('hides before when a before-cut exists', () => {
    const m = truncateMenu(10, 14, 5, null)
    expect(m.map((i) => i.label)).toEqual(['Truncate after'])
  })
  it('disables a before-cut that would invert the window', () => {
    const m = truncateMenu(50, 54, null, 40)
    expect(m[0]).toEqual({ label: 'Truncate before', disabled: true })
  })
  it('disables an after-cut that would invert the window', () => {
    const m = truncateMenu(10, 14, 20, null)
    expect(m[0]).toEqual({ label: 'Truncate after', disabled: true })
  })
})
```

- [ ] **Step 8: Run the tests**

Run: `npm --prefix ui run test -- truncate`
Expected: PASS.

- [ ] **Step 9: Verify build**

Run: `npm --prefix ui run build`
Expected: clean.

- [ ] **Step 10: Commit**

```bash
git add ui/src/components/LogViewport.vue ui/src/truncate.test.ts
git commit -m "Wired truncate into the viewport. The projection drops its identity fast-path when a window is active so the virtualiser tracks the kept region, the row context menu offers Truncate before/after (one cut per side, snapped to the clicked record and disabled when a cut would invert the window), and dashed +N lines before/after banners at the head and tail lift their cut on click. Relates to #7"
```

---

## Task 10: Status bar indicator (minor) and manual verification

**Files:**
- Modify: `ui/src/components/StatusBar.vue`

`StatusBar.vue` declares `defineProps<{ tab: Tab | null }>()` and references the prop directly as `tab` in its template. The lines stat is at line 59: `<span class="stat">{{ formatCount(tab.file.value.line_count) }} lines</span>`. The `.muted` class is a global helper.

- [ ] **Step 1: Add a truncation hint after the lines stat**

Immediately after the lines `<span>` (line 59), add:

```html
        <span
          v-if="tab.truncateBefore.value !== null || tab.truncateAfter.value !== null"
          class="stat muted"
        >(truncated)</span>
```

> The surrounding block is already guarded by a `v-if` on `tab` being non-null, so `tab.truncateBefore` is safe here. Keep it minimal - the per-banner counts already show the hidden totals; this is just a legibility marker. Do not hardcode colour (use the existing `.muted` helper).

- [ ] **Step 2: Verify build**

Run: `npm --prefix ui run build`
Expected: clean.

- [ ] **Step 4: Manual verification (cargo dev)**

Run: `cargo dev`

Verify against `research/cheesecake-prod.log`:
1. Right-click a line mid-file -> *Truncate before* and *Truncate after* both appear.
2. *Truncate after* -> lines below vanish; follow-tail disengages; a dashed "+N lines after" banner shows at the tail; clicking it restores them.
3. *Truncate before* -> lines above vanish; "+N lines before" banner at the head; minimap and the slow-request insights drawer recompute over the window (open the drawer and confirm counts/P95 drop).
4. With both cuts, the row context menu no longer offers either truncate item.
5. Search within a window -> hit count reflects only in-window hits.
6. Close and reopen the file (or restart) -> the window is restored from the session.

- [ ] **Step 5: Commit**

```bash
git add ui/src/components/StatusBar.vue
git commit -m "Added a (truncated) hint to the status bar when a truncate window is active. Relates to #7"
```

---

## Task 11: Final gates, docs and issue close-out

- [ ] **Step 1: Run every CI-equivalent gate**

Run:
```
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm --prefix ui run build
npm --prefix ui run test
```
Expected: all green.

- [ ] **Step 2: Update future-ideas / docs if referenced**

If `docs/future-ideas.md` lists "truncate / collapse above-below" as a candidate, move it to done or remove the entry. Inspect first; only edit if present.

- [ ] **Step 3: Append a final memory entry**

Append a one-line summary to `.wolf/memory.md` describing the shipped feature and the key decision (authoritative backend window; absolute numbering; get_lines and thresholds deliberately not windowed).

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "Logged the truncate-logs feature in project memory and updated the future-ideas list. Closes #7"
```

---

## Self-review notes (for the implementer)

- **Absolute indices everywhere.** `record_idx` from `list_records_by_filters` and `record_first_line` from search stay absolute. The window only changes *which* records/hits are returned, never their numbering. Do not renumber.
- **`get_lines` and `get_slow_request_thresholds` are intentionally NOT windowed.** `get_lines` is absolute-addressed line fetch (the projection never requests out-of-window lines, and clamping would desync the page-array offset). `get_slow_request_thresholds` returns config, not data.
- **Struct shapes are pinned (verified).** `RecordHeader` (no `Default`; six fields: `byte_offset`, `byte_len`, `line_offset`, `line_count`, `level`, `fields`) and `SlowRequestOccurrence` (seven fields: `timestamp_ms`, `duration_ms`, `line_index`, `record_idx`, `dup_count`, `class_method`, `raw_path`) are reproduced exactly in the Task 2/3 test literals. `main.rs` already imports `RecordHeader`/`HeaderFields`/`Level` (lines 19-22), and the `#[cfg(test)] mod tests` uses `use super::*;`, so the literals need no extra imports.
- **`recordIndex` is windowed after Task 2.** The viewport projection, collapse resolver, chevrons and context-menu record lookup all operate on visible (in-window) lines, so the windowed map is sufficient. The full-file fact retained is `file.line_count`, used only for the banner counts.
- **Token names are pinned (verified).** `--bg-viewport`, `--fg-muted`, `--fg-default`, `--border-default` all exist in `ui/src/style.css` for both themes; the banner CSS uses only these.
- **StatusBar is pinned (verified).** `defineProps<{ tab: Tab | null }>()`, template references `tab` directly, lines stat at line 59; the hint slots in right after it.
