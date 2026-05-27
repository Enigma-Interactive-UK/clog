import { describe, it, expect } from 'vitest'
import { decideZenKeyAction } from './useZenMode'

describe('decideZenKeyAction', () => {
  it('toggles on F11 when no input is focused and zen is off', () => {
    expect(decideZenKeyAction({ key: 'F11', zen: false, inputFocused: false })).toBe('toggle')
  })

  it('toggles on F11 when no input is focused and zen is on', () => {
    expect(decideZenKeyAction({ key: 'F11', zen: true, inputFocused: false })).toBe('toggle')
  })

  it('toggles on F11 even when an input is focused (F11 is a non-text shortcut)', () => {
    expect(decideZenKeyAction({ key: 'F11', zen: false, inputFocused: true })).toBe('toggle')
  })

  it('exits on Escape when zen is on and no input is focused', () => {
    expect(decideZenKeyAction({ key: 'Escape', zen: true, inputFocused: false })).toBe('exit')
  })

  it('does nothing on Escape when an input is focused (defers to input blur)', () => {
    expect(decideZenKeyAction({ key: 'Escape', zen: true, inputFocused: true })).toBe('noop')
  })

  it('does nothing on Escape when zen is off', () => {
    expect(decideZenKeyAction({ key: 'Escape', zen: false, inputFocused: false })).toBe('noop')
  })

  it('does nothing on unrelated keys', () => {
    expect(decideZenKeyAction({ key: 'a', zen: true, inputFocused: false })).toBe('noop')
    expect(decideZenKeyAction({ key: 'F1', zen: true, inputFocused: false })).toBe('noop')
  })
})
