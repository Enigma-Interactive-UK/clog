# Collapse Records Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user fold multi-line log records to just their header line, controlled by a three-way mode (None / Errors / All) at global, per-file and per-record granularity, so a multi-line ERROR can no longer push the reader's context off screen.

**Architecture:** Pure-presentation feature. The engine, parser, search and tail are untouched. A new pure helper module (`ui/src/collapse.ts`) holds all the testable logic (effective-mode resolution, the expanded-record predicate, the visible-row index builder, the chevron-toggle resolver). `LogViewport.vue` gains a single combined `visibleRowToLine` / `lineToRow` pair that *replaces* today's `filteredLineIndices` as the one source of "virtual row -> physical line", composing filter-mode and collapse-mode into one mapping. Per-tab collapse state lives on the `Tab` factory (three persisted refs + one in-memory). Persistence extends the per-file `RestoredFile` record and adds one global `Settings` field.

**Tech Stack:** Rust (serde, persistence.rs), Vue 3 + `<script setup>` + TypeScript, `@tanstack/vue-virtual`, vitest (UI unit tests), cargo test (Rust unit tests). CSS uses the project's two-layer custom-property token system with native nested CSS.

---

## Architecture decisions & deviations from the spec

These are engineering resolutions of seams the design spec under-specified. They do not change any user-facing behaviour in the spec; they are called out here so they can be challenged during plan review.

1. **The UI needs a full per-record map, which the spec did not name.** The spec says "the only new structure is `visibleRowToLine` + `lineToRow`", and that the builder "walks `recordHeaders` once". But the viewport is currently *line-indexed*: it fetches `LineRow` pages lazily and never holds the full `(first_line, line_count, level)` array. The existing `list_records_by_filters(file_id, level_mask, thread_group_mask)` IPC already returns exactly `RecordRef { record_idx, record_first_line, record_line_count, level }` for every record passing the masks. **Decision:** call it once with *full* masks (`level_mask = 0xFFFFFFFF`, `thread_group_mask = 0x3F`) to obtain the complete record map. No engine or IPC change is required. This map is a new per-tab ref (`recordIndex`), refreshed on the same triggers as the minimap (open, tail growth, rotation, pattern apply).

2. **`visibleRowToLine` composes over filter mode, with an identity fast-path.** Today `filteredLineIndices` returns a `number[]` (filter mode) or `null` (identity: virtual row == physical line). Collapse must reduce that further. **Decision:** a single computed drives the virtualiser. It returns `null` (identity, same as today) when *nothing is hidden* - i.e. no filter active AND effective mode resolves to `none` AND no manual/transient override hides anything. Otherwise it materialises the array by walking the record list (the filter-passing subset in filter mode, else the full `recordIndex`) and applying the collapse predicate per record. This preserves today's zero-allocation behaviour for the default (`none`) case on large files, and unifies filter+collapse into one mapping that every existing `filteredLineIndices` reader switches to.

