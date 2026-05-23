<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, useTemplateRef, watch } from 'vue'
import { Channel, invoke } from '@tauri-apps/api/core'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { open } from '@tauri-apps/plugin-dialog'
import { openUrl } from '@tauri-apps/plugin-opener'
import { useVirtualizer } from '@tanstack/vue-virtual'
import defaultRulesFile from './highlight/default-rules.json'
import {
  highlightsFor,
  overlay,
  setRules,
  type HighlightRulesFile,
  type LeafSpan,
} from './highlight/engine'

// Load the bundled default rule set once at module-eval time. P8 will swap
// this for a user-editable set; P5 keeps it static and baked-in.
setRules((defaultRulesFile as HighlightRulesFile).rules)

interface IpcError {
  kind: string
  message: string
  path?: string
}

interface HeaderFields {
  level: [number, number] | null
  timestamp: [number, number] | null
  thread: [number, number] | null
  logger: [number, number] | null
  message: [number, number] | null
}

interface OpenedFile {
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

interface LineRow {
  record_idx: number
  line_within_record: number
  byte_offset_in_record: number
  level: string
  fields: HeaderFields | null
  text: string
}

interface HitRef {
  record_idx: number
  record_first_line: number
  record_line_count: number
  level: string
  ranges: [number, number][]
  score: number
}

interface SearchDelta {
  search_id: number
  hits: HitRef[]
  total: number
  done: boolean
}

interface RecordRef {
  record_idx: number
  record_first_line: number
  record_line_count: number
  level: string
}

interface RecordRefsPayload {
  refs: RecordRef[]
}

type SearchMode = 'smart' | 'regex'
type LevelKey = 'trace' | 'debug' | 'info' | 'warn' | 'error' | 'fatal'

// Bit positions must match clog_core::search::level_bit.
const LEVEL_BIT: Record<string, number> = {
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
const LEVEL_KEYS: LevelKey[] = ['trace', 'debug', 'info', 'warn', 'error', 'fatal']

interface LinesPayload {
  start_line: number
  lines: LineRow[]
}

interface Settings {
  schema: number
  theme: 'system' | 'light' | 'dark'
  font_size: number
  recent_files: string[]
  follow_tail_default: boolean
}

interface RestoredFile {
  path: string
  scroll_top: number
  follow_tail: boolean
  level_mask: number
  filter_text: string
  search_mode: SearchMode
  search_case_sensitive: boolean
  filter_mode: boolean
}

interface Session {
  schema: number
  last_file: RestoredFile | null
}

interface DataDirPayload {
  path: string
  portable: boolean
}

interface PatternTestPayload {
  score: number
  sample_size: number
}

interface ApplyPatternPayload {
  record_count: number
  pattern_source: string
  loose: boolean
}

interface LevelMinimapPayload {
  buckets: string[]
  line_count: number
}

interface TailDelta {
  new_record_count: number
  line_count: number
  record_count: number
  last_offset: number
  rotated: boolean
}

const PAGE_SIZE = 256
const ROW_HEIGHT = 18
const OVERSCAN = 32

const file = ref<OpenedFile | null>(null)
const error = ref<string | null>(null)
const busy = ref(false)

// page_index -> array of LineRow (length up to PAGE_SIZE).
const pages = ref(new Map<number, LineRow[]>())
// page_index -> generation stamp of the most-recent fetch for that page.
// Each call to fetchPage bumps `nextGen` and writes its myGen here; when a
// response lands it only applies if its myGen still matches -- so when two
// force-refetches stack up during fast tailing, the older (smaller-end)
// response is silently dropped instead of overwriting the newer one.
const inflight = new Map<number, number>()
let nextGen = 0

// Pattern-paste bar state.
const patternInput = ref<string>('')
const patternMode = ref<'pattern' | 'regex'>('pattern')
const patternScore = ref<number | null>(null)
const patternSampleSize = ref<number>(0)
const patternError = ref<string | null>(null)

// Search + filter state. Hits are keyed by record_idx so renderLine can
// look up the active record's ranges in O(1). The search engine returns
// ranges record-relative; the renderer converts each to a line-local
// range using each LineRow's `byte_offset_in_record`.
const searchMode = ref<SearchMode>('smart')
const searchQuery = ref('')
const searchCaseSensitive = ref(false)
const filterMode = ref(false)
const searchError = ref<string | null>(null)
const searchInflight = ref(false)
const hits = ref(new Map<number, HitRef>())
const hitOrder = ref<number[]>([])
const currentHit = ref<number>(-1)
// Synchronous generation counter, bumped at the start of every
// runSearch. Each call closes over its own `myGen` and compares it to
// the latest value -- any handler whose `myGen` no longer matches has
// been superseded and drops its delta. Critically NOT the backend's
// search_id, which arrives via the invoke response and races against
// the channel's first deltas. Two in-flight runSearch calls also stomp
// `currentSearchId` out of order; the generation gate doesn't.
let runSearchGen = 0
// Records whose level passes the current mask. Refreshed from the
// backend whenever `levelAllow` changes or a file is opened. When the
// mask is full (all levels allowed) this is `null` to mean "no
// narrowing" -- the renderer then takes the fast path of just using the
// raw line_count.
const allowedRecords = ref<RecordRef[] | null>(null)
let pendingSearchTimer: number | null = null

// Level mask: per-level allow flags. The user toggles INFO/DEBUG/etc.
// off to hide that level from both the search and the rendered output.
const levelAllow = ref<Record<string, boolean>>({
  trace: true,
  debug: true,
  info: true,
  warn: true,
  error: true,
  fatal: true,
  off: true,
  all: true,
  unknown: true,
})

function buildLevelMask(): number {
  let mask = 0
  for (const [k, v] of Object.entries(levelAllow.value)) {
    if (v) mask |= LEVEL_BIT[k] ?? 0
  }
  return mask
}

// Tail state.
// --- Settings + session state (P7) ---
const settings = ref<Settings>({
  schema: 1,
  theme: 'system',
  font_size: 13,
  recent_files: [],
  follow_tail_default: true,
})
const settingsOpen = ref(false)
const patternOpen = ref(false)
const dataDir = ref<DataDirPayload | null>(null)
const sessionRestoreInFlight = ref(false)
const pendingRestore = ref<RestoredFile | null>(null)
const sessionSaveTimer = ref<number | null>(null)

const tailing = ref(false)
const followTail = ref(true)
const tailPulse = ref(false)
const rotationToast = ref<string | null>(null)
let tailPulseTimer: number | null = null
let rotationToastTimer: number | null = null
let lastTailLineCount = 0

const scrollEl = useTemplateRef<HTMLDivElement>('scrollEl')
const minimapEl = useTemplateRef<HTMLCanvasElement>('minimapEl')
const searchInputEl = useTemplateRef<HTMLInputElement>('searchInputEl')

// Minimap state. Buckets is a Level-per-pixel array; we re-request when
// the viewport height changes or the file grows. The fetch is debounced
// per requestAnimationFrame to coalesce rapid tail deltas.
const minimapBuckets = ref<string[]>([])
const viewportHeightPx = ref(0)
let minimapFetchPending = false
let lastMinimapLineCount = -1
let lastMinimapHeight = -1
const MINIMAP_WIDTH = 20

// The raw scrollTop drives sticky-header lookup (and any other "what is
// actually at the top of the viewport" calculation). It must NOT be
// inferred from virtualRows[0] because the virtualizer's first item lives
// up to OVERSCAN rows above the visible area.
const viewportScrollTop = ref(0)

// Filter mode: build a flat ascending list of physical-line indices the
// virtualizer should expose. Without filter, that's just [0..line_count).
// With filter, it's the concatenation of each matching record's line
// span, in order. Declared above `useVirtualizer` because the virtualizer
// reads `effectiveCount` synchronously at setup time -- forward-references
// trip Vue's const-TDZ.
// Source records driving the filtered view. Returns null when no
// narrowing applies (full level mask + filter mode off). Composition:
//
// * level mask: an `allowedRecords` list (null = all levels allowed)
// * search query: a `hits` map of matching records
//
// When filter mode is on AND a search is active, the hits set wins
// (the backend already applied the level mask to it). Without a query
// (filter mode on or off), the level mask governs.
const filteredSourceRecords = computed<RecordRef[] | null>(() => {
  if (!file.value) return null
  const allowed = allowedRecords.value
  const isFiltering = filterMode.value
  const hasQuery = searchQuery.value.trim().length > 0

  if (!isFiltering && !allowed) return null

  if (isFiltering && hasQuery) {
    const source: RecordRef[] = []
    for (const recIdx of hitOrder.value) {
      const hit = hits.value.get(recIdx)
      if (hit) source.push(hit)
    }
    return source
  }
  return allowed
})

// Flat concatenation of every visible record's line span, in record
// order. Used by the virtualizer count + `actualLineIndex`. Null means
// "no narrowing" -- the virtualizer just uses the file's full line
// count.
const filteredLineIndices = computed<number[] | null>(() => {
  const source = filteredSourceRecords.value
  if (source === null) return null
  const out: number[] = []
  for (const rec of source) {
    const start = rec.record_first_line
    const end = start + rec.record_line_count
    for (let i = start; i < end; i++) out.push(i)
  }
  return out
})

const effectiveCount = computed(() => {
  if (!file.value) return 0
  const filt = filteredLineIndices.value
  if (filt) return filt.length
  return file.value.line_count
})

function actualLineIndex(virtualIdx: number): number {
  const filt = filteredLineIndices.value
  if (!filt) return virtualIdx
  return filt[virtualIdx] ?? 0
}

const virtualizer = useVirtualizer(
  computed(() => ({
    count: effectiveCount.value,
    getScrollElement: () => scrollEl.value ?? null,
    estimateSize: () => ROW_HEIGHT,
    overscan: OVERSCAN,
  })),
)

const virtualRows = computed(() => virtualizer.value.getVirtualItems())
const totalSize = computed(() => virtualizer.value.getTotalSize())

/** True when the viewport is scrolled to (or within a row of) the bottom.
 *  Drives the floating jump-to-bottom button's visibility -- when the user
 *  is already at the tail there's nothing to jump to. */
const atBottom = computed(() => {
  const total = totalSize.value
  const h = viewportHeightPx.value
  if (total <= 0 || h <= 0) return true
  return viewportScrollTop.value + h >= total - ROW_HEIGHT
})

function basename(p: string): string {
  const m = p.match(/[^\\/]+$/)
  return m ? m[0] : p
}

function formatCount(n: number): string {
  return n.toLocaleString('en-GB')
}

/** Format a raw byte count as a binary-IEC human-readable size (KiB, MiB,
 *  GiB). Under 1 KiB we show the raw byte count so a tiny fixture stays
 *  legible. Two decimals below 10 in the chosen unit, one above. */
function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return `${n}`
  if (n < 1024) return `${n} B`
  const units = ['KiB', 'MiB', 'GiB', 'TiB']
  let value = n / 1024
  let i = 0
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024
    i++
  }
  const digits = value < 10 ? 2 : value < 100 ? 1 : 0
  return `${value.toFixed(digits)} ${units[i]}`
}

function lineRow(index: number): LineRow | null {
  const pageIdx = Math.floor(index / PAGE_SIZE)
  const page = pages.value.get(pageIdx)
  if (!page) return null
  return page[index % PAGE_SIZE] ?? null
}

function lineRowVirtual(virtualIdx: number): LineRow | null {
  return lineRow(actualLineIndex(virtualIdx))
}

async function fetchPage(pageIdx: number, force = false) {
  if (!file.value) return
  if (!force && pages.value.has(pageIdx)) return
  // Non-force callers (the virtualizer watcher) skip in-flight pages so
  // overlapping scrolls don't dogpile the backend. Force callers (tail
  // deltas) MUST keep going even if a prior fetch is in flight -- they
  // need the latest `line_count` to be honoured.
  if (!force && inflight.has(pageIdx)) return
  const start = pageIdx * PAGE_SIZE
  const total = file.value.line_count
  if (start >= total) return
  const end = Math.min(start + PAGE_SIZE, total)
  const myGen = ++nextGen
  inflight.set(pageIdx, myGen)
  try {
    const payload = await invoke<LinesPayload>('get_lines', {
      fileId: file.value.file_id,
      start,
      end,
    })
    // Only apply if no newer fetch has taken over this page since we
    // started. Older responses with a smaller `end` would otherwise
    // overwrite a freshly-fetched page with stale (too-short) data, which
    // is exactly the off-by-one symptom seen during tailing.
    if (inflight.get(pageIdx) !== myGen) return
    // Swap atomically. The old entry stays in the Map until this line runs,
    // so visible rows on this page keep rendering their cached data instead
    // of flickering to blank during the round trip.
    pages.value.set(pageIdx, payload.lines)
    pages.value = new Map(pages.value)
  } catch (e) {
    const err = e as IpcError | string
    error.value = typeof err === 'string' ? err : err.message
  } finally {
    if (inflight.get(pageIdx) === myGen) inflight.delete(pageIdx)
  }
}

watch(virtualRows, (rows) => {
  if (!file.value) return
  const wanted = new Set<number>()
  for (const r of rows) {
    const actual = actualLineIndex(r.index)
    wanted.add(Math.floor(actual / PAGE_SIZE))
  }
  for (const p of wanted) fetchPage(p)
})

// Sticky header: when the topmost visible row is mid-record, overlay the
// header line of that row's record. We render it as a separate row that
// floats above the scroll content.
interface StickyHeader {
  row: LineRow
  lineIndex: number
}

