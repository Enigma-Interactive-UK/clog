# Minimap heatmap upgrade - design

> Status: design approved, awaiting implementation plan.
> Source idea: [docs/future-ideas.md](../../future-ideas.md) - "Minimap / heatmap gutter".

## Goal

Today's minimap paints one worst-severity stripe per bucket. It answers "is
there an error somewhere in this region?" but not "where are the *clusters*
of errors?". A single stray ERROR in a quiet INFO region looks the same as
500 ERRORs in a hot region.

Upgrade the existing 20px minimap so it visually conveys the **density of
error/warning records per bucket** while preserving the current
worst-severity hue. Clean regions remain a quiet wash; ERROR/WARN clusters
blaze proportionally to cluster size.

No new gutter, no new screen real estate.

## Non-goals

- Time-axis ruler (separate future-ideas entry).
- Error-rate sparkline (separate future-ideas entry).
- Density emphasis as a user-toggleable setting (revisit if dim-density
  proves controversial).
- Re-bucketing by timestamp rather than line index (still line-indexed).
- Changes to bookmark accent painting on the minimap.

## Backend changes

### `clog-app` minimap rollup

The rollup currently lives in [crates/clog-app/src/main.rs:618-692](../../../crates/clog-app/src/main.rs#L618-L692)
(`get_level_minimap` + `LevelMinimapPayload`). Not in `clog-core`.

Replace `Vec<Level>` with a parallel `Vec<BucketStat>`:

```rust
#[derive(Debug, Serialize)]
struct BucketStat {
    /// Worst severity touching this bucket. Drives hue. Same semantics as
    /// the existing scalar.
    worst: Level,
    /// Record count in this bucket at level ERROR or FATAL.
    error: u32,
    /// Record count in this bucket at level WARN.
    warn: u32,
    /// Total record count in this bucket. Currently unused by the UI but
    /// emitted so a future density wash can use it without another IPC
    /// round trip.
    total: u32,
}

#[derive(Debug, Serialize)]
struct LevelMinimapPayload {
    buckets: Vec<BucketStat>,
    line_count: u64,
    /// Max of `(error + warn)` across all buckets. The UI uses this to
    /// normalise the per-bucket alpha. Zero means "no error/warn anywhere" -
    /// UI falls back to the base wash only.
    max_error_warn_sum: u32,
    /// Max `total` across all buckets. Reserved for a future density wash.
    max_total: u32,
}
```

Counting rules:

- A record contributes to **every bucket it touches**, same as today's
  worst-severity rollup. A multi-line ERROR record spanning buckets 3..6
  bumps `error` by 1 in buckets 3, 4, 5, and 6.
- Counts are per-record, not per-line. Stack-trace continuations don't
  multiply the count.
- FATAL aggregates into `error` (both drive the same "something failed
  hard" reading; the tooltip can't usefully split them and the existing
  level palette already paints FATAL distinctly via hue).
- `worst` semantics unchanged - `level_rank` still applies.

### Tests

- Existing minimap test extended: assert `buckets.len() == requested`,
  `max_error_warn_sum` matches the true max of `(error + warn)` across
  buckets, ERROR records bump `error`, WARN records bump `warn`, INFO
  records bump neither, FATAL aggregates into `error`.
- Empty-file case: all buckets zeroed, both maxes zero, payload still
  has `bucket_count` entries.

## Frontend changes

### Wire shape

`ui/src/types.ts`: replace the `LevelMinimapPayload` interface to match
the new Rust shape, plus a `BucketStat` interface.

### Paint

In [ui/src/components/LogViewport.vue](../../../ui/src/components/LogViewport.vue),
the minimap canvas paint is currently a single pass over `buckets: Level[]`.
Replace with a two-layer paint:

1. **Base wash** (existing behaviour, dimmer):
   - For every bucket, paint a 1px-tall slice at the worst-severity colour
     with alpha **0.18**. This is the quiet substrate.
   - INFO and UNKNOWN/OFF deliberately read as background; their colour
     tokens are already faded in the current palette. Keep that
     behaviour - the wash is for level-coloured buckets (WARN+).

2. **Hot overlay**:
   - Let `heat = error + warn` for the bucket.
   - For buckets where `heat > 0` AND `max_error_warn_sum > 0`, paint a
     second slice at the same hue, alpha `lerp(0.55, 1.0, heat / max_error_warn_sum)`.
   - A bucket with one stray ERROR thus draws at alpha ~0.55 (clearly
     visible above the wash); the densest bucket draws at full alpha.

Keep the existing run-coalescing optimisation: consecutive buckets with
identical `(worst, alpha)` collapse into one `fillRect` to keep paint
cost flat as bucket count grows.

Bookmark accent stripes paint on top, unchanged.

### Tooltip

The tooltip currently shows the timestamp of the record under the
hovered pixel. Extend it with a **second line** when `error_warn > 0`:

```
14:32:01.421
3 errors, 7 warnings
```

Format rules:

- Omit the line entirely when both `error` and `warn` are zero (which is
  most buckets).
- Zero-count tier omitted: "3 errors" alone if no warnings; "7 warnings"
  alone if no errors. Both nonzero: "3 errors, 7 warnings".
- Plural handling: "1 error", "2 errors".
- The paint alpha is driven by `error + warn` summed UI-side, normalised
  against `max_error_warn_sum`.

The continuation-line timestamp resolution stays as-is. Timestamps are
still derived from record headers; the heatmap counts are independent.

### Repaint triggers

Same triggers as today: file open, pattern apply, tail delta, rotation,
viewport resize. No new triggers needed - `error_warn` is part of the
same `get_level_minimap` response.

## Persistence

None. Counts are derived from records on demand.

## Settings

None. No new toggle in this iteration.

## Files changed

- [crates/clog-app/src/main.rs](../../../crates/clog-app/src/main.rs) -
  `BucketStat` struct, `LevelMinimapPayload` struct, `get_level_minimap`
  rollup logic, unit tests.
- [ui/src/types.ts](../../../ui/src/types.ts) - `BucketStat` interface,
  updated `LevelMinimapPayload` interface.
- [ui/src/components/LogViewport.vue](../../../ui/src/components/LogViewport.vue) -
  two-layer paint, tooltip second line.
- [.wolf/anatomy.md](../../../.wolf/anatomy.md) - update the
  `get_level_minimap` description.

## Verification

- `cargo test --workspace` green (extended minimap test).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo fmt --check` clean.
- `npm --prefix ui run test` green.
- `cargo dev` smoke on `research/solopress-prod.log`: confirm error
  clusters visually pop vs. quiet INFO regions; hover a hot bucket and
  see the split error/warn count line in the tooltip.
