# Thread insights + consolidated filter flyout - design

> Drafted 2026-05-24. Adds a thread-group filter axis alongside the
> existing level mask, and collapses the inline level pills into a
> single "Filters" button that opens a popover hosting both axes.

## Motivation

Today the only record-narrowing axis the UI exposes (aside from search)
is the log level mask: six pill buttons in `SearchBar.vue` that toggle
`TRACE/DEBUG/INFO/WARN/ERROR/FATAL` records on and off, intersected
with the search hit set inside the backend.

Real Play 1.x debugging sessions ask a second question almost as often:
**"was the request layer involved, or was this a background job /
scheduled task / framework plumbing?"** That question is answerable
from the thread name the parser already captures, but the UI does not
expose it. Adding a thread-group axis turns "scroll past 5000 lines of
`play-thread-*` chatter to find the one `jobs-thread-3` error" into a
two-click filter.

While adding the axis, the right thing to do for the search bar is
collapse the level pills (and the new thread pills) behind a single
"Filters" button. Two reasons: (1) the bar is already crowded once
search input, hit counters, case-sensitivity, filter-mode and six
level pills are laid out, and adding five more pills tips it over; (2)
both axes are the same kind of control - "show only records whose X is
in this set" - so they belong in the same UI affordance.

## Out of scope

- **Custom user-defined thread groups.** v1 ships a fixed taxonomy of
  five named groups + Other. User-defined regex groups goes on
  `docs/future-ideas.md`.
- **Thread-group counts shown live per-group in the flyout.** Could be
  added cheaply later (one IPC), but v1 just shows the toggle state.
- **Per-thread filtering** (filtering down to a single `play-thread-7`).
  Group granularity only for v1.
- **Search bar restructure beyond extracting the level mask.** The
  search mode toggle, search input, case toggle, hit counter and
  filter-mode toggle all stay where they are.

## Thread groups (locked for v1)

Classification happens against the thread byte slice exposed in
`RecordHeader.fields.thread`. Rules are tried in order; first match
wins. Matches are anchored unless noted.

| # | Group key | Display | Pattern | Catches |
|---|-----------|---------|---------|---------|
| 1 | `requests` | Requests   | `^play-thread-\d+$` | Play HTTP request workers (dominant volume) |
| 2 | `jobs`     | Jobs       | `^jobs-thread-\d+$` | Play `@On` / `@Every` / `Job` async workers |
| 3 | `scheduler`| Scheduler  | `(?i)quartz` (unanchored, case-insensitive) | `DefaultQuartzScheduler_Worker-N` and any custom Quartz pool naming |
| 4 | `system`   | System     | `^main$` \| `^Thread-\d+$` | JVM lifecycle: startup, shutdown hooks, anonymous `new Thread()` |
| 5 | `infra`    | Infra      | `^pool-\d+-thread-\d+$` \| `^New I/O (worker\|boss) #\d+$` \| `^I/O dispatcher \d+$` \| `^jgroups-` \| `^Memcached IO ` | Framework plumbing: Netty, JGroups, HttpAsyncClient, SpyMemcached, generic `Executors.newXxxPool` |
| 6 | `other`    | Other      | (fallthrough)        | Anything not matched above. Visible by default so nothing disappears silently |

**Default mask:** all six groups on (full mask). A record with no
parsed thread (continuation lines fold into the parent record, so this
only happens for header lines the pattern matched without a `[%t]`
token, e.g. the `prod-no-thread` builtin) classifies as `other`.

Rationale for the group set is captured in the brainstorming
transcript and `.wolf/cerebrum.md` once locked.

## Architecture

### Engine (`clog-core`)

A new module `thread_groups.rs` owns:

- `enum ThreadGroup { Requests, Jobs, Scheduler, System, Infra, Other }`
  with stable `u8` bit values (`Requests = 1`, `Jobs = 2`, ...,
  `Other = 32`) suitable for masking. Full mask = `0x3F`.
- `fn classify(thread: &[u8]) -> ThreadGroup` - hand-written byte
  matchers (no regex crate dependency in the hot path) ordered by the
  table above. The patterns are simple enough that hand-rolled prefix /
  suffix / digit-tail checks are clearer and faster than a regex.
- `fn group_mask_full() -> u8` constant.

The existing `LineIndex` already stores per-record `RecordHeader` with
the thread byte range. A new sibling to `list_records_by_level` is
needed:

