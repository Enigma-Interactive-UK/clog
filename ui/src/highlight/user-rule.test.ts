import { describe, it, expect } from 'vitest'

import { computeWith, overlay, setRules, computeHighlights } from './engine'
import { composeEffectiveRules, userToEngineRule } from './user-rule'
import { newUserRule, type UserHighlightRule } from '../types'

function makeUserRule(overrides: Partial<UserHighlightRule>): UserHighlightRule {
  return { ...newUserRule(), ...overrides }
}

describe('userToEngineRule', () => {
  it('maps palette colour to h-user-<colour> class', () => {
    const r = userToEngineRule(makeUserRule({ colour: 'amber', pattern: 'foo' }))
    expect(r.cls).toBe('h-user-amber')
  })

  it('composes bold/italic/underline classes alongside the colour', () => {
    const r = userToEngineRule(makeUserRule({
      colour: 'blue',
      bold: true,
      italic: true,
      underline: true,
      pattern: 'x',
    }))
    expect(r.cls).toBe('h-user-blue h-user-bold h-user-italic h-user-underline')
  })

  it('maps background palette key to h-user-bg-<colour>', () => {
    const r = userToEngineRule(makeUserRule({
      colour: 'amber',
      background: 'red',
      pattern: 'x',
    }))
    expect(r.cls).toBe('h-user-amber h-user-bg-red')
  })

  it('emits background-only cls when fg is empty', () => {
    const r = userToEngineRule(makeUserRule({
      colour: '',
      background: 'cyan',
      pattern: 'x',
    }))
    expect(r.cls).toBe('h-user-bg-cyan')
  })

  it('emits no cls when colour is empty and no font styles set', () => {
    const r = userToEngineRule(makeUserRule({ colour: '', pattern: 'x' }))
    expect(r.cls).toBeUndefined()
  })

  it('emits style-only cls when colour is empty but bold is on', () => {
    const r = userToEngineRule(makeUserRule({ colour: '', bold: true, pattern: 'x' }))
    expect(r.cls).toBe('h-user-bold')
  })
})

describe('composeEffectiveRules', () => {
  it('keeps defaults and appends enabled user rules in order', () => {
    const defaults = [{ name: 'd1', pattern: 'foo', priority: 10, cls: 'h-d1' }]
    const global = [makeUserRule({ name: 'g1', pattern: 'bar', colour: 'red' })]
    const perFile = [makeUserRule({ name: 'p1', pattern: 'baz', colour: 'green' })]
    const out = composeEffectiveRules(defaults, global, perFile)
    expect(out.map((r) => r.name)).toEqual(['d1', 'g1', 'p1'])
  })

  it('skips disabled and empty-pattern rules', () => {
    const out = composeEffectiveRules(
      [],
      [
        makeUserRule({ name: 'a', pattern: 'x', enabled: false, colour: 'red' }),
        makeUserRule({ name: 'b', pattern: '', colour: 'red' }),
        makeUserRule({ name: 'c', pattern: 'y', colour: 'red' }),
      ],
      [],
    )
    expect(out.map((r) => r.name)).toEqual(['c'])
  })
})

describe('end-to-end: user rule produces rendered class on the leaf span', () => {
  it('paints the user colour class onto the matching text via computeHighlights + overlay', () => {
    const user = makeUserRule({
      name: 'foundation',
      pattern: '\\bFoundation\\b',
      colour: 'amber',
      bold: true,
      priority: 100,
    })
    setRules(composeEffectiveRules([], [user], []))

    const text = 'app started in Foundation mode'
    const axis2 = computeHighlights(text)
    // The engine should produce a single span covering "Foundation" with
    // the composed user classes.
    expect(axis2.length).toBeGreaterThan(0)
    const hit = axis2.find((s) => text.slice(s.start, s.end) === 'Foundation')
    expect(hit, 'no axis-2 span found over "Foundation"').toBeDefined()
    expect(hit!.cls).toBe('h-user-amber h-user-bold')

    // After overlay, the leaf for "Foundation" should still carry the user
    // classes alongside the structural base class.
    const base = [{ start: 0, end: text.length, cls: 'message' }]
    const leaves = overlay(text, base, axis2)
    const foundationLeaf = leaves.find((l) => l.text === 'Foundation')
    expect(foundationLeaf, 'no leaf produced for "Foundation"').toBeDefined()
    expect(foundationLeaf!.cls.split(' ')).toEqual(
      expect.arrayContaining(['s-message', 'h-user-amber', 'h-user-bold']),
    )
  })

  it('computeWith (preview path) yields the same user classes without touching the global engine', () => {
    const user = makeUserRule({
      name: 'errword',
      pattern: 'ERROR',
      colour: 'red',
      priority: 50,
    })
    const result = computeWith([userToEngineRule(user)], 'oh no ERROR happened')
    expect(result.ok).toBe(true)
    if (!result.ok) return
    const hit = result.spans.find((s) => s.cls.includes('h-user-red'))
    expect(hit).toBeDefined()
  })
})