const stickyHeader = computed<StickyHeader | null>(() => {
  if (!file.value) return null
  // The topmost visible row index is derived from scrollTop directly --
  // NOT from virtualRows[0] (which sits up to OVERSCAN rows above the
  // viewport edge and would make the sticky lag the actual content).
  const total = effectiveCount.value
  if (total === 0) return null
  const topVirtual = Math.min(total - 1, Math.floor(viewportScrollTop.value / ROW_HEIGHT))
  const topIdx = actualLineIndex(topVirtual)
  const data = lineRow(topIdx)
  if (!data) return null
  if (data.line_within_record === 0) return null
  // Walk backward to the header row (line_within_record == 0) of the
  // same record. Bounded by the record's first line, so this is cheap.
  // In filter mode, the previous virtual row may belong to a different
  // record entirely -- still find this record's header in the file
  // because the gutter/colour of the sticky cell mirrors the topmost row.
  for (let i = topIdx - 1; i >= 0; i--) {
    const candidate = lineRow(i)
    if (!candidate) {
      fetchPage(Math.floor(i / PAGE_SIZE))
      return null
    }
    if (candidate.record_idx !== data.record_idx) return null
    if (candidate.line_within_record === 0) return { row: candidate, lineIndex: i }
  }
  return null
})

function jumpToStickyStart() {
  const sticky = stickyHeader.value
  const el = scrollEl.value
  if (!sticky || !el) return
  // `sticky.lineIndex` is a FILE line index. In filter mode the virtual
  // index space is `filteredLineIndices`; we need to find the virtual
  // slot whose actual is the sticky header's file line. If the header
  // happens to be filtered out (no virtual slot), bail.
  const filt = filteredLineIndices.value
  let virtualIdx: number
  if (filt) {
    virtualIdx = filt.indexOf(sticky.lineIndex)
    if (virtualIdx < 0) return
  } else {
    virtualIdx = sticky.lineIndex
  }
  // Set scrollTop directly so the resulting position is exactly the
  // record's header row. `scrollToIndex` route went via the virtualizer
  // and consistently overshot by one row -- likely a sub-pixel offset
  // that my row-snap handler then rounded forward.
  el.scrollTop = virtualIdx * ROW_HEIGHT
}

async function pickFile() {
  error.value = null
  const selected = await open({
    multiple: false,
    title: 'Open a log file',
    filters: [
      { name: 'Log files', extensions: ['log', 'out', 'txt'] },
      { name: 'All files', extensions: ['*'] },
    ],
  })
  if (!selected || Array.isArray(selected)) return
  busy.value = true
  try {
    if (file.value) {
      const prev = file.value.file_id
      file.value = null
      pages.value = new Map()
      tailing.value = false
      hits.value = new Map()
      hitOrder.value = []
      currentHit.value = -1
      // Bump the gen so any in-flight runSearch from the previous file
      // is disowned -- its onmessage closure will see myGen !== latest
      // and drop everything.
      runSearchGen++
      allowedRecords.value = null
      await invoke('cancel_search', { fileId: prev }).catch(() => {})
      await invoke('stop_tail', { fileId: prev }).catch(() => {})
      await invoke('close_file', { fileId: prev }).catch(() => {})
    }
    const opened = await invoke<OpenedFile>('open_file', { path: selected })
    file.value = opened
    patternInput.value = opened.pattern_source
    patternMode.value = 'pattern'
    patternScore.value = opened.pattern_score
    patternError.value = null
    lastTailLineCount = opened.line_count
    // Tail is on by default, so start at the bottom: fetch the last page
    // and scroll there. The virtualizer's watcher picks up additional
    // pages as the user scrolls up.
    if (opened.line_count > 0) {
      const lastPage = Math.floor((opened.line_count - 1) / PAGE_SIZE)
      fetchPage(lastPage)
      if (followTail.value) jumpToBottom()
    } else {
      fetchPage(0)
    }
    await startTail()
    scheduleMinimapFetch(true)
    await refreshAllowedRecords()
    // Search query, mode, case flag, filter state, and level mask
    // persist across file opens (the user's "find this every time"
    // expectations). Re-run the active search against the new file so
    // hits + filteredLineIndices reflect the new content.
    if (searchQuery.value.trim().length > 0) {
      scheduleSearch()
    }
  } catch (e) {
    const err = e as IpcError | string
    error.value = typeof err === 'string' ? err : err.message
    file.value = null
  } finally {
    busy.value = false
  }
}

async function startTail() {
  if (!file.value) return
  const fileId = file.value.file_id
  const channel = new Channel<TailDelta>()
  channel.onmessage = handleTailDelta
  try {
    await invoke('start_tail', { fileId, onDelta: channel })
    tailing.value = true
  } catch (e) {
    const err = e as IpcError | string
    error.value = typeof err === 'string' ? err : err.message
    tailing.value = false
  }
}

function handleTailDelta(delta: TailDelta) {
  if (!file.value) return
  // Briefly highlight the tail indicator so users know data arrived.
  tailPulse.value = true
  if (tailPulseTimer !== null) globalThis.clearTimeout(tailPulseTimer)
  tailPulseTimer = globalThis.setTimeout(() => {
    tailPulse.value = false
    tailPulseTimer = null
  }, 250)

  if (delta.rotated) {
    // Drop the page cache; records and offsets have all shifted. Snap to
    // top because the file just shrank.
    pages.value = new Map()
    file.value = {
      ...file.value,
      line_count: delta.line_count,
      record_count: delta.record_count,
      size_bytes: delta.last_offset,
    }
    lastTailLineCount = delta.line_count
    showRotationToast()
    // Re-fetch the first page so the viewport doesn't sit empty.
    fetchPage(0)
    scheduleMinimapFetch(true)
    if (!isFullLevelMask()) void refreshAllowedRecords()
    // Re-run any active search so the new file shape (rotated content)
    // produces a fresh hit set. Debounced via scheduleSearch.
    if (searchQuery.value.trim().length > 0) scheduleSearch()
    return
  }

  // Pure append: the user might be reading a stable section, so we must
  // not let the viewport visually shift. Capture scrollTop before the
  // count bump so we can re-anchor on the next frame if anything moves it.
  const el = scrollEl.value
  const preserveTop = !followTail.value && el ? el.scrollTop : null

  file.value = {
    ...file.value,
    line_count: delta.line_count,
    record_count: delta.record_count,
    size_bytes: delta.last_offset,
  }

  // Force-refetch every page the appended range touches, not just the
  // old last page. Two reasons:
  //
  // 1. The old last page may have been partial (line_count grew past
  //    its end) -- a refetch picks up the new entries below.
  // 2. The growth can cross into a *fresh* page that an earlier tail
  //    tick already populated as a partial page. The non-force path
  //    from the virtualizer watcher would skip it because it's
  //    cached, leaving the new line invisible until the page falls
  //    out of cache. That was the "empty row then the previous line
  //    appears on the next tick" symptom.
  if (lastTailLineCount > 0 && delta.line_count > lastTailLineCount) {
    const oldLastPage = Math.floor((lastTailLineCount - 1) / PAGE_SIZE)
    const newLastPage = Math.floor((delta.line_count - 1) / PAGE_SIZE)
    for (let p = oldLastPage; p <= newLastPage; p++) {
      fetchPage(p, true)
    }
  }
  lastTailLineCount = delta.line_count
  scheduleMinimapFetch()
  // If the user has a non-default level mask, refresh the allowed list
  // so newly-appended records get included. With the default (full)
  // mask, refreshAllowedRecords short-circuits to null without IPC.
  if (!isFullLevelMask()) void refreshAllowedRecords()
  // Re-run any active search against the newly-appended content so new
  // matching records (or continuation lines that turn an existing record
  // into a match) appear in the highlight + filter set. The
  // `new_record_count > 0` guard would skip stack-trace continuations
  // that extend a previous record's text -- those can introduce a hit
  // too. scheduleSearch debounces (50 ms) so a 250 ms tail tick doesn't
  // flood the backend.
  if (searchQuery.value.trim().length > 0) {
    scheduleSearch()
  }

  if (followTail.value) {
    jumpToBottom()
  } else if (preserveTop !== null && el) {
    // Re-anchor across two frames: once for the synchronous count bump,
    // once for any virtualizer-internal re-measure that runs on the next
    // tick. The user explicitly detached -- the visible bytes must not
    // move out from under them.
    const target = el
    const top = preserveTop
    requestAnimationFrame(() => {
      if (Math.abs(target.scrollTop - top) > 0.5) target.scrollTop = top
      requestAnimationFrame(() => {
        if (Math.abs(target.scrollTop - top) > 0.5) target.scrollTop = top
      })
    })
  }
}

function showRotationToast() {
  rotationToast.value = 'File rotated -- re-indexed.'
  if (rotationToastTimer !== null) globalThis.clearTimeout(rotationToastTimer)
  rotationToastTimer = globalThis.setTimeout(() => {
    rotationToast.value = null
    rotationToastTimer = null
  }, 2500)
}

// --- Minimap ---

// Faded level colours for the minimap -- a translucent overlay of the raw
// level palette over the viewport background, mirroring the row-tint
// treatment in style.css. `info` is deliberately absent so the most-common
// level reads as background and the louder severities pop out of it.
// Values are rgba strings the canvas can use directly; alphas are tuned
// per-level so warn/error/fatal carry more visual weight.
const LEVEL_COLOUR: Record<string, string | null> = {
  trace: 'rgba(111, 118, 130, 0.25)',
  debug: 'rgba(158, 197, 255, 0.22)',
  info: null,
  warn: 'rgba(224, 176, 74, 0.55)',
  error: 'rgba(212, 87, 95, 0.65)',
  fatal: 'rgba(179, 134, 232, 0.6)',
  off: 'rgba(74, 84, 102, 0.2)',
  all: 'rgba(108, 199, 135, 0.35)',
  unknown: null,
}
// Viewport background. Resolved from the live `--bg-viewport` CSS token so
// the canvas composites correctly under both themes; light mode flips to a
// near-white surface and the dark hard-coded value would otherwise read as
// a black stripe down the side of the viewport.
function currentMinimapBg(): string {
  const styles = globalThis.getComputedStyle?.(document.documentElement)
  const fromVar = styles?.getPropertyValue('--bg-viewport').trim()
  return fromVar && fromVar.length > 0 ? fromVar : '#0f131a'
}

function scheduleMinimapFetch(force = false) {
  if (minimapFetchPending) return
  minimapFetchPending = true
  requestAnimationFrame(() => {
    minimapFetchPending = false
    void fetchMinimap(force)
  })
}

async function fetchMinimap(force: boolean) {
  if (!file.value) return
  const height = viewportHeightPx.value
  if (height <= 0) return
  // When filtering, compute the minimap client-side over the filtered
  // record set so the stripes map onto the actually-visible content
  // (effectiveCount lines, not file.line_count). Otherwise fetch the
  // backend's full-file minimap as before.
  const source = filteredSourceRecords.value
  if (source !== null) {
    const eff = effectiveCount.value
    const contentPx = eff * ROW_HEIGHT
    const bucketCount = Math.max(1, Math.min(Math.floor(height), contentPx))
    minimapBuckets.value = buildFilteredMinimap(source, eff, bucketCount)
    lastMinimapHeight = bucketCount
    lastMinimapLineCount = eff
    paintMinimap()
    return
  }
  const contentPx = file.value.line_count * ROW_HEIGHT
  const bucketCount = Math.max(1, Math.min(Math.floor(height), contentPx))
  if (
    !force &&
    bucketCount === lastMinimapHeight &&
    file.value.line_count === lastMinimapLineCount
  ) {
    return
  }
  try {
    const payload = await invoke<LevelMinimapPayload>('get_level_minimap', {
      fileId: file.value.file_id,
      bucketCount,
    })
    minimapBuckets.value = payload.buckets
    lastMinimapHeight = bucketCount
    lastMinimapLineCount = payload.line_count
    paintMinimap()
  } catch {
    // Non-fatal: minimap is purely decorative. Leave previous buckets in place.
  }
}

// Worst-severity-wins level ranking. Mirrors clog-app::main::level_rank
// so the client-side filtered-minimap projection matches what the
// backend would produce for the full file.
function minimapLevelRank(l: string): number {
  switch (l) {
    case 'fatal':
      return 7
    case 'error':
      return 6
    case 'warn':
      return 5
    case 'all':
      return 4
    case 'info':
      return 3
    case 'debug':
      return 2
    case 'trace':
      return 1
    default:
      return 0
  }
}

// Walk the filtered record list in virtual-line order and bump each
// bucket to the worst level it covers. `virtualLineCount` is the
// effectiveCount the virtualizer is using; the records' lines map onto
// virtual indices in encounter order.
function buildFilteredMinimap(
  source: RecordRef[],
  virtualLineCount: number,
  bucketCount: number,
): string[] {
  const buckets: string[] = new Array(bucketCount).fill('unknown')
  if (virtualLineCount === 0 || bucketCount === 0) return buckets
  let virtualCursor = 0
  for (const rec of source) {
    const firstLine = virtualCursor
    const lastLine = virtualCursor + Math.max(rec.record_line_count, 1) - 1
    const firstBucket = Math.min(
      bucketCount - 1,
      Math.floor((firstLine * bucketCount) / virtualLineCount),
    )
    const lastBucket = Math.min(
      bucketCount - 1,
      Math.floor((lastLine * bucketCount) / virtualLineCount),
    )
    const rank = minimapLevelRank(rec.level)
    for (let b = firstBucket; b <= lastBucket; b++) {
      if (rank > minimapLevelRank(buckets[b])) buckets[b] = rec.level
    }
    virtualCursor += rec.record_line_count
  }
  return buckets
}

function paintMinimap() {
  const canvas = minimapEl.value
  if (!canvas) return
  const buckets = minimapBuckets.value
  const h = buckets.length
  if (h === 0) {
    const ctx = canvas.getContext('2d')
    if (ctx) ctx.clearRect(0, 0, canvas.width, canvas.height)
    return
  }
  // Render at device-pixel resolution so the stripes stay crisp on
  // fractional-DPI Windows displays.
  const dpr = globalThis.devicePixelRatio || 1
  canvas.width = MINIMAP_WIDTH * dpr
  canvas.height = h * dpr
  canvas.style.width = `${MINIMAP_WIDTH}px`
  canvas.style.height = `${h}px`
  const ctx = canvas.getContext('2d')
  if (!ctx) return
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
  // Paint the bg first so info/unknown buckets (which have no overlay
  // colour) read as the same flat slate as the viewport.
  ctx.fillStyle = currentMinimapBg()
  ctx.fillRect(0, 0, MINIMAP_WIDTH, h)
  // Coalesce identical-colour runs into single fills -- 800px of buckets
  // becomes ~10-30 draw calls on a typical file. `null` runs (info /
  // unknown) are skipped entirely so the bg shows through.
  const colourAt = (i: number): string | null =>
    i < h ? (LEVEL_COLOUR[buckets[i]] ?? null) : null
  let runStart = 0
  let runColour = colourAt(0)
  for (let i = 1; i <= h; i++) {
    const next = colourAt(i)
    if (next !== runColour) {
      if (runColour !== null) {
        ctx.fillStyle = runColour
        ctx.fillRect(0, runStart, MINIMAP_WIDTH, i - runStart)
      }
      runStart = i
      runColour = next
    }
  }
}

