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
