# Collapse records - design spec

> Addresses issue #1 ([Feature Request] - Collapse Records). When a multi-line
> error blows the viewport, the reader's place vanishes. This feature lets the
> user fold multi-line records to just their header line so context is
> preserved, with explicit control over which records collapse and per-file
> overrides.

## tl;dr

- Three-way **collapse mode** per file: `None` (current behaviour, default),
  `Errors` (collapse multi-line ERROR / FATAL records), `All` (collapse every
  multi-line record).
- **Global default** lives in Settings -> Behaviour. **Per-file override** is
  surfaced in the FiltersPopover and in the TabStrip right-click context menu.
  The per-file value can be `Inherit` (follow global) or one of the three
  explicit modes.
- **Visual:** a new ~14px chevron column sits between the level gutter strip
  and the line-number cell. Collapsed multi-line records show a right-pointing
  chevron; expanded ones show a down chevron. A muted-italic `+N lines` badge
  appears at the end of the collapsed header row, pinned to the right edge of
  the visible viewport via `position: sticky`, yielding to natural document
  flow when the user scrolls past the message's true end.
- **Interaction:** chevron click or `Space` (when the viewport is focused)
  toggles the record under the sticky header. Search-hit jumps, bookmark
  jumps, sticky-header re-centre, insights drawer entry clicks, and
  programmatic recentre all **auto-expand** the target record. Minimap clicks
  and scroll do not. Auto-expansions are **transient** - the previous
  auto-expanded record collapses again on the next auto-expand, so navigation
  does not leave a trail of opened records. Manual chevron / Space expansions
  are sticky.
- **Persistence:** per-file `collapseMode`, `manuallyExpanded`, and
  `manuallyCollapsed` sets persist to `session.json` alongside `bookmarks`.
  Transient expansions are in-memory only. All three new bits are cleared on
  detected rotation (same hook as bookmarks).
- **Architecture (Approach 1):** a derived `visibleRowToLine: number[]` plus a
  reverse `lineToRow: Map<number, number>` is the only new structure. The
  virtualiser indexes visible rows; the engine and minimap stay line-indexed
  and untouched.
- **Minimap stays line-indexed and unchanged for v1.** A visual treatment for
  folded ranges in the minimap is recorded as a v1.1 follow-up.

---

## Goals

- Stop a single multi-line ERROR from pushing the reader's context off screen.
- Let the user pick a default policy at three granularities: global default,
  per-file override, and per-record manual override.
- Preserve all existing navigation, search, bookmarking, and tail behaviour
  so this feature reads as additive, not a regime change.
- Contain the change inside the UI layer - no engine, parser, or persistence
  schema changes beyond a small extension of the per-file record shape.

## Non-goals

- Changing how records are scanned, parsed, or indexed.
- A new "expand-all" / "collapse-all" toolbar action. Mode flips already cover
  the bulk case.
- Reworking the minimap. v1 keeps it line-indexed and accepts the
  scroll-feel discontinuity; v1.1 may add a visual fold treatment.
- Persisting transient navigation expansions across sessions.
- A second keyboard shortcut for cycling the mode. The popover and context
  menu cover that path.

## Data model

### Engine (`clog-core`): no changes

`RecordHeader.line_offset` and `line_count` already identify the header row
and the record's full row span. Multi-line detection is `line_count > 1`.
Level filtering uses `RecordHeader.level`.

### Per-tab state (`ui/src/tab.ts`)

Three new refs are added to the Tab and exposed through its public surface:

| Name                 | Type                | Persisted | Purpose                                                            |
|----------------------|---------------------|-----------|--------------------------------------------------------------------|
| `collapseMode`       | `Ref<CollapseMode>` | yes       | `'inherit' \| 'none' \| 'errors' \| 'all'`. Default `'inherit'`.   |
| `manuallyExpanded`   | `Ref<Set<number>>`  | yes       | Header-row line indices the user forced open against the mode.     |
| `manuallyCollapsed`  | `Ref<Set<number>>`  | yes       | Header-row line indices the user forced closed against the mode.  |
| `transientlyExpanded`| `Ref<Set<number>>`  | no        | Header-row line indices auto-expanded by intent navigation.        |

`CollapseMode` is added to `ui/src/types.ts`. The persisted shape on the Rust
side ([crates/clog-app/src/persistence.rs](../../crates/clog-app/src/persistence.rs))
gains three sibling fields next to `bookmarks` on the per-file record:

```rust
pub collapse_mode: CollapseMode,            // serde with snake_case
pub manually_expanded: Vec<u64>,            // sorted, deduped line indices
pub manually_collapsed: Vec<u64>,           // sorted, deduped line indices
```

### Global setting (`Settings`)

`Settings` gains one field, persisted in `settings.json`:

