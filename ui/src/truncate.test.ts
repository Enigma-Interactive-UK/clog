import { describe, it, expect } from 'vitest'

// Import the real pruning rules from tab.ts so the boundary behaviour is
// locked against the single definition that snapshot and restore also use.
import { pruneTruncateBefore, pruneTruncateAfter } from './tab'

describe('truncate snapshot/restore pruning', () => {
  it('keeps in-range bounds', () => {
    expect(pruneTruncateBefore(10, 100)).toBe(10)
    expect(pruneTruncateAfter(90, 100)).toBe(90)
  })
  it('drops a before-cut at or past line_count', () => {
    expect(pruneTruncateBefore(100, 100)).toBeNull()
    expect(pruneTruncateBefore(150, 100)).toBeNull()
  })
  it('keeps an after-cut exactly at line_count', () => {
    expect(pruneTruncateAfter(100, 100)).toBe(100)
  })
  it('drops an after-cut past line_count', () => {
    expect(pruneTruncateAfter(101, 100)).toBeNull()
  })
  it('drops null', () => {
    expect(pruneTruncateBefore(null, 100)).toBeNull()
    expect(pruneTruncateAfter(null, 100)).toBeNull()
  })
})
