// Axis-2 highlight engine. Runs JS-side over visible-line text. Designed to
// be paired with axis-1 structural slicing in the renderer: each engine pass
// produces an ordered, non-overlapping list of highlight spans which is then
// overlaid onto the axis-1 spans by the renderer.

import { ref } from 'vue'

/**
 * Reactive version counter. Bumped on every `setRules()` call so any Vue
 * template that reads it picks up rule changes immediately (the engine's
 * line cache is keyed on the same number, so `highlightsFor` returns fresh
 * spans after the bump). Read `rulesVersionRef.value` inside a render
 * function to register the dep.
 */
export const rulesVersionRef = ref(0)

export interface HighlightSubgroup {
  cls?: string
  priority?: number
}

export interface HighlightRule {
  name: string
  pattern: string
  flags?: string
  priority: number
  cls?: string
  // `'self'` makes the whole match clickable with its own text as the URL.
  // Any other literal makes every match link to that constant URL.
  url?: string
  // Map of named-capture-group -> sub-style.
  subgroups?: Record<string, HighlightSubgroup>
}

export interface HighlightSpan {
  start: number
  end: number
  cls: string
  url?: string
}

export interface HighlightRulesFile {
  schema: number
  rules: HighlightRule[]
}

interface CompiledRule {
  name: string
  re: RegExp
  priority: number
  cls?: string
  url?: string
  subgroups?: Record<string, HighlightSubgroup>
}

let RULES: CompiledRule[] = []
let VERSION = 0
const CACHE = new Map<string, HighlightSpan[]>()
const CACHE_MAX = 4000

/**
 * Replace the current rule set. Compiles every rule's regex once and forces
 * the `g` (global) and `d` (indices) flags so the engine can iterate matches
 * and read sub-group offsets. Calling this bumps {@link rulesVersion} so any
 * external caller can invalidate its own line-level caches.
 */
export function setRules(rules: HighlightRule[]): void {
  RULES = rules.map(compile)
  VERSION++
  CACHE.clear()
  rulesVersionRef.value = VERSION
}

export function rulesVersion(): number {
  return VERSION
}

export function ruleCount(): number {
  return RULES.length
}

function compile(rule: HighlightRule): CompiledRule {
  const flags = ensureFlags(rule.flags ?? '')
  let re: RegExp
  try {
    re = new RegExp(rule.pattern, flags)
  } catch (e) {
    throw new Error(
      `highlight rule "${rule.name}" failed to compile: ${(e as Error).message}`,
    )
  }
  return {
    name: rule.name,
    re,
    priority: rule.priority,
    cls: rule.cls,
    url: rule.url,
    subgroups: rule.subgroups,
  }
}

function ensureFlags(flags: string): string {
  let out = flags
  if (!out.includes('g')) out += 'g'
  if (!out.includes('d')) out += 'd'
  return out
}

/**
 * Run every loaded rule across `text` and merge the matches into a flat,
 * ordered, non-overlapping list of highlight spans. Higher-priority matches
 * win on overlap; sub-group spans always sit above their parent match
 * (their default priority is `parent + 1`).
 *
 * Returns an empty array for empty input or when no rules are loaded.
 */
