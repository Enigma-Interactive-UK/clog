# Minimap heatmap implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade the existing 20px minimap so it conveys density of ERROR/FATAL/WARN records per bucket (heat), not just worst-severity hue, and surface error/warn counts in the minimap tooltip.

**Architecture:** Backend rollup in `crates/clog-app/src/main.rs` grows from `Vec<Level>` to `Vec<BucketStat { worst, error, warn, total }>` plus `max_error_warn_sum` for normalisation. UI consumes the richer shape and paints two layers (dim wash + hot overlay scaled to `error + warn`). Filter-mode minimap (built UI-side from the visible record subset) gets the same upgrade so filtered views also heatmap. Tooltip gains a second line with split error/warn counts.

**Tech Stack:** Rust (clog-app, serde), Tauri v2 IPC, Vue 3 + TypeScript + `<script setup>`, HTML5 canvas.

**Spec:** [docs/superpowers/specs/2026-05-23-minimap-heatmap-design.md](../specs/2026-05-23-minimap-heatmap-design.md)

---

## File map

- **Modify** `crates/clog-app/src/main.rs` - `LevelMinimapPayload` struct, new `BucketStat` struct, extracted pure rollup helper `build_level_minimap_payload`, `get_level_minimap` command, new unit tests in the existing `#[cfg(test)] mod tests` block.
- **Modify** `ui/src/types.ts` - replace `LevelMinimapPayload` interface, add `BucketStat` interface.
- **Modify** `ui/src/components/LogViewport.vue` - `minimapBuckets` type, `fetchMinimap`, `buildFilteredMinimap`, `paintMinimap`, `MinimapTooltip` shape, `updateMinimapTooltip`, template tooltip block, scoped styles for the new tooltip line.
- **Modify** `.wolf/anatomy.md` - update the `get_level_minimap` description.
- **Modify** `.wolf/memory.md` - append a one-line entry per OpenWolf protocol.

---

## Task 1: Extract the rollup into a pure, testable helper (no behaviour change)

The current rollup is inlined inside the `#[tauri::command] fn get_level_minimap` and reads `state.files` under a Mutex. Unit tests can't call it directly. Extract the maths into a free function that takes a record slice plus a line count plus a bucket count and returns the payload. This is a pure refactor — the existing wire shape is unchanged. Lock the current behaviour with a test before the next task changes it.

**Files:**
- Modify: `crates/clog-app/src/main.rs` (around lines 618-692 + the test mod near line 1622)

- [ ] **Step 1: Add a failing baseline test for the current behaviour**

Open `crates/clog-app/src/main.rs`, locate the `#[cfg(test)] mod tests { ... }` block (starts around line 1622). Add this test inside it, after the existing helpers (`fresh_file`, `extend`):

```rust
#[test]
fn level_minimap_baseline_worst_severity_per_bucket() {
    let (mut file, scanner) = fresh_file();
    // 3 records: INFO, ERROR, INFO -- with the wsl-dev pattern. Each is one
    // line, so line_count == 3.
    let body = concat!(
        "[INFO ] 2026-05-22 16:28:59.246 [main] play - one\n",
        "[ERROR] 2026-05-22 16:28:59.247 [main] play - two\n",
        "[INFO ] 2026-05-22 16:28:59.248 [main] play - three\n",
    );
    extend(&mut file, &scanner, body.as_bytes());
    assert_eq!(file.line_count, 3);

    // 3 buckets, one record per bucket. Bucket 1 should be ERROR.
    let payload = build_level_minimap_payload(&file.records, file.line_count, 3);
    assert_eq!(payload.buckets.len(), 3);
    assert_eq!(payload.buckets[0], Level::Info);
    assert_eq!(payload.buckets[1], Level::Error);
    assert_eq!(payload.buckets[2], Level::Info);
    assert_eq!(payload.line_count, 3);
}
```

- [ ] **Step 2: Run the test and confirm it fails (compile error - `build_level_minimap_payload` not defined)**

```powershell
cargo test -p clog-app level_minimap_baseline_worst_severity_per_bucket
```

Expected: compile error `cannot find function build_level_minimap_payload in this scope`.

- [ ] **Step 3: Extract the helper**

In `crates/clog-app/src/main.rs`, immediately above `fn get_level_minimap` (currently around line 647), insert:

