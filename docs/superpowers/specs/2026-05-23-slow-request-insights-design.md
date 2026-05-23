# Slow request insights - design

> Status: draft, awaiting user review.
> Source idea: ad-hoc request, sibling to the post-v1 [docs/future-ideas.md](../../future-ideas.md) "Analysis / insights" theme (per-logger histograms, error-rate sparkline, similar-record clustering).

## Goal

Play 1.x emits a `SLOW REQUEST` line whenever a controller action exceeds
its threshold. In the sample log they appear in two formats - same
information, two phrasings:

```
2026-05-21 00:00:44.830 INFO [play-thread-11] - SLOW REQUEST: 5064ms - /preflight/killpreflightrequest.json (SoloPreflightFront.killPreflightRequest_JSON)
2026-05-21 00:00:44.830 INFO [play-thread-11] - SLOW REQUEST (5064ms) - /preflight/killpreflightrequest.json [SoloPreflightFront.killPreflightRequest_JSON] - consider using an asynchronous call to ease the load on the threadpool.
```

Same wall-clock millisecond, same path, same class.method - the second is
a duplicate of the first, just with a tail suggestion. Today these are
buried in 75k lines of INFO noise; the user can grep for them but cannot
*see* which endpoints are slow most often, by total time spent, or how
their duration distributes.

Build an **insights drawer** that aggregates SLOW REQUEST records into a
sortable per-endpoint table the user can scan while reading the log.
Clicking a group jumps the viewport to that endpoint's longest hit.

## Non-goals

- **Time-series sparkline** per endpoint group (future-ideas
  "Error-rate sparkline" sibling - revisit after this lands).
- **Configurable detection patterns**. v1 ships a hardcoded matcher;
  surface in Settings later if other Play apps emit different phrasing.
- **Cross-tab aggregation**. Insights are per-file; merging across tabs
  is the "Merge view across tabs" line item in future-ideas and is its
  own design.
- **Filter-mode integration**. Clicking a group scrolls the viewport, it
  does not constrain the visible record set to that endpoint. (Open
  question OQ-4.)
- **Persistence**. The drawer's open / closed state and current sort are
  per-session UI; they do not survive a restart.
- **Generalising into a "rules engine" for arbitrary aggregations**.
  Slow requests are the only kind for v1. The marker system is a separate
  primitive and stays separate.

## Detection

### Pattern

A single regex captures both formats by alternation on the duration
delimiter and the class.method delimiter:

```text
^SLOW REQUEST\s*(?::\s*(\d+)ms|\((\d+)ms\))\s*-\s*(\S+)\s+[\(\[]([^\)\]]+)[\)\]]
```

Capture groups:

1. duration (colon form, `: 5064ms`)
2. duration (paren form, `(5064ms)`)
3. raw path - first whitespace-bounded token after the duration
4. class.method - bracketed by `(...)` or `[...]`

Anchored to `^SLOW REQUEST` so trailing copy after the closing bracket
(`- consider using an asynchronous call...`) is allowed and ignored.

### Where the regex runs

Against the **record's first physical line, message-bytes-only** -
i.e. `bytes[message_start..message_end]` where `message_start` /
`message_end` come from `RecordHeader.fields.message`. This guarantees:

- Stack-trace continuations cannot be mis-flagged as slow requests
  (same guard as the marker scanner).
- Timestamps, levels, threads, and logger fields can never accidentally
  match the pattern - they live outside the message span.

If a record's pattern has no `%msg` token (so `fields.message` is `None`),
the whole first-line bytes minus the header field spans are used as a
fallback. This is a defensive degradation: most Play patterns parse a
clean message, but custom patterns may not.

### Dedup

Two records form a duplicate pair when they share **all three**:

- `timestamp_ms` (the parsed record timestamp truncated to ms)
- `normalised_path` (see below)
- `class_method`

The earlier-line-index record wins; the later one is folded into the
winner's `dup_count`. Duration is taken from the winner (in the observed
data both copies carry the same number, so this is consistent).

A record without a parseable timestamp falls back to using
`record.line_offset` as the dedup-time key, which effectively disables
dedup for that record - acceptable since timestamp-less records are rare
and conservative behaviour (keep both) is safer than collapsing
unrelated events.

### Path normalisation

Two layers:

- **Raw path**: the captured `\S+` token as-is.
- **Normalised path**: the aggregation key. Built from the raw path by:
  - Stripping any query string (`?...` to end).
  - Lower-casing the scheme/host if one is present (it never is in the
    samples, but the rule is cheap insurance).
  - Replacing each path segment that matches `^\d+$` (pure digits) or
    `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$` (UUID)
    or `^[0-9a-f]{12,}$` (long hex run) with the literal `{id}`.
  - Preserving the trailing slash when present. `/foo` and `/foo/` are
    distinct groups - they hit different Play routes in practice.