const minimapIndicator = computed(() => {
  if (!file.value || file.value.line_count === 0) {
    return { top: 0, height: 0, visible: false }
  }
  const el = scrollEl.value
  const h = viewportHeightPx.value
  if (!el || h <= 0) return { top: 0, height: 0, visible: false }
  const total = el.scrollHeight
  if (total <= h) return { top: 0, height: h, visible: false }
  const top = (viewportScrollTop.value / total) * h
  const height = Math.max(8, (h / total) * h)
  return { top, height, visible: true }
})

function scrollToMinimapY(clientY: number) {
  const canvas = minimapEl.value
  const el = scrollEl.value
  if (!canvas || !el) return
  const rect = canvas.getBoundingClientRect()
  const ratio = Math.max(0, Math.min(1, (clientY - rect.top) / rect.height))
  const total = el.scrollHeight - el.clientHeight
  el.scrollTop = ratio * total
}

// Tooltip surfaced while hovering the minimap. The line index updates on
// every pointermove; the timestamp text follows whenever the relevant
// page lands in the cache (we trigger a background fetch on miss).
interface MinimapTooltip {
  visible: boolean
  top: number
  left: number
  lineIndex: number
  timestamp: string | null
}
const minimapTooltip = ref<MinimapTooltip>({
  visible: false,
  top: 0,
  left: 0,
  lineIndex: 0,
  timestamp: null,
})

function tooltipLineFromY(clientY: number): number | null {
  const canvas = minimapEl.value
  if (!canvas || !file.value || effectiveCount.value === 0) return null
  const rect = canvas.getBoundingClientRect()
  if (rect.height <= 0) return null
  const ratio = Math.max(0, Math.min(1, (clientY - rect.top) / rect.height))
  // Virtual line index within the current (filtered or not) view.
  const virtualIdx = Math.min(
    effectiveCount.value - 1,
    Math.floor(ratio * effectiveCount.value),
  )
  // Convert to actual file line so lineRow lookups (and the displayed
  // line number) point at the right record.
  return actualLineIndex(virtualIdx)
}

function timestampForLine(lineIndex: number): string | null {
  const row = lineRow(lineIndex)
  if (!row) return null
  // Continuation rows don't carry their own timestamp -- the record header
  // does. Walk back within the same record until we find it.
  if (row.fields?.timestamp) {
    const [s, e] = row.fields.timestamp
    return row.text.slice(s, e)
  }
  for (let i = lineIndex - 1; i >= 0; i--) {
    const candidate = lineRow(i)
    if (!candidate || candidate.record_idx !== row.record_idx) break
    if (candidate.fields?.timestamp) {
      const [s, e] = candidate.fields.timestamp
      return candidate.text.slice(s, e)
    }
  }
  return null
}

function updateMinimapTooltip(ev: PointerEvent) {
  const idx = tooltipLineFromY(ev.clientY)
  if (idx === null) {
    minimapTooltip.value = { visible: false, top: 0, left: 0, lineIndex: 0, timestamp: null }
    return
  }
  // Make sure the page covering this line is in flight if not cached, so a
  // subsequent move (or this same one re-evaluated next frame) can resolve
  // the timestamp without further user input.
  const pageIdx = Math.floor(idx / PAGE_SIZE)
  if (!pages.value.has(pageIdx)) fetchPage(pageIdx)
  const ts = timestampForLine(idx)
  // `top` is the viewport-coordinate Y of the cursor. The tooltip uses
  // position: fixed so it can float above the status bar without being
  // clipped by .viewport-shell's overflow.
  const canvas = minimapEl.value
  const rect = canvas?.getBoundingClientRect()
  const left = rect ? rect.left : ev.clientX
  minimapTooltip.value = {
    visible: true,
    top: ev.clientY,
    left,
    lineIndex: idx,
    timestamp: ts,
  }
}

// Re-paint the minimap whenever the filtered source set changes so the
// stripes (and effective bucket count) follow the filter live -- no
// stale full-file projection sitting under a narrow view.
watch(filteredSourceRecords, () => {
  scheduleMinimapFetch(true)
})

// Re-resolve the timestamp text whenever a new page lands, so the tooltip
// fills in shortly after a cache miss without the user having to wiggle.
watch(
  () => pages.value,
  () => {
    if (!minimapTooltip.value.visible) return
    const idx = minimapTooltip.value.lineIndex
    const ts = timestampForLine(idx)
    if (ts !== minimapTooltip.value.timestamp) {
      minimapTooltip.value = { ...minimapTooltip.value, timestamp: ts }
    }
  },
)

function onMinimapPointerEnter(ev: PointerEvent) {
  updateMinimapTooltip(ev)
}

function onMinimapPointerLeave() {
  minimapTooltip.value = { visible: false, top: 0, left: 0, lineIndex: 0, timestamp: null }
}

let minimapDragging = false

function onMinimapPointerDown(ev: PointerEvent) {
  // Detach follow-tail when the user grabs the minimap -- otherwise the
  // next tail delta would yank them back to the bottom.
  followTail.value = false
  minimapDragging = true
  ;(ev.currentTarget as HTMLElement).setPointerCapture(ev.pointerId)
  scrollToMinimapY(ev.clientY)
}

function onMinimapPointerMove(ev: PointerEvent) {
  updateMinimapTooltip(ev)
  if (!minimapDragging) return
  scrollToMinimapY(ev.clientY)
}

function onMinimapPointerUp(ev: PointerEvent) {
  minimapDragging = false
  ;(ev.currentTarget as HTMLElement).releasePointerCapture(ev.pointerId)
}

let resizeObserver: ResizeObserver | null = null

function jumpToBottom() {
  if (!file.value || effectiveCount.value === 0) return
  // Defer to the next frame so any virtualizer resize from the count bump
  // has settled before we ask for a scroll target.
  requestAnimationFrame(() => {
    if (!file.value || effectiveCount.value === 0) return
    // Index space is the virtualizer's count -- effectiveCount, not the
    // file's raw line_count. In filter mode they diverge, and asking
    // scrollToIndex for an out-of-bounds index sent scrollTop past the
    // last legal row, which landed the topmost-visible row on the new
    // tail line (treated as a continuation) and spuriously triggered
    // the sticky header overlay.
    virtualizer.value.scrollToIndex(effectiveCount.value - 1, { align: 'end' })
  })
}

function toggleFollowTail() {
  followTail.value = !followTail.value
  if (followTail.value) jumpToBottom()
}

function onViewportScroll() {
  const el = scrollEl.value
  if (!el) return
  // Snap scrollTop to a multiple of ROW_HEIGHT so the topmost visible row
  // is always flush with the viewport edge -- no half-line ever hangs
  // above the list area. We round (not floor) so a tiny upward nudge
  // settles cleanly without dragging the user backward, and we avoid
  // touching the bottom snap-point so follow-tail can park exactly there.
  const raw = el.scrollTop
  const maxScroll = el.scrollHeight - el.clientHeight
  const rem = raw % ROW_HEIGHT
  if (rem !== 0 && raw < maxScroll - 0.5) {
    const snapped = Math.round(raw / ROW_HEIGHT) * ROW_HEIGHT
    if (snapped !== raw) {
      el.scrollTop = snapped
      // The assignment fires another scroll event; bail and let that
      // pass do the bookkeeping below with the snapped value.
      return
    }
  }
  viewportScrollTop.value = el.scrollTop
  if (!sessionRestoreInFlight.value) scheduleSessionSave()
  // If the user scrolls away from the bottom, disable follow-tail. We
  // compare against a small slack so single-row jitter doesn't disengage.
  if (!followTail.value) return
  const distance = el.scrollHeight - el.scrollTop - el.clientHeight
  if (distance > ROW_HEIGHT * 4) {
    followTail.value = false
  }
}

async function testPattern() {
  if (!file.value) return
  patternError.value = null
  try {
    const args: Record<string, unknown> = { fileId: file.value.file_id }
    if (patternMode.value === 'pattern') args.pattern = patternInput.value
    else args.regex = patternInput.value
    const payload = await invoke<PatternTestPayload>('test_pattern', args)
    patternScore.value = payload.score
    patternSampleSize.value = payload.sample_size
  } catch (e) {
    const err = e as IpcError | string
    patternError.value = typeof err === 'string' ? err : err.message
    patternScore.value = null
  }
}

// --- Search ---

function scheduleSearch() {
  if (pendingSearchTimer !== null) globalThis.clearTimeout(pendingSearchTimer)
  // Debounce so a fast typist's keystrokes don't fire one backend search
  // per character. ~60Hz is the upper bound from design.md s7; 50ms is
  // close enough and survives a human's slow finger.
  pendingSearchTimer = globalThis.setTimeout(() => {
    pendingSearchTimer = null
    void runSearch()
  }, 50)
}

async function runSearch() {
  if (!file.value) return
  const fileId = file.value.file_id
  const query = searchQuery.value
  const mask = buildLevelMask()
  // Bump the generation IMMEDIATELY and synchronously. Any previous
  // runSearch's onmessage closure now sees `myGen !== runSearchGen` and
  // self-disables -- no chance of a slow earlier search clobbering a
  // newer one's hit set when its delta lands after the newer one's
  // started.
  const myGen = ++runSearchGen
  // Empty query: cancel any pending search, clear hits. Filter mode
  // with empty query then narrows by level mask alone -- we synthesize
  // an "all-records" view by leaving hits empty and showing nothing in
  // filter mode (matches the user mental model: no hits => nothing to
  // filter to). When filter mode is off, this just clears the overlay.
  if (query.trim().length === 0) {
    try {
      await invoke('cancel_search', { fileId })
    } catch {
      // best effort
    }
    // Only clear if we're still the latest run. (A newer runSearch
    // could have been scheduled while we awaited cancel_search.)
    if (myGen !== runSearchGen) return
    hits.value = new Map()
    hitOrder.value = []
    currentHit.value = -1
    searchInflight.value = false
    searchError.value = null
    return
  }
  searchError.value = null
  searchInflight.value = true
  const channel = new Channel<SearchDelta>()
  // Accumulate hits as they arrive. Fresh local buffers each call so a
  // stale stream that lingers after a newer one was launched can never
  // mutate a buffer the new run is using.
  const local = new Map<number, HitRef>()
  const order: number[] = []
  channel.onmessage = (delta: SearchDelta) => {
    // The closure's own generation gate. The backend `delta.search_id`
    // is deliberately NOT consulted -- it arrives via the invoke
    // response and can race against the channel's first deltas (the
    // response and channel messages travel separate IPC paths and
    // aren't ordered relative to each other). `runSearchGen` is bumped
    // synchronously at the top of every runSearch, so this check is
    // race-free.
    if (myGen !== runSearchGen) return
    for (const h of delta.hits) {
      if (!local.has(h.record_idx)) {
        local.set(h.record_idx, h)
        order.push(h.record_idx)
      }
    }
    // Publish a fresh snapshot so Vue's reactivity tracking picks it up.
    hits.value = new Map(local)
    hitOrder.value = order.slice()
    if (delta.done) {
      searchInflight.value = false
      // Park the cursor at the first hit if we don't have one yet.
      if (currentHit.value < 0 && order.length > 0) currentHit.value = 0
    }
  }
  try {
    await invoke('start_search', {
      fileId,
      request: {
        mode: searchMode.value,
        query,
        case_sensitive: searchCaseSensitive.value,
        level_mask: mask,
      },
      onHits: channel,
    })
  } catch (e) {
    // A newer run may have already started while this invoke was in
    // flight -- don't stomp its state with this run's error.
    if (myGen !== runSearchGen) return
    const err = e as IpcError | string
    const message = typeof err === 'string' ? err : err.message
    // Regex compile errors are expected during typing -- surface inline,
    // never modal.
    searchError.value = message
    searchInflight.value = false
    hits.value = new Map()
    hitOrder.value = []
  }
}

function nextHit() {
  if (hitOrder.value.length === 0) return
  currentHit.value = (currentHit.value + 1) % hitOrder.value.length
  scrollToCurrentHit()
}

function prevHit() {
  if (hitOrder.value.length === 0) return
  const n = hitOrder.value.length
  currentHit.value = (currentHit.value - 1 + n) % n
  scrollToCurrentHit()
}

function scrollToCurrentHit() {
  if (currentHit.value < 0) return
  const recIdx = hitOrder.value[currentHit.value]
  const hit = hits.value.get(recIdx)
  if (!hit) return
  // In filter mode, find this hit's position within filteredLineIndices.
  // Otherwise it's just the record's first line.
  const filt = filteredLineIndices.value
  let targetVirtual: number
  if (filt) {
    const want = hit.record_first_line
    // Linear scan would be O(filt.length) per click; not ideal but fine
    // for typical hit counts. If a 75k-hit file becomes the norm, swap
    // for a binary search.
    targetVirtual = filt.indexOf(want)
    if (targetVirtual < 0) return
  } else {
    targetVirtual = hit.record_first_line
  }
  followTail.value = false
  virtualizer.value.scrollToIndex(targetVirtual, { align: 'center' })
  // After the virtualizer has mounted the row, find the actual hit
  // span in the DOM and bring it into the row's per-row horizontal
  // viewport. The virtualizer's scroll is asynchronous (it sets
  // scrollTop, the browser schedules a scroll event, Vue then
  // re-renders virtualRows, and only then does the row exist in the
  // DOM). Two rAF ticks aren't enough on a fresh page; instead poll
  // up to FRAMES_MAX times and bail.
  scheduleHitFocus()
}