```rust
/// Pure rollup used by `get_level_minimap` and by tests. Maps each
/// record's physical line span onto a `bucket_count`-wide grid and keeps
/// the worst severity per bucket. Equivalent to the inlined logic that
/// used to live inside the command.
fn build_level_minimap_payload(
    records: &[RecordHeader],
    line_count: u64,
    bucket_count: usize,
) -> LevelMinimapPayload {
    let bucket_count = bucket_count.max(1);
    let mut buckets = vec![Level::Unknown; bucket_count];
    if line_count == 0 || records.is_empty() {
        return LevelMinimapPayload {
            buckets,
            line_count,
        };
    }
    let lc = line_count;
    let bc = bucket_count as u64;
    for rec in records {
        let first_line = u64::from(rec.line_offset);
        let last_line = first_line + u64::from(rec.line_count.max(1)) - 1;
        let first_bucket =
            usize::try_from(first_line.saturating_mul(bc) / lc).unwrap_or(bucket_count - 1);
        let last_bucket = usize::try_from(last_line.saturating_mul(bc) / lc)
            .unwrap_or(bucket_count - 1)
            .min(bucket_count - 1);
        for b in &mut buckets[first_bucket..=last_bucket] {
            if level_rank(rec.level) > level_rank(*b) {
                *b = rec.level;
            }
        }
    }
    LevelMinimapPayload {
        buckets,
        line_count,
    }
}
```

`RecordHeader` is the existing per-record struct on `OpenedFile.records` (see `crates/clog-app/src/main.rs:129`). The helper reads only `rec.line_offset`, `rec.line_count`, and `rec.level` — the same fields the inlined loop already touches.

- [ ] **Step 4: Replace the inlined body in `get_level_minimap` with a call to the helper**

In `fn get_level_minimap` (around line 648), replace the body that follows the `state.files.lock() ... ok_or(IpcError::UnknownFile { ... })?` lookup. Keep the lookup. Replace everything from the `let bucket_count = bucket_count.max(1) as usize;` line through the final `Ok(LevelMinimapPayload { ... })` with:

```rust
    Ok(build_level_minimap_payload(
        &file.records,
        file.line_count,
        bucket_count as usize,
    ))
```

- [ ] **Step 5: Run the baseline test and confirm it passes**

```powershell
cargo test -p clog-app level_minimap_baseline_worst_severity_per_bucket
```

Expected: PASS.

- [ ] **Step 6: Run the full workspace test + lint suite**

```powershell
cargo test --workspace
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all green. If clippy complains about the new helper's argument count or naming, fix inline.

- [ ] **Step 7: Commit**

```powershell
git add crates/clog-app/src/main.rs
git commit -m "Extracted minimap rollup into pure helper build_level_minimap_payload, locked current worst-severity behaviour with a unit test, no wire change."
```

---

## Task 2: Grow the payload to carry per-bucket error/warn/total counts

Replace `Vec<Level>` with `Vec<BucketStat>` and add `max_error_warn_sum` + `max_total`. Update both the helper and the TypeScript wire interface. The UI's `paintMinimap`/`buildFilteredMinimap` stay one-pass-worst-severity for now (Task 3 will paint the second layer); they just adapt to the new bucket shape by reading `.worst`. This keeps the build green after this commit.

**Files:**
- Modify: `crates/clog-app/src/main.rs` - `LevelMinimapPayload`, new `BucketStat`, `build_level_minimap_payload`, tests.
- Modify: `ui/src/types.ts` - interfaces.
- Modify: `ui/src/components/LogViewport.vue` - adapter wiring only.

- [ ] **Step 1: Write failing tests for the new payload shape**

In `crates/clog-app/src/main.rs`, inside `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn level_minimap_counts_errors_and_warns_per_bucket() {
    let (mut file, scanner) = fresh_file();
    // 4 single-line records, all in one bucket: WARN, ERROR, FATAL, INFO.
    let body = concat!(
        "[WARN ] 2026-05-22 16:28:59.246 [main] play - w\n",
        "[ERROR] 2026-05-22 16:28:59.247 [main] play - e\n",
        "[FATAL] 2026-05-22 16:28:59.248 [main] play - f\n",
        "[INFO ] 2026-05-22 16:28:59.249 [main] play - i\n",
    );
    extend(&mut file, &scanner, body.as_bytes());
    assert_eq!(file.line_count, 4);

    // 1 bucket -> everything lumps together.
    let p = build_level_minimap_payload(&file.records, file.line_count, 1);
    assert_eq!(p.buckets.len(), 1);
    let b = &p.buckets[0];
    assert_eq!(b.worst, Level::Fatal);
    // FATAL aggregates into `error` (per spec).
    assert_eq!(b.error, 2, "ERROR + FATAL count");
    assert_eq!(b.warn, 1, "WARN count");
    assert_eq!(b.total, 4);
    assert_eq!(p.max_error_warn_sum, 3, "(error + warn) max");
    assert_eq!(p.max_total, 4);
}

