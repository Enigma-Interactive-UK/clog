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

// Mirror of the context-menu enable/disable rules in onRowContextMenu.
function truncateMenu(
  recBefore: number,
  recAfter: number,
  tb: number | null,
  ta: number | null,
) {
  const out: Array<{ label: string; disabled: boolean }> = []
  if (tb === null) out.push({ label: 'Truncate before', disabled: ta !== null && recBefore >= ta })
  if (ta === null) out.push({ label: 'Truncate after', disabled: tb !== null && recAfter <= tb })
  return out
}

describe('truncate context menu', () => {
  it('offers both when no cuts exist', () => {
    const m = truncateMenu(10, 14, null, null)
    expect(m.map((i) => i.label)).toEqual(['Truncate before', 'Truncate after'])
    expect(m.every((i) => !i.disabled)).toBe(true)
  })
  it('hides before when a before-cut exists', () => {
    const m = truncateMenu(10, 14, 5, null)
    expect(m.map((i) => i.label)).toEqual(['Truncate after'])
  })
  it('disables a before-cut that would invert the window', () => {
    const m = truncateMenu(50, 54, null, 40)
    expect(m[0]).toEqual({ label: 'Truncate before', disabled: true })
  })
  it('disables an after-cut that would invert the window', () => {
    const m = truncateMenu(10, 14, 20, null)
    expect(m[0]).toEqual({ label: 'Truncate after', disabled: true })
  })
})
