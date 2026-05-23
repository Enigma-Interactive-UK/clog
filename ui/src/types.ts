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

export interface LevelMinimapPayload {
  buckets: string[]
  line_count: number
}

export interface TailDelta {
  new_record_count: number
  line_count: number
  record_count: number
  last_offset: number
  rotated: boolean
}

export const PAGE_SIZE = 256
export const ROW_HEIGHT = 18
export const OVERSCAN = 32