#[test]
fn level_minimap_empty_file_zeroes_counts() {
    let (file, _scanner) = fresh_file();
    let p = build_level_minimap_payload(&file.records, file.line_count, 8);
    assert_eq!(p.buckets.len(), 8);
    for b in &p.buckets {
        assert_eq!(b.worst, Level::Unknown);
        assert_eq!(b.error, 0);
        assert_eq!(b.warn, 0);
        assert_eq!(b.total, 0);
    }
    assert_eq!(p.max_error_warn_sum, 0);
    assert_eq!(p.max_total, 0);
}

#[test]
fn level_minimap_multi_line_record_bumps_every_touched_bucket_once() {
    let (mut file, scanner) = fresh_file();
    // One ERROR header followed by two continuation lines (no header
    // pattern on those, so the parser folds them into the previous record
    // via the standard continuation rule). line_count == 3, records == 1.
    let body = concat!(
        "[ERROR] 2026-05-22 16:28:59.246 [main] play - boom\n",
        "    at com.example.A.foo(A.java:12)\n",
        "    at com.example.B.bar(B.java:34)\n",
    );
    extend(&mut file, &scanner, body.as_bytes());
    assert_eq!(file.line_count, 3);
    assert_eq!(file.records.len(), 1);

    // 3 buckets, one line per bucket. The record spans all three.
    let p = build_level_minimap_payload(&file.records, file.line_count, 3);
    assert_eq!(p.buckets.len(), 3);
    for (i, b) in p.buckets.iter().enumerate() {
        assert_eq!(b.worst, Level::Error, "bucket {i}");
        assert_eq!(b.error, 1, "bucket {i} - counted once per touched bucket");
        assert_eq!(b.warn, 0, "bucket {i}");
        assert_eq!(b.total, 1, "bucket {i}");
    }
    assert_eq!(p.max_error_warn_sum, 1);
    assert_eq!(p.max_total, 1);
}
```

Update the baseline test added in Task 1 so its assertions read `.worst` rather than the bare Level:

```rust
#[test]
fn level_minimap_baseline_worst_severity_per_bucket() {
    let (mut file, scanner) = fresh_file();
    let body = concat!(
        "[INFO ] 2026-05-22 16:28:59.246 [main] play - one\n",
        "[ERROR] 2026-05-22 16:28:59.247 [main] play - two\n",
        "[INFO ] 2026-05-22 16:28:59.248 [main] play - three\n",
    );
    extend(&mut file, &scanner, body.as_bytes());
    assert_eq!(file.line_count, 3);

    let payload = build_level_minimap_payload(&file.records, file.line_count, 3);
    assert_eq!(payload.buckets.len(), 3);
    assert_eq!(payload.buckets[0].worst, Level::Info);
    assert_eq!(payload.buckets[1].worst, Level::Error);
    assert_eq!(payload.buckets[2].worst, Level::Info);
    assert_eq!(payload.line_count, 3);
}
```

- [ ] **Step 2: Run the tests and confirm they fail with the expected compile errors**

```powershell
cargo test -p clog-app level_minimap
```

Expected: compile errors. `BucketStat` does not exist. `LevelMinimapPayload` has no `max_error_warn_sum`.

- [ ] **Step 3: Update the Rust structs and the helper**

In `crates/clog-app/src/main.rs`, replace the `LevelMinimapPayload` definition near line 619 with:

```rust
#[derive(Debug, Clone, Copy, Serialize)]
struct BucketStat {
    /// Worst severity touching this bucket. Same semantics as the
    /// previous scalar minimap payload. Drives hue UI-side.
    worst: Level,
    /// Record count in this bucket at level ERROR or FATAL. Counted per
    /// record, not per physical line -- a multi-line ERROR contributes
    /// 1 to every bucket it touches.
    error: u32,
    /// Record count in this bucket at level WARN.
    warn: u32,
    /// Total record count in this bucket. Reserved for a future
    /// density wash; emitted now so the UI doesn't need another IPC
    /// round trip.
    total: u32,
}