const HIT_FOCUS_FRAMES_MAX = 30
let hitFocusFramesLeft = 0
let hitFocusScheduled = false

function scheduleHitFocus() {
  hitFocusFramesLeft = HIT_FOCUS_FRAMES_MAX
  if (hitFocusScheduled) return
  hitFocusScheduled = true
  const step = () => {
    hitFocusScheduled = false
    if (bringCurrentHitMatchIntoView()) return
    if (--hitFocusFramesLeft <= 0) return
    hitFocusScheduled = true
    requestAnimationFrame(step)
  }
  requestAnimationFrame(step)
}

function isCurrentHitRow(row: LineRow | null): boolean {
  if (!row) return false
  if (currentHit.value < 0) return false
  const recIdx = hitOrder.value[currentHit.value]
  return row.record_idx === recIdx
}

function bringCurrentHitMatchIntoView(): boolean {
  const el = scrollEl.value
  if (!el) return false
  // The current hit's row is tagged with `.is-current-hit` (see the
  // template binding); inside it the first `.h-search-match` is the
  // match we want centred. We deliberately do NOT use the native
  // scrollIntoView() because it walks up to *every* scrollable
  // ancestor -- including the viewport -- and would re-trigger
  // vertical scrolling. Instead, scroll the row's .txt cell directly
  // by computing the offset from .txt to the match.
  const match = el.querySelector('.row.is-current-hit .h-search-match') as HTMLElement | null
  if (!match) return false
  const txt = match.closest('.txt') as HTMLElement | null
  if (!txt) return false
  if (txt.scrollWidth <= txt.clientWidth) {
    // Nothing to scroll -- the match already fits.
    return true
  }
  const matchRect = match.getBoundingClientRect()
  const txtRect = txt.getBoundingClientRect()
  // Position of the match within the txt's scroll content. Convert
  // from viewport coordinates by subtracting txt's left edge and
  // adding txt's current scrollLeft.
  const matchLeftInContent = matchRect.left - txtRect.left + txt.scrollLeft
  // Centre the match within the visible txt width.
  const targetScrollLeft =
    matchLeftInContent - txt.clientWidth / 2 + match.offsetWidth / 2
  const maxScrollLeft = txt.scrollWidth - txt.clientWidth
  txt.scrollLeft = Math.max(0, Math.min(maxScrollLeft, targetScrollLeft))
  return true
}

function toggleFilterMode() {
  filterMode.value = !filterMode.value
  // Snap to top whenever filter changes -- the previous scrollTop won't
  // map cleanly onto the new virtual line set.
  const el = scrollEl.value
  if (el) el.scrollTop = 0
}

function toggleLevel(level: LevelKey) {
  levelAllow.value = { ...levelAllow.value, [level]: !levelAllow.value[level] }
  // Refresh the level-allowed record list so the view narrows even when
  // no search is active. If a search IS active, also re-run it so its
  // hit set is rebuilt under the new mask.
  void refreshAllowedRecords()
  if (searchQuery.value.trim().length > 0) scheduleSearch()
}

function clearSearch() {
  if (searchQuery.value.length === 0) return
  searchQuery.value = ''
  searchError.value = null
  // Cancel any in-flight search and reset hit state. scheduleSearch
  // would do this asynchronously after the debounce; clearing the box
  // should feel instant.
  if (pendingSearchTimer !== null) {
    globalThis.clearTimeout(pendingSearchTimer)
    pendingSearchTimer = null
  }
  if (file.value) {
    invoke('cancel_search', { fileId: file.value.file_id }).catch(() => {})
  }
  hits.value = new Map()
  hitOrder.value = []
  currentHit.value = -1
  searchInflight.value = false
  // Keep keyboard focus in the input so the user can type a fresh
  // query immediately.
  searchInputEl.value?.focus()
}

function setSearchMode(mode: SearchMode) {
  if (searchMode.value === mode) return
  searchMode.value = mode
  scheduleSearch()
}

function isFullLevelMask(): boolean {
  // The "full mask" is every bit set across the levels we track. If the
  // user has all level buttons on AND nothing surprising is present (off
  // / all / unknown are always allowed in the UI), we can take the
  // null-allowed fast path.
  for (const lvl of LEVEL_KEYS) {
    if (!levelAllow.value[lvl]) return false
  }
  return true
}

async function refreshAllowedRecords() {
  if (!file.value) {
    allowedRecords.value = null
    return
  }
  if (isFullLevelMask()) {
    // No narrowing necessary. Use the null sentinel so the renderer
    // skips the filter path entirely.
    allowedRecords.value = null
    return
  }
  try {
    const payload = await invoke<RecordRefsPayload>('list_records_by_level', {
      fileId: file.value.file_id,
      levelMask: buildLevelMask(),
    })
    allowedRecords.value = payload.refs
  } catch {
    // Non-fatal: leave the previous list (or null) in place so the view
    // doesn't blank.
  }
}

// Auto-cancel on file change is handled at pickFile/onBeforeUnmount.

async function applyPattern() {
  if (!file.value) return
  patternError.value = null
  try {
    const args: Record<string, unknown> = { fileId: file.value.file_id }
    if (patternMode.value === 'pattern') args.pattern = patternInput.value
    else args.regex = patternInput.value
    const payload = await invoke<ApplyPatternPayload>('set_pattern', args)
    if (file.value) {
      file.value = {
        ...file.value,
        record_count: payload.record_count,
        pattern_source: payload.pattern_source,
        pattern_name: null,
        loose: payload.loose,
      }
    }
    pages.value = new Map()
    fetchPage(0)
    scheduleMinimapFetch(true)
  } catch (e) {
    const err = e as IpcError | string
    patternError.value = typeof err === 'string' ? err : err.message
  }
}

// Webview's built-in Ctrl+F (and Ctrl+G "find next") opens a find-in-page
// dialog that only searches the DOM, which on a virtualised list is just
// the ~50-100 currently-rendered rows. That's actively misleading -- a
// "no match" answer says nothing about the rest of the file. Swallow it
// here; clog's own search (P6) is the only correct surface.
function suppressBrowserFind(ev: KeyboardEvent) {
  if (!(ev.ctrlKey || ev.metaKey) || ev.altKey) return
  const k = ev.key.toLowerCase()
  if (k === 'f' || k === 'g') {
    ev.preventDefault()
    ev.stopPropagation()
  }
}

// --- Theme + font scaling (P7) ---

const systemDarkMql =
  typeof globalThis !== 'undefined' && typeof globalThis.matchMedia === 'function'
    ? globalThis.matchMedia('(prefers-color-scheme: dark)')
    : null
const systemPrefersDark = ref(!!systemDarkMql?.matches)

function applyTheme(theme: 'system' | 'light' | 'dark') {
  let wantDark = !!systemDarkMql?.matches
  if (theme === 'dark') wantDark = true
  else if (theme === 'light') wantDark = false
  document.documentElement.setAttribute('data-theme', wantDark ? 'dark' : 'light')
  // The minimap's canvas-painted background reads `--bg-viewport` at paint
  // time; switching themes only updates the token, so trigger a repaint
  // here so the stripe doesn't stay in the previous theme's surface colour.
  if (typeof requestAnimationFrame === 'function') {
    requestAnimationFrame(() => paintMinimap())
  }
}

function applyFontSize(px: number) {
  const clamped = Math.max(9, Math.min(24, Math.round(px)))
  document.documentElement.style.setProperty('--font-size-base', `${clamped}px`)
}

async function loadSettings() {
  try {
    const s = (await invoke('get_settings')) as Settings
    settings.value = s
    applyTheme(s.theme)
    applyFontSize(s.font_size)
  } catch (e) {
    // First launch / corrupted file: keep defaults, write them back so the
    // next launch starts from a known shape.
    applyTheme('system')
    applyFontSize(13)
    void e
  }
}

async function updateSettings(patch: Partial<Settings>) {
  try {
    const s = (await invoke('update_settings', { patch })) as Settings
    settings.value = s
    if (patch.theme !== undefined) applyTheme(s.theme)
    if (patch.font_size !== undefined) applyFontSize(s.font_size)
  } catch (e) {
    error.value = (e as IpcError).message ?? String(e)
  }
}

function onSystemThemeChange() {
  systemPrefersDark.value = !!systemDarkMql?.matches
  if (settings.value.theme === 'system') applyTheme('system')
}

function bumpFontSize(delta: number) {
  const target = settings.value.font_size + delta
  void updateSettings({ font_size: target })
}

function resetFontSize() {
  void updateSettings({ font_size: 13 })
}

const THEME_CYCLE = ['system', 'light', 'dark'] as const
const THEME_GLYPH: Record<'light' | 'dark', string> = {
  light: '☀', // sun
  dark: '☽', // crescent moon
}
const THEME_LABEL: Record<'system' | 'light' | 'dark', string> = {
  system: 'Theme: auto (follows OS)',
  light: 'Theme: light',
  dark: 'Theme: dark',
}

/** Glyph to render in the toggle for the current selection. `system` picks
 *  whichever the OS currently resolves to so the icon reflects the live
 *  appearance. */
const themeToggleGlyph = computed(() => {
  if (settings.value.theme === 'light') return THEME_GLYPH.light
  if (settings.value.theme === 'dark') return THEME_GLYPH.dark
  return systemPrefersDark.value ? THEME_GLYPH.dark : THEME_GLYPH.light
})

function cycleTheme() {
  const cur = settings.value.theme
  const idx = THEME_CYCLE.indexOf(cur as typeof THEME_CYCLE[number])
  const next = THEME_CYCLE[(idx + 1) % THEME_CYCLE.length]
  void updateSettings({ theme: next })
}

function handleFontShortcut(ev: KeyboardEvent): boolean {
  if (!(ev.ctrlKey || ev.metaKey) || ev.altKey || ev.shiftKey) return false
  if (ev.key === '+' || ev.key === '=') {
    ev.preventDefault()
    bumpFontSize(1)
    return true
  }
  if (ev.key === '-' || ev.key === '_') {
    ev.preventDefault()
    bumpFontSize(-1)
    return true
  }
  if (ev.key === '0') {
    ev.preventDefault()
    resetFontSize()
    return true
  }
  return false
}

// --- Session restore + autosave (P7) ---

function captureSession(): RestoredFile | null {
  if (!file.value) return null
  return {
    path: file.value.path,
    scroll_top: scrollEl.value?.scrollTop ?? 0,
    follow_tail: followTail.value,
    level_mask: buildLevelMask(),
    filter_text: searchQuery.value,
    search_mode: searchMode.value,
    search_case_sensitive: searchCaseSensitive.value,
    filter_mode: filterMode.value,
  }
}

function scheduleSessionSave() {
  if (sessionSaveTimer.value !== null) {
    globalThis.clearTimeout(sessionSaveTimer.value)
  }
  sessionSaveTimer.value = globalThis.setTimeout(() => {
    sessionSaveTimer.value = null
    const last = captureSession()
    void invoke('save_session', { session: { schema: 1, last_file: last } }).catch(() => {})
  }, 400) as unknown as number
}

async function restoreSession() {
  try {
    const sess = (await invoke('get_session')) as Session
    const last = sess.last_file
    if (!last) return
    pendingRestore.value = last
    sessionRestoreInFlight.value = true
    await openPath(last.path)
  } catch {
    // Last file gone or unreadable. Forget it from recents + session and
    // launch into the empty state.
    if (pendingRestore.value?.path) {
      void invoke('forget_recent', { path: pendingRestore.value.path }).catch(() => {})
    }
    pendingRestore.value = null
    sessionRestoreInFlight.value = false
    void invoke('save_session', { session: { schema: 1, last_file: null } }).catch(() => {})
  }
}

async function applyPendingRestore() {
  const r = pendingRestore.value
  if (!r) return
  // Restore filter/search state BEFORE the search fires, so the first
  // search run uses the persisted level mask + case flag.
  for (const lvl of LEVEL_KEYS) {
    levelAllow.value[lvl] = !!(r.level_mask & LEVEL_BIT[lvl])
  }
  searchMode.value = r.search_mode === 'regex' ? 'regex' : 'smart'
  searchCaseSensitive.value = !!r.search_case_sensitive
  filterMode.value = !!r.filter_mode
  searchQuery.value = r.filter_text ?? ''
  followTail.value = !!r.follow_tail
  // Wait for the virtualizer to mount the rows, then restore scroll.
  globalThis.requestAnimationFrame(() => {
    globalThis.requestAnimationFrame(() => {
      if (scrollEl.value && r.scroll_top > 0) {
        scrollEl.value.scrollTop = r.scroll_top
      }
      // Run search now that all the restored knobs are in place.
      if (searchQuery.value.trim().length > 0) scheduleSearch()
      else if (!isFullLevelMask()) void refreshAllowedRecords()
      pendingRestore.value = null
      sessionRestoreInFlight.value = false
    })
  })
}

// --- Custom window chrome (P7 polish) ---

const windowMaximized = ref(false)
const appWindow = getCurrentWindow()
const appWebview = getCurrentWebview()
let unlistenWindow: (() => void) | null = null
let unlistenDragDrop: (() => void) | null = null
const dragHover = ref(false)

async function refreshMaximized() {
  try {
    windowMaximized.value = await appWindow.isMaximized()
  } catch {
    windowMaximized.value = false
  }
}

async function minimizeWindow() {
  try {
    await appWindow.minimize()
  } catch (e) {
    error.value = (e as IpcError).message ?? String(e)
  }
}

async function toggleMaximizeWindow() {
  try {
    await appWindow.toggleMaximize()
    await refreshMaximized()
  } catch (e) {
    error.value = (e as IpcError).message ?? String(e)
  }
}

async function closeWindow() {
  try {
    await appWindow.close()
  } catch (e) {
    error.value = (e as IpcError).message ?? String(e)
  }
}

