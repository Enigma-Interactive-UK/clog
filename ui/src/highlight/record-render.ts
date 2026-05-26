/**
 * Render a log line's text into LeafSpans, applying axis-1 header field
 * slicing (level / timestamp / thread / logger / message), axis-2 user
 * highlight rules, optional search-hit overlay and level colouring on
 * the level segment. Shared between the main viewport and the full-
 * record modal so both produce identical markup.
 */
import { highlightsFor, overlay, type LeafSpan } from './engine'
import type { HeaderFields, HitRef, LineRow } from '../types'

interface BaseMark { start: number; end: number; cls: string }

export function headerBaseSpans(text: string, fields: HeaderFields): BaseMark[] {
  const marks: BaseMark[] = []
  if (fields.level) marks.push({ start: fields.level[0], end: fields.level[1], cls: 'level' })
  if (fields.timestamp) marks.push({ start: fields.timestamp[0], end: fields.timestamp[1], cls: 'timestamp' })
  if (fields.thread) marks.push({ start: fields.thread[0], end: fields.thread[1], cls: 'thread' })
  if (fields.logger) marks.push({ start: fields.logger[0], end: fields.logger[1], cls: 'logger' })
  if (fields.message) marks.push({ start: fields.message[0], end: fields.message[1], cls: 'message' })
  marks.sort((a, b) => a.start - b.start)
  const out: BaseMark[] = []
  let cursor = 0
  for (const m of marks) {
    if (m.start > cursor) out.push({ start: cursor, end: m.start, cls: 'sep' })
    out.push(m)
    cursor = m.end
  }
  if (cursor < text.length) out.push({ start: cursor, end: text.length, cls: 'sep' })
  return out
}

function searchSpansForLine(row: LineRow, hit: HitRef | null | undefined): BaseMark[] {
  if (!hit) return []
  const boff = row.byte_offset_in_record
  const len = row.text.length
  const out: BaseMark[] = []
  for (const [s, e] of hit.ranges) {
    const ls = Math.max(0, s - boff)
    const le = Math.min(len, e - boff)
    if (le > ls) out.push({ start: ls, end: le, cls: 'h-search-match' })
  }
  return out
}

function decorateLevels(leaves: LeafSpan[], level: string): LeafSpan[] {
  if (!leaves.some((l) => l.cls.includes('s-level'))) return leaves
  return leaves.map((l) =>
    l.cls.includes('s-level') ? { ...l, cls: l.cls + ' level-' + level } : l,
  )
}

export function renderLineSpans(row: LineRow, hit: HitRef | null | undefined): LeafSpan[] {
  const search = searchSpansForLine(row, hit)
  if (row.fields) {
    const base = headerBaseSpans(row.text, row.fields)
    const axis2 = highlightsFor(row.text)
    const combined = search.length === 0 ? axis2 : [...search, ...axis2]
    const leaves = overlay(row.text, base, combined)
    return decorateLevels(leaves, row.level)
  }
  const base = [{ start: 0, end: row.text.length, cls: 'message' }]
  const axis2 = highlightsFor(row.text)
  const combined = search.length === 0 ? axis2 : [...axis2, ...search]
  return overlay(row.text, base, combined)
}