#[derive(Debug, Serialize)]
struct LevelMinimapPayload {
    /// One stat per bucket, top-of-file first. Length == requested
    /// `bucket_count` (clamped to >= 1). When the file is empty every
    /// bucket reads as `Level::Unknown` with zeroed counts.
    buckets: Vec<BucketStat>,
    /// The line span this minimap was computed over. UIs compare this to
    /// the current `line_count` to know whether a refetch is warranted.
    line_count: u64,
    /// Maximum value of `(error + warn)` across all buckets. The UI uses
    /// this to normalise hot-overlay alpha. Zero means "no error/warn
    /// anywhere" -- UI falls back to the dim wash only.
    max_error_warn_sum: u32,
    /// Maximum `total` across all buckets. Reserved for a future
    /// density wash.
    max_total: u32,
}
```

Replace `build_level_minimap_payload` (added in Task 1) with the count-aware version:

```rust
fn build_level_minimap_payload(
    records: &[RecordHeader],
    line_count: u64,
    bucket_count: usize,
) -> LevelMinimapPayload {
    let bucket_count = bucket_count.max(1);
    let empty = BucketStat {
        worst: Level::Unknown,
        error: 0,
        warn: 0,
        total: 0,
    };
    let mut buckets = vec![empty; bucket_count];
    if line_count == 0 || records.is_empty() {
        return LevelMinimapPayload {
            buckets,
            line_count,
            max_error_warn_sum: 0,
            max_total: 0,
        };
    }
    let lc = line_count;
    let bc = bucket_count as u64;
    for rec in records {
        let first_line = u64::from(rec.line_offset);
        let last_line = first_line + u64::from(rec.line_count.max(1)) - 1;
        let first_bucket =
            usize::try_from(first_line.saturating_mul(bc) / lc).unwrap_or(bucket_count - 1);
        let last_bucket = usize::try_from(last_line.saturating_mul(bc) / lc)
            .unwrap_or(bucket_count - 1)
            .min(bucket_count - 1);
        for b in &mut buckets[first_bucket..=last_bucket] {
            if level_rank(rec.level) > level_rank(b.worst) {
                b.worst = rec.level;
            }
            b.total = b.total.saturating_add(1);
            match rec.level {
                Level::Error | Level::Fatal => {
                    b.error = b.error.saturating_add(1);
                }
                Level::Warn => {
                    b.warn = b.warn.saturating_add(1);
                }
                _ => {}
            }
        }
    }
    let mut max_error_warn_sum = 0u32;
    let mut max_total = 0u32;
    for b in &buckets {
        let heat = b.error.saturating_add(b.warn);
        if heat > max_error_warn_sum {
            max_error_warn_sum = heat;
        }
        if b.total > max_total {
            max_total = b.total;
        }
    }
    LevelMinimapPayload {
        buckets,
        line_count,
        max_error_warn_sum,
        max_total,
    }
}
```

- [ ] **Step 4: Run the tests**

```powershell
cargo test -p clog-app level_minimap
```

Expected: all 4 tests PASS.

- [ ] **Step 5: Update the TypeScript wire interface**

Open `ui/src/types.ts`. Replace the `LevelMinimapPayload` interface (currently at lines 138-141) with:

```ts
export interface BucketStat {
  /** Worst severity touching this bucket. Same alphabet as `LineRow.level`
   *  ('trace' | 'debug' | 'info' | 'warn' | 'error' | 'fatal' | 'off' | 'all' | 'unknown'). */
  worst: string
  /** Record count at level ERROR or FATAL in this bucket. */
  error: number
  /** Record count at level WARN in this bucket. */
  warn: number
  /** Total record count in this bucket. */
  total: number
}

