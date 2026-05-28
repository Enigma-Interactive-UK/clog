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