async function openPath(path: string) {
  if (busy.value) return
  busy.value = true
  error.value = null
  try {
    if (file.value) {
      const oldId = file.value.file_id
      void invoke('cancel_search', { fileId: oldId }).catch(() => {})
      void invoke('stop_tail', { fileId: oldId }).catch(() => {})
      void invoke('close_file', { fileId: oldId }).catch(() => {})
    }
    pages.value = new Map()
    inflight.clear()
    hits.value = new Map()
    hitOrder.value = []
    currentHit.value = -1
    allowedRecords.value = null
    const opened = (await invoke('open_file', { path })) as OpenedFile
    file.value = opened
    patternInput.value = opened.pattern_source
    patternMode.value = opened.pattern_source.startsWith('regex:') ? 'regex' : 'pattern'
    patternScore.value = null
    patternError.value = null
    lastTailLineCount = opened.line_count
    void startTail()
    void fetchMinimap(true)
    if (pendingRestore.value) {
      void applyPendingRestore()
    }
  } catch (e) {
    error.value = (e as IpcError).message ?? String(e)
    pendingRestore.value = null
    sessionRestoreInFlight.value = false
  } finally {
    busy.value = false
  }
}

async function openSettings() {
  try {
    dataDir.value = (await invoke('get_data_dir')) as DataDirPayload
  } catch {
    dataDir.value = null
  }
  settingsOpen.value = true
}

async function openDataFolder() {
  try {
    await invoke('open_data_dir')
  } catch (e) {
    error.value = (e as IpcError).message ?? String(e)
  }
}

async function resetData(scope: 'settings' | 'session' | 'patterns' | 'index' | 'all') {
  try {
    await invoke('reset_data', { req: { scope } })
    if (scope === 'settings' || scope === 'all') {
      await loadSettings()
    }
  } catch (e) {
    error.value = (e as IpcError).message ?? String(e)
  }
}

async function openRecent(path: string) {
  settingsOpen.value = false
  await openPath(path)
}

async function forgetRecent(path: string) {
  try {
    const s = (await invoke('forget_recent', { path })) as Settings
    settings.value = s
  } catch (e) {
    error.value = (e as IpcError).message ?? String(e)
  }
}

// Persist the user-visible knobs to session.json whenever they change. The
// scroll position autosaves from onViewportScroll (added below); everything
// else flows through this watcher.
watch(
  [
    () => file.value?.path,
    () => followTail.value,
    () => searchMode.value,
    () => searchQuery.value,
    () => searchCaseSensitive.value,
    () => filterMode.value,
    () => buildLevelMask(),
  ],
  () => {
    if (sessionRestoreInFlight.value) return
    scheduleSessionSave()
  },
)

function onGlobalKey(ev: KeyboardEvent) {
  if (handleFontShortcut(ev)) return
  suppressBrowserFind(ev)
}

onMounted(() => {
  globalThis.addEventListener('keydown', onGlobalKey, { capture: true })
  // Theme + settings boot, then session restore.
  void (async () => {
    await loadSettings()
    systemDarkMql?.addEventListener?.('change', onSystemThemeChange)
    void refreshMaximized()
    try {
      const unlistenResize = await appWindow.onResized(() => void refreshMaximized())
      unlistenWindow = unlistenResize
    } catch {
      unlistenWindow = null
    }
    try {
      unlistenDragDrop = await appWebview.onDragDropEvent((evt) => {
        const t = evt.payload.type
        if (t === 'enter' || t === 'over') {
          dragHover.value = true
        } else if (t === 'leave') {
          dragHover.value = false
        } else if (t === 'drop') {
          dragHover.value = false
          const paths = (evt.payload as { paths?: string[] }).paths ?? []
          const first = paths.find((p) => typeof p === 'string' && p.length > 0)
          if (first) void openPath(first)
        }
      })
    } catch {
      unlistenDragDrop = null
    }
    await restoreSession()
  })()
  // Track viewport height so the minimap buckets match available pixels.
  // Pure-CSS height = bucket count; ResizeObserver tells us when to refetch.
  resizeObserver = new ResizeObserver((entries) => {
    for (const entry of entries) {
      const h = Math.floor(entry.contentRect.height)
      if (h !== viewportHeightPx.value) {
        viewportHeightPx.value = h
        scheduleMinimapFetch()
      }
    }
  })
  // Bind once the viewport DOM exists (it's gated by `v-if="file"`); watch
  // the ref and (re)attach as files open/close.
  watch(
    () => scrollEl.value,
    (el) => {
      if (resizeObserver) resizeObserver.disconnect()
      if (el && resizeObserver) {
        resizeObserver.observe(el)
        viewportHeightPx.value = Math.floor(el.clientHeight)
        scheduleMinimapFetch()
      }
    },
    { immediate: true },
  )
})

onBeforeUnmount(() => {
  globalThis.removeEventListener('keydown', onGlobalKey, { capture: true })
  systemDarkMql?.removeEventListener?.('change', onSystemThemeChange)
  if (unlistenWindow) {
    unlistenWindow()
    unlistenWindow = null
  }
  if (unlistenDragDrop) {
    unlistenDragDrop()
    unlistenDragDrop = null
  }
  if (sessionSaveTimer.value !== null) globalThis.clearTimeout(sessionSaveTimer.value)
  if (resizeObserver) {
    resizeObserver.disconnect()
    resizeObserver = null
  }
  if (tailPulseTimer !== null) globalThis.clearTimeout(tailPulseTimer)
  if (rotationToastTimer !== null) globalThis.clearTimeout(rotationToastTimer)
  if (file.value) {
    const id = file.value.file_id
    invoke('cancel_search', { fileId: id }).catch(() => {})
    invoke('stop_tail', { fileId: id }).catch(() => {})
    invoke('close_file', { fileId: id }).catch(() => {})
  }
  if (pendingSearchTimer !== null) globalThis.clearTimeout(pendingSearchTimer)
})

// --- Header-line span slicing (axis-1) + axis-2 highlight overlay. ---

/**
 * Slice `text` into axis-1 base spans driven by `fields`. Any gap between
 * known fields is emitted as a `sep` span (the literal text between two
 * structural fields, e.g. brackets, dashes, spaces). The result is then
 * overlaid with the current axis-2 highlight rule set in {@link renderLine}.
 */
function headerBaseSpans(
  text: string,
  fields: HeaderFields,
): Array<{ start: number; end: number; cls: string }> {
  type Mark = { start: number; end: number; cls: string }
  const marks: Mark[] = []
  if (fields.level) marks.push({ start: fields.level[0], end: fields.level[1], cls: 'level' })
  if (fields.timestamp)
    marks.push({ start: fields.timestamp[0], end: fields.timestamp[1], cls: 'timestamp' })
  if (fields.thread) marks.push({ start: fields.thread[0], end: fields.thread[1], cls: 'thread' })
  if (fields.logger) marks.push({ start: fields.logger[0], end: fields.logger[1], cls: 'logger' })
  if (fields.message)
    marks.push({ start: fields.message[0], end: fields.message[1], cls: 'message' })
  marks.sort((a, b) => a.start - b.start)
  const out: Mark[] = []
  let cursor = 0
  for (const m of marks) {
    if (m.start > cursor) out.push({ start: cursor, end: m.start, cls: 'sep' })
    out.push(m)
    cursor = m.end
  }
  if (cursor < text.length) out.push({ start: cursor, end: text.length, cls: 'sep' })
  return out
}

/**
 * Produce the final leaf spans for a row, blending axis-1 (structural) and
 * axis-2 (highlight rules). For a header row we slice by fields; for a
 * continuation row we treat the whole text as a single `message` base span
 * so the gutter / indentation styling continues to apply.
 */
function searchSpansForLine(row: LineRow): { start: number; end: number; cls: string }[] {
  const hit = hits.value.get(row.record_idx)
  if (!hit) return []
  const boff = row.byte_offset_in_record
  const len = row.text.length
  const out: { start: number; end: number; cls: string }[] = []
  for (const [s, e] of hit.ranges) {
    // Hit ranges are record-relative bytes; line text is ASCII-assumed
    // (matches axis-1 field-span treatment). Clamp into [0, len).
    const ls = Math.max(0, s - boff)
    const le = Math.min(len, e - boff)
    if (le > ls) out.push({ start: ls, end: le, cls: 'h-search-match' })
  }
  return out
}