export interface LevelMinimapPayload {
  buckets: BucketStat[]
  line_count: number
  /** Max of `(error + warn)` across all buckets; normaliser for hot overlay. */
  max_error_warn_sum: number
  /** Max `total` across all buckets; reserved for density wash. */
  max_total: number
}
```

- [ ] **Step 6: Adapt LogViewport.vue to the new shape (minimal, no paint changes yet)**

Open `ui/src/components/LogViewport.vue`. Three small changes — keep behaviour identical for now.

(a) Change the `minimapBuckets` declaration around line 45 from:

```ts
const minimapBuckets = ref<string[]>([])
```

to:

```ts
import type { BucketStat } from '../types'
// ...existing imports...
const minimapBuckets = ref<BucketStat[]>([])
let lastMaxErrorWarnSum = 0
```

Add `BucketStat` to the existing import from `'../types'` (around line 21) rather than a new import line if you prefer; either is fine, just keep one import block per source.

(b) In `fetchMinimap`, the IPC-driven branch currently does `minimapBuckets.value = payload.buckets`. Keep that assignment; also store the normaliser:

```ts
minimapBuckets.value = payload.buckets
lastMaxErrorWarnSum = payload.max_error_warn_sum
```

In the filter-mode branch, replace `minimapBuckets.value = buildFilteredMinimap(...)` with a call that returns the new shape (function rewrite next).

(c) Replace `buildFilteredMinimap` (around lines 357-383) so it returns `BucketStat[]` and also computes the normaliser. Update the call site to capture both:

```ts
function buildFilteredMinimap(
  source: RecordRef[],
  virtualLineCount: number,
  bucketCount: number,
): { buckets: BucketStat[]; maxErrorWarnSum: number } {
  const empty = (): BucketStat => ({ worst: 'unknown', error: 0, warn: 0, total: 0 })
  const buckets: BucketStat[] = new Array(bucketCount)
  for (let i = 0; i < bucketCount; i++) buckets[i] = empty()
  if (virtualLineCount === 0 || bucketCount === 0) {
    return { buckets, maxErrorWarnSum: 0 }
  }
  let virtualCursor = 0
  for (const rec of source) {
    const firstLine = virtualCursor
    const lastLine = virtualCursor + Math.max(rec.record_line_count, 1) - 1
    const firstBucket = Math.min(
      bucketCount - 1,
      Math.floor((firstLine * bucketCount) / virtualLineCount),
    )
    const lastBucket = Math.min(
      bucketCount - 1,
      Math.floor((lastLine * bucketCount) / virtualLineCount),
    )
    const rank = minimapLevelRank(rec.level)
    for (let b = firstBucket; b <= lastBucket; b++) {
      const bucket = buckets[b]
      if (rank > minimapLevelRank(bucket.worst)) bucket.worst = rec.level
      bucket.total += 1
      if (rec.level === 'error' || rec.level === 'fatal') bucket.error += 1
      else if (rec.level === 'warn') bucket.warn += 1
    }
    virtualCursor += rec.record_line_count
  }
  let maxErrorWarnSum = 0
  for (const b of buckets) {
    const heat = b.error + b.warn
    if (heat > maxErrorWarnSum) maxErrorWarnSum = heat
  }
  return { buckets, maxErrorWarnSum }
}
```

In `fetchMinimap`, update the filter-mode branch:

```ts
const { buckets, maxErrorWarnSum } = buildFilteredMinimap(source, eff, bucketCount)
minimapBuckets.value = buckets
lastMaxErrorWarnSum = maxErrorWarnSum
```

(d) Update `paintMinimap` to read `.worst` rather than the bare string. Only the inner `colourAt` and the run-coalescing loop need to change:

```ts
const colourAt = (i: number): string | null =>
  i < h ? (LEVEL_COLOUR[buckets[i].worst] ?? null) : null
```

Everything else in `paintMinimap` stays as it is. The dim/bright distinction is added in Task 3.

Reference to `lastMaxErrorWarnSum`: declared in (a), populated in (b)+(c), unused in `paintMinimap` until Task 3.

- [ ] **Step 7: Run UI build and tests**

```powershell
npm --prefix ui run build
npm --prefix ui run test
```

Expected: build green, tests green. Visually identical to before (no paint change yet).

- [ ] **Step 8: Smoke-test the dev shell to confirm no regression**

```powershell
cargo dev
```

Open `research/solopress-prod.log`. The minimap should look identical to before. Close.

- [ ] **Step 9: Run the full lint+test sweep**

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: all green.

- [ ] **Step 10: Commit**

```powershell
git add crates/clog-app/src/main.rs ui/src/types.ts ui/src/components/LogViewport.vue
git commit -m "Grew the level minimap payload from Vec<Level> to Vec<BucketStat> carrying worst severity plus per-bucket error / warn / total counts. Added max_error_warn_sum normaliser. UI wired to the new shape, visual output unchanged for now.

Added three unit tests covering count rollup (ERROR + FATAL into error, WARN into warn), the empty-file zeroed-counts case, and per-record-not-per-line counting across multi-bucket records."
```

---

## Task 3: Paint the heatmap (two-layer dim wash + hot overlay)

Now that the data is flowing, paint the bright overlay so error/warn clusters pop. The base layer keeps the existing palette but at lower alpha; the hot layer adds a second `fillRect` pass at `lerp(0.55, 1.0, heat / max)` alpha for any bucket with error/warn records.

**Files:**
- Modify: `ui/src/components/LogViewport.vue` - `LEVEL_COLOUR`, `paintMinimap`, plus a small alpha helper.

- [ ] **Step 1: Lower the existing `LEVEL_COLOUR` alphas to read as a dim wash**

Open `ui/src/components/LogViewport.vue`. Replace the `LEVEL_COLOUR` map (lines 272-282) with:

```ts
// Dim "wash" alpha used as the base layer for every bucket. Buckets that
// also have ERROR/FATAL/WARN records get a brighter second pass painted
// on top in paintMinimap. Keep INFO/UNKNOWN deliberately null so quiet
// regions read as background.
const LEVEL_COLOUR: Record<string, string | null> = {
  trace: 'rgba(111, 118, 130, 0.12)',
  debug: 'rgba(158, 197, 255, 0.12)',
  info: null,
  warn: 'rgba(224, 176, 74, 0.18)',
  error: 'rgba(212, 87, 95, 0.18)',
  fatal: 'rgba(179, 134, 232, 0.18)',
  off: 'rgba(74, 84, 102, 0.12)',
  all: 'rgba(108, 199, 135, 0.18)',
  unknown: null,
}

