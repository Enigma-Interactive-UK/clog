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
