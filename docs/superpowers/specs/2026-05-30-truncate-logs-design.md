# Truncate logs (collapse above/below) - design

Issue: [#7 Collapse above/below](https://github.com/Enigma-Interactive-UK/clog/issues/7)

## Problem

When trawling logs from a particular point, the lines above or below the area
of interest are noise. The user wants to selectively hide everything above or
below a chosen line so the remaining region is easier to follow.

This is distinct from the existing collapse/expand (record-folding) mechanism.
Truncate is a **view onto the file**: the lines outside the kept region cease to
exist for every purpose - the minimap, slow-request insights, search, markers -
*other than as a count* of how many lines were hidden. It is not a per-record
fold; it is a file-global clip.

## Mental model

A truncate window is a contiguous physical-line range that the whole app treats
as the entire file. Everything derived from the file - aggregates, search,
record map - is computed as if the hidden lines were never there. The only
full-file fact retained is the total line count, used solely to render the
"+N lines before/after" affordance.

## Data model

The window is authoritative state on the backend `OpenedFile`:

- `truncate_before: Option<u64>` - first visible physical line, inclusive. When
  set, lines `[0, before)` cease to exist. Set by *Truncate before*.
- `truncate_after: Option<u64>` - one-past-last visible physical line,
  exclusive. When set, lines `[after, end)` cease to exist. Set by
  *Truncate after*; also disengages follow-tail.

At most one of each. Both set produces a window `[before, after)`. A "below"
truncate plus a "before" truncate yields a small window in the middle of the
file.

### Snapping to record boundaries

Truncate operates on the *record* containing the clicked line, never on a raw
physical line, so a cut never splits a multi-line record (e.g. a stack trace):

- *Truncate before* on a line in record R -> `before = R.first_line`. R and
  everything after it stay visible; records before R are hidden.
- *Truncate after* on a line in record R -> `after = R.first_line +
  R.line_count`. R and everything before it stay visible; records after R are
  hidden.

Both bounds therefore land on record boundaries, so the visible record set is
exactly those records whose `first_line` falls in `[before, after)`.

### Numbering stays absolute

Physical line N is always line N. The window hides ranges; it does not renumber.
Bookmarks, markers and line references stay valid across truncate/untruncate; a
hidden bookmark simply reappears when its region is restored.

## Backend

### One new command

`set_truncate(file_id, before: Option<u64>, after: Option<u64>)`:

- Validates `before < after` when both are present; rejects an empty or
  inverted window.
- Stores the bounds on `OpenedFile`.
- Returns the windowed line count and record count (for the status bar).
- `(None, None)` clears the window.

### Every aggregate / search / record-list command honours the window

Each of these already snapshots its `Vec<RecordHeader>` under the lock. A shared
helper slices that vec to the records whose `first_line` is in
`[before, after)` (a `partition_point` pair) before the command computes:

- `list_records_by_filters` (`main.rs:1328`) - filter records by `first_line`.
- `get_level_minimap` (`main.rs:1191`) - window-relative record copies + span.
- `get_markers` (`main.rs:1293`) - filter emitted markers by `line_index`.
- `get_slow_requests` (`main.rs:978`) - filter occurrences by `line_index`.
- `get_slow_request_speeds` (`main.rs:1082`) - window-relative occurrence
  copies + span.
- `start_search` (`main.rs:1434`) - filter hits by `record_first_line` in the
  emit path (absolute indices preserved).

After this, insights P95/avg/counts, the speed rail, the minimap, search hits
and the record map all compute as if the hidden lines were never in the file.

`get_slow_request_thresholds` (`main.rs:1114`) is **not** windowed: it returns
configuration (per-file / global overrides, or fixed auto constants), not data
derived from the records, so the window does not affect it.

### `get_lines` is deliberately NOT windowed

`get_lines` (`main.rs:570`) is pure absolute-addressed line-content fetch. The
projection layer never maps a virtual row to an out-of-window line, so those
lines are never requested. Clamping `get_lines` would desync the page-array
offset (a clamped page would start at a different line than its page index
implies), so it stays unchanged. The window is enforced at the projection layer
and in the aggregate/search commands only.

## Frontend

### Tab state (`tab.ts`)

- `truncateBefore: Ref<number|null>`, `truncateAfter: Ref<number|null>`.
- `setTruncate(before, after)`: calls `set_truncate`, stores the bounds, and
  when `after` becomes non-null sets `followTail.value = false`. On success it
  refreshes the windowed views (`refreshRecordIndex`, re-run search when a
  query is active, trigger minimap/insights refetch).

### Viewport projection (`LogViewport.vue`)

Because the backend now returns windowed data, `recordIndex`, `allowedRecords`,
`hits` and markers arrive already clipped - the frontend does not re-filter.

The one change: `projectionRecords` (`LogViewport.vue:204`) must drop the
identity fast-path when a window is active and return the windowed `recordIndex`
instead of `null`, so `effectiveCount` / `filteredLineIndices` reflect the
window rather than the full `file.line_count`. Everything downstream (virtualizer
count, scrollbar, minimap source, follow-tail bottom) then tracks the window.

### Full count retained for the affordance

`file.line_count` stays the true full count (tail keeps updating it). Hidden
counts are then: before = `truncateBefore`; after = `file.line_count -
truncateAfter`.

### Context menu (log-row menu)

Two items beside the existing *Search* item:

- *Truncate before* / *Truncate after*. Each is shown only when its bound is
  unset (max one of each), and disabled when the cut would produce an
  empty/inverted window against the existing opposite cut. The handler resolves
  the clicked line's record via `recordOfLine`, computes the snapped bound, and
  calls `tab.setTruncate`.

### Dashed banners

A thin dashed bar at the **head** of the log ("+N lines before") and/or the
**tail** ("+N lines after"), each clickable to lift that cut (`setTruncate` with
that bound -> `null`). They sit at the top/bottom edges of the scroll content as
absolutely-positioned overlays so they never perturb the virtual-row index math.

### Status bar (minor)

When a window is active, show "X of Y lines" so the truncation is legible.

## Interactions

- **Tail & follow.** Setting an *after* cut disengages follow-tail; while it is
  active the follow toggle is disabled (lifting the cut re-enables it). The
  backend keeps indexing past the cut, but those records are outside the window,
  so only the "+N lines after" count climbs.
- **Rotation.** Both cuts are cleared on a rotation delta
  (`setTruncate(null, null)`), alongside the existing `clearBookmarks()` /
  `clearCollapseOverrides()`, because line numbers stop meaning what they meant.
- **Pattern apply.** Re-parsing moves record boundaries, so the snapped cuts are
  stale and cleared on `applyPattern`, mirroring `clearCollapseOverrides`.
- **Filter / search composition.** The window is applied first (backend slices
  the record list before masking), so a level/thread filter or search narrows
  *within* the window automatically.
- **Bookmarks & markers.** Out-of-window ones are hidden, not deleted, and
  reappear when the region is restored. No pruning.

## Persistence

Truncate points persist across sessions, like bookmarks and collapse state:

- `truncate_before` / `truncate_after` join `snapshot()` / `applyRestored()` in
  `tab.ts`, the frontend `RestoredFile` type, and the Rust `RestoredFile` struct
  (`persistence.rs:193`) as `Option<u64>` with `#[serde(default)]` - additive,
  no schema bump, matching the bookmarks/collapse precedent.
- On restore they are pruned against the current `line_count` (dropped if out of
  range) and pushed to the backend via `set_truncate`.

## Testing

- **Rust**: `set_truncate` validation (rejects `before >= after`); each windowed
  command computes over the slice (slow-request summary, speed grid, search
  hits, `list_records_by_filters`, minimap) against the prod fixture with a
  window applied; record-boundary snapping.
- **Vitest**: `projectionRecords` leaves the identity fast-path when a window is
  active; `effectiveCount` equals the window line count; hidden-line counts;
  context-menu enable/disable resolver; persistence round-trip.
- **Playwright**: deferred, consistent with prior phases.

## Out of scope

- Multiple windows / more than one cut per side.
- Renumbering visible lines to a 1-based window-local sequence.
- A drag-to-select region gesture (truncate is driven from the context menu).