The UI's "Path mode" toggle (Normalised | Raw) flips which key drives
aggregation. Raw mode treats every observed path as its own group;
normalised mode merges `/order/12345/edit` and `/order/67890/edit` into
`/order/{id}/edit`. Default is Normalised.

## Backend changes

### `clog-core/src/slow_requests.rs` (new)

The detector and aggregator are pure engine code so they can be
unit-tested without Tauri. New module:

```rust
pub struct SlowRequestEntry {
    /// Aggregation key under the active path mode (normalised or raw).
    pub path: String,
    /// Distinct raw paths observed in this group. Always exactly 1 in
    /// raw mode; can be many in normalised mode.
    pub raw_paths: Vec<String>,
    /// Hits in this group after dedup.
    pub count: u32,
    /// Sum of durations in milliseconds. u64 to survive 65k * 10s.
    pub total_ms: u64,
    pub min_ms: u32,
    pub max_ms: u32,
    pub avg_ms: u32,
    /// Nearest-rank p95 over the deduped durations.
    pub p95_ms: u32,
    /// Line index of the slowest hit in this group; the UI uses this
    /// for the "click to jump" action.
    pub longest_line: u64,
    /// Top occurrences by duration, capped at OCCURRENCE_CAP (50).
    /// Provided so the UI can show a drill-down without paginating IPC.
    pub occurrences: Vec<SlowRequestOccurrence>,
}

pub struct SlowRequestOccurrence {
    /// Parsed record timestamp in epoch milliseconds, or None if the
    /// record had no timestamp.
    pub timestamp_ms: Option<i64>,
    pub duration_ms: u32,
    /// Physical line index of the occurrence.
    pub line_index: u64,
    pub record_idx: u32,
    /// 1 means "unique", >1 means N duplicates were collapsed onto this row.
    pub dup_count: u32,
    pub class_method: String,
    /// As-observed path before normalisation.
    pub raw_path: String,
}

pub struct SlowRequestSummary {
    pub entries: Vec<SlowRequestEntry>,
    /// Total deduped hit count across all groups.
    pub total_hits: u32,
    /// How many input records were folded into another row by dedup.
    pub deduped: u32,
    /// Sum of durations across all groups, milliseconds.
    pub total_ms: u64,
}

pub fn extract_slow_requests(
    records: &[RecordHeader],
    bytes: &[u8],
    line_offsets: &[u64],
    mode: PathMode,
) -> SlowRequestSummary;

pub enum PathMode { Normalised, Raw }
```

Constants:

- `OCCURRENCE_CAP: usize = 50` - per-entry occurrence cap. A group with
  10k hits keeps only the 50 slowest in its detail panel; the headline
  count remains accurate (it tracks the full deduped count, not the
  capped list).

Algorithmic shape:

1. Single pass over `records`. For each record:
   - Slice the first-line message bytes via `fields.message`.
   - Apply the compiled regex; bail on no match.
   - Parse duration, raw_path, class_method.
   - Parse `record.fields.timestamp` (when present) into epoch
     milliseconds. Fall back to `line_offset` as a dedup salt when
     absent.
   - Normalise the path per `mode`.
   - Look up `(dedup_key)` in a `HashMap<DedupKey, usize>` pointing at
     a temporary `Vec<RawOccurrence>`; if present, bump `dup_count` and
     keep the lower `line_index`; if absent, push.
2. Second pass: group `RawOccurrence`s by aggregation key into
   `HashMap<String, SlowRequestEntry>`; compute count, sum, min, max,
   avg, p95 (nearest-rank: sort ascending, pick `dur[max(0, ceil(0.95 * n) - 1)]`).
3. Cap each entry's `occurrences` to the top 50 by duration descending.
4. Sort entries by `total_ms` descending into the returned vec. The UI
   can re-sort client-side; ordering on the wire is just a default.

Dedup key shape:

```rust
struct DedupKey {
    // ms since epoch, or line_offset when timestamp_ms is None
    bucket: i64,
    normalised_path: String,
    class_method: String,
}
```

### `clog-app` IPC

New command in [crates/clog-app/src/main.rs](../../../crates/clog-app/src/main.rs):

```rust
#[tauri::command]
fn get_slow_requests(
    state: State<'_, AppState>,
    file_id: u64,
    mode: PathMode,
) -> Result<SlowRequestSummary, IpcError>;
```

