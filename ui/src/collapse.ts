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
