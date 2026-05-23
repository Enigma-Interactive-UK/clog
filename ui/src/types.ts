/**
 * Shared TypeScript interfaces used across the UI. Mirrors the wire shapes
 * defined in `crates/clog-app/src/main.rs` plus a handful of UI-side helper
 * types. Lives outside `App.vue` so the per-tab module and any future split
 * components can pull them in without circular re-exports.
 */

export interface IpcError {
  kind: string
  message: string
  path?: string
}

export interface HeaderFields {
  level: [number, number] | null
  timestamp: [number, number] | null
  thread: [number, number] | null
  logger: [number, number] | null
  message: [number, number] | null
}

export interface OpenedFile {
  file_id: number
  path: string
  size_bytes: number
  line_count: number
  record_count: number
  pattern_name: string | null
  pattern_source: string
  pattern_score: number
  cache_hit: boolean
  loose: boolean
}

export interface LineRow {
  record_idx: number
  line_within_record: number
  byte_offset_in_record: number
  level: string
  fields: HeaderFields | null
  text: string
}

export interface HitRef {
  record_idx: number
  record_first_line: number
  record_line_count: number
  level: string
  ranges: [number, number][]
  score: number
}

export interface SearchDelta {
  search_id: number
  hits: HitRef[]
  total: number
  done: boolean
}

export interface RecordRef {
  record_idx: number
  record_first_line: number
  record_line_count: number
  level: string
}

export interface RecordRefsPayload {
  refs: RecordRef[]
}

export type SearchMode = 'smart' | 'regex'
export type LevelKey = 'trace' | 'debug' | 'info' | 'warn' | 'error' | 'fatal'

// Bit positions must match clog_core::search::level_bit.
export const LEVEL_BIT: Record<string, number> = {
  trace: 1 << 0,
  debug: 1 << 1,
  info: 1 << 2,
  warn: 1 << 3,
  error: 1 << 4,
  fatal: 1 << 5,
  off: 1 << 6,
  all: 1 << 7,
  unknown: 1 << 8,
}
export const LEVEL_KEYS: LevelKey[] = ['trace', 'debug', 'info', 'warn', 'error', 'fatal']

export interface LinesPayload {
  start_line: number
  lines: LineRow[]
}

export interface Settings {
  schema: number
  theme: 'system' | 'light' | 'dark'
  font_size: number
  recent_files: string[]
  follow_tail_default: boolean
  slow_request_thresholds?: SlowRequestThresholds | null
}

export interface RestoredFile {
  path: string
  scroll_top: number
  follow_tail: boolean
  level_mask: number
  filter_text: string
  search_mode: SearchMode
  search_case_sensitive: boolean
  filter_mode: boolean
  bookmarks?: number[]
}

export interface Session {
  schema: number
  /** Legacy single-file slot. Older sessions still load via this; the
   *  Rust side coalesces it into `tabs` at load time. */
  last_file: RestoredFile | null
  tabs: RestoredFile[]
  active_tab: number
}

export interface DataDirPayload {
  path: string
  portable: boolean
}

export interface PatternTestPayload {
  score: number
  sample_size: number
}

export interface ApplyPatternPayload {
  record_count: number
  pattern_source: string
  loose: boolean
}

export interface BucketStat {
  /** Worst severity touching this bucket. Same alphabet as `LineRow.level`
   *  ('trace' | 'debug' | 'info' | 'warn' | 'error' | 'fatal' | 'off' | 'all' | 'unknown'). */
  worst: string
  /** Record count at level ERROR or FATAL in this bucket. */
  error: number
  /** Record count at level WARN in this bucket. */
  warn: number
  /** Total record count in this bucket. */
  total: number
}

export interface LevelMinimapPayload {
  buckets: BucketStat[]
  line_count: number
  /** Max of `(error + warn)` across all buckets; normaliser for hot overlay. */
  max_error_warn_sum: number
  /** Max `total` across all buckets; reserved for density wash. */
  max_total: number
}

/** Significant event marker (e.g. site restart) extracted by the backend.
 *  New kinds may appear as `BUILTIN_MARKER_RULES` grows on the Rust side -
 *  treat `kind` as an open string so the UI degrades gracefully (unknown
 *  kinds simply paint with the fallback colour). */
export interface MarkerRef {
  kind: 'restart' | (string & {})
  /** Physical line index of the marker (first line of the matching record). */
  line_index: number
  record_idx: number
}

export interface TailDelta {
  new_record_count: number
  line_count: number
  record_count: number
  last_offset: number
  rotated: boolean
}

// --- Slow request insights --------------------------------------------------

export type SlowRequestPathMode = 'normalised' | 'raw'

export interface SlowRequestThresholds {
  fast_ms: number
  slow_ms: number
}

export type ThresholdSource = 'auto' | 'global' | 'per_file'

export interface EffectiveThresholds {
  source: ThresholdSource
  effective: SlowRequestThresholds
  per_file: SlowRequestThresholds | null
  global: SlowRequestThresholds | null
}

export interface SpeedBucket {
  count: number
  avg_ms: number
  max_ms: number
}

export interface SpeedGrid {
  buckets: SpeedBucket[]
  min_avg_ms: number
  max_avg_ms: number
}

export interface SlowRequestOccurrence {
  timestamp_ms: number | null
  duration_ms: number
  line_index: number
  record_idx: number
  dup_count: number
  class_method: string
  raw_path: string
}

export interface SlowRequestEntry {
  path: string
  raw_paths: string[]
  count: number
  total_ms: number
  min_ms: number
  max_ms: number
  avg_ms: number
  p95_ms: number
  longest_line: number
  occurrences: SlowRequestOccurrence[]
}

export interface SlowRequestSummary {
  entries: SlowRequestEntry[]
  total_hits: number
  deduped: number
  total_ms: number
}

/**
 * User-editable highlight rule, persisted to `highlight-rules.json` (global)
 * or `per-file-rules/<hash>.json` (per-file). Carries the user-facing knobs
 * (colour, bold, italic, underline, enabled) which the UI translates into
 * engine-facing class names at engine-feed time.
 */
export interface UserHighlightRule {
  name: string
  pattern: string
  flags: string
  priority: number
  /** Palette key (`'red'`, `'amber'`, `'yellow'`, `'green'`, `'cyan'`, `'blue'`, `'magenta'`, `'pink'`) or empty. */
  colour: string
  /** Background palette key (same alphabet as `colour`) or empty. */
  background: string
  bold: boolean
  italic: boolean
  underline: boolean
  enabled: boolean
}

export interface HighlightRulesFile {
  schema: number
  rules: UserHighlightRule[]
}

export interface PerFileRulesFile {
  schema: number
  path: string
  rules: UserHighlightRule[]
}

export const USER_RULE_PALETTE = [
  'red', 'amber', 'yellow', 'green', 'cyan', 'blue', 'magenta', 'pink',
] as const

export function newUserRule(name = 'new-rule'): UserHighlightRule {
  return {
    name,
    pattern: '',
    flags: '',
    priority: 100,
    colour: 'amber',
    background: '',
    bold: false,
    italic: false,
    underline: false,
    enabled: true,
  }
}

export const PAGE_SIZE = 256
export const ROW_HEIGHT = 18
export const OVERSCAN = 32