Reads the file's records under the existing mutex and calls
`extract_slow_requests`. No caching - the walk is O(records); on a
75k-record file the regex pass is well under a frame budget. If it turns
out to be slow under tail mode, cache the parsed-but-unaggregated
`Vec<RawOccurrence>` on `OpenedFile` and re-aggregate on mode flip; defer
the optimisation until measurement shows a problem.

### Tail behaviour

Same triggers as the minimap and markers: file open, pattern apply, tail
delta, rotation. The UI re-invokes `get_slow_requests` on each trigger
when the drawer is open. When the drawer is closed, no fetch fires.

### Tests

Cover:

1. **Format A parsing** (`SLOW REQUEST: 5064ms - /path (Class.method)`).
2. **Format B parsing** (`SLOW REQUEST (5064ms) - /path [Class.method] - consider...`).
3. **Dedup** of A+B at the same timestamp_ms.
4. **No dedup** when timestamps differ by 1ms.
5. **Path normalisation**: numeric segment, UUID segment, long-hex
   segment, query string, trailing slash preservation.
6. **Raw mode**: every observed path is its own group.
7. **p95** with small N (e.g. N=1, N=2, N=20): nearest-rank picks the
   right element.
8. **Stack-trace continuation** containing "SLOW REQUEST" must not flag.
9. **OCCURRENCE_CAP**: a group with 200 hits keeps the top 50 by
   duration and the headline `count` stays 200.
10. **Empty file** / **no slow records**: returns empty summary, zeroed
    totals, valid struct.

Plus a smoke test against `research/solopress-prod.log` asserting at
least one `SLOW REQUEST` entry parses cleanly (regression guard against
regex drift).

## Frontend changes

### Wire shape

`ui/src/types.ts` gains mirrors of the Rust structs:

```ts
export type SlowRequestPathMode = 'normalised' | 'raw'

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

### Drawer component

New file `ui/src/components/InsightsDrawer.vue`. The drawer lives on the
**right side**, between the existing `.marker-rail` + `.minimap`
column and the right edge of the `.viewport-shell`. It is `position:
relative; flex: 0 0 auto;` with a transitioned `width` (0 -> 360px) so
opening pushes the minimap rightward smoothly.

A toggle button in the header bar (a new icon next to the existing
settings cog) flips `insightsOpen` on the active tab. Open / closed
state is per-tab so flipping tabs preserves whichever drawer the user
last had open.

### Drawer contents

Three regions, top to bottom:

1. **Header**: title + totals chip ("Slow requests - 412 hits across 37
   endpoints, 22 dedupes") + a close button.
2. **Toolbar**:
   - Path mode segmented toggle: `Normalised | Raw`.
   - Free-text filter input (substring match against `entry.path`,
     case-insensitive, debounced 80ms).
   - Sort dropdown: `Total time | Count | Max | p95 | Avg | Path`.
     Default `Total time` descending; clicking the same option again
     flips direction.
3. **Table**: vertically scrollable list of entry rows. Each row:

```
/checkout/setdeliveryaddress.json
  12 hits . total 84.2s . avg 7.0s . p95 9.2s . max 9.2s
