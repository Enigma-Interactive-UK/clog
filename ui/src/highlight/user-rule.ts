// User-rule <-> engine-rule conversion.
//
// User rules are persisted with palette-key + bold/italic/underline knobs;
// the engine speaks `cls` strings only. The conversion happens here so the
// editor can stay UI-friendly and the engine stays dumb about palettes.

import type { HighlightRule } from './engine'
import type { UserHighlightRule } from '../types'

export function userToEngineRule(u: UserHighlightRule): HighlightRule {
  const classes: string[] = []
  if (u.colour) classes.push(`h-user-${u.colour}`)
  if (u.background) classes.push(`h-user-bg-${u.background}`)
  if (u.bold) classes.push('h-user-bold')
  if (u.italic) classes.push('h-user-italic')
  if (u.underline) classes.push('h-user-underline')
  return {
    name: u.name,
    pattern: u.pattern,
    flags: u.flags || undefined,
    priority: u.priority,
    cls: classes.length > 0 ? classes.join(' ') : undefined,
  }
}

/**
 * Compose the effective engine rule set from the three layers:
 * baked-in defaults (lowest), global user rules, per-file overrides
 * (highest). Disabled rules are skipped. Empty-pattern rules are skipped
 * so an in-progress new rule doesn't blow up the engine.
 */
export function composeEffectiveRules(
  defaults: HighlightRule[],
  global: UserHighlightRule[],
  perFile: UserHighlightRule[],
): HighlightRule[] {
  const out: HighlightRule[] = [...defaults]
  for (const u of global) {
    if (!u.enabled || !u.pattern) continue
    out.push(userToEngineRule(u))
  }
  for (const u of perFile) {
    if (!u.enabled || !u.pattern) continue
    out.push(userToEngineRule(u))
  }
  return out
}