// Hot-overlay colours, used as a second layer on top of the wash for
// buckets where (error + warn) > 0. Alpha is modulated per bucket from
// HOT_ALPHA_MIN..HOT_ALPHA_MAX based on `heat / max_error_warn_sum`.
const LEVEL_HOT: Record<string, string | null> = {
  warn: 'rgba(224, 176, 74, ALPHA)',
  error: 'rgba(212, 87, 95, ALPHA)',
  fatal: 'rgba(179, 134, 232, ALPHA)',
}
const HOT_ALPHA_MIN = 0.55
const HOT_ALPHA_MAX = 1.0
```

- [ ] **Step 2: Rewrite `paintMinimap` to paint both layers**

Replace the entire `paintMinimap` function (lines 418-454) with:

```ts
function hotColour(level: string, heat: number, max: number): string | null {
  if (heat <= 0 || max <= 0) return null
  const template = LEVEL_HOT[level]
  if (!template) return null
  const t = Math.max(0, Math.min(1, heat / max))
  const alpha = HOT_ALPHA_MIN + (HOT_ALPHA_MAX - HOT_ALPHA_MIN) * t
  return template.replace('ALPHA', alpha.toFixed(3))
}

function paintMinimap() {
  const canvas = minimapEl.value
  if (!canvas) return
  const buckets = minimapBuckets.value
  const h = buckets.length
  if (h === 0) {
    const ctx = canvas.getContext('2d')
    if (ctx) ctx.clearRect(0, 0, canvas.width, canvas.height)
    return
  }
  const dpr = globalThis.devicePixelRatio || 1
  canvas.width = MINIMAP_WIDTH * dpr
  canvas.height = h * dpr
  canvas.style.width = `${MINIMAP_WIDTH}px`
  canvas.style.height = `${h}px`
  const ctx = canvas.getContext('2d')
  if (!ctx) return
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
  ctx.fillStyle = currentMinimapBg()
  ctx.fillRect(0, 0, MINIMAP_WIDTH, h)

  // Pass 1: base wash (worst-severity, low alpha). Same run-coalescing
  // strategy as before so paint cost stays flat in bucket count.
  const washAt = (i: number): string | null =>
    i < h ? (LEVEL_COLOUR[buckets[i].worst] ?? null) : null
  let runStart = 0
  let runColour = washAt(0)
  for (let i = 1; i <= h; i++) {
    const next = washAt(i)
    if (next !== runColour) {
      if (runColour !== null) {
        ctx.fillStyle = runColour
        ctx.fillRect(0, runStart, MINIMAP_WIDTH, i - runStart)
      }
      runStart = i
      runColour = next
    }
  }

  // Pass 2: hot overlay. Per-bucket alpha is bucket-local, so no run
  // coalescing here -- a one-pixel-per-bucket loop is fine at ~viewport
  // height (a few hundred buckets at most).
  const max = lastMaxErrorWarnSum
  if (max > 0) {
    for (let i = 0; i < h; i++) {
      const b = buckets[i]
      const heat = b.error + b.warn
      if (heat === 0) continue
      const colour = hotColour(b.worst, heat, max)
      if (!colour) continue
      ctx.fillStyle = colour
      ctx.fillRect(0, i, MINIMAP_WIDTH, 1)
    }
  }

  paintBookmarkMarkers(ctx, h)
}
```

- [ ] **Step 3: Run UI build to confirm no type errors**

```powershell
npm --prefix ui run build
```

Expected: PASS.

- [ ] **Step 4: Smoke-test on the prod fixture**

```powershell
cargo dev
```

Open `research/solopress-prod.log`. Confirm:

- Quiet INFO regions look like a faint wash (much dimmer than before).
- ERROR/WARN clusters pop visibly: a single stray ERROR is clearly visible above the wash, and dense ERROR runs blaze near full alpha.
- Bookmark accent stripes still paint on top.
- Toggle to filter mode (level mask off INFO) and confirm filtered view also heatmaps correctly.

Close the dev shell.

- [ ] **Step 5: Commit**

```powershell
git add ui/src/components/LogViewport.vue
git commit -m "Painted the minimap as two layers. The base wash uses the existing per-level palette at a dim alpha (0.12-0.18) so quiet regions recede into the background. A second hot-overlay pass paints WARN / ERROR / FATAL buckets at alpha lerp(0.55, 1.0, heat / max_error_warn_sum), so a single stray error stays clearly visible while dense clusters blaze."
```

---

## Task 4: Tooltip second line - split error/warn counts

The minimap tooltip currently shows the line number and the timestamp of the record under the cursor. Add a third line when the hovered bucket has any error or warn records.

**Files:**
- Modify: `ui/src/components/LogViewport.vue` - `MinimapTooltip` shape, `tooltipLineFromY` / `updateMinimapTooltip`, template, scoped styles.

- [ ] **Step 1: Track the hovered bucket index alongside the line index**

In `LogViewport.vue`, locate the `MinimapTooltip` interface (around line 509). Replace with:

```ts
interface MinimapTooltip {
  visible: boolean
  top: number
  left: number
  lineIndex: number
  timestamp: string | null
  error: number
  warn: number
}
const minimapTooltip = ref<MinimapTooltip>({
  visible: false,
  top: 0,
  left: 0,
  lineIndex: 0,
  timestamp: null,
  error: 0,
  warn: 0,
})
```

Update both reset sites (in `onMinimapPointerLeave` around line 580, and the early return inside `updateMinimapTooltip` around line 559) to:

```ts
minimapTooltip.value = {
  visible: false, top: 0, left: 0,
  lineIndex: 0, timestamp: null, error: 0, warn: 0,
}
```

- [ ] **Step 2: Compute the bucket index from the pointer Y and fill the counts**

Replace `tooltipLineFromY` (around line 524) with a richer helper that returns both the line index and the bucket index:

```ts
function tooltipTargetFromY(
  clientY: number,
): { lineIndex: number; bucketIndex: number } | null {
  const canvas = minimapEl.value
  if (!canvas || effectiveCount.value === 0) return null
  const rect = canvas.getBoundingClientRect()
  if (rect.height <= 0) return null
  const ratio = Math.max(0, Math.min(1, (clientY - rect.top) / rect.height))
  const virtualIdx = Math.min(
    effectiveCount.value - 1,
    Math.floor(ratio * effectiveCount.value),
  )
  const bucketCount = minimapBuckets.value.length
  const bucketIndex = bucketCount === 0
    ? -1
    : Math.min(bucketCount - 1, Math.floor(ratio * bucketCount))
  return { lineIndex: actualLineIndex(virtualIdx), bucketIndex }
}
```

Replace the `updateMinimapTooltip` function with:

```ts
function updateMinimapTooltip(ev: PointerEvent) {
  const target = tooltipTargetFromY(ev.clientY)
  if (target === null) {
    minimapTooltip.value = {
      visible: false, top: 0, left: 0,
      lineIndex: 0, timestamp: null, error: 0, warn: 0,
    }
    return
  }
  const { lineIndex, bucketIndex } = target
  const pageIdx = Math.floor(lineIndex / PAGE_SIZE)
  if (!props.tab.pages.value.has(pageIdx)) void props.tab.fetchPage(pageIdx)
  const ts = timestampForLine(lineIndex)
  const canvas = minimapEl.value
  const rect = canvas?.getBoundingClientRect()
  const left = rect ? rect.left : ev.clientX
  const bucket = bucketIndex >= 0 ? minimapBuckets.value[bucketIndex] : null
  minimapTooltip.value = {
    visible: true,
    top: ev.clientY,
    left,
    lineIndex,
    timestamp: ts,
    error: bucket?.error ?? 0,
    warn: bucket?.warn ?? 0,
  }
}
```

Delete the now-unused `tooltipLineFromY`.

- [ ] **Step 3: Render the new line in the template**

In the template block near lines 928-936, replace the tooltip body with:

```html
<div
  v-if="minimapTooltip.visible"
  class="minimap-tooltip"
  :style="{ top: `${minimapTooltip.top}px`, left: `${minimapTooltip.left}px` }"