function renderLine(row: LineRow): LeafSpan[] {
  const search = searchSpansForLine(row)
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

// The level-colour class is keyed by the row's level, not by the cls itself,
// so we tack it on after overlay. Only the s-level base span carries it.
function decorateLevels(leaves: LeafSpan[], level: string): LeafSpan[] {
  if (!leaves.some((l) => l.cls.includes('s-level'))) return leaves
  return leaves.map((l) =>
    l.cls.includes('s-level') ? { ...l, cls: l.cls + ' level-' + level } : l,
  )
}

async function onSpanClick(span: LeafSpan, ev: MouseEvent) {
  if (!span.url) return
  ev.preventDefault()
  try {
    await openUrl(span.url)
  } catch (e) {
    // Surface the failure but do not interrupt rendering. The opener can
    // legitimately refuse mailto:/javascript: targets, and a broken URL on
    // one line should not break the viewer.
    error.value = (e as Error).message
  }
}

function levelGutterVar(level: string): string {
  // Trust-but-verify: only the known set drives a CSS variable; everything
  // else falls back to --level-unknown.
  const known = new Set([
    'trace',
    'debug',
    'info',
    'warn',
    'error',
    'fatal',
    'off',
    'all',
    'unknown',
  ])
  const key = known.has(level) ? level : 'unknown'
  return `var(--level-${key})`
}
</script>

<template>
  <main class="shell">
    <header class="bar" data-tauri-drag-region>
      <h1 data-tauri-drag-region class="app-title">
        <img src="/clog-icon.png" alt="Clog" class="app-icon" data-tauri-drag-region title="Clog: The Core log viewer" />
      </h1>
      <button :disabled="busy" @click="pickFile">
        {{ busy ? 'Reading...' : 'Open file...' }}
      </button>
      <button
        type="button"
        class="settings-btn"
        title="Settings"
        aria-label="Open settings"
        @click="openSettings"
      >&#9881;</button>
      <span v-if="file" class="meta">
        <strong>{{ basename(file.path) }}</strong>
      </span>
      <span v-if="file" class="tail-controls">
        <span
          class="tail-indicator"
          :class="{
            'is-active': tailing,
            'is-pulsing': tailPulse,
          }"
          :title="tailing ? 'Tailing this file' : 'Tail inactive'"
        >
          <span class="dot" />
          {{ tailing ? 'tailing' : 'idle' }}
        </span>
        <button
          type="button"
          class="follow-toggle"
          :class="{ 'is-on': followTail }"
          :title="followTail ? 'Auto-scroll is on -- click to detach' : 'Auto-scroll is off'"
          @click="toggleFollowTail"
        >
          {{ followTail ? 'Following' : 'Follow' }}
        </button>
      </span>
      <span class="window-controls" :class="{ 'no-file': !file }">
        <button
          type="button"
          class="win-btn"
          title="Minimize"
          aria-label="Minimize"
          @click="minimizeWindow"
        >&#9472;</button>
        <button
          type="button"
          class="win-btn"
          :title="windowMaximized ? 'Restore' : 'Maximize'"
          :aria-label="windowMaximized ? 'Restore' : 'Maximize'"
          @click="toggleMaximizeWindow"
        >{{ windowMaximized ? '⧉' : '□' }}</button>
        <button
          type="button"
          class="win-btn close"
          title="Close"
          aria-label="Close"
          @click="closeWindow"
        >&times;</button>
      </span>
    </header>

    <section v-if="error" class="error">{{ error }}</section>

    <div v-if="rotationToast" class="rotation-toast">{{ rotationToast }}</div>

    <div v-if="file" class="viewport-shell">
    <div ref="scrollEl" class="viewport" @scroll.passive="onViewportScroll">
      <div v-if="stickyHeader" class="sticky-shell">
        <div
          class="row is-header"
          :class="'level-row-' + stickyHeader.row.level"
          :style="{ '--gutter-color': levelGutterVar(stickyHeader.row.level) }"
        >
          <span class="gutter" />
          <button
            type="button"
            class="idx jump-up"
            :title="`Jump to start of record ${stickyHeader.row.record_idx + 1}`"
            @click="jumpToStickyStart"
          >&uarr;</button>
          <span class="txt">
            <span
              v-for="(span, si) in renderLine(stickyHeader.row)"
              :key="si"
              :class="span.cls"
              :data-url="span.url || null"
              @click="span.url && onSpanClick(span, $event)"
            >{{ span.text }}</span>
          </span>
        </div>
      </div>
      <div class="total" :style="{ height: `${totalSize}px` }">
        <template v-for="vrow in virtualRows" :key="String(vrow.key)">
        <div
          v-if="lineRowVirtual(vrow.index)"
          class="row"
          :class="[
            {
              'is-header': lineRowVirtual(vrow.index)?.line_within_record === 0,
              'is-continuation': (lineRowVirtual(vrow.index)?.line_within_record ?? 0) > 0,
              'is-current-hit': isCurrentHitRow(lineRowVirtual(vrow.index)),
            },
            'level-row-' + (lineRowVirtual(vrow.index)?.level ?? 'unknown'),
          ]"
          :style="{
            transform: `translateY(${vrow.start}px)`,
            height: `${vrow.size}px`,
            '--gutter-color': levelGutterVar(lineRowVirtual(vrow.index)?.level ?? 'unknown'),
          }"
        >
          <span class="gutter" />
          <span class="idx">{{ actualLineIndex(vrow.index) + 1 }}</span>
          <span class="txt">
              <span
                v-for="(span, si) in renderLine(lineRowVirtual(vrow.index)!)"
                :key="si"
                :class="span.cls"
                :data-url="span.url || null"
                @click="span.url && onSpanClick(span, $event)"
              >{{ span.text }}</span>
          </span>
        </div>
        </template>
      </div>
    </div>
      <div
        class="minimap"
        @pointerdown="onMinimapPointerDown"
        @pointermove="onMinimapPointerMove"
        @pointerup="onMinimapPointerUp"
        @pointercancel="onMinimapPointerUp"
        @pointerenter="onMinimapPointerEnter"
        @pointerleave="onMinimapPointerLeave"
      >
        <canvas ref="minimapEl" class="minimap-canvas" />
        <div
          v-if="minimapIndicator.visible"
          class="minimap-indicator"
          :style="{ top: `${minimapIndicator.top}px`, height: `${minimapIndicator.height}px` }"
        />
        <div
          v-if="minimapTooltip.visible"
          class="minimap-tooltip"
          :style="{ top: `${minimapTooltip.top}px`, left: `${minimapTooltip.left}px` }"
        >
          <span class="line-no">line {{ minimapTooltip.lineIndex + 1 }}</span>
          <span v-if="minimapTooltip.timestamp" class="ts">{{ minimapTooltip.timestamp }}</span>
          <span v-else class="ts muted">--</span>
        </div>
      </div>
      <button
        v-if="!followTail && !atBottom"
        type="button"
        class="jump-bottom-floating"
        title="Jump to bottom and re-enable follow"
        aria-label="Jump to bottom"
        @click="toggleFollowTail"
      >&darr;</button>
    </div>
    <p v-else class="placeholder">No file open. Click <em>Open file...</em> to pick one.</p>

    <section v-if="file" class="search-bar">
      <fieldset class="mode-toggle">
        <legend class="sr-only">Search mode</legend>
        <span class="mode-label">Search:</span>
        <button
          type="button"
          class="mode-btn"
          :class="{ 'is-on': searchMode === 'smart' }"
          :aria-pressed="searchMode === 'smart'"
          title="Smart proximity-ranked substring search"
          @click="setSearchMode('smart')"
        >Smart</button>
        <button
          type="button"
          class="mode-btn"
          :class="{ 'is-on': searchMode === 'regex' }"
          :aria-pressed="searchMode === 'regex'"
          title="Regular expression search (regex::bytes)"
          @click="setSearchMode('regex')"
        >Regex</button>
      </fieldset>
      <span class="search-input-wrap">
        <input
          ref="searchInputEl"
          v-model="searchQuery"
          class="search-input"
          :class="{ 'has-error': !!searchError }"
          :placeholder="searchMode === 'smart' ? `e.g., 'installed core'...` : `regular expression, e.g., 'installed.*core'...`"
          spellcheck="false"
          @input="scheduleSearch"
          @keydown.enter.prevent="nextHit"
          @keydown.shift.enter.prevent="prevHit"
          @keydown.esc.prevent="clearSearch"
        />
        <button
          v-if="searchQuery.length > 0"
          type="button"
          class="clear-search"
          title="Clear search (Esc)"
          aria-label="Clear search"
          @click="clearSearch"
        >&times;</button>
      </span>
      <label class="case" title="Case-sensitive search" @click="scheduleSearch">
        <input type="checkbox" v-model="searchCaseSensitive" @change="scheduleSearch" />
        Aa
      </label>
      <span v-if="hitOrder.length > 0" class="hit-count">
        <strong>{{ currentHit + 1 }}</strong> / {{ hitOrder.length }}
      </span>
      <span v-else-if="searchQuery.trim() && !searchInflight && !searchError" class="hit-count muted">
        0 hits
      </span>
      <span v-else-if="searchInflight" class="hit-count muted">searching...</span>
      <button type="button" :disabled="hitOrder.length === 0" @click="prevHit">&uarr;</button>
      <button type="button" :disabled="hitOrder.length === 0" @click="nextHit">&darr;</button>
      <button
        type="button"
        class="filter-toggle"
        :class="{ 'is-on': filterMode }"
        :title="filterMode ? 'Showing only matching records -- click to show all' : 'Filter to matching records'"
        @click="toggleFilterMode"
      >
        {{ filterMode ? 'Filter on' : 'Filter' }}
      </button>
      <span class="level-mask">
        <button
          v-for="lvl in LEVEL_KEYS"
          :key="lvl"
          type="button"
          class="lvl-btn"
          :class="['lvl-' + lvl, { 'is-off': !levelAllow[lvl] }]"
          :title="`Toggle ${lvl.toUpperCase()} records`"
          @click="toggleLevel(lvl)"
        >{{ lvl.toUpperCase() }}</button>
      </span>
      <span v-if="searchError" class="search-error">{{ searchError }}</span>
    </section>

    <footer class="status-bar">
      <span class="slot left">
        <span v-if="file?.cache_hit" class="cache-hint" title="Records loaded from the on-disk index cache">cached</span>
      </span>
      <span class="slot right">
        <template v-if="file">
          <span class="stat">{{ formatCount(file.record_count) }} records</span>
          <span class="stat">{{ formatCount(file.line_count) }} lines</span>
          <span class="stat" :title="`${formatCount(file.size_bytes)} bytes`">{{ formatBytes(file.size_bytes) }}</span>
        </template>
        <button
          type="button"
          class="theme-toggle"
          :class="{ 'is-auto': settings.theme === 'system' }"
          :title="THEME_LABEL[settings.theme]"
          :aria-label="THEME_LABEL[settings.theme]"
          @click="cycleTheme"
        >{{ themeToggleGlyph }}</button>
        <span class="font-size-hint" :title="`Base font size (Ctrl-+ / Ctrl-- / Ctrl-0)`">
          {{ settings.font_size }}px
        </span>
        <span v-if="file" class="pattern-status">
          <span class="pattern-label" :title="file.pattern_source">
            Pattern:
            <strong>{{ file.pattern_name ?? 'custom' }}</strong>
          </span>
          <button
            type="button"
            class="pattern-edit-btn"
            title="Edit pattern"
            aria-label="Edit pattern"
            @click="patternOpen = true"
          >Edit</button>
        </span>
      </span>
    </footer>

    <div v-if="dragHover" class="drop-overlay">
      <div class="drop-hint">
        <span class="arrow">&darr;</span>
        Drop a log file to open it
      </div>
    </div>

    <div v-if="patternOpen && file" class="modal-backdrop" @click.self="patternOpen = false">
      <div class="modal pattern-modal" role="dialog" aria-label="Pattern">
        <header class="modal-head">
          <h2>Pattern</h2>
          <button
            type="button"
            class="modal-close"
            aria-label="Close pattern editor"
            @click="patternOpen = false"
          >&times;</button>
        </header>
        <section class="modal-body">
          <div class="row-grid">
            <label>Kind</label>
            <select v-model="patternMode">
              <option value="pattern">PatternLayout</option>
              <option value="regex">Regex</option>
            </select>
          </div>
          <div class="pattern-input-row">
            <input
              v-model="patternInput"
              class="pat-input"
              :placeholder="patternMode === 'pattern'
                ? '[%-5level] %d{yyyy-MM-dd HH:mm:ss.SSS} [%t] %c{1} - %msg%n'
                : '^(?P&lt;timestamp&gt;\\d{4}-...) (?P&lt;level&gt;INFO|WARN|ERROR) ...'"
              spellcheck="false"
            />
            <button type="button" @click="testPattern">Test</button>
            <button type="button" @click="applyPattern">Apply</button>
          </div>
          <p v-if="patternScore !== null" class="score">
            Match score: <strong>{{ (patternScore * 100).toFixed(1) }}%</strong>
            <span v-if="patternSampleSize > 0" class="muted"> of {{ patternSampleSize }} lines</span>
          </p>
          <p v-if="file.pattern_name" class="muted">
            Auto-detected: <strong>{{ file.pattern_name }}</strong>
          </p>
          <p v-if="patternError" class="pat-error">{{ patternError }}</p>
          <p class="muted">
            Apply saves the pattern as a per-file override; the next time you open this file the same pattern is used automatically.
          </p>
        </section>
      </div>
    </div>

    <div v-if="settingsOpen" class="modal-backdrop" @click.self="settingsOpen = false">
      <div class="modal" role="dialog" aria-label="Settings">
        <header class="modal-head">
          <h2>Settings</h2>
          <button
            type="button"
            class="modal-close"
            aria-label="Close settings"
            @click="settingsOpen = false"
          >&times;</button>
        </header>
        <section class="modal-body">
          <h3>Appearance</h3>
          <div class="row-grid">
            <span class="row-label">Theme</span>
            <span class="seg">
              <button
                v-for="opt in (['system', 'light', 'dark'] as const)"
                :key="opt"
                type="button"
                class="seg-btn"
                :class="{ 'is-on': settings.theme === opt }"
                @click="updateSettings({ theme: opt })"
              >{{ opt[0].toUpperCase() + opt.slice(1) }}</button>
            </span>
          </div>
          <div class="row-grid">
            <span class="row-label">Font size</span>
            <span class="seg font-seg">
              <button type="button" class="seg-btn" @click="bumpFontSize(-1)" title="Decrease (Ctrl--)">&minus;</button>
              <button type="button" class="seg-btn font-val" @click="resetFontSize" title="Reset to default (Ctrl-0)">{{ settings.font_size }}px</button>
              <button type="button" class="seg-btn" @click="bumpFontSize(1)" title="Increase (Ctrl-+)">+</button>
            </span>
          </div>

          <h3>Behaviour</h3>
          <div class="row-grid">
            <label for="follow-tail-default">Follow tail by default</label>
            <span class="control-cell">
              <input
                id="follow-tail-default"
                type="checkbox"
                :checked="settings.follow_tail_default"
                @change="(e: Event) => updateSettings({ follow_tail_default: (e.target as HTMLInputElement).checked })"
              />
            </span>
          </div>

          <h3>Recent files</h3>
          <ul v-if="settings.recent_files.length > 0" class="recent-list">
            <li v-for="p in settings.recent_files" :key="p">
              <button type="button" class="open-btn" @click="openRecent(p)">{{ basename(p) }}</button>
              <span class="path">{{ p }}</span>
              <button type="button" class="forget-btn" @click="forgetRecent(p)" title="Remove from list">&times;</button>
            </li>
          </ul>
          <p v-else class="muted">No recent files yet. Open a log to populate this list.</p>

          <h3>Advanced</h3>
          <div class="row-grid">
            <span class="row-label">Data folder</span>
            <span class="control-cell data-cell">
              <code class="data-path">{{ dataDir?.path ?? '(loading...)' }}</code>
              <span v-if="dataDir?.portable" class="badge">portable</span>
              <button type="button" class="seg-btn" @click="openDataFolder">Open folder</button>
            </span>
          </div>
          <div class="reset-grid">
            <div class="row-grid">
              <span class="row-label">Session state</span>
              <span class="control-cell"><button type="button" class="seg-btn" @click="resetData('session')">Reset</button></span>
            </div>
            <div class="row-grid">
              <span class="row-label">Settings</span>
              <span class="control-cell"><button type="button" class="seg-btn" @click="resetData('settings')">Reset</button></span>
            </div>
            <div class="row-grid">
              <span class="row-label">Pattern overrides</span>
              <span class="control-cell"><button type="button" class="seg-btn" @click="resetData('patterns')">Reset</button></span>
            </div>
            <div class="row-grid">
              <span class="row-label">Index cache</span>
              <span class="control-cell"><button type="button" class="seg-btn" @click="resetData('index')">Clear</button></span>
            </div>
            <div class="row-grid">
              <span class="row-label">Everything</span>
              <span class="control-cell"><button type="button" class="seg-btn danger" @click="resetData('all')">Reset all data</button></span>
            </div>
          </div>

          <p class="footer-note muted">
            Custom highlighting rules and automatic update checks are planned for a later milestone.
            Built-in highlights cover Java exceptions, <code>Caused by:</code>, stack frames, file paths and URLs.
          </p>
        </section>
      </div>
    </div>
  </main>
</template>

<style scoped>
.shell {
  display: flex;
  flex-direction: column;
  height: 100vh;
  font-family: var(--font-sans);
  color: var(--fg-default);
  background: var(--bg-app);
  /* Frameless window: a 1px hairline keeps the app visually contained
     against the desktop and gives a subtle theme-aware edge. */
  border: 1px solid var(--border-default);
}

.bar {
  display: flex;
  align-items: center;
  gap: 0.8rem;
  padding: 0.6rem 1rem;
  border-bottom: 1px solid var(--border-default);
  flex-wrap: wrap;

  h1 {
    margin: 0;
    font-size: 1.1rem;
    letter-spacing: 0.02em;
    display: inline-flex;
    align-items: center;
  }

  .app-icon {
    display: block;
    height: 22px;
    width: 22px;
    /* Let the browser do its default high-quality bilinear/bicubic
       downscaling. Any explicit `image-rendering` hint (`pixelated`,
       `-webkit-optimize-contrast`) on a downscale ends up looking like
       nearest-neighbour on Chromium. */
    image-rendering: auto;
    object-fit: contain;
    pointer-events: none;
  }

  button {
    background: var(--bg-button);
    color: var(--fg-default);
    border: 1px solid var(--border-button);
    padding: 0.35rem 0.9rem;
    border-radius: var(--radius-sm);
    font-size: 0.9rem;
    cursor: pointer;

    &:hover:not(:disabled) { background: var(--bg-button-hover); }
    &:disabled { opacity: 0.6; cursor: progress; }
  }

  .meta {
    color: var(--fg-muted);
    font-family: var(--font-mono);
    font-size: 0.85rem;

    .sep { color: var(--fg-separator); margin: 0 0.5rem; }
  }

  .settings-btn {
    margin-left: 0.2rem;
    padding: 0.35rem 0.55rem;
    font-size: 1rem;
    line-height: 1;
  }

  /* Frameless-window controls. Always sit at the far right of the header.
     When no file is open, the tail-controls cluster (which normally owns
     margin-left: auto) is absent, so this cluster carries the push. */
  .window-controls {
    display: inline-flex;
    align-items: center;
    gap: 0.15rem;
    -webkit-app-region: no-drag;

    &.no-file { margin-left: auto; }

    .win-btn {
      background: transparent;
      color: var(--fg-muted);
      border: 0;
      padding: 0.3rem 0.7rem;
      font-size: 0.95rem;
      line-height: 1;
      cursor: pointer;
      border-radius: var(--radius-sm);

      &:hover {
        background: var(--bg-button-hover);
        color: var(--fg-default);
      }

      &.close:hover {
        background: var(--level-error);
        color: var(--fg-on-accent);
      }
    }
  }

  .tail-controls {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.85rem;
    font-family: var(--font-mono);

    .tail-indicator {
      display: inline-flex;
      align-items: center;
      gap: 0.35rem;
      color: var(--fg-dim);

      .dot {
        width: 0.55rem;
        height: 0.55rem;
        border-radius: 50%;
        background: var(--fg-dim);
        transition: background 0.15s ease;
      }

      &.is-active {
        color: var(--fg-default);

        .dot { background: var(--level-info); }
      }

      &.is-pulsing .dot {
        background: var(--level-warn);
        box-shadow: 0 0 6px var(--level-warn);
      }
    }

    .follow-toggle {
      background: var(--bg-button);
      color: var(--fg-default);
      border: 1px solid var(--border-button);
      padding: 0.25rem 0.7rem;
      border-radius: var(--radius-sm);
      font-size: 0.8rem;
      font-family: var(--font-mono);
      cursor: pointer;

      &:hover { background: var(--bg-button-hover); }
    }

    .follow-toggle.is-on {
      border-color: var(--level-info);
      color: var(--level-info);
    }
  }
}