```rust
// In clog-core::search (or a new module if it fits better)
pub fn list_records_by_filters(
    index: &LineIndex,
    headers: &[RecordHeader],
    raw: &[u8],
    level_mask: u8,
    thread_group_mask: u8,
) -> Vec<RecordRef>;
```

Each record contributes iff `(record.level_bit & level_mask) != 0
&& (classify(record.thread_bytes) & thread_group_mask) != 0`.

`list_records_by_level` is removed - the only caller (the UI) cuts
over to `list_records_by_filters` in the same PR. There is no
intermediate wrapper phase.

Search (the streaming `start_search` IPC) currently takes a
`level_mask` in its `SearchRequest`. It gains a sibling
`thread_group_mask: u8`. The intersection inside the search worker
walks the same predicate as above before yielding hits.

### Tauri commands (`clog-app::main`)

- `list_records_by_level` -> `list_records_by_filters { file_id,
  level_mask: u8, thread_group_mask: u8 }`. Same payload shape
  (`RecordRefsPayload`). Old command removed; one call site in the UI
  changes.
- `start_search` `SearchRequest` payload grows a `thread_group_mask:
  u8` field. Default in deserialisation = full mask, so older session
  payloads still deserialise.

### Persistence

`SessionFile` (per-tab snapshot in `session.json`) grows one field:

```rust
pub thread_group_mask: u8,  // default: 0x3F
```

Deserialisation uses `#[serde(default = "default_thread_group_mask")]`
returning `0x3F`, so existing session files keep loading unchanged.

### Frontend (`ui/src`)

**Tab state (`tab.ts`):**

- New constant `THREAD_GROUP_KEYS = ['requests','jobs','scheduler','system','infra','other'] as const` and `THREAD_GROUP_BIT: Record<ThreadGroupKey, number>`.
- New ref `threadGroupAllow = ref<Record<ThreadGroupKey, boolean>>(defaultThreadGroupAllow())`.
- New helpers `buildThreadGroupMaskFromAllow`, `isFullThreadGroupMask`, `applyThreadGroupMaskToAllow`, `defaultThreadGroupAllow`, mirroring the level-mask helpers.
- New method `toggleThreadGroup(key)` mirroring `toggleLevel`. It calls `refreshAllowedRecords()` and re-runs search the same way.
- `refreshAllowedRecords` is the single place that calls the renamed `list_records_by_filters` IPC and passes both masks. The "skip the IPC when both masks are full" early-out becomes `isFullLevelMask(...) && isFullThreadGroupMask(...)`.
- `runSearch` passes `thread_group_mask: buildThreadGroupMaskFromAllow(...)` in the `SearchRequest`.
- `applyRestored` / `snapshot` round-trip `thread_group_mask`.

**Filters flyout (`FiltersPopover.vue`, new):**

A small popover anchored under a new "Filters" button in `SearchBar.vue`.

```
+-- Filters -----------------------------+
| Levels                                  |
|   [TRACE] [DEBUG] [INFO] [WARN] ...     |
|                                         |
| Threads                                  |
|   [Requests] [Jobs] [Scheduler]          |
|   [System] [Infra] [Other]               |
|                                         |
|                       Reset all filters |
+-----------------------------------------+
```

- Two sections, each a flex row of toggle pills using the same `.lvl-btn` styling treatment (off state: 0.35 opacity + strike-through).
- Level pills retain their per-level colour token (`--level-trace` ... `--level-fatal`).
- Thread pills get a single muted accent treatment - no per-group colour for v1 (we have no semantic colour for "Jobs" the way we do for "ERROR").
- "Reset all filters" turns both masks fully on. Disabled when both are already full.
- Click-outside closes. Esc closes. Tab focus order: levels first, then threads, then reset.

**SearchBar.vue changes:**

- Remove the inline `.level-mask` `<span>` block (lines 128-138 in the current file).
- Replace with a single button:

```vue
<button
  type="button"
  class="filters-toggle"
  :class="{ 'is-on': filtersOpen, 'has-active': hasNonDefaultFilters }"
  :title="filtersSummary"
  @click="filtersOpen = !filtersOpen"
>
  Filters
  <span v-if="hasNonDefaultFilters" class="filters-badge" aria-hidden="true" />
</button>
<FiltersPopover
  v-if="filtersOpen"
  :tab="tab"
  @close="filtersOpen = false"
/>
```

- `hasNonDefaultFilters` = `!isFullLevelMask(tab.levelAllow.value) || !isFullThreadGroupMask(tab.threadGroupAllow.value)`.
- `filtersSummary` (used as the `title` tooltip) lists what is currently filtered, e.g. `"Hiding TRACE, DEBUG; only Jobs, Scheduler threads"`. Quick orientation without opening the popover.
- The badge is a small accent-coloured dot top-right of the button.