>
  <span class="line-no">line {{ minimapTooltip.lineIndex + 1 }}</span>
  <span v-if="minimapTooltip.timestamp" class="ts">{{ minimapTooltip.timestamp }}</span>
  <span v-else class="ts muted">--</span>
  <span
    v-if="minimapTooltip.error > 0 || minimapTooltip.warn > 0"
    class="heat"
  >{{ heatLine(minimapTooltip.error, minimapTooltip.warn) }}</span>
</div>
```

Add a `heatLine` helper in the script block (next to other small helpers, e.g. above `defineExpose`):

```ts
function heatLine(error: number, warn: number): string {
  const parts: string[] = []
  if (error > 0) parts.push(`${error} ${error === 1 ? 'error' : 'errors'}`)
  if (warn > 0) parts.push(`${warn} ${warn === 1 ? 'warning' : 'warnings'}`)
  return parts.join(', ')
}
```

- [ ] **Step 4: Style the new line**

In the `<style scoped>` block, find the `.minimap-tooltip` rule (around line 1041) and add a `.heat` selector next to the existing `.line-no` / `.ts` ones:

```css
.line-no { color: var(--fg-muted); }
.ts { color: var(--fg-default); }
.ts.muted { color: var(--fg-dim); }
.heat { color: var(--hl-search-fg); font-weight: 600; }
```

`--hl-search-fg` is the existing search-match foreground token; reusing it gives the heat line a colour that reads as "attention" without inventing a new palette entry.

- [ ] **Step 5: Build and smoke-test**

```powershell
npm --prefix ui run build
cargo dev
```

Open `research/solopress-prod.log`. Hover over:

- A quiet INFO region: tooltip shows line + timestamp only (no heat line).
- An ERROR cluster: tooltip shows line + timestamp + "N errors" (or "N errors, M warnings").
- A WARN-only region: tooltip shows "M warnings".
- A bucket with exactly one error: "1 error" (singular).

Close.

- [ ] **Step 6: Run the full lint + test sweep**

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm --prefix ui run test
```