```

The path renders as a clickable link; clicking it scrolls the viewport
to `entry.longest_line` (via the same `jumpToLine` plumbing the marker
rail uses). A small expand caret reveals the per-occurrence list:

```
2026-05-21 00:00:44.830    5064ms    line 11   x2
2026-05-21 00:00:49.588    3172ms    line 15
...
```

Each occurrence row is also clickable, scrolling to its `line_index`.
The `x2` chip appears only when `dup_count > 1`.

### Empty / loading states

- **Loading**: while the IPC is in flight, render a skeleton with three
  blank rows.
- **Empty**: when the response is `total_hits === 0`, the body says
  "No slow requests detected in this file." with a one-line muted hint
  explaining the matched patterns.
- **IPC error**: surface inline at the top of the drawer with a Retry
  button; do not propagate to the App-level error banner (this is a
  per-feature panel, not a global concern).

### Header-bar toggle

Add an `<button class="insights-toggle">` next to the settings cog in
[ui/src/components/AppHeader.vue](../../../ui/src/components/AppHeader.vue).
The icon is the same "bar chart" glyph from common icon sets; ship it as
inline SVG so we don't pick up a dependency. Active state (drawer open
on the active tab) gets the same focus ring + accent treatment the
settings cog uses today.

### Per-tab state

`ui/src/tab.ts` gains:

```ts
insightsOpen: ref<boolean>(false)
slowRequestMode: ref<SlowRequestPathMode>('normalised')
slowRequestSort: ref<{ field: 'total' | 'count' | 'max' | 'p95' | 'avg' | 'path'; dir: 'asc' | 'desc' }>({
  field: 'total', dir: 'desc',
})
slowRequestFilter: ref<string>('')
slowRequestSummary: shallowRef<SlowRequestSummary | null>(null)
```

The drawer reads from `tab.slowRequestSummary` and invokes
`get_slow_requests` when:

- `insightsOpen` flips to true (initial fetch).
- `slowRequestMode` changes.
- The tab receives a tail / rotation delta and the drawer is open.

Sort and filter are applied client-side over the cached summary so they
do not refetch.

### Visual layout impact

The current `.viewport-shell` flex row is `[viewport | marker-rail |
minimap]`. The drawer slots in as a new rightmost child:

```
[viewport | marker-rail | minimap | insights-drawer]
```

Width transitions are on `flex-basis` so the viewport text reflows
during open / close. Minimum viewport width clamps at 480px; below that
the drawer becomes an overlay (`position: absolute; right: 0; top: 0;
bottom: 0`) with a subtle drop shadow. (Threshold matches the existing
narrow-window behaviour of the search bar - keeps the rules consistent.)

## Persistence

None in v1. Open / closed, mode, sort, and filter are session-only.
Reconsider after dogfooding - if users consistently reopen the drawer
on every tab they touch, lift `insightsOpen` and `slowRequestMode`
into the per-tab `RestoredFile` schema and bump its `schema` version
with `#[serde(default)]` cover for back-compat.

## Settings

None in v1. The detection regex and normalisation rules are baked in.

## Files changed

- **New** `crates/clog-core/src/slow_requests.rs` - detector, aggregator,
  `PathMode`, `SlowRequest*` structs, unit tests.
- `crates/clog-core/src/lib.rs` - re-export the public surface.
- `crates/clog-app/src/main.rs` - `get_slow_requests` IPC command, add
  to `invoke_handler!`.
- `ui/src/types.ts` - `SlowRequest*` interfaces, `SlowRequestPathMode`.
- `ui/src/tab.ts` - per-tab insights state.
- **New** `ui/src/components/InsightsDrawer.vue`.
- `ui/src/components/LogViewport.vue` - drawer slot in the viewport
  shell (passive - the drawer renders itself), expose `jumpToLine` if
  not already public.
- `ui/src/components/AppHeader.vue` - insights toggle button.
- `ui/src/App.vue` - wire the toggle to the active tab's
  `insightsOpen` ref.
- `ui/src/style.css` - drawer width / transition tokens, insights toggle
  active-state colour reusing existing palette tokens (no new tokens
  expected).
- `.wolf/anatomy.md` - new `get_slow_requests` IPC entry, new
  `slow_requests` module summary, new component entries.

## Verification

- `cargo test --workspace` green (new `slow_requests` module + smoke
  against `research/solopress-prod.log`).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo fmt --check` clean.
- `npm --prefix ui run test` green.
- `npm --prefix ui run build` green.
- `cargo dev` smoke on `research/solopress-prod.log`: open the
  drawer; confirm at least the `/preflight/killpreflightrequest.json`
  and `/productfront/getupdatedproductoptions.json/` groups appear with
  sensible totals; flip Normalised / Raw and confirm the row counts
  change as expected; click a row and confirm the viewport jumps to the
  slowest hit; trigger a tail append (`fake_tailer`) and confirm new
  hits land in the table.

## Open questions

These choices are baked into the spec at sensible defaults; flag during
review if any should flip.

- **OQ-1. Drawer position.** Right rail (current default) vs. bottom
  drawer vs. modal. Right rail wins on "scan while reading the log"
  but costs horizontal real estate. Bottom drawer is friendlier on
  narrow windows but competes vertically with the log viewport.
- **OQ-2. Default sort.** Total time desc (default) vs. Count desc vs.
  Max desc. Total time surfaces "where is time actually being spent";
  Count surfaces "what is firing constantly"; Max surfaces "worst
  individual offender".
- **OQ-3. Trailing-slash normalisation.** Currently preserved. Should
  `/foo` and `/foo/` collapse into the same group? They map to different
  Play routes in practice, which is why this spec keeps them separate.
- **OQ-4. Click-to-filter.** Currently a click on a path scrolls the
  viewport to the longest hit but does not constrain visible records.
  Should it also flip the viewport into filter mode against that path's
  records?
- **OQ-5. Click-to-search.** Alternative to OQ-4: have the row click
  populate the search bar with a `SLOW REQUEST.*<path>` regex so the
  existing hit-list machinery takes over.
