import { describe, it, expect } from 'vitest'

// Mirror of the snapshot/restore pruning rules in tab.ts so the boundary
// behaviour is locked without standing up a full Tab + Tauri invoke mock.
function pruneBefore(value: number | null, lineCount: number): number | null {
  return value !== null && value >= 0 && value < lineCount ? value : null
}
function pruneAfter(value: number | null, lineCount: number): number | null {
  return value !== null && value > 0 && value <= lineCount ? value : null
}

describe('truncate snapshot/restore pruning', () => {
  it('keeps in-range bounds', () => {
    expect(pruneBefore(10, 100)).toBe(10)
    expect(pruneAfter(90, 100)).toBe(90)
  })
  it('drops a before-cut at or past line_count', () => {
    expect(pruneBefore(100, 100)).toBeNull()
    expect(pruneBefore(150, 100)).toBeNull()
  })
  it('keeps an after-cut exactly at line_count', () => {
    expect(pruneAfter(100, 100)).toBe(100)
  })
  it('drops an after-cut past line_count', () => {
    expect(pruneAfter(101, 100)).toBeNull()
  })
  it('drops null', () => {
    expect(pruneBefore(null, 100)).toBeNull()
    expect(pruneAfter(null, 100)).toBeNull()
  })
})