Expected: all green.

- [ ] **Step 7: Commit**

```powershell
git add ui/src/components/LogViewport.vue
git commit -m "Added a heat line to the minimap tooltip showing the hovered bucket's error and warning counts. The line is omitted when both counts are zero, and uses singular / plural correctly (1 error vs 2 errors)."
```

---

## Task 5: Update OpenWolf project memory

OpenWolf protocol requires `anatomy.md` to stay current and a one-line entry per significant change in `memory.md`. Apply both now that the feature is in.

**Files:**
- Modify: `.wolf/anatomy.md` - update the `get_level_minimap` description in the clog-app section.
- Modify: `.wolf/memory.md` - append a one-line entry.

- [ ] **Step 1: Update the anatomy entry**

Open `.wolf/anatomy.md`. Locate the line describing `get_level_minimap` (currently near the IPC commands block for `crates/clog-app/`, mentions "worst severity per bucket via `level_rank`"). Replace that sentence with:

```
- `get_level_minimap(file_id, bucket_count)` -> `LevelMinimapPayload { buckets: Vec<BucketStat { worst, error, warn, total }>, line_count, max_error_warn_sum, max_total }`. Walks records and keeps the worst severity per bucket (drives hue) plus error / warn / total record counts per bucket (drives the hot overlay alpha). FATAL aggregates into `error`. The pure rollup lives in `build_level_minimap_payload(records, line_count, bucket_count)`, unit-tested independently of the command.
```

- [ ] **Step 2: Append a memory entry**

Open `.wolf/memory.md` and append at the end (one line):

```
- 2026-05-23: minimap heatmap upgrade landed - payload now carries per-bucket BucketStat (worst, error, warn, total) plus max_error_warn_sum. UI paints a dim wash + hot overlay; tooltip gained a heat line.
```

- [ ] **Step 3: Commit**

```powershell
git add .wolf/anatomy.md .wolf/memory.md
git commit -m "Updated OpenWolf anatomy and memory for the minimap heatmap upgrade."
```

---

## Final verification

- [ ] **Step 1: Full sweep**

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm --prefix ui run build
npm --prefix ui run test
```

Expected: all green.

- [ ] **Step 2: Manual smoke**

```powershell
cargo dev
```

- Open `research/solopress-prod.log` (the 74k-line fixture).
- Visually confirm error clusters pop vs. quiet INFO runs.
- Hover a hot bucket and confirm the tooltip's heat line.
- Hover a quiet INFO bucket and confirm no heat line.
- Toggle filter mode (mask off INFO) and confirm the heatmap repaints over the filtered subset.
- Tail a live file with `cargo run -p clog-core --example fake_tailer -- <somepath> --rate 5` in a second shell, open it in the dev shell, and confirm the minimap repaints as new ERROR records land.

Close everything.

- [ ] **Step 3: Done**

Feature complete. No further commits.