.rotation-toast {
  position: fixed;
  bottom: 1rem;
  right: 1rem;
  z-index: 10;
  padding: 0.5rem 0.8rem;
  background: var(--bg-elevated);
  border: 1px solid var(--level-warn);
  border-radius: var(--radius-sm);
  color: var(--fg-default);
  font-family: var(--font-mono);
  font-size: 0.85rem;
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.4);
}

.pattern-bar {
  display: flex;
  align-items: center;
  gap: 0.6rem;
  padding: 0.5rem 1rem;
  border-bottom: 1px solid var(--border-default);
  background: var(--bg-elevated);
  flex-wrap: wrap;
  font-size: 0.85rem;
  color: var(--fg-muted);

  .kind {
    display: flex;
    align-items: center;
    gap: 0.3rem;

    select {
      background: var(--bg-button);
      color: var(--fg-default);
      border: 1px solid var(--border-button);
      border-radius: var(--radius-sm);
      padding: 0.2rem 0.4rem;
      font-size: 0.85rem;
    }
  }

  .pat-input {
    flex: 1 1 24rem;
    min-width: 18rem;
    background: var(--bg-viewport);
    color: var(--fg-default);
    border: 1px solid var(--border-button);
    border-radius: var(--radius-sm);
    padding: 0.3rem 0.5rem;
    font-family: var(--font-mono);
    font-size: 0.85rem;
  }

  button {
    background: var(--bg-button);
    color: var(--fg-default);
    border: 1px solid var(--border-button);
    border-radius: var(--radius-sm);
    padding: 0.3rem 0.8rem;
    font-size: 0.85rem;
    cursor: pointer;

    &:hover { background: var(--bg-button-hover); }
  }

  .score, .auto {
    font-family: var(--font-mono);

    strong { color: var(--fg-default); }
    .muted { color: var(--fg-dim); }
  }

  .pat-error {
    color: var(--fg-error);
    font-family: var(--font-mono);
  }
}

.error {
  margin: 0.6rem 1rem;
  padding: 0.6rem 0.8rem;
  background: var(--bg-error);
  border: 1px solid var(--border-error);
  border-radius: var(--radius-sm);
  color: var(--fg-error);
}

.search-bar {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.5rem 1rem;
  border-bottom: 1px solid var(--border-default);
  background: var(--bg-elevated);
  flex-wrap: wrap;
  font-size: 0.85rem;
  color: var(--fg-muted);

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }

  /* Segmented control. The two buttons share a border and the active
     one inverts to read as "currently selected" without needing a
     separate indicator. */
  .mode-toggle {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    border: none;
    padding: 0;
    margin: 0;

    .mode-label {
      color: var(--fg-muted);
    }

    .mode-btn {
      background: var(--bg-button);
      color: var(--fg-muted);
      border: 1px solid var(--border-button);
      padding: 0.25rem 0.7rem;
      font-size: 0.8rem;
      font-family: var(--font-mono);
      cursor: pointer;

      /* Join the two buttons into a single segmented control. */
      &:first-of-type {
        border-radius: var(--radius-sm) 0 0 var(--radius-sm);
        border-right-width: 0;
      }
      &:last-of-type {
        border-radius: 0 var(--radius-sm) var(--radius-sm) 0;
      }

      &:hover:not(.is-on) { background: var(--bg-button-hover); }

      &.is-on {
        background: var(--level-info);
        color: var(--fg-on-accent);
        border-color: var(--level-info);
        font-weight: 600;
      }
    }
  }

  /* Wrap the input so the clear button can overlay its right edge. */
  .search-input-wrap {
    flex: 1 1 16rem;
    min-width: 12rem;
    position: relative;
    display: inline-flex;
    align-items: stretch;
  }

  .search-input {
    flex: 1 1 auto;
    width: 100%;
    background: var(--bg-viewport);
    color: var(--fg-default);
    border: 1px solid var(--border-button);
    border-radius: var(--radius-sm);
    /* Right padding leaves room for the clear button so a long query
       doesn't slide under the cross. */
    padding: 0.3rem 1.6rem 0.3rem 0.5rem;
    font-family: var(--font-mono);
    font-size: 0.85rem;

    &.has-error {
      border-color: var(--level-error);
      color: var(--fg-error);
      text-decoration: underline;
      text-decoration-color: var(--level-error);
      text-decoration-style: wavy;
    }

    &::placeholder {
      color: var(--fg-dim);
      font-style: italic;
    }
  }

  .clear-search {
    position: absolute;
    top: 50%;
    right: 0.3rem;
    transform: translateY(-50%);
    width: 1.1rem;
    height: 1.1rem;
    padding: 0;
    background: transparent;
    color: var(--fg-dim);
    border: none;
    border-radius: 50%;
    font-family: var(--font-sans);
    font-size: 1.1rem;
    line-height: 1;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;

    &:hover {
      background: var(--bg-button-hover);
      color: var(--fg-default);
    }

    &:focus-visible {
      outline: 1px solid var(--level-info);
      outline-offset: 1px;
    }
  }

  .case {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    font-family: var(--font-mono);
    cursor: pointer;
    user-select: none;
  }

  .hit-count {
    font-family: var(--font-mono);
    color: var(--fg-default);

    strong { color: var(--level-info); }
    &.muted { color: var(--fg-dim); }
  }

  button {
    background: var(--bg-button);
    color: var(--fg-default);
    border: 1px solid var(--border-button);
    border-radius: var(--radius-sm);
    padding: 0.25rem 0.55rem;
    font-size: 0.8rem;
    font-family: var(--font-mono);
    cursor: pointer;

    &:hover:not(:disabled) { background: var(--bg-button-hover); }
    &:disabled { opacity: 0.4; cursor: default; }
  }

  .filter-toggle.is-on {
    border-color: var(--level-info);
    color: var(--level-info);
  }

  .level-mask {
    display: inline-flex;
    gap: 0.15rem;

    .lvl-btn {
      padding: 0.2rem 0.4rem;
      font-size: 0.72rem;
      letter-spacing: 0.04em;
      border-color: var(--border-button);

      &.is-off {
        opacity: 0.35;
        text-decoration: line-through;
      }
    }
    .lvl-trace { color: var(--level-trace); }
    .lvl-debug { color: var(--level-debug); }
    .lvl-info { color: var(--level-info); }
    .lvl-warn { color: var(--level-warn); }
    .lvl-error { color: var(--level-error); }
    .lvl-fatal { color: var(--level-fatal); }
  }

  .search-error {
    color: var(--fg-error);
    font-family: var(--font-mono);
    flex-basis: 100%;
  }
}

.placeholder {
  margin: 2rem;
  color: var(--fg-dim);
}

.status-bar {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 0.6rem;
  padding: 0.25rem 0.75rem;
  border-top: 1px solid var(--border-default);
  background: var(--bg-elevated);
  color: var(--fg-muted);
  font-family: var(--font-mono);
  font-size: 0.78rem;
  min-height: 1.6rem;

  .slot {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }

  .slot.right {
    margin-left: auto;
    gap: 1.5em;
  }

  .stat { color: var(--fg-muted); }

  .cache-hint {
    padding: 0.05rem 0.4rem;
    border-radius: var(--radius-sm);
    background: var(--bg-button);
    color: var(--fg-dim);
    font-size: 0.72rem;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .font-size-hint { color: var(--fg-dim); }

  .theme-toggle {
    background: transparent;
    color: var(--fg-muted);
    border: 1px solid var(--border-button);
    border-radius: var(--radius-sm);
    padding: 0.05rem 0.5rem;
    font-size: 0.85rem;
    line-height: 1.2;
    cursor: pointer;

    /* Auto-follow-OS state shows whichever glyph the system currently
       resolves to, dimmed to 50% so it reads as "no manual override". */
    &.is-auto { opacity: 0.5; }

    &:hover {
      background: var(--bg-button-hover);
      color: var(--fg-default);
      opacity: 1;
    }
  }

  .pattern-status {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
  }

  .pattern-label strong {
    color: var(--fg-default);
    font-weight: 600;
  }

  .pattern-edit-btn {
    background: var(--bg-button);
    color: var(--fg-default);
    border: 1px solid var(--border-button);
    border-radius: var(--radius-sm);
    padding: 0.05rem 0.45rem;
    font-size: 0.72rem;
    line-height: 1.2;
    cursor: pointer;

    &:hover { background: var(--bg-button-hover); }
  }
}

.pattern-modal {
  width: min(720px, 92vw);

  .pattern-input-row {
    display: flex;
    gap: 0.4rem;
    margin: 0.6rem 0;

    .pat-input {
      flex: 1;
      min-width: 0;
      background: var(--bg-viewport);
      color: var(--fg-default);
      border: 1px solid var(--border-default);
      border-radius: var(--radius-sm);
      padding: 0.4rem 0.6rem;
      font-family: var(--font-mono);
      font-size: 0.85rem;
    }

    button {
      background: var(--bg-button);
      color: var(--fg-default);
      border: 1px solid var(--border-button);
      border-radius: var(--radius-sm);
      padding: 0.3rem 0.8rem;
      font-size: 0.85rem;
      cursor: pointer;

      &:hover { background: var(--bg-button-hover); }
    }
  }

  .score, .muted, .pat-error {
    font-size: 0.85rem;
    margin: 0.3rem 0;
  }
  .muted { color: var(--fg-muted); }
  .pat-error {
    color: var(--fg-error);
    font-family: var(--font-mono);
  }
}

/* --- Drag-drop overlay ------------------------------------------------- */
.drop-overlay {
  position: fixed;
  inset: 0;
  pointer-events: none;
  background: color-mix(in srgb, var(--level-info) 18%, transparent);
  border: 3px dashed var(--level-info);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 250;

  .drop-hint {
    background: var(--bg-elevated);
    color: var(--fg-default);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    padding: 1rem 1.6rem;
    font-size: 1rem;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.4);
    display: flex;
    align-items: center;
    gap: 0.6rem;

    .arrow {
      color: var(--level-info);
      font-size: 1.4rem;
    }
  }
}

/* --- Settings modal (P7) ----------------------------------------------- */
.modal-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 200;
}

.modal {
  width: min(720px, 92vw);
  max-height: 88vh;
  background: var(--bg-elevated);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
  display: flex;
  flex-direction: column;
  overflow: hidden;

  .modal-head {
    display: flex;
    align-items: center;
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--border-default);
    background: var(--bg-app);

    h2 {
      margin: 0;
      font-size: 1rem;
    }
  }

  .modal-close {
    margin-left: auto;
    background: transparent;
    color: var(--fg-default);
    border: 0;
    font-size: 1.4rem;
    line-height: 1;
    cursor: pointer;
  }

  .modal-body {
    padding: 1rem 1.2rem 1.5rem;
    overflow-y: auto;

    h3 {
      margin: 1.2rem 0 0.4rem;
      font-size: 0.95rem;
      border-bottom: 1px solid var(--border-default);
      padding-bottom: 0.25rem;
    }
    h3:first-of-type { margin-top: 0; }

    h4 {
      margin: 0.8rem 0 0.4rem;
      font-size: 0.85rem;
      color: var(--fg-muted);
    }

    p.muted {
      color: var(--fg-muted);
      font-size: 0.85rem;
      margin: 0.4rem 0;
    }

    code {
      background: var(--bg-button);
      padding: 0.05rem 0.3rem;
      border-radius: 3px;
      font-family: var(--font-mono);
    }

    .row-grid {
      display: grid;
      grid-template-columns: 10rem 1fr;
      align-items: center;
      gap: 0.8rem;
      margin: 0.35rem 0;
    }

    .row-label {
      color: var(--fg-muted);
      font-size: 0.85rem;
    }

    .control-cell {
      display: inline-flex;
      align-items: center;
      gap: 0.5rem;
      min-width: 0;
    }

    .seg {
      display: inline-flex;
      gap: 0.3rem;
    }
    .seg-btn {
      background: var(--bg-button);
      color: var(--fg-default);
      border: 1px solid var(--border-button);
      border-radius: var(--radius-sm);
      padding: 0.3rem 0.7rem;
      font-size: 0.85rem;
      cursor: pointer;

      &.is-on {
        background: var(--fg-default);
        color: var(--bg-app);
        border-color: var(--fg-default);
      }

      &.danger {
        color: var(--level-error);
        border-color: var(--level-error);
      }
    }

    .font-seg .font-val {
      font-family: var(--font-mono);
      min-width: 3.5rem;
      text-align: center;
    }

    .recent-list {
      list-style: none;
      padding: 0;
      margin: 0.3rem 0 0;
      max-height: 14rem;
      overflow-y: auto;
      border: 1px solid var(--border-default);
      border-radius: var(--radius-sm);

      li {
        display: flex;
        align-items: center;
        gap: 0.5rem;
        padding: 0.3rem 0.5rem;
        border-bottom: 1px dashed var(--border-default);
        font-size: 0.82rem;

        &:last-child { border-bottom: 0; }

        .open-btn {
          background: transparent;
          border: 0;
          color: var(--level-info);
          cursor: pointer;
          padding: 0;
          font-weight: 600;
          flex: 0 0 auto;

          &:hover { text-decoration: underline; }
        }

        .path {
          color: var(--fg-dim);
          font-family: var(--font-mono);
          overflow: hidden;
          text-overflow: ellipsis;
          white-space: nowrap;
          flex: 1;
          min-width: 0;
        }

        .forget-btn {
          background: transparent;
          border: 0;
          color: var(--fg-dim);
          cursor: pointer;
          font-size: 1rem;
          line-height: 1;

          &:hover { color: var(--level-error); }
        }
      }
    }

    .data-cell {
      flex-wrap: wrap;

      .data-path {
        background: var(--bg-button);
        padding: 0.2rem 0.45rem;
        border-radius: var(--radius-sm);
        font-family: var(--font-mono);
        font-size: 0.8rem;
        color: var(--fg-default);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        min-width: 0;
        flex: 1;
      }

      .badge {
        padding: 0.05rem 0.4rem;
        border-radius: var(--radius-sm);
        background: var(--level-info);
        color: var(--bg-app);
        font-size: 0.7rem;
        text-transform: uppercase;
        letter-spacing: 0.04em;
      }
    }

    .reset-grid {
      margin-top: 0.4rem;
      display: flex;
      flex-direction: column;
      gap: 0;
    }

    .footer-note {
      margin-top: 1.2rem;
      padding-top: 0.6rem;
      border-top: 1px solid var(--border-default);
      font-size: 0.8rem;
    }
  }
}