export function computeHighlights(text: string): HighlightSpan[] {
  if (RULES.length === 0 || text.length === 0) return []
  const n = text.length
  const arrs: PaintArrays = {
    cls: new Array(n).fill(null),
    pri: new Array(n).fill(-1),
    url: new Array(n).fill(null),
  }

  for (const rule of RULES) {
    rule.re.lastIndex = 0
    let m: RegExpExecArray | null
    // Hard cap iterations per (rule, line) so a pathological regex can't
    // melt the renderer on a single visible row.
    let guard = 0
    while ((m = rule.re.exec(text)) !== null) {
      if (++guard > 256) break
      const start = m.index
      const end = m.index + m[0].length
      if (end === start) {
        // Avoid infinite loops on zero-width matches.
        rule.re.lastIndex = start + 1
        continue
      }
      const matchUrl = matchUrlFor(rule, m[0])
      if (rule.cls) {
        paint(arrs, start, end, rule.cls, rule.priority, matchUrl)
      }
      if (rule.subgroups) {
        // RegExp match indices live on `m.indices` when the `d` flag is set,
        // which `ensureFlags` guarantees.
        const indices = (m as unknown as { indices?: { groups?: Record<string, [number, number] | undefined> } }).indices
        const groupIdx = indices?.groups
        if (groupIdx) {
          for (const [name, def] of Object.entries(rule.subgroups)) {
            const idx = groupIdx[name]
            if (!idx) continue
            const [s, e] = idx
            if (s === e) continue
            const p = def.priority ?? rule.priority + 1
            if (def.cls) paint(arrs, s, e, def.cls, p, null)
          }
        }
      }
    }
  }

  // Collapse the per-char arrays into runs of identical (cls, url).
  const out: HighlightSpan[] = []
  let i = 0
  while (i < n) {
    if (arrs.cls[i] === null) {
      i++
      continue
    }
    const c = arrs.cls[i]
    const u = arrs.url[i]
    let j = i + 1
    while (j < n && arrs.cls[j] === c && arrs.url[j] === u) j++
    const span: HighlightSpan = { start: i, end: j, cls: c as string }
    if (u !== null) span.url = u
    out.push(span)
    i = j
  }
  return out
}

interface PaintArrays {
  cls: (string | null)[]
  pri: number[]
  url: (string | null)[]
}

function paint(
  arrs: PaintArrays,
  s: number,
  e: number,
  cls: string,
  pri: number,
  url: string | null,
): void {
  for (let i = s; i < e; i++) {
    if (pri > arrs.pri[i]) {
      arrs.cls[i] = cls
      arrs.pri[i] = pri
      arrs.url[i] = url
    }
  }
}

function matchUrlFor(rule: CompiledRule, match: string): string | null {
  if (!rule.url) return null
  if (rule.url === 'self') return match
  return rule.url
}

/**
 * Cached entry point. The cache key is `version + text` so a rule edit
 * automatically invalidates prior entries without the caller doing anything.
 * The cache is sized for the typical visible window plus generous overscan;
 * eviction drops the oldest quarter when the cap is hit.
 */
export function highlightsFor(text: string): HighlightSpan[] {
  const key = VERSION + '\x00' + text
  const hit = CACHE.get(key)
  if (hit) return hit
  const spans = computeHighlights(text)
  if (CACHE.size >= CACHE_MAX) {
    const drop = Math.floor(CACHE_MAX / 4)
    let i = 0
    for (const k of CACHE.keys()) {
      CACHE.delete(k)
      if (++i >= drop) break
    }
  }
  CACHE.set(key, spans)
  return spans
}

// --- Overlay onto axis-1 spans -----------------------------------------------

export interface BaseSpan {
  start: number
  end: number
  cls: string
}

export interface LeafSpan {
  text: string
  cls: string // space-joined axis-1 cls + axis-2 cls
  url?: string
}

/**
 * Merge an axis-1 base-span list (covering the full text contiguously) with
 * an axis-2 highlight-span list (sparse, non-overlapping). The result is an
 * ordered list of leaf spans where each one carries the combined class names
 * from both axes. Each leaf inherits the URL from whichever axis-2 span it
 * sits inside, if any.
 */
export function overlay(
  text: string,
  base: BaseSpan[],
  axis2: HighlightSpan[],
): LeafSpan[] {
  if (text.length === 0) return []
  // Build a sorted list of "boundary" offsets: every start/end of every span
  // on both axes, plus 0 and text.length.
  const boundaries = new Set<number>([0, text.length])
  for (const b of base) {
    boundaries.add(b.start)
    boundaries.add(b.end)
  }
  for (const h of axis2) {
    boundaries.add(h.start)
    boundaries.add(h.end)
  }
  const sorted = Array.from(boundaries).sort((a, b) => a - b)
  const out: LeafSpan[] = []
  for (let i = 0; i < sorted.length - 1; i++) {
    const s = sorted[i]
    const e = sorted[i + 1]
    if (s === e) continue
    const baseCls = findBaseCls(base, s, e)
    const axis2Hit = findAxis2(axis2, s, e)
    const classes: string[] = []
    if (baseCls) classes.push('s-' + baseCls)
    if (axis2Hit) classes.push(axis2Hit.cls)
    if (classes.length === 0) classes.push('s-message')
    const leaf: LeafSpan = {
      text: text.slice(s, e),
      cls: classes.join(' '),
    }
    if (axis2Hit?.url) leaf.url = axis2Hit.url
    out.push(leaf)
  }
  return mergeAdjacent(out)
}