```ts
collapse_records_default: 'none' | 'errors' | 'all'   // default 'none'
```

### Effective mode and visibility predicate

```text
effectiveMode = collapseMode === 'inherit'
              ? settings.collapse_records_default
              : collapseMode

For a record header at line L with line_count = N and level = lvl:
  if N === 1:                             always expanded (no chevron)
  else:
    defaultExpanded =
      effectiveMode === 'none'
      || (effectiveMode === 'errors' && lvl is not Error and lvl is not Fatal)
    expanded =
      transientlyExpanded.has(L)
      || (defaultExpanded
            ? !manuallyCollapsed.has(L)
            : manuallyExpanded.has(L))
```

`Unknown` level is treated as non-error: under `'errors'` mode, multi-line
Unknown records remain expanded. Under `'all'` mode, they collapse.

### Visible-row index (Approach 1)

A computed array drives the virtualiser:

```ts
visibleRowToLine: number[]           // length = total visible rows
lineToRow:       Map<number, number> // reverse lookup; only contains
                                     // currently-visible line indices
```

Construction walks `recordHeaders` once. For each header, if the record is
expanded, push `line_offset .. line_offset + line_count - 1` into the array;
otherwise push only `line_offset`. The reverse map is populated in the same
pass.

Both recompute when any of these change:
- `recordHeaders.length` (tail growth, file load)
- `effectiveMode`
- `manuallyExpanded`, `manuallyCollapsed`, `transientlyExpanded`

The cost is O(records), not O(lines). For a 75k-line file with ~30k records
the rebuild is sub-millisecond on a typical dev machine.

## UI

### Chevron column

A new grid column is inserted into each row in
[ui/src/components/LogViewport.vue](../../ui/src/components/LogViewport.vue),
between the level gutter strip and the line-number cell. Column width is
~14px; always present for layout stability.

| Row type                                  | Chevron content    | Click target  |
|-------------------------------------------|--------------------|---------------|
| Single-line record header                 | empty              | inert         |
| Collapsed multi-line header               | `▸` (right)        | toggle expand |
| Expanded multi-line header                | `▾` (down)         | toggle collapse |
| Continuation row of an expanded record    | empty              | inert         |
| Continuation row of a collapsed record    | (not rendered)     | -             |

Glyph colour inherits the row foreground at ~75% opacity, full opacity on
hover. `title` attribute reads `"Collapse record"` or `"Expand record (+12
lines)"`. The entire 14px cell is the click target.

### Sticky `+N lines` badge

The collapsed header row appends a badge inside its existing `.txt` span:

```html
<span class="collapse-badge">+12 lines</span>
```

CSS makes the badge `position: sticky; right: 0;` so it pins to the right
edge of the visible viewport as the user scrolls horizontally. A
`::before` pseudo-element on the badge paints a short
transparent-to-row-background gradient so message text fades out behind the
badge rather than colliding with its left edge. When the user scrolls the
row all the way to the end of the message text, the badge falls into normal
document flow at the message's true end, exposing the final characters.

Style: muted italic, same colour family as the existing
`.idx` / line-number text. Click is inert.

### Mode reset on per-file mode change

Changing `collapseMode` (popover, context menu, or as a side effect of any
future automation) clears both `manuallyExpanded` and `manuallyCollapsed`.
`transientlyExpanded` is also cleared. Rationale: the mode is the new rule;
sticky overrides from the previous regime would confuse the user.

### Sticky header

The sticky header reads from `visibleRowToLine[0]` (or the topmost row in
view) the same way it does today, so no special handling is needed for the
folded state. When the active record is a collapsed multi-line, the sticky
header shows that single header line; pressing `Space` or clicking the
chevron expands it in place.

### Tail follow

`followTail` scrolls to the end of `visibleRowToLine.length`, not
`line_count`. New tail-arrived records apply the current effective mode
immediately, so an ERROR appended during tail in `'errors'` mode lands
collapsed.

## Controls

### Global default (SettingsModal -> Behaviour tab)

A new control between the existing behaviour rows:

```
Collapse records by default     ( ) None    ( ) Errors    ( ) All
```

Bound to `settings.collapse_records_default`. Hint text below: *Multi-line
records are folded to just their header line. Per-file overrides live in the
filters popover.*

### Per-file override (FiltersPopover)

Beneath the existing thread-group toggles, a new section:

```
Collapse records
[ Inherit ] [ None ] [ Errors ] [ All ]
```

Four-button segmented control. Selected button uses the popover's existing
active-button style. When `Inherit` is selected, a faint hint line below
reads: *Inheriting global default (currently "Errors")* - showing what is
actually in force.

### Per-file override (TabStrip context menu)

Right-click on a tab gains a submenu:

```
Collapse records   >   Inherit
                       None
                       Errors
                       All
```