3. **`Space` is handled inside `LogViewport.vue`, not `useAppShortcuts.ts`.** The spec lists `useAppShortcuts.ts` as the home for the `Space` handler. But `useAppShortcuts` has no handle to the active viewport's sticky header (which is per-tab DOM state owned by `LogViewport`), whereas `LogViewport` already owns a `document` `keydown` listener (for Escape / cluster-popover dismissal) plus the sticky-header computed and the toggle target. **Decision:** add the `Space` handler to `LogViewport`'s existing `onDocumentKey`, guarded so it only fires when focus is inside the viewport and not in an input/textarea. `useAppShortcuts.ts` is left unchanged. (If review prefers the spec's literal wiring, the handler can be lifted later; the toggle logic is pure and lives in `collapse.ts` regardless.)

4. **Rust models the modes as `String`, not a new enum.** `RestoredFile.search_mode` and `Settings.theme` are already `String` with a `#[serde(default = "...")]` helper. **Decision:** `collapse_mode` and `collapse_records_default` follow that exact convention (`String` + default fn), keeping persistence.rs internally consistent. The TypeScript side keeps the precise `CollapseMode` union for safety.

---

## File structure

**New files:**
- `ui/src/collapse.ts` - all pure collapse logic (mode resolution, expanded predicate, visible-row index builder, chevron-toggle resolver). One clear responsibility: the collapse domain rules, with zero Vue/DOM dependencies so it is exhaustively unit-testable.
- `ui/src/collapse.test.ts` - vitest suite for the above.

**Modified files (in build order):**
| File | Change |
|------|--------|
| `ui/src/types.ts` | `CollapseMode` union; extend `Settings` and `RestoredFile`. |
| `crates/clog-app/src/persistence.rs` | `collapse_mode` + `manually_expanded` + `manually_collapsed` on `RestoredFile`; `collapse_records_default` on `Settings`; serde round-trip tests. |
| `ui/src/tab.ts` | Four new refs + helpers; `recordIndex` + `refreshRecordIndex`; prune-on-load; clear-on-rotation; `snapshot`/`applyRestored`; `setCollapseMode`. |
| `ui/src/composables/useSettings.ts` | `collapse_records_default` in `defaultSettings()`. |
| `ui/src/composables/useSession.ts` | Add collapse fields to the autosave fingerprint. |
| `ui/src/components/SettingsModal.vue` | "Collapse records by default" segmented control in the Behaviour tab. |
| `ui/src/components/FiltersPopover.vue` | "Collapse records" 4-button segmented control + inherit hint. |
| `ui/src/components/TabStrip.vue` | Right-click "Collapse records" context-menu submenu. |
| `ui/src/components/LogViewport.vue` | `recordIndex` wiring; combined `visibleRowToLine`/`lineToRow`; chevron column; sticky `+N lines` badge; `revealLine` auto-expand; `Space` handler; tail-follow over visible count. |
| `ui/src/style.css` | `--chevron-width` token; chevron column + `.collapse-badge` sticky styles. |

---

## A note on running the verification commands

All commands run from the workspace root `e:\Work\clog` in PowerShell.

- UI unit tests: `npm --prefix ui run test`
- UI typecheck/build: `npm --prefix ui run build`
- Rust tests: `cargo test --workspace`
- Rust lints (must stay green): `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings`

---

## Task 1: `CollapseMode` types and persisted-shape extensions (TypeScript)

**Files:**
- Modify: `ui/src/types.ts`

- [ ] **Step 1: Add the `CollapseMode` union and `GlobalCollapseDefault` type**

In `ui/src/types.ts`, add after the `SearchMode` / `LevelKey` exports (near line 71):

```ts
/** Per-file collapse mode. `'inherit'` follows the global default. */
export type CollapseMode = 'inherit' | 'none' | 'errors' | 'all'
/** Global default (no `'inherit'` - it is the thing inherited). */
export type GlobalCollapseDefault = 'none' | 'errors' | 'all'
```

- [ ] **Step 2: Extend the `Settings` interface**

In the `Settings` interface (around line 123-139), add one field before the closing brace:

```ts
  /** Global default collapse mode for multi-line records. Default 'none'. */
  collapse_records_default?: GlobalCollapseDefault
```

- [ ] **Step 3: Extend the `RestoredFile` interface**

In the `RestoredFile` interface (around line 141-152), add three fields before the closing brace, mirroring `bookmarks?`:

```ts
  /** Per-file collapse mode. Absent = 'inherit'. */
  collapse_mode?: CollapseMode
  /** Header-row physical line indices forced open against the mode. */
  manually_expanded?: number[]
  /** Header-row physical line indices forced closed against the mode. */
  manually_collapsed?: number[]
```

- [ ] **Step 4: Verify the UI still typechecks**

Run: `npm --prefix ui run build`
Expected: build succeeds (these are additive optional fields; no consumer breaks yet).

- [ ] **Step 5: Commit**

```bash
git add ui/src/types.ts
git commit -m "Added CollapseMode types and extended the Settings and RestoredFile shapes for collapse records."
```

---

## Task 2: Pure collapse logic - effective mode + expanded predicate

**Files:**
- Create: `ui/src/collapse.ts`
- Create: `ui/src/collapse.test.ts`

- [ ] **Step 1: Write the failing test for `effectiveMode` and `isRecordExpanded`**

Create `ui/src/collapse.test.ts`:

```ts
import { describe, it, expect } from 'vitest'
import { effectiveMode, isRecordExpanded, type CollapseSets } from './collapse'

function sets(partial: Partial<CollapseSets> = {}): CollapseSets {
  return {
    manuallyExpanded: partial.manuallyExpanded ?? new Set<number>(),
    manuallyCollapsed: partial.manuallyCollapsed ?? new Set<number>(),
    transientlyExpanded: partial.transientlyExpanded ?? new Set<number>(),
  }
}

describe('effectiveMode', () => {
  it('resolves inherit to the global default', () => {
    expect(effectiveMode('inherit', 'errors')).toBe('errors')
    expect(effectiveMode('inherit', 'none')).toBe('none')
  })
  it('passes explicit modes through unchanged', () => {
    expect(effectiveMode('all', 'none')).toBe('all')
    expect(effectiveMode('none', 'all')).toBe('none')
  })
})

describe('isRecordExpanded', () => {
  it('always expands single-line records (no chevron)', () => {
    expect(isRecordExpanded(10, 1, 'error', 'all', sets())).toBe(true)
  })
  it('none mode leaves every multi-line record expanded', () => {
    expect(isRecordExpanded(10, 5, 'error', 'none', sets())).toBe(true)
    expect(isRecordExpanded(10, 5, 'info', 'none', sets())).toBe(true)
  })
  it('errors mode collapses ERROR/FATAL multi-line, leaves others expanded', () => {
    expect(isRecordExpanded(10, 5, 'error', 'errors', sets())).toBe(false)
    expect(isRecordExpanded(10, 5, 'fatal', 'errors', sets())).toBe(false)
    expect(isRecordExpanded(10, 5, 'warn', 'errors', sets())).toBe(true)
    expect(isRecordExpanded(10, 5, 'unknown', 'errors', sets())).toBe(true)
  })
  it('all mode collapses every multi-line record incl. unknown', () => {
    expect(isRecordExpanded(10, 5, 'info', 'all', sets())).toBe(false)
    expect(isRecordExpanded(10, 5, 'unknown', 'all', sets())).toBe(false)
  })
  it('manuallyCollapsed overrides a default-expanded record', () => {
    expect(
      isRecordExpanded(10, 5, 'info', 'none', sets({ manuallyCollapsed: new Set([10]) })),
    ).toBe(false)
  })
  it('manuallyExpanded overrides a default-collapsed record', () => {
    expect(
      isRecordExpanded(10, 5, 'error', 'errors', sets({ manuallyExpanded: new Set([10]) })),
    ).toBe(true)
  })
  it('transientlyExpanded forces expansion regardless of mode', () => {
    expect(
      isRecordExpanded(10, 5, 'error', 'all', sets({ transientlyExpanded: new Set([10]) })),
    ).toBe(true)
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npm --prefix ui run test -- collapse`
Expected: FAIL - `Cannot find module './collapse'`.

- [ ] **Step 3: Write the minimal implementation**

Create `ui/src/collapse.ts`:

```ts
/**
 * Pure collapse-records logic. No Vue or DOM dependencies so every rule is
 * unit-testable in isolation. Consumed by tab.ts (state mutation) and
 * LogViewport.vue (visible-row index + chevron interaction).
 */
import type { CollapseMode, GlobalCollapseDefault, RecordRef } from './types'

export type { CollapseMode, GlobalCollapseDefault } from './types'

/** Resolve a per-file mode against the global default. */
export function effectiveMode(
  perFile: CollapseMode,
  globalDefault: GlobalCollapseDefault,
): GlobalCollapseDefault {
  return perFile === 'inherit' ? globalDefault : perFile
}

/** ERROR and FATAL are the "error" levels for `'errors'` mode. Unknown is not. */
export function isErrorLevel(level: string): boolean {
  return level === 'error' || level === 'fatal'
}

export interface CollapseSets {
  manuallyExpanded: Set<number>
  manuallyCollapsed: Set<number>
  transientlyExpanded: Set<number>
}

/**
 * Whether the record whose header sits at physical line `firstLine`, spanning
 * `lineCount` lines at `level`, is expanded under `mode` + the override sets.
 * Single-line records are always expanded (they get no chevron).
 */
export function isRecordExpanded(
  firstLine: number,
  lineCount: number,
  level: string,
  mode: GlobalCollapseDefault,
  sets: CollapseSets,
): boolean {
  if (lineCount <= 1) return true
  if (sets.transientlyExpanded.has(firstLine)) return true
  const defaultExpanded =
    mode === 'none' || (mode === 'errors' && !isErrorLevel(level))
  return defaultExpanded
    ? !sets.manuallyCollapsed.has(firstLine)
    : sets.manuallyExpanded.has(firstLine)
}

/** The mode-derived default with NO overrides applied. Used by the chevron
 *  toggle resolver to decide which manual set a fresh toggle lands in. */
export function defaultExpandedFor(
  lineCount: number,
  level: string,
  mode: GlobalCollapseDefault,
): boolean {
  if (lineCount <= 1) return true
  return mode === 'none' || (mode === 'errors' && !isErrorLevel(level))
}

// Re-export so callers importing from collapse.ts get RecordRef without a
// second import line.
export type { RecordRef }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `npm --prefix ui run test -- collapse`
Expected: PASS (all `effectiveMode` and `isRecordExpanded` cases green).

- [ ] **Step 5: Commit**

```bash
git add ui/src/collapse.ts ui/src/collapse.test.ts
git commit -m "Added pure collapse-mode resolution and the expanded-record predicate with unit tests."
```

---

## Task 3: Pure collapse logic - visible-row index builder

**Files:**
- Modify: `ui/src/collapse.ts`
- Modify: `ui/src/collapse.test.ts`

- [ ] **Step 1: Write the failing test**

Append to `ui/src/collapse.test.ts`:

```ts
import { buildVisibleRowIndex } from './collapse'
import type { RecordRef } from './types'

function rec(idx: number, first: number, count: number, level = 'info'): RecordRef {
  return { record_idx: idx, record_first_line: first, record_line_count: count, level }
}

describe('buildVisibleRowIndex', () => {
  it('pushes every line of an expanded record and only the header of a collapsed one', () => {
    // record 0: lines 0..2 expanded; record 1: lines 3..5 collapsed (header 3 only)
    const records = [rec(0, 0, 3), rec(1, 3, 3)]
    const { visibleRowToLine, lineToRow } = buildVisibleRowIndex(
      records,
      (r) => r.record_idx === 0, // record 0 expanded, record 1 collapsed
    )
    expect(visibleRowToLine).toEqual([0, 1, 2, 3])
    expect(lineToRow.get(0)).toBe(0)
    expect(lineToRow.get(2)).toBe(2)
    expect(lineToRow.get(3)).toBe(3)
    // Hidden continuation lines are absent from the reverse map.
    expect(lineToRow.has(4)).toBe(false)
    expect(lineToRow.has(5)).toBe(false)
  })

  it('round-trips an all-expanded list to the identity sequence', () => {
    const records = [rec(0, 0, 2), rec(1, 2, 1), rec(2, 3, 4)]
    const { visibleRowToLine } = buildVisibleRowIndex(records, () => true)
    expect(visibleRowToLine).toEqual([0, 1, 2, 3, 4, 5, 6])
  })

  it('collapses every multi-line record under all-collapsed', () => {
    const records = [rec(0, 0, 3), rec(1, 3, 1), rec(2, 4, 5)]
    const { visibleRowToLine } = buildVisibleRowIndex(records, (r) => r.record_line_count <= 1)
    // record 0 header (0), record 1 single line (3), record 2 header (4)
    expect(visibleRowToLine).toEqual([0, 3, 4])
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npm --prefix ui run test -- collapse`
Expected: FAIL - `buildVisibleRowIndex is not a function`.

- [ ] **Step 3: Write the minimal implementation**

Append to `ui/src/collapse.ts`:

```ts
export interface VisibleRowIndex {
  /** virtual row -> physical line index. */
  visibleRowToLine: number[]
  /** physical line index -> virtual row, for currently-visible lines only. */
  lineToRow: Map<number, number>
}

/**
 * Walk `records` in order, projecting each to its visible rows: every physical
 * line for an expanded record, just the header line for a collapsed one.
 *
 * `records` is the ordered list to project - the full record map in normal
 * mode, or the filter-passing subset (level/thread mask or search hits) in
 * filter mode. `expanded(rec)` supplies the collapse decision per record.
 */
export function buildVisibleRowIndex(
  records: readonly RecordRef[],
  expanded: (rec: RecordRef) => boolean,
): VisibleRowIndex {
  const visibleRowToLine: number[] = []
  const lineToRow = new Map<number, number>()
  for (const r of records) {
    const first = r.record_first_line
    if (expanded(r)) {
      const end = first + r.record_line_count
      for (let l = first; l < end; l++) {
        lineToRow.set(l, visibleRowToLine.length)
        visibleRowToLine.push(l)
      }
    } else {
      lineToRow.set(first, visibleRowToLine.length)
      visibleRowToLine.push(first)
    }
  }
  return { visibleRowToLine, lineToRow }
}

/**
 * Binary-search the owning record of a physical line. `records` MUST be sorted
 * ascending by `record_first_line` (the engine produces them in order).
 * Returns the record whose span contains `line`, or null if out of range.
 */
export function recordOfLine(
  records: readonly RecordRef[],
  line: number,
): RecordRef | null {
  let lo = 0
  let hi = records.length - 1
  let ans: RecordRef | null = null
  while (lo <= hi) {
    const mid = (lo + hi) >> 1
    if (records[mid].record_first_line <= line) {
      ans = records[mid]
      lo = mid + 1
    } else {
      hi = mid - 1
    }
  }
  if (!ans) return null
  const end = ans.record_first_line + ans.record_line_count
  return line >= ans.record_first_line && line < end ? ans : null
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `npm --prefix ui run test -- collapse`
Expected: PASS (`buildVisibleRowIndex` cases green; `recordOfLine` untested here, covered in Task 4).

- [ ] **Step 5: Commit**

```bash
git add ui/src/collapse.ts ui/src/collapse.test.ts
git commit -m "Added the visible-row index builder and a record-of-line binary search with unit tests."
```

---

## Task 4: Pure collapse logic - chevron toggle resolver + `recordOfLine`

**Files:**
- Modify: `ui/src/collapse.ts`
- Modify: `ui/src/collapse.test.ts`

- [ ] **Step 1: Write the failing test**

Append to `ui/src/collapse.test.ts`:

```ts
import { resolveChevronToggle, recordOfLine } from './collapse'

describe('recordOfLine', () => {
  const records = [rec(0, 0, 3), rec(1, 3, 1), rec(2, 4, 5)]
  it('finds the owning record for a header line', () => {
    expect(recordOfLine(records, 0)?.record_idx).toBe(0)
    expect(recordOfLine(records, 4)?.record_idx).toBe(2)
  })
  it('finds the owning record for a continuation line', () => {
    expect(recordOfLine(records, 2)?.record_idx).toBe(0)
    expect(recordOfLine(records, 8)?.record_idx).toBe(2)
  })
  it('returns null past the end', () => {
    expect(recordOfLine(records, 9)).toBeNull()
    expect(recordOfLine(records, -1)).toBeNull()
  })
})

describe('resolveChevronToggle', () => {
  it('default-expanded record gains a manuallyCollapsed entry', () => {
    const r = resolveChevronToggle(10, true, sets())
    expect(r.manuallyCollapsed.has(10)).toBe(true)
    expect(r.manuallyExpanded.has(10)).toBe(false)
  })
  it('default-collapsed record gains a manuallyExpanded entry', () => {
    const r = resolveChevronToggle(10, false, sets())
    expect(r.manuallyExpanded.has(10)).toBe(true)
    expect(r.manuallyCollapsed.has(10)).toBe(false)
  })
  it('toggling out of manuallyExpanded clears it', () => {
    const r = resolveChevronToggle(10, false, sets({ manuallyExpanded: new Set([10]) }))
    expect(r.manuallyExpanded.has(10)).toBe(false)
  })
  it('toggling out of manuallyCollapsed clears it', () => {
    const r = resolveChevronToggle(10, true, sets({ manuallyCollapsed: new Set([10]) }))
    expect(r.manuallyCollapsed.has(10)).toBe(false)
  })
  it('toggling a transient expansion collapses it (removes from transient)', () => {
    const r = resolveChevronToggle(10, true, sets({ transientlyExpanded: new Set([10]) }))
    expect(r.transientlyExpanded.has(10)).toBe(false)
    expect(r.manuallyCollapsed.has(10)).toBe(false)
  })
  it('does not mutate the input sets', () => {
    const input = sets()
    resolveChevronToggle(10, true, input)
    expect(input.manuallyCollapsed.size).toBe(0)
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npm --prefix ui run test -- collapse`
Expected: FAIL - `resolveChevronToggle is not a function`.

- [ ] **Step 3: Write the minimal implementation**

Append to `ui/src/collapse.ts`:

```ts
export type ChevronToggleResult = CollapseSets

/**
 * Resolve a chevron click / Space toggle on the record header at `firstLine`.
 * Returns fresh Set instances (inputs are not mutated). `defaultExpanded` is
 * the record's mode-derived default with no overrides applied
 * (see `defaultExpandedFor`).
 *
 * Precedence matches the design spec's toggle table:
 *   in manuallyExpanded  -> remove (back to default)
 *   in manuallyCollapsed -> remove (back to default)
 *   in transientlyExpanded (and neither manual) -> remove (collapses)
 *   else default-expanded  -> add to manuallyCollapsed
 *   else default-collapsed -> add to manuallyExpanded
 */
export function resolveChevronToggle(
  firstLine: number,
  defaultExpanded: boolean,
  current: CollapseSets,
): ChevronToggleResult {
  const manuallyExpanded = new Set(current.manuallyExpanded)
  const manuallyCollapsed = new Set(current.manuallyCollapsed)
  const transientlyExpanded = new Set(current.transientlyExpanded)

  if (manuallyExpanded.has(firstLine)) {
    manuallyExpanded.delete(firstLine)
  } else if (manuallyCollapsed.has(firstLine)) {
    manuallyCollapsed.delete(firstLine)
  } else if (transientlyExpanded.has(firstLine)) {
    transientlyExpanded.delete(firstLine)
  } else if (defaultExpanded) {
    manuallyCollapsed.add(firstLine)
  } else {
    manuallyExpanded.add(firstLine)
  }
  return { manuallyExpanded, manuallyCollapsed, transientlyExpanded }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `npm --prefix ui run test -- collapse`
Expected: PASS (all collapse.test.ts cases green).

- [ ] **Step 5: Commit**

```bash
git add ui/src/collapse.ts ui/src/collapse.test.ts
git commit -m "Added the chevron toggle resolver covering all five record states with unit tests."
```

---

## Task 5: Rust persistence - extend `RestoredFile` and `Settings`

**Files:**
- Modify: `crates/clog-app/src/persistence.rs`

- [ ] **Step 1: Write the failing serde round-trip tests**

In `crates/clog-app/src/persistence.rs`, inside `mod thresholds_tests` (after the existing `restored_file_round_trips_thread_group_mask` test, around line 482), add:

```rust
    #[test]
    fn restored_file_defaults_collapse_fields_when_absent() {
        let raw = r#"{"path":"/x","scroll_top":0,"follow_tail":true,"level_mask":63,"thread_group_mask":63,"filter_text":"","search_mode":"smart","search_case_sensitive":false,"filter_mode":false,"bookmarks":[]}"#;
        let r: RestoredFile = serde_json::from_str(raw).expect("v1 RestoredFile decodes");
        assert_eq!(r.collapse_mode, "inherit");
        assert!(r.manually_expanded.is_empty());
        assert!(r.manually_collapsed.is_empty());
    }

    #[test]
    fn restored_file_round_trips_collapse_fields() {
        let r = RestoredFile {
            path: "/x".into(),
            scroll_top: 0.0,
            follow_tail: true,
            level_mask: 63,
            thread_group_mask: 0x3F,
            filter_text: String::new(),
            search_mode: "smart".into(),
            search_case_sensitive: false,
            filter_mode: false,
            bookmarks: vec![],
            collapse_mode: "errors".into(),
            manually_expanded: vec![3, 9, 12],
            manually_collapsed: vec![7],
        };
        let json = serde_json::to_string(&r).expect("serialises");
        let back: RestoredFile = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(back.collapse_mode, "errors");
        assert_eq!(back.manually_expanded, vec![3, 9, 12]);
        assert_eq!(back.manually_collapsed, vec![7]);
    }

    #[test]
    fn settings_defaults_collapse_records_default_to_none() {
        let raw = r#"{"schema":1,"theme":"dark","font_size":13,"recent_files":[],"follow_tail_default":true}"#;
        let s: Settings = serde_json::from_str(raw).expect("v1 settings decodes");
        assert_eq!(s.collapse_records_default, "none");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p clog-app collapse`
Expected: FAIL to compile - `RestoredFile` has no field `collapse_mode` / `Settings` has no field `collapse_records_default`.

- [ ] **Step 3: Add the `Settings` field**

In the `Settings` struct (after `mono_font_family`, around line 74), add:

```rust
    /// Global default collapse mode for multi-line records.
    /// `"none"` | `"errors"` | `"all"`. Default `"none"`.
    #[serde(default = "default_collapse_records_default")]
    pub collapse_records_default: String,
```

In `impl Default for Settings` (around line 90), add to the struct literal before the closing brace:

```rust
            collapse_records_default: default_collapse_records_default(),
```

Add the default helper next to the other `default_*` fns (around line 112):

```rust
fn default_collapse_records_default() -> String {
    "none".to_string()
}
```

- [ ] **Step 4: Add the `RestoredFile` fields**

In the `RestoredFile` struct (after `bookmarks`, around line 207), add:

```rust
    /// Per-file collapse mode: `"inherit"` | `"none"` | `"errors"` | `"all"`.
    /// Default `"inherit"` so a v1 session restores to "follow global".
    #[serde(default = "default_collapse_mode")]
    pub collapse_mode: String,
    /// Sorted, deduped header-row physical line indices forced open against
    /// the mode. Out-of-range entries are dropped UI-side on restore.
    #[serde(default)]
    pub manually_expanded: Vec<u64>,
    /// Sorted, deduped header-row physical line indices forced closed against
    /// the mode.
    #[serde(default)]
    pub manually_collapsed: Vec<u64>,
```

Add the default helper next to `default_smart` (around line 216):

```rust
fn default_collapse_mode() -> String {
    "inherit".to_string()
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p clog-app`
Expected: PASS (the three new tests plus all existing persistence tests green).

Note: the existing tests `restored_file_round_trips_thread_group_mask` and `restored_file_loads_old_payload_without_thread_group_mask` construct `RestoredFile` literals. The round-trip test builds a full literal (now needs the three new fields - already included in Step 1's literal). The `..._loads_old_payload...` test deserialises from JSON, so it is unaffected by the new `#[serde(default)]` fields. Confirm both still pass.

- [ ] **Step 6: Run lints**

Run: `cargo fmt --check; cargo clippy -p clog-app --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/clog-app/src/persistence.rs
git commit -m "Extended the persisted per-file record with collapse_mode plus the manual expand/collapse sets, and added a global collapse_records_default to Settings. Added serde round-trip and back-compat tests."
```

---

## Task 6: `Tab` state - collapse refs, helpers, record map, persistence wiring

**Files:**
- Modify: `ui/src/tab.ts`

This task adds all per-tab collapse state and the record map. No DOM. The visible-row index itself lives in `LogViewport` (Task 9) because it needs the virtualiser; `tab.ts` only owns the raw state and the record map.

- [ ] **Step 1: Add imports**

In `ui/src/tab.ts`, extend the `./types` import block (around line 21-46) to add `CollapseMode` and `RecordRef` (RecordRef is already imported - confirm; add `CollapseMode`):

```ts
  type CollapseMode,
```

- [ ] **Step 2: Add the four collapse refs**

In `createTab`, just after the `bookmarks` ref (around line 203), add:

```ts
  // --- Collapse records ---
  // Per-file mode. 'inherit' follows settings.collapse_records_default.
  const collapseMode = ref<CollapseMode>('inherit')
  // Header-row physical line indices the user forced open / closed against
  // the mode. Persisted. Kept as Sets for O(1) toggle/lookup; sorted+deduped
  // on snapshot().
  const manuallyExpanded = ref<Set<number>>(new Set<number>())
  const manuallyCollapsed = ref<Set<number>>(new Set<number>())
  // Header-row line indices auto-expanded by intent navigation. In-memory
  // only - navigation crumbs, not preferences.
  const transientlyExpanded = ref<Set<number>>(new Set<number>())

  // --- Record map (collapse + minimap need every record's span/level) ---
  // The full (first_line, line_count, level) list, fetched with full masks.
  // Refreshed on open, tail growth, rotation and pattern apply.
  const recordIndex = ref<RecordRef[]>([])
```

- [ ] **Step 3: Add the collapse helpers**

After the bookmark helpers (`prunedBookmarks`, around line 239), add:

```ts
  // Drop manual-set entries pointing past the current line_count (file
  // shrank, rotated, or restored against a smaller file). Sorted ascending.
  function prunedManualSet(set: Set<number>): number[] {
    const out: number[] = []
    const limit = file.value.line_count
    for (const idx of set) {
      if (idx >= 0 && idx < limit) out.push(idx)
    }
    out.sort((a, b) => a - b)
    return out
  }

  // Clear all collapse override state. Called on rotation and on mode change.
  function clearCollapseOverrides() {
    if (manuallyExpanded.value.size > 0) manuallyExpanded.value = new Set()
    if (manuallyCollapsed.value.size > 0) manuallyCollapsed.value = new Set()
    if (transientlyExpanded.value.size > 0) transientlyExpanded.value = new Set()
  }

  // Set the per-file mode. The mode is the new rule, so sticky overrides from
  // the previous regime are cleared (design spec: "Mode reset on per-file
  // mode change").
  function setCollapseMode(mode: CollapseMode) {
    collapseMode.value = mode
    clearCollapseOverrides()
  }

  // Fetch the full record map with full masks so collapse has every record's
  // span + level regardless of the active filter. Non-fatal on error.
  async function refreshRecordIndex(): Promise<void> {
    try {
      const payload = await invoke<RecordRefsPayload>('list_records_by_filters', {
        fileId: file.value.file_id,
        levelMask: 0xffffffff,
        threadGroupMask: 0x3f,
      })
      recordIndex.value = payload.refs
    } catch {
      // non-fatal -- keep the previous map; collapse falls back to identity
    }
  }
```

(`RecordRefsPayload` is already imported in tab.ts - confirm it is in the `./types` import list; it is.)

- [ ] **Step 4: Refresh the record map on tail growth and rotation**

In `handleTailDelta`, in the rotation branch (around line 321, right after `clearBookmarks()`), add:

```ts
      clearCollapseOverrides()
      void refreshRecordIndex()
```

In the same function, in the append branch (after `lastTailLineCount = delta.line_count`, around line 345), add:

```ts
    void refreshRecordIndex()
```

(Rationale: tail growth changes record spans / adds records; the minimap already refetches here via the viewport watcher, and the record map must track the same growth.)

- [ ] **Step 5: Refresh the record map on pattern apply**

In `applyPattern`, after `void fetchPage(0)` (around line 527), add:

```ts
      void refreshRecordIndex()
```

- [ ] **Step 6: Wire `snapshot()`**

In `snapshot()` (around line 557-570), add three fields to the returned object before the closing brace:

```ts
      collapse_mode: collapseMode.value,
      manually_expanded: prunedManualSet(manuallyExpanded.value),
      manually_collapsed: prunedManualSet(manuallyCollapsed.value),
```

- [ ] **Step 7: Wire `applyRestored()`**

In `applyRestored()` (around line 536-555), after the `bookmarks` restore block, add:

```ts
    collapseMode.value = r.collapse_mode ?? 'inherit'
    const limit = file.value.line_count
    const pruneIn = (arr: number[] | undefined): Set<number> => {
      const next = new Set<number>()
      if (Array.isArray(arr)) {
        for (const idx of arr) {
          if (Number.isFinite(idx) && idx >= 0 && idx < limit) next.add(idx)
        }
      }
      return next
    }
    manuallyExpanded.value = pruneIn(r.manually_expanded)
    manuallyCollapsed.value = pruneIn(r.manually_collapsed)
    transientlyExpanded.value = new Set() // never restored
```

- [ ] **Step 8: Export the new state and methods on the `api` object**

In the `api` object literal (around line 593-652), add under `// state` (next to `bookmarks`):

```ts
    collapseMode,
    manuallyExpanded,
    manuallyCollapsed,
    transientlyExpanded,
    recordIndex,
```

and under `// methods`:

```ts
    setCollapseMode,
    clearCollapseOverrides,
    refreshRecordIndex,
```

- [ ] **Step 9: Verify the UI typechecks**

Run: `npm --prefix ui run build`
Expected: build succeeds. (Nothing reads the new state yet; this is pure additive state.)

- [ ] **Step 10: Commit**

```bash
git add ui/src/tab.ts
git commit -m "Added per-tab collapse state (mode plus manual and transient expand/collapse sets) and the full record map, wired through snapshot, restore, rotation, tail growth and pattern apply."
```

---

## Task 7: `useSettings` default for the global collapse mode

**Files:**
- Modify: `ui/src/composables/useSettings.ts`

- [ ] **Step 1: Add the field to `defaultSettings()`**

In `ui/src/composables/useSettings.ts`, in `defaultSettings()` (around line 29-41), add before the closing brace:

```ts
    collapse_records_default: 'none',
```

(No per-field DOM apply is needed - `updateSettings(patch)` already forwards arbitrary patches to the backend and stores the returned `Settings`. The collapse default does not touch `<html>` tokens.)

- [ ] **Step 2: Verify the UI typechecks**

Run: `npm --prefix ui run build`
Expected: build succeeds.

- [ ] **Step 3: Commit**

```bash
git add ui/src/composables/useSettings.ts
git commit -m "Defaulted the new collapse_records_default setting to none in the UI settings factory."
```

---

## Task 8: Session autosave fingerprint

**Files:**
- Modify: `ui/src/composables/useSession.ts`

The autosave fires when the fingerprint string changes. The new persisted state (`collapseMode` + the two manual sets) must be in the fingerprint or a mode/override change will not trigger a save.

- [ ] **Step 1: Extend the fingerprint**

In `ui/src/composables/useSession.ts`, in the `watch` source mapper (around line 83), append to each tab's template string, right after the bookmarks segment (`bm:${t.bookmarks.value.size}`):

```ts
|cm:${t.collapseMode.value}|me:${t.manuallyExpanded.value.size}|mc:${t.manuallyCollapsed.value.size}
```

So the per-tab template gains `...|bm:${t.bookmarks.value.size}|cm:${t.collapseMode.value}|me:${t.manuallyExpanded.value.size}|mc:${t.manuallyCollapsed.value.size}` before the closing backtick.

(Set *size* is sufficient as a change signal, matching the existing `bm:` convention - any add/remove changes the size. A toggle that removes one and adds another in the same tick is not possible through the UI, so size is a safe fingerprint.)

- [ ] **Step 2: Verify the UI typechecks**

Run: `npm --prefix ui run build`
Expected: build succeeds.

- [ ] **Step 3: Commit**

```bash
git add ui/src/composables/useSession.ts
git commit -m "Added collapse mode and manual-set sizes to the session autosave fingerprint so collapse changes are persisted."
```

---

## Task 9: `LogViewport` integration - the core change

**Files:**
- Modify: `ui/src/components/LogViewport.vue`

This is the integration point. It is the largest task; do the steps in order and run `npm --prefix ui run build` after each script-level step to keep the type errors local.

### 9a: The combined visible-row index

- [ ] **Step 1: Add imports**

Extend the existing `'../types'` import (line 22-34) and add a `collapse` import. After the `import type { Tab } from '../tab'` line (line 35), add:

```ts
import {
  effectiveMode,
  isRecordExpanded,
  buildVisibleRowIndex,
  recordOfLine,
  resolveChevronToggle,
  defaultExpandedFor,
  type GlobalCollapseDefault,
} from '../collapse'
```

Add `RecordRef` to the `'../types'` import if not already present (it is, line 30).

- [ ] **Step 2: Add the effective-mode computed**

After the `settings` inject + derived computeds (around line 65), add:

```ts
const collapseDefault = computed<GlobalCollapseDefault>(() => {
  const v = settings?.value.collapse_records_default
  return v === 'errors' || v === 'all' ? v : 'none'
})
const collapseEffectiveMode = computed<GlobalCollapseDefault>(() =>
  effectiveMode(props.tab.collapseMode.value, collapseDefault.value),
)

// The record's collapse decision, reused by the index builder and the chevron.
function recordExpanded(rec: RecordRef): boolean {
  const tab = props.tab
  return isRecordExpanded(
    rec.record_first_line,
    rec.record_line_count,
    typeof rec.level === 'string' ? rec.level : String(rec.level),
    collapseEffectiveMode.value,
    {
      manuallyExpanded: tab.manuallyExpanded.value,
      manuallyCollapsed: tab.manuallyCollapsed.value,
      transientlyExpanded: tab.transientlyExpanded.value,
    },
  )
}
```

- [ ] **Step 3: Replace `filteredLineIndices` with the combined index**

The existing `filteredLineIndices` computed (lines 117-127) produces the filter-mode line array. Replace it with a computed that ALSO applies collapse, and keeps the identity fast-path. Replace lines 117-127 with:

```ts
// Does collapse currently hide ANY line? If not (mode resolves to none and no
// override hides anything), we can keep the identity fast-path and avoid
// materialising a per-line array on large files.
const collapseHidesSomething = computed<boolean>(() => {
  if (collapseEffectiveMode.value !== 'none') {
    // errors/all hide multi-line records unless every one is forced open.
    // Cheap conservative check: assume something is hidden if there is any
    // multi-line record. (A precise check would walk recordIndex; the array
    // walk in the builder is the same cost, so just assume true here.)
    return props.tab.recordIndex.value.some((r) => r.record_line_count > 1)
  }
  // none mode: only manuallyCollapsed can hide a record.
  return props.tab.manuallyCollapsed.value.size > 0
})

// The ordered record list to project: filter-passing subset in filter mode,
// else the full record map.
const projectionRecords = computed<RecordRef[] | null>(() => {
  const filt = filteredSourceRecords.value
  if (filt !== null) return filt
  // No filter active. Only build from the full map when collapse hides
  // something; otherwise signal identity by returning null.
  if (!collapseHidesSomething.value) return null
  return props.tab.recordIndex.value
})

// Combined virtual-row -> physical-line mapping (filter + collapse). Null =
// identity (virtual row == physical line == today's behaviour).
const visibleIndex = computed(() => {
  const recs = projectionRecords.value
  if (recs === null) return null
  return buildVisibleRowIndex(recs, recordExpanded)
})

const filteredLineIndices = computed<number[] | null>(
  () => visibleIndex.value?.visibleRowToLine ?? null,
)
```

Note: `filteredSourceRecords` (lines 100-115) is kept unchanged - it still produces the filter-passing record list. The only change is that collapse is layered on top in `visibleIndex`. Every existing reader of `filteredLineIndices` (effectiveCount, actualLineIndex, stickyHeader, jumpToStickyStart, scrollToCurrentHit, jumpToLine, markerVisuals, searchHitVisuals, bookmarkVisuals, buildFilteredMinimap path, tooltipTargetFromY) keeps working unchanged because the shape (`number[] | null`) is identical - they now simply see collapse-aware indices.

- [ ] **Step 4: Add a `lineToRow` accessor for auto-expand**

After the `visibleIndex` computed, add:

```ts
// Reverse map for "is this physical line currently visible?" checks. Null in
// identity mode (every line < line_count is visible).
const lineToRow = computed<Map<number, number> | null>(
  () => visibleIndex.value?.lineToRow ?? null,
)

function lineIsVisible(lineIdx: number): boolean {
  const map = lineToRow.value
  if (map === null) return lineIdx >= 0 && lineIdx < props.tab.file.value.line_count
  return map.has(lineIdx)
}
```

- [ ] **Step 5: Build and typecheck**

Run: `npm --prefix ui run build`
Expected: build succeeds. The viewport now renders collapse-aware rows for filter mode and (once a mode is set) collapsed records, but there is no chevron, no auto-expand, no badge yet.

- [ ] **Step 6: Manual smoke - mode flips fold records**

Run: `cargo dev`. Open `research/cheesecake-prod.log`. There is no UI control yet, so temporarily verify via the Vue devtools or by setting `collapseMode` - SKIP if no quick path; the controls land in Tasks 10-12 and the full smoke is Task 13. (This step is a checkpoint, not a hard gate.)

- [ ] **Step 7: Commit**

```bash
git add ui/src/components/LogViewport.vue
git commit -m "Composed collapse folding into the viewport's virtual-row index, layered over filter mode with an identity fast-path for the default none case."
```

### 9b: `revealLine` auto-expand for intent navigation

- [ ] **Step 8: Add the `revealLine` helper**

After `lineIsVisible` (from 9a Step 4), add:

```ts
// Auto-expand the record containing `lineIdx` if it is currently hidden inside
// a collapsed record. Single-shot: clears any prior transient expansion first
// so navigation does not leave a trail of opened records (manual expansions
// are untouched). Returns true if it changed the visible set.
function revealLine(lineIdx: number): boolean {
  if (lineIsVisible(lineIdx)) {
    // Target already visible; still clear stale transients so the previous
    // auto-open collapses back. Only if the target is not itself a transient.
    const tab = props.tab
    if (tab.transientlyExpanded.value.size > 0) {
      const rec = recordOfLine(tab.recordIndex.value, lineIdx)
      const keep = rec ? tab.transientlyExpanded.value.has(rec.record_first_line) : false
      if (!keep) tab.transientlyExpanded.value = new Set()
    }
    return false
  }
  const tab = props.tab
  const rec = recordOfLine(tab.recordIndex.value, lineIdx)
  if (!rec) return false
  // Manual expansions are sticky and not part of the transient sweep.
  const next = tab.manuallyExpanded.value.has(rec.record_first_line)
    ? new Set(tab.transientlyExpanded.value)
    : new Set<number>()
  next.add(rec.record_first_line)
  tab.transientlyExpanded.value = next
  return true
}
```

- [ ] **Step 9: Call `revealLine` from `scrollToCurrentHit`**

In `scrollToCurrentHit` (lines 299-318), after computing `targetLine = hitTargetLine(hit)` (line 305) and BEFORE reading `filt`/`filteredLineIndices.value`, add:

```ts
  revealLine(targetLine)
```

Because `revealLine` mutates `transientlyExpanded`, the `filteredLineIndices` computed re-derives synchronously, so the subsequent `filt.indexOf(targetLine)` finds the now-visible row. (Vue computeds are synchronous on read.)

- [ ] **Step 10: Call `revealLine` from `jumpToLine`**

In `jumpToLine` (lines 776-787), at the top of the function after the `const v = virtualizer.value; if (!v) return` guard, add:

```ts
  revealLine(lineIdx)
```

This covers bookmark-jump and insights-drawer entry clicks (both route through `jumpToLine` via the `@jump` wiring), satisfying the spec's auto-expand triggers.

- [ ] **Step 11: Build and typecheck**

Run: `npm --prefix ui run build`
Expected: build succeeds.

- [ ] **Step 12: Commit**

```bash
git add ui/src/components/LogViewport.vue
git commit -m "Added revealLine single-shot auto-expand and wired it into search-hit, bookmark and insights-drawer navigation."
```

### 9c: The chevron column (template + script + styles)

- [ ] **Step 13: Add the chevron toggle handler and per-row chevron state**

After the `recordExpanded` function (9a Step 2), add:

```ts
interface ChevronState {
  show: boolean        // multi-line record header -> render a chevron
  expanded: boolean    // current expanded state
  hiddenCount: number  // continuation lines hidden when collapsed (line_count - 1)
}

// Resolve the chevron for a header row at physical line `lineIdx`.
function chevronFor(row: LineRow | null, lineIdx: number): ChevronState {
  const blank: ChevronState = { show: false, expanded: true, hiddenCount: 0 }
  if (!row || row.line_within_record !== 0) return blank
  const rec = recordOfLine(props.tab.recordIndex.value, lineIdx)
  if (!rec || rec.record_line_count <= 1) return blank
  return {
    show: true,
    expanded: recordExpanded(rec),
    hiddenCount: rec.record_line_count - 1,
  }
}

// Toggle the record header at `lineIdx`. Mutates the manual/transient sets via
// the pure resolver.
function toggleCollapse(lineIdx: number) {
  const tab = props.tab
  const rec = recordOfLine(tab.recordIndex.value, lineIdx)
  if (!rec || rec.record_line_count <= 1) return
  const lvl = typeof rec.level === 'string' ? rec.level : String(rec.level)
  const def = defaultExpandedFor(rec.record_line_count, lvl, collapseEffectiveMode.value)
  const next = resolveChevronToggle(rec.record_first_line, def, {
    manuallyExpanded: tab.manuallyExpanded.value,
    manuallyCollapsed: tab.manuallyCollapsed.value,
    transientlyExpanded: tab.transientlyExpanded.value,
  })
  tab.manuallyExpanded.value = next.manuallyExpanded
  tab.manuallyCollapsed.value = next.manuallyCollapsed
  tab.transientlyExpanded.value = next.transientlyExpanded
}
```

- [ ] **Step 14: Insert the chevron cell into the virtual row template**

In the virtual-row `<div class="row" ...>` block (lines 1565-1612), between `<span class="gutter" />` (line 1594) and the `<span class="idx idx-interactive" ...>` (line 1595), insert:

```vue
            <span
              class="chevron"
              :class="{ 'is-clickable': chevronFor(lineRowVirtual(vrow.index), actualLineIndex(vrow.index)).show }"
              :title="chevronFor(lineRowVirtual(vrow.index), actualLineIndex(vrow.index)).show
                ? (chevronFor(lineRowVirtual(vrow.index), actualLineIndex(vrow.index)).expanded
                    ? 'Collapse record'
                    : `Expand record (+${chevronFor(lineRowVirtual(vrow.index), actualLineIndex(vrow.index)).hiddenCount} lines)`)
                : null"
              @click.stop="chevronFor(lineRowVirtual(vrow.index), actualLineIndex(vrow.index)).show && toggleCollapse(actualLineIndex(vrow.index))"
            >{{ chevronFor(lineRowVirtual(vrow.index), actualLineIndex(vrow.index)).show
                ? (chevronFor(lineRowVirtual(vrow.index), actualLineIndex(vrow.index)).expanded ? '▾' : '▸')
                : '' }}</span>
```

(`▾` = down chevron, `▸` = right chevron. The repeated `chevronFor(...)` calls are cheap - O(log records) binary search - but if review prefers, the row template can be refactored to a single per-row computed; kept inline here to avoid restructuring the existing `v-for` which calls `lineRowVirtual(vrow.index)` repeatedly already.)

- [ ] **Step 15: Insert an empty chevron cell into the sticky header row**

In the sticky-header `<div class="row is-header" ...>` block (lines 1536-1561), between `<span class="gutter" />` (line 1545) and the `<button class="idx jump-up" ...>` (line 1546), insert:

```vue
          <span
            class="chevron"
            :class="{ 'is-clickable': chevronFor(stickyHeader.row, stickyHeader.lineIndex).show }"
            :title="chevronFor(stickyHeader.row, stickyHeader.lineIndex).show
              ? (chevronFor(stickyHeader.row, stickyHeader.lineIndex).expanded ? 'Collapse record' : `Expand record (+${chevronFor(stickyHeader.row, stickyHeader.lineIndex).hiddenCount} lines)`)
              : null"
            @click.stop="chevronFor(stickyHeader.row, stickyHeader.lineIndex).show && toggleCollapse(stickyHeader.lineIndex)"
          >{{ chevronFor(stickyHeader.row, stickyHeader.lineIndex).show
              ? (chevronFor(stickyHeader.row, stickyHeader.lineIndex).expanded ? '▾' : '▸')
              : '' }}</span>
```

- [ ] **Step 16: Widen the row grid to four columns (scoped CSS)**

In `LogViewport.vue`'s scoped `<style>`, in `.row` (line 2166), change:

```css
    grid-template-columns: var(--gutter-width) var(--line-num-width) max-content;
```

to:

```css
    grid-template-columns: var(--gutter-width) var(--chevron-width) var(--line-num-width) max-content;
```

Then add a `.chevron` rule inside `.row` (after the `.gutter` rule, around line 2175):

```css
    .chevron {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      width: var(--chevron-width);
      height: 100%;
      font-size: 0.8em;
      color: color-mix(in srgb, var(--fg-row) 75%, transparent);
      user-select: none;

      &.is-clickable {
        cursor: pointer;
        &:hover { color: var(--fg-row); }
      }
    }
```

The sticky-header `.row` inherits the same `grid-template-columns` (it is `.sticky-shell .row`, which does not override the columns), so the empty chevron cell keeps the sticky header aligned with body rows automatically.

- [ ] **Step 17: Build and smoke**

Run: `npm --prefix ui run build`
Expected: build succeeds.
Run: `cargo dev`, open a file - rows now show an empty 14px column (mode still `inherit`->`none` by default, so no chevrons until a mode is set in Tasks 10-12).

- [ ] **Step 18: Commit**

```bash
git add ui/src/components/LogViewport.vue ui/src/style.css
git commit -m "Added the chevron column to the viewport rows and sticky header, with the toggle wired through the pure chevron resolver."
```

### 9d: The sticky `+N lines` badge

- [ ] **Step 19: Append the badge to collapsed header rows in the template**

In the virtual-row `<span class="txt">` block (lines 1603-1611), AFTER the closing `</span>` of the inner `v-for` span list but still inside `.txt`, add:

```vue
              <span
                v-if="!chevronFor(lineRowVirtual(vrow.index), actualLineIndex(vrow.index)).expanded && chevronFor(lineRowVirtual(vrow.index), actualLineIndex(vrow.index)).show"
                class="collapse-badge"
              >+{{ chevronFor(lineRowVirtual(vrow.index), actualLineIndex(vrow.index)).hiddenCount }} lines</span>
```

(The badge renders only when the record is collapsed - i.e. `show && !expanded`.)

- [ ] **Step 20: Add the badge styles (global, in `style.css`)**

The badge mirrors the existing `.is-marker::after` sticky pattern. Add to `ui/src/style.css` near the other axis-1/row span rules (after `.s-sep`, around line 520):

```css
/* Collapsed-record "+N lines" badge: muted italic, pinned to the right edge
   of the viewport via position: sticky so it stays visible as the user scrolls
   the collapsed header horizontally; it falls into normal flow at the message
   end. The ::before paints a short fade so message text doesn't collide with
   the badge's left edge. */
.collapse-badge {
  position: sticky;
  right: 0.4rem;
  margin-left: 0.6rem;
  padding: 0 0.3rem;
  font-style: italic;
  font-size: 0.85em;
  color: var(--fg-gutter);
  background: var(--bg-viewport);
  border-radius: 3px;
  pointer-events: none;
  white-space: nowrap;
}
.collapse-badge::before {
  content: '';
  position: absolute;
  left: -1.2rem;
  top: 0;
  bottom: 0;
  width: 1.2rem;
  background: linear-gradient(to right, transparent, var(--bg-viewport));
  pointer-events: none;
}
```

Note on the fade background var: the spec calls for the fade to track the row background (including hover / current-hit / bookmark rows). Those row backgrounds are gradients painted on `.row` in the viewport's scoped CSS, not a single token. To keep this v1-simple and robust, the badge uses the base `--bg-viewport` (the dominant background); a follow-up can introduce a `--row-bg` custom property set per-row if the fade looks wrong over a coloured row. Record this as a known cosmetic limitation rather than blocking the slice.

- [ ] **Step 21: Build and commit**

Run: `npm --prefix ui run build`
Expected: build succeeds.

```bash
git add ui/src/components/LogViewport.vue ui/src/style.css
git commit -m "Added the sticky +N lines badge on collapsed record headers with a left-edge fade."
```

### 9e: `Space` keyboard toggle + tail-follow over visible count

- [ ] **Step 22: Handle `Space` in the existing document keydown listener**

In `onDocumentKey` (lines 959-961), extend it:

```ts
function onDocumentKey(ev: KeyboardEvent) {
  if (ev.key === 'Escape') closeClusterPopover()
  if (ev.key === ' ' || ev.key === 'Spacebar') {
    // Only when focus is inside this viewport and not in an input/textarea
    // (so Space still types in the search box and the pattern editor).
    const active = document.activeElement as HTMLElement | null
    const el = scrollEl.value
    if (!el) return
    const inViewport = el === active || el.contains(active)
    const tag = active?.tagName
    const editable = tag === 'INPUT' || tag === 'TEXTAREA' || active?.isContentEditable
    if (!inViewport || editable) return
    const sticky = stickyHeader.value
    if (!sticky) return
    ev.preventDefault()
    toggleCollapse(sticky.lineIndex)
  }
}
```

For the viewport to receive focus, make the scroll element focusable. In the template, on `<div ref="scrollEl" class="viewport" ...>` (line 1534), add `tabindex="0"`:

```vue
    <div ref="scrollEl" class="viewport" tabindex="0" @scroll.passive="onViewportScroll">
```

And suppress the default focus outline (scoped CSS, in `.viewport`, around line 2086):

```css
  &:focus-visible { outline: none; }
```

- [ ] **Step 23: Confirm tail-follow already uses the visible count**

`jumpToBottom` (lines 252-260) and `followTail` proximity logic (lines 242-249) already key off `effectiveCount.value` / `el.scrollHeight`, both of which derive from `filteredLineIndices` -> now the collapse-aware index. No change needed. Verify by reading: `effectiveCount` (line 129) returns `filteredLineIndices.value?.length ?? line_count`, which is now collapse-aware. A new ERROR appended during tail in `'errors'` mode lands collapsed because `recordExpanded` consults the live mode. (The `refreshRecordIndex` call added in Task 6 Step 4 keeps the record map current so the new record is in `recordIndex`.)

- [ ] **Step 24: Build and commit**

Run: `npm --prefix ui run build`
Expected: build succeeds.

```bash
git add ui/src/components/LogViewport.vue
git commit -m "Added Space-to-toggle the record under the sticky header, gated to viewport focus, and made the viewport focusable."
```

### 9f: Fetch the record map on mount

- [ ] **Step 25: Kick the initial record-map fetch**

In `onMounted` (lines 1319-1379), after the initial page fetch block (around line 1327), add:

```ts
  void props.tab.refreshRecordIndex()
```

(On a fresh open the tab's `recordIndex` is empty; this populates it. Tail/rotation/pattern refreshes are wired in Task 6.)

- [ ] **Step 26: Build and commit**

Run: `npm --prefix ui run build`
Expected: build succeeds.

```bash
git add ui/src/components/LogViewport.vue
git commit -m "Fetched the full record map on viewport mount so collapse has every record's span and level."
```

---

## Task 10: FiltersPopover - per-file segmented control

**Files:**
- Modify: `ui/src/components/FiltersPopover.vue`

- [ ] **Step 1: Add the collapse-mode constant and handler in the script**

In `ui/src/components/FiltersPopover.vue` `<script setup>`, after the imports (line 16) add:

```ts
import { computed } from 'vue'
import type { CollapseMode } from '../types'
import { effectiveMode } from '../collapse'

const COLLAPSE_OPTIONS: CollapseMode[] = ['inherit', 'none', 'errors', 'all']
const COLLAPSE_LABEL: Record<CollapseMode, string> = {
  inherit: 'Inherit',
  none: 'None',
  errors: 'Errors',
  all: 'All',
}

const settings = inject<Ref<Settings> | null>('settings', null)
const inheritedMode = computed(() => {
  const def = settings?.value.collapse_records_default
  return def === 'errors' || def === 'all' ? def : 'none'
})
const effectiveLabel = computed(() =>
  COLLAPSE_LABEL[effectiveMode('inherit', inheritedMode.value)],
)

function setCollapseMode(mode: CollapseMode) {
  props.tab.setCollapseMode(mode)
}
```

Add `inject` and `Ref` to the existing `vue` import (line 7), and add `Settings` to the `../types` import (line 8-14):

```ts
import { computed, inject, onBeforeUnmount, onMounted, ref, type Ref } from 'vue'
```

(`Settings` and `CollapseMode` join the `../types` import block.)

- [ ] **Step 2: Add the segmented-control section to the template**

In the template, after the "Threads" `</section>` (line 107) and before `<footer class="filters-footer">` (line 108), insert:

```vue
    <section class="filters-section">
      <h4 class="filters-heading">Collapse records</h4>
      <div class="filters-row collapse-seg">
        <button
          v-for="opt in COLLAPSE_OPTIONS"
          :key="opt"
          type="button"
          class="filter-pill"
          :class="{ 'is-on': tab.collapseMode.value === opt }"
          @click="setCollapseMode(opt)"
        >{{ COLLAPSE_LABEL[opt] }}</button>
      </div>
      <p v-if="tab.collapseMode.value === 'inherit'" class="collapse-hint">
        Inheriting global default (currently "{{ effectiveLabel }}")
      </p>
    </section>
```

- [ ] **Step 3: Add the active-button + hint styles (scoped)**

In FiltersPopover's scoped `<style>`, after the `.filter-pill` rule (around line 164), add:

```css
.filter-pill.is-on {
  background: var(--accent);
  color: var(--fg-on-accent);
  border-color: var(--accent);
}

.collapse-hint {
  margin: 0.3rem 0 0;
  font-size: 0.72rem;
  color: var(--fg-muted);
  font-style: italic;
}
```

- [ ] **Step 4: Build and smoke**

Run: `npm --prefix ui run build`
Expected: build succeeds.
Run: `cargo dev`, open a file, open the filters popover - the "Collapse records" segmented control appears; clicking Errors/All folds matching multi-line records; Inherit shows the hint. Confirm switching modes clears any manual overrides (chevron-collapse a record, then flip mode - it returns to the mode default).

- [ ] **Step 5: Commit**

```bash
git add ui/src/components/FiltersPopover.vue
git commit -m "Added the per-file collapse-records segmented control and the inherit hint to the filters popover."
```

---

## Task 11: SettingsModal - global default segmented control

**Files:**
- Modify: `ui/src/components/SettingsModal.vue`

- [ ] **Step 1: Add the control to the Behaviour tab**

In `ui/src/components/SettingsModal.vue`, after the "Follow tail by default" `.row-grid` block (ends line 435) and before the section's closing tag (line 437), insert (mirroring the Theme segmented control at lines 306-316):

```vue
      <div class="row-grid">
        <span class="row-label">Collapse records by default</span>
        <span class="seg">
          <button
            v-for="opt in (['none', 'errors', 'all'] as const)"
            :key="opt"
            type="button"
            class="seg-btn"
            :class="{ 'is-on': (settings.collapse_records_default ?? 'none') === opt }"
            @click="emit('update', { collapse_records_default: opt })"
          >{{ opt[0].toUpperCase() + opt.slice(1) }}</button>
        </span>
      </div>
      <p class="hint-row">
        Multi-line records are folded to just their header line. Per-file
        overrides live in the filters popover.
      </p>
```

(Confirm a `.hint-row` style exists; if not, add `.hint-row { grid-column: 1 / -1; margin: 0.2rem 0 0; font-size: 0.78rem; color: var(--fg-muted); }` to the scoped styles. Check the existing modal for a hint/help class first and reuse it.)

- [ ] **Step 2: Build and smoke**

Run: `npm --prefix ui run build`
Expected: build succeeds.
Run: `cargo dev`, open Settings -> Behaviour. The "Collapse records by default" None/Errors/All control appears and persists (reopen the modal - the choice sticks). A tab on `inherit` follows it live.

- [ ] **Step 3: Commit**

```bash
git add ui/src/components/SettingsModal.vue
git commit -m "Added the global collapse-records-default control to the Settings Behaviour tab."
```

---

## Task 12: TabStrip - context-menu submenu

**Files:**
- Modify: `ui/src/components/TabStrip.vue`

`useContextMenu` is a shared singleton composable exposing `show(ev, items)` and a `MenuItem` union that includes `kind: 'submenu'` (with `children`) and `kind: 'action'`. The same host that renders the row context menu renders this one.

- [ ] **Step 1: Add the context-menu handler in the script**

In `ui/src/components/TabStrip.vue` `<script setup>`, after the imports (line 18), add:

```ts
import { useContextMenu, type MenuItem } from '../composables/useContextMenu'
import type { CollapseMode } from '../types'

const { show: showContextMenu } = useContextMenu()

const COLLAPSE_ITEMS: { mode: CollapseMode; label: string }[] = [
  { mode: 'inherit', label: 'Inherit' },
  { mode: 'none', label: 'None' },
  { mode: 'errors', label: 'Errors' },
  { mode: 'all', label: 'All' },
]

function onTabContextMenu(ev: MouseEvent, tab: Tab) {
  ev.preventDefault()
  const items: MenuItem[] = [
    {
      kind: 'submenu',
      label: 'Collapse records',
      children: COLLAPSE_ITEMS.map((opt) => ({
        kind: 'action' as const,
        // A leading check marks the active per-file value.
        label: `${tab.collapseMode.value === opt.mode ? '✓ ' : ' '}${opt.label}`,
        onSelect: () => tab.setCollapseMode(opt.mode),
      })),
    },
  ]
  showContextMenu({ clientX: ev.clientX, clientY: ev.clientY }, items)
}
```

(`✓` = check mark, ` ` = em-space for alignment on unchecked items.)

- [ ] **Step 2: Wire the `@contextmenu` on the tab element**

On the `<li class="tab" ...>` element (lines 150-164), add a `@contextmenu` handler to the existing `@mousedown`:

```vue
        @mousedown="onMiddleClick($event, t.localId); onTabMouseDown($event, t.localId)"
        @contextmenu="onTabContextMenu($event, t)"
```

- [ ] **Step 3: Build and smoke**

Run: `npm --prefix ui run build`
Expected: build succeeds.
Run: `cargo dev`, right-click a tab -> "Collapse records" submenu with Inherit/None/Errors/All and a check on the current value. Selecting an item folds/unfolds and matches the filters-popover behaviour (clears manual overrides).

If the submenu does not render, confirm `useContextMenu()` returns the shared singleton state that the global `ContextMenu` host (mounted in App.vue) reads; if it is per-call state, route the menu through an App.vue-level handler instead (emit a `context-menu` event up, as TabStrip already emits other events). Verify before assuming.

- [ ] **Step 4: Commit**

```bash
git add ui/src/components/TabStrip.vue
git commit -m "Added a right-click Collapse records submenu to tabs with a check on the active per-file mode."
```

---

## Task 13: Full verification and manual smoke

**Files:** none (verification only).

- [ ] **Step 1: Run the full UI unit suite**

Run: `npm --prefix ui run test`
Expected: PASS - the existing 14+ cases plus the new `collapse.test.ts` cases all green.

- [ ] **Step 2: Run the full Rust suite**

Run: `cargo test --workspace`
Expected: PASS - existing tests plus the three new persistence tests green.

- [ ] **Step 3: Run lints**

Run: `cargo fmt --check`
Run: `cargo clippy --workspace --all-targets -- -D warnings`
Run: `npm --prefix ui run build`
Expected: all clean.

- [ ] **Step 4: Manual smoke against the prod fixture**

Run: `cargo dev`. Open `research/cheesecake-prod.log` (74,921 lines). Verify each spec behaviour:

1. Default (mode none): no chevrons hide anything; viewport identical to before.
2. Settings -> Behaviour -> Collapse records by default -> Errors: multi-line ERROR/FATAL records fold to their header with a `+N lines` badge and a right chevron; WARN/INFO/Unknown stay expanded.
3. All: every multi-line record folds (incl. Unknown).
4. Chevron click toggles a single record; the badge appears/disappears.
5. `Space` with the viewport focused toggles the record under the sticky header; `Space` in the search box still types a space.
6. Search for a string that lives only inside a collapsed stack -> the containing record auto-expands and centres; pressing next-hit to a different collapsed record collapses the first (single-shot transient) and expands the second.
7. Bookmark a line inside a collapsed record, jump to it from the marker rail -> auto-expands.
8. Per-file override via the filters popover and via right-click on the tab; "Inherit" shows "(currently X)".
9. Flip the per-file mode -> manual chevron overrides clear.
10. Close and reopen the app -> `collapseMode` + manual sets restore; transient expansions are gone.
11. Tail a growing file (use `cargo run --example fake_tailer --rotate` against a temp file, or append to a copy) in Errors mode -> a new multi-line ERROR lands collapsed; rotation clears collapse state alongside bookmarks.

- [ ] **Step 5: Final commit (if any smoke fixes were needed)**

```bash
git add -A
git commit -m "Tidied collapse-records behaviour after manual smoke against the prod fixture."
```

---

## Self-review notes (for the implementer)

- **Spec coverage map:** Data model -> Tasks 1,5,6. Effective-mode + predicate -> Task 2. Visible-row index (Approach 1) -> Task 3 + 9a. Chevron column -> 9c. Sticky badge -> 9d. Mode reset on change -> Task 6 (`setCollapseMode` clears sets). Sticky header -> 9a (unchanged read path) + 9c (empty cell). Tail follow -> 9e Step 23. Global control -> Task 11. Per-file popover -> Task 10. Per-file context menu -> Task 12. Keyboard `Space` -> 9e. Chevron toggle paths -> Task 4. Auto-expand triggers -> 9b. Persistence + prune + rotation clear -> Tasks 5,6. Tests -> Tasks 2-5 + 13.

- **Deliberately NOT auto-expanded** (per spec): minimap click, scroll, RecordModal open, bookmark add/remove on idx. These route through `onMinimapPointerDown` / `scrollToMinimapY` / `onIdxClick`, none of which call `revealLine` - confirm none were accidentally wired.

- **Known v1 cosmetic limitations** (recorded in the spec's own non-goals / edge cases, accepted here): the `+N lines` badge fade uses `--bg-viewport` rather than a per-row background var, so over a coloured (hover/current-hit/bookmark) row the fade is approximate; the minimap stays line-indexed (no fold treatment); the `· N hits` suffix on collapsed headers with hidden hits (spec "Edge cases" last bullet) is **not** in this slice - add as a fast-follow if the search-into-collapsed flow needs it (the auto-expand already surfaces the hit, so it is non-blocking).

- **Performance:** the identity fast-path (9a Step 3) keeps the default `none` non-filter case allocation-free on the 75k-line fixture. When a mode is active, `buildVisibleRowIndex` is O(visible rows) and rebuilds on mode/override/record-map change - sub-millisecond per the spec's estimate; confirm no jank on the prod fixture during Task 13 smoke.