### Tail handling

Unchanged in shape: tail deltas call `refreshAllowedRecords()` when
either mask is non-default and re-run the search when there's a query.
The intersection just considers both masks now.

## Data flow

```
                              SearchBar.vue
                                   |
                          (click) Filters
                                   v
                          FiltersPopover.vue
                                   |
                   toggleLevel(k)  |  toggleThreadGroup(k)
                                   v
                                tab.ts
                                   |
        refreshAllowedRecords()    |    runSearch()
                                   v
                  invoke('list_records_by_filters',
                         { file_id, level_mask, thread_group_mask })
                                   |
                                   v
              clog-core::list_records_by_filters
                                   |
        walks RecordHeader[]; classify(thread_bytes) -> ThreadGroup
                                   |
                                   v
                       Vec<RecordRef>  ->  UI narrowing
```

## Persistence shape

```jsonc
// session.json file entry, after change
{
  "path": "...",
  "scroll_top": 0,
  "follow_tail": true,
  "level_mask": 63,             // existing
  "thread_group_mask": 63,      // NEW; default 0x3F = all-on
  "filter_text": "",
  "search_mode": "smart",
  "search_case_sensitive": false,
  "filter_mode": false,
  "bookmarks": []
}
```

## Test strategy

**`clog-core`:**

- Unit tests for `classify(thread: &[u8])` covering every observed
  thread name from `research/solopress-prod.log` and
  `research/solopress-wsl-oink.out`, plus a `b""` (empty) input and a
  garbage UTF-8 input.
- Property test: any input that matches `^play-thread-\d+$`
  classifies as `Requests`. Same for `jobs-thread-\d+` -> `Jobs`.
- Integration test for `list_records_by_filters` against a small fake
  `LineIndex` + `RecordHeader[]` covering: full mask both axes
  (returns all), zero level mask (returns none), zero thread mask
  (returns none), mixed (only records whose level AND group are
  allowed).

**`ui/src`:**

- Vitest for `buildThreadGroupMaskFromAllow` / `applyThreadGroupMaskToAllow` round-trip.
- Vitest for `isFullThreadGroupMask`.
- Component test for `FiltersPopover.vue`: toggling a pill mutates the
  tab's allow ref; Reset clears both masks; click-outside emits
  `close`.

**Manual smoke tests (on `research/solopress-prod.log`):**

1. Open the file. Open Filters. Toggle off Requests. Viewport should
   collapse from ~75k records to a much smaller set dominated by Jobs
   and main.
2. Toggle off everything except Other. Confirm the residual records
   are the rare Memcached / Netty / JGroups header lines.
3. Combine: level mask = ERROR only, thread group = Jobs only.
   Confirm only error-level records from `jobs-thread-N` appear.
4. Save + reopen tab. Confirm both masks round-trip from `session.json`.
5. Tail the file with a non-default mask (use `fake_tailer` example).
   Confirm new appended records respect both masks.

## Migration / rollout

Backend changes are additive (new IPC, extended payload, new
persistence field with serde default). Frontend cuts over in a single
PR because the IPC rename is the only breaking call.

No build-phase gate; this lands as a single feature PR against `master`.
Update `docs/future-ideas.md` to remove the "field-scoped operators"
crossover line (this implements a chunk of it) and add a "Custom
user-defined thread groups" entry.

## Files touched (approximate)

- `crates/clog-core/src/thread_groups.rs` (new)
- `crates/clog-core/src/search.rs` (add `list_records_by_filters`, extend `SearchRequest`)
- `crates/clog-core/src/lib.rs` (re-export `ThreadGroup`)
- `crates/clog-app/src/main.rs` (rename IPC; extend search payload)
- `crates/clog-app/src/persistence.rs` (`thread_group_mask` field)
- `ui/src/types.ts` (`ThreadGroupKey`, `THREAD_GROUP_KEYS`, `THREAD_GROUP_BIT`, extend `RestoredFile` + `SearchRequest`)
- `ui/src/tab.ts` (state + helpers + `toggleThreadGroup` + `refreshAllowedRecords` extension + restore/snapshot)
- `ui/src/components/FiltersPopover.vue` (new)
- `ui/src/components/SearchBar.vue` (remove level pills; add Filters button + popover wiring)
- `ui/src/composables/useSession.ts` (round-trip the new field)
- Tests for each of the above.
