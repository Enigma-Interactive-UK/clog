import { describe, it, expect, beforeAll } from 'vitest'
import defaultRulesFile from './default-rules.json'
import {
  computeHighlights,
  highlightsFor,
  overlay,
  rulesVersion,
  setRules,
  type HighlightRulesFile,
  type HighlightSpan,
} from './engine'

const defaults = (defaultRulesFile as HighlightRulesFile).rules

beforeAll(() => {
  setRules(defaults)
})

function findCls(spans: HighlightSpan[], cls: string): HighlightSpan | undefined {
  return spans.find((s) => s.cls === cls)
}

describe('default highlight rules', () => {
  it('flags Java exception class names', () => {
    const line = 'java.lang.IllegalStateException: not now'
    const spans = computeHighlights(line)
    const hit = findCls(spans, 'h-exception')
    expect(hit).toBeDefined()
    expect(line.slice(hit!.start, hit!.end)).toBe('IllegalStateException')
  })

  it('flags Caused by:', () => {
    const line = 'Caused by: java.lang.RuntimeException: boom'
    const spans = computeHighlights(line)
    expect(findCls(spans, 'h-caused-by')).toBeDefined()
  })

  it('splits a stack frame into fqn / file / line sub-spans', () => {
    const line = '\tat com.example.Foo.bar(Foo.java:42)'
    const spans = computeHighlights(line)
    const fqn = findCls(spans, 'h-stack-fqn')
    const file = findCls(spans, 'h-stack-file')
    const lineNo = findCls(spans, 'h-stack-line')
    expect(fqn).toBeDefined()
    expect(file).toBeDefined()
    expect(lineNo).toBeDefined()
    expect(line.slice(fqn!.start, fqn!.end)).toBe('com.example.Foo.bar')
    expect(line.slice(file!.start, file!.end)).toBe('Foo.java')
    expect(line.slice(lineNo!.start, lineNo!.end)).toBe('42')
  })

  it('detects URLs and carries the URL through to the span', () => {
    const line = 'see https://example.com/docs?x=1 for details'
    const spans = computeHighlights(line)
    const url = findCls(spans, 'h-url')
    expect(url).toBeDefined()
    expect(url!.url).toBe('https://example.com/docs?x=1')
  })

  it('detects Windows paths', () => {
    const line = 'wrote C:\\Users\\foo\\app.log successfully'
    const spans = computeHighlights(line)
    expect(findCls(spans, 'h-path')).toBeDefined()
  })

  it('detects Unix paths', () => {
    const line = 'reading /var/log/app.log now'
    const spans = computeHighlights(line)
    const path = findCls(spans, 'h-path')
    expect(path).toBeDefined()
    expect(line.slice(path!.start, path!.end)).toBe('/var/log/app.log')
  })

  it('returns no spans for empty text', () => {
    expect(computeHighlights('')).toEqual([])
  })

  it('does not produce overlapping spans', () => {
    const line = 'at com.acme.MyException.bar(MyException.java:10) caused IllegalStateException'
    const spans = computeHighlights(line)
    for (let i = 1; i < spans.length; i++) {
      expect(spans[i].start).toBeGreaterThanOrEqual(spans[i - 1].end)
    }
  })
})

describe('engine cache and versioning', () => {
  it('returns the same array reference on a repeat call', () => {
    setRules(defaults)
    const line = 'java.lang.RuntimeException at start'
    const a = highlightsFor(line)
    const b = highlightsFor(line)
    expect(a).toBe(b)
  })

  it('bumps the version when rules are replaced', () => {
    const v1 = rulesVersion()
    setRules(defaults)
    expect(rulesVersion()).toBe(v1 + 1)
  })

  it('returns no spans when no rules are loaded', () => {
    setRules([])
    expect(computeHighlights('any text here')).toEqual([])
    setRules(defaults)
  })
})

describe('overlay', () => {
  it('keeps axis-1 spans intact when no axis-2 hits', () => {
    const text = '[INFO] hello world'
    const base = [
      { start: 0, end: 6, cls: 'level' },
      { start: 6, end: text.length, cls: 'message' },
    ]
    const leaves = overlay(text, base, [])
    expect(leaves.map((l) => l.text).join('')).toBe(text)
    expect(leaves.map((l) => l.cls)).toEqual(['s-level', 's-message'])
  })

  it('splits axis-1 spans where axis-2 spans land', () => {
    const text = 'msg java.lang.NullPointerException tail'
    const base = [{ start: 0, end: text.length, cls: 'message' }]
    setRules(defaults)
    const axis2 = computeHighlights(text)
    const leaves = overlay(text, base, axis2)
    // Concatenation must equal the original text exactly.
    expect(leaves.map((l) => l.text).join('')).toBe(text)
    // At least one leaf must combine axis-1 + axis-2 class names.
    expect(leaves.some((l) => l.cls.includes('s-message') && l.cls.includes('h-exception'))).toBe(
      true,
    )
  })

  it('preserves URL on the overlaid leaf', () => {
    const text = 'see https://x.test for more'
    const base = [{ start: 0, end: text.length, cls: 'message' }]
    setRules(defaults)
    const axis2 = computeHighlights(text)
    const leaves = overlay(text, base, axis2)
    const urlLeaf = leaves.find((l) => l.cls.includes('h-url'))
    expect(urlLeaf?.url).toBe('https://x.test')
  })
})