.viewport-shell {
  flex: 1 1 auto;
  display: flex;
  flex-direction: row;
  min-height: 0;
  position: relative;
  /* The minimap tooltip is positioned absolutely with `right` so it
     extends leftward past the .minimap container; without clipping here
     it pushes the shell horizontally and pulls in a page-level scrollbar.
     The shell already bounds the viewport vertically, so clipping is
     safe -- the tooltip stays well within it. */
  overflow: hidden;

  .jump-bottom-floating {
    position: absolute;
    /* Park clear of the minimap (20px wide + 1px border-left) and a
       comfortable touch-target distance up from the bottom edge. */
    right: 32px;
    bottom: 16px;
    width: 32px;
    height: 32px;
    border-radius: 50%;
    border: 1px solid var(--border-button);
    background: var(--bg-elevated);
    color: var(--fg-default);
    font-size: 1.1rem;
    line-height: 1;
    cursor: pointer;
    opacity: 0.25;
    transition: opacity 120ms ease-out, background 120ms ease-out;
    z-index: 10;
    display: flex;
    align-items: center;
    justify-content: center;
    box-shadow: 0 2px 6px rgba(0, 0, 0, 0.35);

    &:hover, &:focus-visible {
      opacity: 1;
      background: var(--bg-button-hover);
      outline: none;
    }
  }
}

.minimap {
  flex: 0 0 auto;
  width: 20px;
  position: relative;
  background: var(--bg-viewport);
  border-left: 1px solid var(--border-default);
  cursor: pointer;
  user-select: none;

  .minimap-canvas {
    display: block;
    width: 20px;
    height: 100%;
    image-rendering: pixelated;
  }

  .minimap-indicator {
    position: absolute;
    left: 0;
    right: 0;
    background: rgba(255, 255, 255, 0.22);
    border-top: 2px solid rgba(255, 255, 255, 0.85);
    border-bottom: 2px solid rgba(255, 255, 255, 0.85);
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.55) inset;
    pointer-events: none;
  }

  &:hover .minimap-indicator {
    background: rgba(255, 255, 255, 0.32);
    border-color: var(--fg-default);
  }
}

.minimap-tooltip {
  position: fixed;
  /* `left` is the minimap's left edge (set inline); translateX -100% then
     -4px slides the tooltip just clear of the minimap. translateY -50%
     vertically centres it on the cursor Y. position: fixed lets it float
     above the status bar / outside .viewport-shell's overflow clip. */
  transform: translate(calc(-100% - 4px), -50%);
  z-index: 100;
  display: flex;
  flex-direction: column;
  gap: 0.15rem;
  padding: 0.3rem 0.55rem;
  background: var(--bg-elevated);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.5);
  font-family: var(--font-mono);
  font-size: 0.78rem;
  color: var(--fg-default);
  pointer-events: none;
  white-space: nowrap;

  .line-no { color: var(--fg-muted); }
  .ts { color: var(--fg-default); }
  .ts.muted { color: var(--fg-dim); }

  /* Callout pointer aimed at the minimap. Two stacked triangles fake the
     1px border: the outer (border-coloured) triangle sits one pixel
     further right than the inner (bg-coloured) one, so the seam between
     the tooltip body and the pointer reads as a continuous outline. */
  &::before,
  &::after {
    content: '';
    position: absolute;
    top: 50%;
    width: 0;
    height: 0;
    border-top: 6px solid transparent;
    border-bottom: 6px solid transparent;
    transform: translateY(-50%);
  }
  &::before {
    right: -7px;
    border-left: 7px solid var(--border-default);
  }
  &::after {
    right: -6px;
    border-left: 6px solid var(--bg-elevated);
  }
}

.viewport {
  flex: 1 1 auto;
  overflow: auto;
  scrollbar-width: none;
  font-family: var(--font-mono);
  font-size: var(--font-size-base);
  line-height: var(--row-height);
  background-color: var(--bg-viewport);

  &::-webkit-scrollbar { display: none; }

  /* Skeleton backdrop lives on `.total` (the scroll content), NOT
     `.viewport`. Anchoring it to .total bounds it to the actual log
     length: when the file is shorter than the viewport, the empty
     space below the last row stays clean (plain --bg-viewport). With
     row-snap forcing scrollTop to multiples of ROW_HEIGHT, every
     stripe in the repeating pattern lines up with where a real row
     would be, so visually the backdrop reads as fixed even though it
     scrolls with .total. Four layered gradients sketch the shape of a
     row: gutter strip, line-number bar, message bar, and a faint
     full-row band -- all muted greys since the real row level is
     unknown until data loads. */
  .total {
    position: relative;
    width: 100%;
    background-image:
      /* 1. Gutter strip: full row height, --gutter-width wide. */
      linear-gradient(
        to bottom,
        var(--bg-skeleton-gutter) 0,
        var(--bg-skeleton-gutter) 100%
      ),
      /* 2. Line-number bar: 8px tall (y=5..13), centered vertically. */
      linear-gradient(
        to bottom,
        transparent 0,
        transparent 5px,
        var(--bg-skeleton-num) 5px,
        var(--bg-skeleton-num) 13px,
        transparent 13px
      ),
      /* 3. Message bar: 8px tall (y=5..13), centered vertically. */
      linear-gradient(
        to bottom,
        transparent 0,
        transparent 5px,
        var(--bg-skeleton) 5px,
        var(--bg-skeleton) 13px,
        transparent 13px
      ),
      /* 4. Faint full-row band so the eye reads the area as one row. */
      linear-gradient(
        to bottom,
        var(--bg-skeleton-row) 0,
        var(--bg-skeleton-row) calc(var(--row-height) - 1px),
        transparent calc(var(--row-height) - 1px)
      );
    background-position:
      0 0,
      calc(var(--gutter-width) + 0.6rem) 0,
      calc(var(--gutter-width) + var(--line-num-width)) 0,
      0 0;
    background-size:
      var(--gutter-width) var(--row-height),
      calc(var(--line-num-width) - 1.2rem) var(--row-height),
      100% var(--row-height),
      100% var(--row-height);
    background-repeat: repeat-y;
  }

  .row {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    z-index: 1;
    display: grid;
    grid-template-columns: var(--gutter-width) var(--line-num-width) 1fr;
    align-items: center;
    white-space: pre;
    color: var(--fg-row);
    /* Opaque base so the row paints over the skeleton backdrop. Level
       tints (below) layer on top via background-image + this colour. */
    background-color: var(--bg-viewport);

    .gutter {
      background: var(--gutter-color, var(--level-unknown));
      height: 100%;
    }

    .idx {
      color: var(--fg-gutter);
      text-align: right;
      padding-right: 0.6rem;
      user-select: none;
    }

    .txt {
      padding-right: 0.6rem;
      /* Per-row horizontal scroll exists ONLY as a programmatic surface
         so `scrollIntoView` can bring an off-screen search match into
         view (see bringCurrentHitMatchIntoView). The visible scrollbar
         was ugly and hard to use, so it's hidden -- overflow-x stays
         `auto` (not `hidden`) because scrollLeft only takes effect on
         scrollable elements. */
      overflow-x: auto;
      overflow-y: hidden;
      scrollbar-width: none;

      &::-webkit-scrollbar { display: none; }
    }

    &.is-continuation .txt {
      padding-left: var(--continuation-indent);
      color: var(--fg-message);
    }

    .s-level {
      font-weight: 600;
    }
    .level-trace { color: var(--level-trace); }
    .level-debug { color: var(--level-debug); }
    .level-info { color: var(--level-info); }
    .level-warn { color: var(--level-warn); }
    .level-error { color: var(--level-error); }
    .level-fatal { color: var(--level-fatal); }
    .level-off { color: var(--level-off); }
    .level-all { color: var(--level-all); }
    .level-unknown { color: var(--level-unknown); }

    .s-timestamp { color: var(--fg-timestamp); }
    .s-thread { color: var(--fg-thread); }
    .s-logger { color: var(--fg-logger); font-style: italic; }
    .s-message { color: var(--fg-message); }
    .s-sep { color: var(--fg-separator-dash); }

    .continuation { color: var(--fg-message); }

    /* Axis-2 highlight overlays. These ride on top of axis-1 spans, so they
       only set colour / weight / decoration -- never background, so the row
       hover state stays uniform. */
    .h-exception {
      color: var(--hl-exception-fg);
      font-weight: 700;
    }
    .h-caused-by {
      color: var(--hl-caused-by-fg);
      font-weight: 700;
    }
    .h-stack-frame { color: var(--fg-message); }
    .h-stack-fqn {
      color: var(--hl-stack-fqn-fg);
      font-weight: 600;
    }
    .h-stack-file {
      color: var(--hl-stack-file-fg);
      text-decoration: underline;
      text-decoration-style: dotted;
    }
    .h-stack-line { color: var(--hl-stack-line-fg); }
    .h-path {
      color: var(--hl-path-fg);
      text-decoration: underline;
      text-decoration-style: dotted;
    }
    .h-url {
      color: var(--hl-url-fg);
      text-decoration: underline;
      cursor: pointer;

      &:hover { text-decoration-thickness: 2px; }
    }
    .h-search-match {
      background: var(--hl-search-bg);
      color: var(--hl-search-fg);
      font-weight: 600;
      border-radius: 2px;
      box-shadow: 0 0 0 1px var(--hl-search-bg);
    }

    /* Subtle row tint for the louder severities. We layer a flat colour
       image over the opaque `background-color: var(--bg-viewport)` so the
       row stays fully opaque (the skeleton must NOT bleed through) while
       still showing the severity tint. */
    &.level-row-warn {
      background-image: linear-gradient(
        color-mix(in srgb, var(--level-warn) 10%, transparent),
        color-mix(in srgb, var(--level-warn) 10%, transparent)
      );
    }
    &.level-row-error {
      background-image: linear-gradient(
        color-mix(in srgb, var(--level-error) 10%, transparent),
        color-mix(in srgb, var(--level-error) 10%, transparent)
      );
    }
    &.level-row-fatal {
      background-image: linear-gradient(
        color-mix(in srgb, var(--level-fatal) 10%, transparent),
        color-mix(in srgb, var(--level-fatal) 10%, transparent)
      );
    }

    /* Active search hit: wash the whole row in the search-highlight
       hue so prev/next navigation lands somewhere obvious without the
       user having to spot the small per-token match span. Layered as
       a separate background-image so it composites cleanly on top of
       any level-row tint, and uses a bright outline at the top/bottom
       edges to read like a focus ring rather than another level
       background. */
    &.is-current-hit {
      background-image: linear-gradient(
        color-mix(in srgb, var(--hl-search-bg) 22%, transparent),
        color-mix(in srgb, var(--hl-search-bg) 22%, transparent)
      );
      box-shadow:
        inset 0 1px 0 var(--hl-search-bg),
        inset 0 -1px 0 var(--hl-search-bg);
    }
  }

  /* Zero-height sticky anchor: keeps the sticky header pinned to the
     viewport's top edge without ever displacing `.total`. The visible
     row inside is absolutely positioned relative to this anchor, so
     toggling stickiness on/off costs nothing in layout terms -- no
     flicker between adjacent multi-line records AND no empty band
     above row 1 when the file is scrolled to the very top. */
  .sticky-shell {
    position: sticky;
    top: 0;
    z-index: 2;
    height: 0;
    overflow: visible;

    .row {
      position: absolute;
      top: 0;
      left: 0;
      right: 0;
      height: var(--row-height);
      background: var(--bg-sticky);
      backdrop-filter: blur(2px);
      border-bottom: 1px solid var(--border-sticky);
    }

    .jump-up {
      background: transparent;
      border: none;
      color: var(--fg-muted);
      font-family: var(--font-mono);
      font-size: 0.95em;
      padding: 0 0.6rem 0 0;
      cursor: pointer;
      text-align: right;
      line-height: 1;

      &:hover { color: var(--fg-default); }
      &:focus-visible { outline: 1px solid var(--level-info); outline-offset: -1px; }
    }
  }
}

@keyframes skeleton-pulse {
  0%, 100% { opacity: 0.5; }
  50% { opacity: 0.85; }
}
</style>