A check appears on the current per-file value. Selecting an item produces
the same mutation as the popover (sets `collapseMode`, clears the manual and
transient sets).

### Keyboard

`Space`, when the LogViewport is the focused element, toggles the record
under the sticky header. The toggle mutates `manuallyExpanded` or
`manuallyCollapsed` per the rules in
[Chevron toggle paths](#chevron-toggle-paths). Wired through
`useAppShortcuts.ts`. Existing `Space` bindings on input fields / dialogs
are not affected (capture-phase guard checks `document.activeElement`).

### Chevron toggle paths

| Current state of record at line L           | Effect of chevron click / `Space` |
|---------------------------------------------|------------------------------------|
| Default-expanded (mode + no overrides)      | Add L to `manuallyCollapsed`       |
| Default-collapsed (mode + no overrides)     | Add L to `manuallyExpanded`        |
| In `manuallyExpanded`                       | Remove from `manuallyExpanded`     |
| In `manuallyCollapsed`                      | Remove from `manuallyCollapsed`    |
| In `transientlyExpanded` (and not in either manual set) | Remove from `transientlyExpanded` (collapses) |

This means a user who wants to keep a transient expansion can chevron-click
to collapse, then chevron-click again to manually expand (which adds to
`manuallyExpanded`). Two clicks, but predictable.

## Interaction

### Auto-expand triggers

When a navigation event targets a line that is currently not in
`lineToRow` (i.e. lives inside a collapsed record), the viewport runs the
following sequence:

1. Clear `transientlyExpanded` (collapses everything previously
   auto-opened back to its natural state).
2. Add the header line index of the target record to `transientlyExpanded`.
3. Recompute `visibleRowToLine` / `lineToRow`.
4. Perform the original scroll / highlight in the new row coordinates.

A record already in `manuallyExpanded` is untouched by step 1.

The triggering events are:

| Trigger                                | Auto-expand? |
|----------------------------------------|:------------:|
| Search next / prev / result-list click | ✅ |
| Bookmark jump (from sidebar)           | ✅ |
| Sticky-header click (re-centre)        | ✅ |
| Insights drawer entry click            | ✅ |
| Programmatic recentre (single-instance forwarded position, future go-to-timestamp) | ✅ |
| Minimap click                          | ❌ |
| Scroll wheel / scrollbar drag          | ❌ |
| Bookmark add/remove on idx cell        | ❌ |
| RecordModal open                       | ❌ |

The minimap is treated as coarse positional navigation; clicking on it
lands at whatever visible row is closest to the click and leaves collapsed
records collapsed.

### Row click

Clicking the row body (not the chevron, not the idx cell) is a no-op for
collapse purposes. The row's existing right-click context menu is
unchanged. Reserved for future row-selection work.

### RecordModal

Opening the full-record modal shows the raw text as today and does not
mutate inline collapse state. Closing returns to whatever inline state
existed.

### Insights drawer & speed rail

Operate on records and per-bucket aggregations respectively. Neither is
keyed on visible rows. Both continue to function unchanged. Clicking a
record reference in the insights drawer is an auto-expand trigger
(intent-to-view-a-specific-record); the speed rail itself does not auto-expand
on hover.

## Persistence

- `collapseMode`, `manuallyExpanded`, `manuallyCollapsed` are added to the
  per-file persisted shape in
  [crates/clog-app/src/persistence.rs](../../crates/clog-app/src/persistence.rs)
  alongside `bookmarks`. Stored as `Vec<u64>` with sort + dedupe on write
  (same convention as `bookmarks`).
- On session restore, both manual sets are pruned to in-range line indices
  by the same path that produces `prunedBookmarks()`
  ([tab.ts:229-234](../../ui/src/tab.ts#L229-L234)).
- On detected rotation, all three sets are cleared at the same call site
  that calls `clearBookmarks()`
  ([tab.ts:319-321](../../ui/src/tab.ts#L319-L321)).
- `transientlyExpanded` is in-memory only. Closing a tab loses it. This is
  correct - those expansions were navigation crumbs, not preferences.
- `collapse_records_default` is persisted in `settings.json` next to the
  other behaviour settings.

## Architecture summary

The whole feature is a presentation-layer concern. The engine, parser,
indexer, search, tail, and persistence schemas are unchanged in shape; only
the per-file persisted record gains three sibling fields.

- **Engine:** untouched.
- **UI:** `LogViewport.vue` gains a derived `visibleRowToLine` /
  `lineToRow` pair. The virtualiser indexes visible rows. Sticky header,
  search-jump, bookmark-jump, and programmatic recentre call into a single
  `revealLine(lineIdx)` helper that handles auto-expand. The minimap and
  speed-rail painters remain line-indexed.
- **Tab state:** four new refs on Tab (three persisted, one in-memory), mirrored shape to `bookmarks`.
- **Settings:** one new global enum.
- **Persistence:** three new sibling fields on the per-file record.

## Files touched

| File                                                           | Change                                                                  |
|----------------------------------------------------------------|-------------------------------------------------------------------------|
| `crates/clog-app/src/persistence.rs`                           | Add `collapse_mode`, `manually_expanded`, `manually_collapsed` per file.|
| `ui/src/types.ts`                                              | `CollapseMode` union; extend `Settings` and per-file persisted shape.   |
| `ui/src/tab.ts`                                                | New refs and helpers; prune-on-load; clear-on-rotation extension.       |
| `ui/src/composables/useSettings.ts`                            | Load/save `collapse_records_default`.                                   |
| `ui/src/composables/useAppShortcuts.ts`                        | `Space` handler routed to the active tab.                               |
| `ui/src/components/LogViewport.vue`                            | `visibleRowToLine` / `lineToRow`; chevron column; sticky badge; auto-expand `revealLine`. |
| `ui/src/components/SearchBar.vue` and search-hit nav           | Route hit-jump through `revealLine`.                                    |
| `ui/src/components/InsightsDrawer.vue`                         | Route record-jump clicks through `revealLine`.                          |
| `ui/src/components/FiltersPopover.vue`                         | "Collapse records" segmented control + hint line.                       |
| `ui/src/components/TabStrip.vue`                               | "Collapse records" context-menu submenu.                                |
| `ui/src/components/SettingsModal.vue`                          | "Collapse records by default" control in Behaviour tab.                 |
| `ui/src/style.css`                                             | Chevron column styles; collapse-badge sticky positioning + fade.        |

## Tests

UI logic is exercised through vitest unit tests against pure helpers
extracted from `tab.ts` and `LogViewport.vue` (the visible-row index
builder, the visibility predicate, the chevron toggle resolver).

Coverage targets:

- Visible-row index round-trips correctly under each mode (`none` /
  `errors` / `all`) and combinations of the three sets.
- Mode-change clears both manual sets and the transient set.
- Rotation hook clears all three new sets.
- Prune-on-load drops out-of-range entries from both manual sets.
- Auto-expand single-shot sweep: firing two `revealLine` calls in
  sequence leaves only the second target expanded.
- Auto-expand on hidden search hit: searching for a string that appears
  only inside a collapsed stack expands the containing record and
  scrolls; subsequent next-hit jumps re-sweep.
- Chevron toggle resolver: each of (default-expanded, default-collapsed,
  manual-expanded, manual-collapsed, transient-expanded) lands in the
  expected resulting state.
- Persistence round-trip restores `collapseMode`, `manuallyExpanded`,
  `manuallyCollapsed`; `transientlyExpanded` is empty after restore.
- Tail arrival of a multi-line ERROR under `'errors'` mode lands
  collapsed.

A small Rust unit test in `persistence.rs` covers serde round-trip of the
extended per-file record (including back-compat: missing fields default to
`'inherit'` and empty sets).

## Edge cases

- **Unknown-level records** are treated as non-error; never collapsed
  under `'errors'` mode; collapsed under `'all'` mode if multi-line.
- **Trailing partial record during active tail:** while a record is still
  growing (only one physical line in `line_count` so far), it renders
  expanded. Once a subsequent tail delta lifts `line_count > 1`, the
  effective-mode rules apply and the record may collapse on the next
  recompute.
- **Sticky badge background.** The `::before` fade must use the same
  background variable as the row, including hover-row, light-theme, and
  dark-theme variants. Implemented via CSS custom-property indirection on
  `.row`.
- **Layout stability.** The chevron column is always present even when
  every row in view is single-line. This prevents a layout shift when a
  tail arrival turns a previously single-line record into a multi-line
  one.
- **Search-hit highlight on collapsed header rows.** The header row gains
  a small `· N hits` suffix before the `+N lines` badge when the record
  has hidden hits inside its continuations. Auto-expand on next/prev hit
  removes the suffix as the hits become visible.

## Future ideas (not in scope)

- **Minimap fold treatment.** A faint band or tinted region in the
  minimap where multi-line records are folded, so the user can see which
  parts of the file are compressed. Likely v1.1; ship A first and feel
  out whether it's needed.
- **Promote-transient-to-manual gesture.** Shift-click the chevron, or
  pressing `Space` twice in quick succession, to pin a transient
  expansion without the manual-re-click dance. Wait until the existing
  flow is exercised before deciding.
- **Expand-all / collapse-all toolbar action.** Possibly redundant with
  mode-switching. Add only if users ask.
- **Bookmark sidebar jump auto-expand.** Already designed for; bookmark
  sidebar entry clicks should route through `revealLine`. Already noted
  in the auto-expand trigger table; will be wired when the bookmark
  sidebar gains a click-to-jump affordance.