function findBaseCls(base: BaseSpan[], s: number, e: number): string | undefined {
  for (const sp of base) {
    if (sp.start <= s && sp.end >= e) return sp.cls
  }
  return undefined
}

function findAxis2(axis2: HighlightSpan[], s: number, e: number): HighlightSpan | undefined {
  for (const sp of axis2) {
    if (sp.start <= s && sp.end >= e) return sp
  }
  return undefined
}

// --- Ad-hoc compile + match (used by the editor's live preview) -----------
//
// Lets callers compute highlights against a candidate rule set without
// touching the global engine state or its cache. Compiles each rule fresh
// per call; suitable for preview pane recomputes where the rule set is
// being edited keystroke-by-keystroke. Returns either the spans or a
// compile error so the editor can flag the offending rule inline.

export interface RuleCompileError {
  rule: string
  message: string
}

export type ComputeWithResult =
  | { ok: true; spans: HighlightSpan[] }
  | { ok: false; errors: RuleCompileError[] }

export function computeWith(rules: HighlightRule[], text: string): ComputeWithResult {
  const compiled: CompiledRule[] = []
  const errors: RuleCompileError[] = []
  for (const r of rules) {
    try {
      compiled.push(compile(r))
    } catch (e) {
      errors.push({ rule: r.name, message: (e as Error).message })
    }
  }
  if (errors.length > 0) return { ok: false, errors }

  if (text.length === 0 || compiled.length === 0) return { ok: true, spans: [] }
  const n = text.length
  const arrs: PaintArrays = {
    cls: new Array(n).fill(null),
    pri: new Array(n).fill(-1),
    url: new Array(n).fill(null),
  }
  for (const rule of compiled) {
    rule.re.lastIndex = 0
    let m: RegExpExecArray | null
    let guard = 0
    while ((m = rule.re.exec(text)) !== null) {
      if (++guard > 256) break
      const start = m.index
      const end = m.index + m[0].length
      if (end === start) {
        rule.re.lastIndex = start + 1
        continue
      }
      const matchUrl = matchUrlFor(rule, m[0])
      if (rule.cls) paint(arrs, start, end, rule.cls, rule.priority, matchUrl)
      if (rule.subgroups) {
        const indices = (m as unknown as { indices?: { groups?: Record<string, [number, number] | undefined> } }).indices
        const groupIdx = indices?.groups
        if (groupIdx) {
          for (const [name, def] of Object.entries(rule.subgroups)) {
            const idx = groupIdx[name]
            if (!idx) continue
            const [s, e] = idx
            if (s === e) continue
            const p = def.priority ?? rule.priority + 1
            if (def.cls) paint(arrs, s, e, def.cls, p, null)
          }
        }
      }
    }
  }
  const out: HighlightSpan[] = []
  let i = 0
  while (i < n) {
    if (arrs.cls[i] === null) { i++; continue }
    const c = arrs.cls[i]
    const u = arrs.url[i]
    let j = i + 1
    while (j < n && arrs.cls[j] === c && arrs.url[j] === u) j++
    const span: HighlightSpan = { start: i, end: j, cls: c as string }
    if (u !== null) span.url = u
    out.push(span)
    i = j
  }
  return { ok: true, spans: out }
}

/**
 * Try-compile a single rule and report the error, if any. Used by the
 * editor to flag a row with a bad regex *before* the user attempts to save.
 */
export function tryCompileRule(rule: HighlightRule): string | null {
  try {
    compile(rule)
    return null
  } catch (e) {
    return (e as Error).message
  }
}

function mergeAdjacent(spans: LeafSpan[]): LeafSpan[] {
  const out: LeafSpan[] = []
  for (const span of spans) {
    const prev = out.at(-1)
    if (prev?.cls === span.cls && prev?.url === span.url) {
      prev.text += span.text
    } else {
      out.push({ ...span })
    }
  }
  return out
}
