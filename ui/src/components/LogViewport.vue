<script setup lang="ts">
/**
 * Per-tab viewport. Owns the virtualised line list, the minimap canvas,
 * the sticky-header overlay, the scroll element, and the
 * jump-to-bottom button. Receives the active `tab` as a prop; remounts
 * (via :key="tab.localId" in the parent) when the active tab changes
 * so virtualizer state, scrollTop and minimap buckets start fresh per
 * tab.
 *
 * Tail/search deltas are written into the tab's reactive state from
 * outside this component. The viewport reacts to those mutations via
 * watchers: file.line_count growth triggers fetch-and-paint of the
 * new pages, the minimap, and -- when `tab.followTail` -- a scroll to
 * the bottom.
 */
import { computed, onBeforeUnmount, onMounted, ref, useTemplateRef, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { openUrl } from '@tauri-apps/plugin-opener'
import { useVirtualizer } from '@tanstack/vue-virtual'
import { highlightsFor, overlay, rulesVersionRef, type LeafSpan } from '../highlight/engine'
import {
  OVERSCAN,
  PAGE_SIZE,
  ROW_HEIGHT,
  type BucketStat,
  type HeaderFields,
  type HitRef,
  type LevelMinimapPayload,
  type LineRow,
  type MarkerRef,
  type RecordRef,
  type SpeedGrid,
} from '../types'
import type { Tab } from '../tab'

const props = defineProps<{
  tab: Tab
}>()

const emit = defineEmits<{
  (e: 'error', message: string): void
}>()

// --- DOM refs ---
const scrollEl = useTemplateRef<HTMLDivElement>('scrollEl')
const minimapEl = useTemplateRef<HTMLCanvasElement>('minimapEl')
const speedRailEl = useTemplateRef<HTMLCanvasElement>('speedRailEl')
const speedGrid = ref<SpeedGrid | null>(null)
const speedRailVisible = computed(() => {
  const g = speedGrid.value
  if (!g) return false
  return g.buckets.length > 0 && (g.max_avg_ms > 0 || g.buckets.some((b) => b.count > 0))
})
const SPEED_RAIL_WIDTH = 4

// --- Local UI state (per-mount) ---
const minimapBuckets = ref<BucketStat[]>([])
const viewportHeightPx = ref(0)
const viewportScrollTop = ref(0)
let minimapFetchPending = false
let lastMinimapLineCount = -1
let lastMinimapHeight = -1
let lastMaxErrorWarnSum = 0
const MINIMAP_WIDTH = 20

// Significant event markers (e.g. site restarts) rendered as small
// coloured triangles in a gutter to the left of the minimap canvas. The
// list is refreshed alongside the minimap on file open / pattern apply /
// tail growth / rotation. Always indexed by physical line; the gutter
// projects through `filteredLineIndices` when filter mode is active.
const markers = ref<MarkerRef[]>([])
let lastMarkerLineCount = -1

// Filter-mode source records: respect either the level mask (allowedRecords)
// or the active search hit set.
const filteredSourceRecords = computed<RecordRef[] | null>(() => {
  const tab = props.tab
  const allowed = tab.allowedRecords.value
  const isFiltering = tab.filterMode.value
  const hasQuery = tab.searchQuery.value.trim().length > 0
  if (!isFiltering && !allowed) return null
  if (isFiltering && hasQuery) {
    const source: RecordRef[] = []
    for (const recIdx of tab.hitOrder.value) {
      const hit = tab.hits.value.get(recIdx)
      if (hit) source.push(hit)
    }
    return source
  }
  return allowed
})

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
  const filt = filteredLineIndices.value
  if (filt) return filt.length
  return props.tab.file.value.line_count
})

function actualLineIndex(virtualIdx: number): number {
  const filt = filteredLineIndices.value
  if (!filt) return virtualIdx
  return filt[virtualIdx] ?? 0
}

function lineRowVirtual(virtualIdx: number): LineRow | null {
  return props.tab.lineRow(actualLineIndex(virtualIdx))
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

const atBottom = computed(() => {
  const total = totalSize.value
  const h = viewportHeightPx.value
  if (total <= 0 || h <= 0) return true
  return viewportScrollTop.value + h >= total - ROW_HEIGHT
})

// --- Page fetch driven by visible rows ---
watch(virtualRows, (rows) => {
  const wanted = new Set<number>()
  for (const r of rows) {
    const actual = actualLineIndex(r.index)
    wanted.add(Math.floor(actual / PAGE_SIZE))
  }
  for (const p of wanted) void props.tab.fetchPage(p)
})

// --- Sticky header ---
interface StickyHeader {
  row: LineRow
  lineIndex: number
}
const stickyHeader = computed<StickyHeader | null>(() => {
  const tab = props.tab
  const total = effectiveCount.value
  if (total === 0) return null
  const topVirtual = Math.min(total - 1, Math.floor(viewportScrollTop.value / ROW_HEIGHT))
  const topIdx = actualLineIndex(topVirtual)
  const data = tab.lineRow(topIdx)
  if (!data) return null
  if (data.line_within_record === 0) return null
  for (let i = topIdx - 1; i >= 0; i--) {
    const candidate = tab.lineRow(i)
    if (!candidate) {
      void tab.fetchPage(Math.floor(i / PAGE_SIZE))
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
  const filt = filteredLineIndices.value
  let virtualIdx: number
  if (filt) {
    virtualIdx = filt.indexOf(sticky.lineIndex)
    if (virtualIdx < 0) return
  } else {
    virtualIdx = sticky.lineIndex
  }
  el.scrollTop = virtualIdx * ROW_HEIGHT
}

// --- Scroll handling ---
function onViewportScroll() {
  const el = scrollEl.value
  if (!el) return
  const raw = el.scrollTop
  const maxScroll = el.scrollHeight - el.clientHeight
  const rem = raw % ROW_HEIGHT
  if (rem !== 0 && raw < maxScroll - 0.5) {
    const snapped = Math.round(raw / ROW_HEIGHT) * ROW_HEIGHT
    if (snapped !== raw) {
      el.scrollTop = snapped
      return
    }
  }
  viewportScrollTop.value = el.scrollTop
  props.tab.scrollTop.value = el.scrollTop
  if (props.tab.followTail.value) {
    const distance = el.scrollHeight - el.scrollTop - el.clientHeight
    if (distance > ROW_HEIGHT * 4) {
      props.tab.followTail.value = false
    }
  }
}

function jumpToBottom() {
  const tab = props.tab
  if (effectiveCount.value === 0) return
  requestAnimationFrame(() => {
    if (effectiveCount.value === 0) return
    void tab // capture for type
    virtualizer.value.scrollToIndex(effectiveCount.value - 1, { align: 'end' })
  })
}

function toggleFollowTail() {
  props.tab.followTail.value = !props.tab.followTail.value
  if (props.tab.followTail.value) jumpToBottom()
}

// Resolve which physical line within the hit's record actually contains the
// first match. Multi-line records (stack traces, multiline messages) carry a
// single HitRef per record_idx with record-relative byte ranges; centring on
// `record_first_line` lands the viewport on the record header even when the
// real match is buried dozens of continuation lines below. Fall back to
// `record_first_line` when pages aren't loaded yet -- the post-scroll
// `scheduleHitFocus` retry re-centres once the row enters the DOM.
function hitTargetLine(hit: HitRef): number {
  if (hit.ranges.length === 0) return hit.record_first_line
  const firstStart = hit.ranges[0][0]
  let target = hit.record_first_line
  for (let i = 0; i < hit.record_line_count; i++) {
    const li = hit.record_first_line + i
    const row = props.tab.lineRow(li)
    if (!row) {
      // Page not loaded yet; nudge it in so the next scheduleHitFocus pass
      // can refine the scroll position.
      props.tab.fetchPage(Math.floor(li / PAGE_SIZE)).catch(() => {})
      break
    }
    const start = row.byte_offset_in_record
    const end = start + row.text.length + 1
    if (firstStart >= start && firstStart < end) {
      target = li
      break
    }
    if (firstStart >= start) target = li
  }
  return target
}

// Scroll-to-current-hit, defined here because it needs the virtualizer.
function scrollToCurrentHit() {
  const tab = props.tab
  if (tab.currentHit.value < 0) return
  const recIdx = tab.hitOrder.value[tab.currentHit.value]
  const hit = tab.hits.value.get(recIdx)
  if (!hit) return
  const targetLine = hitTargetLine(hit)
  const filt = filteredLineIndices.value
  let targetVirtual: number
  if (filt) {
    targetVirtual = filt.indexOf(targetLine)
    if (targetVirtual < 0) targetVirtual = filt.indexOf(hit.record_first_line)
    if (targetVirtual < 0) return
  } else {
    targetVirtual = targetLine
  }
  tab.followTail.value = false
  virtualizer.value.scrollToIndex(targetVirtual, { align: 'center' })
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
  const tab = props.tab
  if (tab.currentHit.value < 0) return false
  const recIdx = tab.hitOrder.value[tab.currentHit.value]
  return row.record_idx === recIdx
}

function bringCurrentHitMatchIntoView(): boolean {
  const el = scrollEl.value
  if (!el) return false
  const match = el.querySelector('.row.is-current-hit .h-search-match') as HTMLElement | null
  if (!match) return false
  const elRect = el.getBoundingClientRect()
  // Vertical: centre the matching row, not the record header. hitTargetLine
  // does this on the initial scrollToIndex, but the page containing the
  // match may have arrived after that scroll, so refine here once the row
  // has rendered.
  const rowEl = match.closest('.row') as HTMLElement | null
  if (rowEl) {
    const rowRect = rowEl.getBoundingClientRect()
    const rowCentreInContent = rowRect.top - elRect.top + el.scrollTop + rowEl.offsetHeight / 2
    const targetScrollTop = rowCentreInContent - el.clientHeight / 2
    const maxScrollTop = el.scrollHeight - el.clientHeight
    const next = Math.max(0, Math.min(maxScrollTop, targetScrollTop))
    if (Math.abs(next - el.scrollTop) > 1) el.scrollTop = next
  }
  if (el.scrollWidth > el.clientWidth) {
    const matchRect = match.getBoundingClientRect()
    const matchLeftInContent = matchRect.left - elRect.left + el.scrollLeft
    const targetScrollLeft = matchLeftInContent - el.clientWidth / 2 + match.offsetWidth / 2
    const maxScrollLeft = el.scrollWidth - el.clientWidth
    el.scrollLeft = Math.max(0, Math.min(maxScrollLeft, targetScrollLeft))
  }
  return true
}

// --- Minimap ---
// Dim "wash" alpha used as the base layer for every non-INFO bucket. The
// wash is hue-neutral on purpose -- it conveys "data lives here" without
// competing with the hot overlay, which carries all the per-level colour.
// INFO/UNKNOWN stay null so quiet regions read as pure background.
const NEUTRAL_WASH = 'rgba(180, 184, 196, 0.14)'
const LEVEL_COLOUR: Record<string, string | null> = {
  trace: NEUTRAL_WASH,
  debug: NEUTRAL_WASH,
  info: null,
  warn: NEUTRAL_WASH,
  error: NEUTRAL_WASH,
  fatal: NEUTRAL_WASH,
  off: NEUTRAL_WASH,
  all: NEUTRAL_WASH,
  unknown: null,
}

// Hot-overlay colours, used as a second layer on top of the wash for
// buckets where (error + warn) > 0. Alpha is modulated per bucket from
// HOT_ALPHA_MIN..HOT_ALPHA_MAX based on `heat / max_error_warn_sum`.
const LEVEL_HOT: Record<string, string | null> = {
  warn: 'rgba(224, 176, 74, ALPHA)',
  error: 'rgba(212, 87, 95, ALPHA)',
  fatal: 'rgba(179, 134, 232, ALPHA)',
}
const HOT_ALPHA_MIN = 0.15
const HOT_ALPHA_MAX = 1.0

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
    void fetchMarkers(force)
    void fetchSpeedGrid()
  })
}

async function fetchSpeedGrid() {
  const height = viewportHeightPx.value
  if (height <= 0) return
  const bucketCount = Math.max(1, Math.floor(height))
  try {
    const payload = await invoke<SpeedGrid>('get_slow_request_speeds', {
      fileId: props.tab.file.value.file_id,
      bucketCount,
    })
    speedGrid.value = payload
    paintSpeedRail()
  } catch {
    // non-fatal
  }
}

function readCssColour(varName: string): string {
  const styles = globalThis.getComputedStyle?.(document.documentElement)
  const v = styles?.getPropertyValue(varName).trim()
  return v && v.length > 0 ? v : '#15803d'
}

function resolveToRgb(colour: string): [number, number, number] {
  const probe = document.createElement('span')
  probe.style.color = colour
  probe.style.display = 'none'
  document.body.appendChild(probe)
  const computed = globalThis.getComputedStyle(probe).color
  probe.remove()
  const m = computed.match(/rgba?\((\d+),\s*(\d+),\s*(\d+)/)
  if (!m) return [0, 0, 0]
  return [Number(m[1]), Number(m[2]), Number(m[3])]
}

function lerpColour(a: string, b: string, t: number): string {
  const ca = resolveToRgb(a)
  const cb = resolveToRgb(b)
  const r = Math.round(ca[0] + (cb[0] - ca[0]) * t)
  const g = Math.round(ca[1] + (cb[1] - ca[1]) * t)
  const bb = Math.round(ca[2] + (cb[2] - ca[2]) * t)
  return `rgb(${r}, ${g}, ${bb})`
}

function bucketColour(avgMs: number, fastMs: number, slowMs: number): string {
  const fast = readCssColour('--speed-fast')
  const mid = readCssColour('--speed-mid')
  const slow = readCssColour('--speed-slow')
  if (avgMs <= fastMs || slowMs <= fastMs) return fast
  if (avgMs >= slowMs) return slow
  const t = (avgMs - fastMs) / (slowMs - fastMs)
  if (t < 0.5) return lerpColour(fast, mid, t * 2)
  return lerpColour(mid, slow, (t - 0.5) * 2)
}

function paintSpeedRail() {
  const canvas = speedRailEl.value
  const grid = speedGrid.value
  if (!canvas || !grid || grid.buckets.length === 0) return
  const h = grid.buckets.length
  const dpr = globalThis.devicePixelRatio || 1
  canvas.width = SPEED_RAIL_WIDTH * dpr
  canvas.height = h * dpr
  canvas.style.width = `${SPEED_RAIL_WIDTH}px`
  canvas.style.height = `${h}px`
  const ctx = canvas.getContext('2d')
  if (!ctx) return
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
  const fast = grid.min_avg_ms
  const slow = Math.max(grid.max_avg_ms, fast + 1)
  const gradient = ctx.createLinearGradient(0, 0, 0, h)
  for (let i = 0; i < h; i++) {
    const b = grid.buckets[i]
    const avg = b.count > 0 ? b.avg_ms : fast
    const colour = bucketColour(avg, fast, slow)
    const offset = h === 1 ? 0 : (i + 0.5) / h
    gradient.addColorStop(Math.max(0, Math.min(1, offset)), colour)
  }
  const firstAvg = grid.buckets[0].count > 0 ? grid.buckets[0].avg_ms : fast
  const lastAvg = grid.buckets[h - 1].count > 0 ? grid.buckets[h - 1].avg_ms : fast
  gradient.addColorStop(0, bucketColour(firstAvg, fast, slow))
  gradient.addColorStop(1, bucketColour(lastAvg, fast, slow))
  ctx.fillStyle = gradient
  ctx.fillRect(0, 0, SPEED_RAIL_WIDTH, h)
}

async function fetchMarkers(force: boolean) {
  const lc = props.tab.file.value.line_count
  if (!force && lc === lastMarkerLineCount) return
  try {
    const payload = await invoke<MarkerRef[]>('get_markers', {
      fileId: props.tab.file.value.file_id,
    })
    markers.value = payload
    lastMarkerLineCount = lc
  } catch {
    // non-fatal
  }
}

async function fetchMinimap(force: boolean) {
  const height = viewportHeightPx.value
  if (height <= 0) return
  const source = filteredSourceRecords.value
  if (source !== null) {
    const eff = effectiveCount.value
    const contentPx = eff * ROW_HEIGHT
    const bucketCount = Math.max(1, Math.min(Math.floor(height), contentPx))
    const { buckets, maxErrorWarnSum } = buildFilteredMinimap(source, eff, bucketCount)
    minimapBuckets.value = buckets
    lastMaxErrorWarnSum = maxErrorWarnSum
    lastMinimapHeight = bucketCount
    lastMinimapLineCount = eff
    paintMinimap()
    return
  }
  const contentPx = props.tab.file.value.line_count * ROW_HEIGHT
  const bucketCount = Math.max(1, Math.min(Math.floor(height), contentPx))
  if (
    !force &&
    bucketCount === lastMinimapHeight &&
    props.tab.file.value.line_count === lastMinimapLineCount
  ) {
    return
  }
  try {
    const payload = await invoke<LevelMinimapPayload>('get_level_minimap', {
      fileId: props.tab.file.value.file_id,
      bucketCount,
    })
    minimapBuckets.value = payload.buckets
    lastMaxErrorWarnSum = payload.max_error_warn_sum
    lastMinimapHeight = bucketCount
    lastMinimapLineCount = payload.line_count
    paintMinimap()
  } catch {
    // non-fatal
  }
}

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

function buildFilteredMinimap(
  source: RecordRef[],
  virtualLineCount: number,
  bucketCount: number,
): { buckets: BucketStat[]; maxErrorWarnSum: number } {
  const empty = (): BucketStat => ({ worst: 'unknown', error: 0, warn: 0, total: 0 })
  const buckets: BucketStat[] = new Array(bucketCount)
  for (let i = 0; i < bucketCount; i++) buckets[i] = empty()
  if (virtualLineCount === 0 || bucketCount === 0) {
    return { buckets, maxErrorWarnSum: 0 }
  }
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
      const bucket = buckets[b]
      if (rank > minimapLevelRank(bucket.worst)) bucket.worst = rec.level
      bucket.total += 1
      if (rec.level === 'error' || rec.level === 'fatal') bucket.error += 1
      else if (rec.level === 'warn') bucket.warn += 1
    }
    virtualCursor += rec.record_line_count
  }
  let maxErrorWarnSum = 0
  for (const b of buckets) {
    const heat = b.error + b.warn
    if (heat > maxErrorWarnSum) maxErrorWarnSum = heat
  }
  return { buckets, maxErrorWarnSum }
}

// Physical line indices the user has bookmarked, projected to the
// visible (possibly filter-mode) virtual index. Filter-mode bookmarks
// whose line is hidden by the level mask or hit set are excluded from
// the minimap but stay persisted on the tab.
interface BookmarkVisual {
  lineIdx: number
  virtualIdx: number
}

interface MarkerVisual {
  lineIdx: number
  virtualIdx: number
  kind: string
}

// Project physical-line markers to the virtual index space so the rail
// lines up with the minimap regardless of filter mode. Filter-mode
// markers whose line is hidden by the active mask / hit set are skipped.
const markerVisuals = computed<MarkerVisual[]>(() => {
  const list = markers.value
  if (list.length === 0) return []
  const filt = filteredLineIndices.value
  const out: MarkerVisual[] = []
  if (filt) {
    const lookup = new Map<number, number>()
    for (let v = 0; v < filt.length; v++) lookup.set(filt[v], v)
    for (const m of list) {
      const v = lookup.get(m.line_index)
      if (v !== undefined) out.push({ lineIdx: m.line_index, virtualIdx: v, kind: m.kind })
    }
  } else {
    const lc = props.tab.file.value.line_count
    for (const m of list) {
      if (m.line_index >= 0 && m.line_index < lc) {
        out.push({ lineIdx: m.line_index, virtualIdx: m.line_index, kind: m.kind })
      }
    }
  }
  out.sort((a, b) => a.virtualIdx - b.virtualIdx)
  return out
})

// Physical-line -> marker lookup so renderRow can apply a row-level
// highlight class when a row is itself a marker.
const markerLineLookup = computed<Map<number, string>>(() => {
  const out = new Map<number, string>()
  for (const m of markers.value) out.set(m.line_index, m.kind)
  return out
})

const MARKER_LABEL: Record<string, string> = {
  restart: 'Site restart',
}

function markerLabel(kind: string): string {
  return MARKER_LABEL[kind] ?? kind
}

function markerColourVar(kind: string): string {
  return `var(--marker-${kind}, var(--accent))`
}

function jumpToLine(lineIdx: number) {
  const v = virtualizer.value
  if (!v) return
  const filt = filteredLineIndices.value
  if (filt) {
    const virtIdx = filt.indexOf(lineIdx)
    if (virtIdx === -1) return
    v.scrollToIndex(virtIdx, { align: 'center' })
  } else {
    v.scrollToIndex(lineIdx, { align: 'center' })
  }
}

const bookmarkVisuals = computed<BookmarkVisual[]>(() => {
  const set = props.tab.bookmarks.value
  if (set.size === 0) return []
  const filt = filteredLineIndices.value
  const out: BookmarkVisual[] = []
  if (filt) {
    // O(n) lookup per bookmark would be quadratic; build an index once.
    const lookup = new Map<number, number>()
    for (let v = 0; v < filt.length; v++) lookup.set(filt[v], v)
    for (const idx of set) {
      const v = lookup.get(idx)
      if (v !== undefined) out.push({ lineIdx: idx, virtualIdx: v })
    }
  } else {
    for (const idx of set) {
      if (idx >= 0 && idx < props.tab.file.value.line_count) {
        out.push({ lineIdx: idx, virtualIdx: idx })
      }
    }
  }
  out.sort((a, b) => a.virtualIdx - b.virtualIdx)
  return out
})

function hotColour(level: string, heat: number, max: number): string | null {
  if (heat <= 0 || max <= 0) return null
  const template = LEVEL_HOT[level]
  if (!template) return null
  // Log scale so a stray single error stays barely visible while big
  // clusters dominate -- linear modulation flattens both ends.
  const t = Math.max(0, Math.min(1, Math.log1p(heat) / Math.log1p(max)))
  const alpha = HOT_ALPHA_MIN + (HOT_ALPHA_MAX - HOT_ALPHA_MIN) * t
  return template.replace('ALPHA', alpha.toFixed(3))
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
  const dpr = globalThis.devicePixelRatio || 1
  canvas.width = MINIMAP_WIDTH * dpr
  canvas.height = h * dpr
  canvas.style.width = `${MINIMAP_WIDTH}px`
  canvas.style.height = `${h}px`
  const ctx = canvas.getContext('2d')
  if (!ctx) return
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
  ctx.fillStyle = currentMinimapBg()
  ctx.fillRect(0, 0, MINIMAP_WIDTH, h)

  // Pass 1: base wash (worst-severity, low alpha). Same run-coalescing
  // strategy as before so paint cost stays flat in bucket count.
  const washAt = (i: number): string | null =>
    i < h ? (LEVEL_COLOUR[buckets[i].worst] ?? null) : null
  let runStart = 0
  let runColour = washAt(0)
  for (let i = 1; i <= h; i++) {
    const next = washAt(i)
    if (next !== runColour) {
      if (runColour !== null) {
        ctx.fillStyle = runColour
        ctx.fillRect(0, runStart, MINIMAP_WIDTH, i - runStart)
      }
      runStart = i
      runColour = next
    }
  }

  // Pass 2: hot overlay. Per-bucket alpha is bucket-local, so no run
  // coalescing here -- a one-pixel-per-bucket loop is fine at ~viewport
  // height (a few hundred buckets at most).
  const max = lastMaxErrorWarnSum
  if (max > 0) {
    for (let i = 0; i < h; i++) {
      const b = buckets[i]
      const heat = b.error + b.warn
      if (heat === 0) continue
      const colour = hotColour(b.worst, heat, max)
      if (!colour) continue
      ctx.fillStyle = colour
      ctx.fillRect(0, i, MINIMAP_WIDTH, 1)
    }
  }

  paintBookmarkMarkers(ctx, h)
}

function currentAccent(): string {
  const styles = globalThis.getComputedStyle?.(document.documentElement)
  const fromVar = styles?.getPropertyValue('--accent').trim()
  return fromVar && fromVar.length > 0 ? fromVar : '#de6826'
}

function paintBookmarkMarkers(ctx: CanvasRenderingContext2D, h: number) {
  const visuals = bookmarkVisuals.value
  const eff = effectiveCount.value
  if (visuals.length === 0 || eff === 0 || h === 0) return
  const accent = currentAccent()
  ctx.fillStyle = accent
  // Dashed stripe: 3px dash, 2px gap. Pure fillRect rather than setLineDash
  // so the dashes align to whole device pixels regardless of dpr scaling.
  const DASH = 3
  const GAP = 2
  for (const bm of visuals) {
    const y = Math.min(h - 1, Math.floor((bm.virtualIdx * h) / eff))
    for (let x = 0; x < MINIMAP_WIDTH; x += DASH + GAP) {
      ctx.fillRect(x, y, Math.min(DASH, MINIMAP_WIDTH - x), 2)
    }
  }
}

const minimapIndicator = computed(() => {
  if (props.tab.file.value.line_count === 0) {
    return { top: 0, height: 0, visible: false }
  }
  const h = viewportHeightPx.value
  if (h <= 0) return { top: 0, height: 0, visible: false }
  // Use the virtualizer's total size (reactive) rather than el.scrollHeight,
  // which doesn't recompute when switching tabs.
  const total = totalSize.value
  // Hide the indicator when the log fully fits the viewport - the handle
  // covering the entire minimap is meaningless and looks like a bug.
  if (total <= 0 || total - h < 1) {
    return { top: 0, height: h, visible: false }
  }
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

interface MinimapTooltip {
  visible: boolean
  top: number
  left: number
  lineIndex: number
  timestamp: string | null
  error: number
  warn: number
}
const minimapTooltip = ref<MinimapTooltip>({
  visible: false,
  top: 0,
  left: 0,
  lineIndex: 0,
  timestamp: null,
  error: 0,
  warn: 0,
})

function tooltipTargetFromY(
  clientY: number,
): { lineIndex: number; bucketIndex: number } | null {
  const canvas = minimapEl.value
  if (!canvas || effectiveCount.value === 0) return null
  const rect = canvas.getBoundingClientRect()
  if (rect.height <= 0) return null
  const ratio = Math.max(0, Math.min(1, (clientY - rect.top) / rect.height))
  const virtualIdx = Math.min(
    effectiveCount.value - 1,
    Math.floor(ratio * effectiveCount.value),
  )
  const bucketCount = minimapBuckets.value.length
  const bucketIndex = bucketCount === 0
    ? -1
    : Math.min(bucketCount - 1, Math.floor(ratio * bucketCount))
  return { lineIndex: actualLineIndex(virtualIdx), bucketIndex }
}

function timestampForLine(lineIndex: number): string | null {
  const tab = props.tab
  const row = tab.lineRow(lineIndex)
  if (!row) return null
  if (row.fields?.timestamp) {
    const [s, e] = row.fields.timestamp
    return row.text.slice(s, e)
  }
  for (let i = lineIndex - 1; i >= 0; i--) {
    const candidate = tab.lineRow(i)
    if (!candidate || candidate.record_idx !== row.record_idx) break
    if (candidate.fields?.timestamp) {
      const [s, e] = candidate.fields.timestamp
      return candidate.text.slice(s, e)
    }
  }
  return null
}

function updateMinimapTooltip(ev: PointerEvent) {
  const target = tooltipTargetFromY(ev.clientY)
  if (target === null) {
    minimapTooltip.value = {
      visible: false, top: 0, left: 0,
      lineIndex: 0, timestamp: null, error: 0, warn: 0,
    }
    return
  }
  const { lineIndex, bucketIndex } = target
  const pageIdx = Math.floor(lineIndex / PAGE_SIZE)
  if (!props.tab.pages.value.has(pageIdx)) void props.tab.fetchPage(pageIdx)
  const ts = timestampForLine(lineIndex)
  const canvas = minimapEl.value
  const rect = canvas?.getBoundingClientRect()
  const left = rect ? rect.left : ev.clientX
  const bucket = bucketIndex >= 0 ? minimapBuckets.value[bucketIndex] : null
  minimapTooltip.value = {
    visible: true,
    top: ev.clientY,
    left,
    lineIndex,
    timestamp: ts,
    error: bucket?.error ?? 0,
    warn: bucket?.warn ?? 0,
  }
}

function onMinimapPointerEnter(ev: PointerEvent) {
  updateMinimapTooltip(ev)
}
function onMinimapPointerLeave() {
  minimapTooltip.value = { visible: false, top: 0, left: 0, lineIndex: 0, timestamp: null, error: 0, warn: 0 }
}
let minimapDragging = false
function bookmarkVirtualIdxAtY(clientY: number): number | null {
  const visuals = bookmarkVisuals.value
  if (visuals.length === 0) return null
  const canvas = minimapEl.value
  if (!canvas) return null
  const rect = canvas.getBoundingClientRect()
  const eff = effectiveCount.value
  if (rect.height <= 0 || eff === 0) return null
  // Match the stripe Y projection used in paintBookmarkMarkers.
  const HIT_TOLERANCE_PX = 4
  let best: { v: number; dist: number } | null = null
  for (const bm of visuals) {
    const y = rect.top + Math.min(rect.height - 1, Math.floor((bm.virtualIdx * rect.height) / eff))
    const dist = Math.abs(clientY - y)
    if (dist > HIT_TOLERANCE_PX) continue
    if (!best || dist < best.dist) best = { v: bm.virtualIdx, dist }
  }
  return best ? best.v : null
}

function onMinimapPointerDown(ev: PointerEvent) {
  props.tab.followTail.value = false
  const bookmarkV = bookmarkVirtualIdxAtY(ev.clientY)
  if (bookmarkV !== null) {
    virtualizer.value.scrollToIndex(bookmarkV, { align: 'center' })
    return
  }
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

// --- Watchers tying everything together ---
watch(filteredSourceRecords, () => {
  scheduleMinimapFetch(true)
})

// Repaint when bookmarks change so the accent stripes refresh.
watch(
  () => props.tab.bookmarks.value,
  () => paintMinimap(),
)

watch(
  () => props.tab.pages.value,
  () => {
    if (!minimapTooltip.value.visible) return
    const idx = minimapTooltip.value.lineIndex
    const ts = timestampForLine(idx)
    if (ts !== minimapTooltip.value.timestamp) {
      minimapTooltip.value = { ...minimapTooltip.value, timestamp: ts }
    }
  },
)

// Tail-driven line_count growth: fetch the latest page and, when
// following, jump to the bottom.
watch(
  () => props.tab.file.value.line_count,
  (cur, prev) => {
    if (cur === prev) return
    scheduleMinimapFetch()
    if (props.tab.followTail.value) jumpToBottom()
  },
)

// Restore scroll on mount (per-tab persisted scrollTop).
let resizeObserver: ResizeObserver | null = null
// Theme observer: the minimap canvas reads `--bg-viewport` at paint time,
// so a theme swap on `<html data-theme="...">` leaves a stale stripe down
// the side of the viewport. Repaint whenever the attribute changes.
let themeObserver: MutationObserver | null = null

onMounted(() => {
  // Initial fetch
  const lc = props.tab.file.value.line_count
  if (lc > 0) {
    const lastPage = Math.floor((lc - 1) / PAGE_SIZE)
    void props.tab.fetchPage(lastPage)
  } else {
    void props.tab.fetchPage(0)
  }

  // Restore per-tab scrollTop after the virtualizer has mounted the
  // initial overscan window. Two rAFs so the total height has settled.
  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      const el = scrollEl.value
      if (!el) return
      if (props.tab.scrollTop.value > 0) {
        el.scrollTop = props.tab.scrollTop.value
      } else if (props.tab.followTail.value && effectiveCount.value > 0) {
        jumpToBottom()
      }
    })
  })

  // Drop unread badge once visible.
  props.tab.unread.value = false

  resizeObserver = new ResizeObserver((entries) => {
    for (const entry of entries) {
      const h = Math.floor(entry.contentRect.height)
      if (h !== viewportHeightPx.value) {
        viewportHeightPx.value = h
        scheduleMinimapFetch()
      }
    }
  })
  const el = scrollEl.value
  if (el) {
    resizeObserver.observe(el)
    viewportHeightPx.value = Math.floor(el.clientHeight)
    scheduleMinimapFetch(true)
  }

  // Repaint the minimap whenever the theme attribute on <html> flips.
  themeObserver = new MutationObserver(() => {
    requestAnimationFrame(() => paintMinimap())
  })
  themeObserver.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ['data-theme'],
  })
})

onBeforeUnmount(() => {
  if (resizeObserver) {
    resizeObserver.disconnect()
    resizeObserver = null
  }
  if (themeObserver) {
    themeObserver.disconnect()
    themeObserver = null
  }
  // Save current scroll into the tab so a return tab-switch restores it.
  const el = scrollEl.value
  if (el) props.tab.scrollTop.value = el.scrollTop
})

// --- Rendering ---
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
  if (fields.message) marks.push({ start: fields.message[0], end: fields.message[1], cls: 'message' })
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

function searchSpansForLine(row: LineRow): { start: number; end: number; cls: string }[] {
  const hit = props.tab.hits.value.get(row.record_idx)
  if (!hit) return []
  const boff = row.byte_offset_in_record
  const len = row.text.length
  const out: { start: number; end: number; cls: string }[] = []
  for (const [s, e] of hit.ranges) {
    const ls = Math.max(0, s - boff)
    const le = Math.min(len, e - boff)
    if (le > ls) out.push({ start: ls, end: le, cls: 'h-search-match' })
  }
  return out
}

function renderLine(row: LineRow): LeafSpan[] {
  // Register reactive dep on the engine's rule version so saved rule edits
  // re-render the viewport without needing a scroll or tab switch.
  // eslint-disable-next-line @typescript-eslint/no-unused-expressions
  rulesVersionRef.value
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
    emit('error', (e as Error).message)
  }
}

function levelGutterVar(level: string): string {
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

function onIdxClick(lineIdx: number, ev: MouseEvent) {
  ev.stopPropagation()
  props.tab.toggleBookmark(lineIdx)
}

function onIdxContextMenu(lineIdx: number, ev: MouseEvent) {
  ev.preventDefault()
  ev.stopPropagation()
  props.tab.removeBookmark(lineIdx)
}

function heatLine(error: number, warn: number): string {
  const parts: string[] = []
  if (error > 0) parts.push(`${error} ${error === 1 ? 'error' : 'errors'}`)
  if (warn > 0) parts.push(`${warn} ${warn === 1 ? 'warning' : 'warnings'}`)
  return parts.join(', ')
}

defineExpose({
  scrollToCurrentHit,
  jumpToBottom,
})
</script>

<template>
  <div class="viewport-shell">
    <div ref="scrollEl" class="viewport" @scroll.passive="onViewportScroll">
      <div v-if="stickyHeader" class="sticky-shell">
        <div
          class="row is-header"
          :class="[
            'level-row-' + stickyHeader.row.level,
            { 'is-bookmarked': tab.isBookmarked(stickyHeader.lineIndex) },
          ]"
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
                'is-bookmarked': tab.isBookmarked(actualLineIndex(vrow.index)),
                'is-marker': markerLineLookup.get(actualLineIndex(vrow.index)) !== undefined,
              },
              'level-row-' + (lineRowVirtual(vrow.index)?.level ?? 'unknown'),
              markerLineLookup.get(actualLineIndex(vrow.index))
                ? `marker-row-${markerLineLookup.get(actualLineIndex(vrow.index))}`
                : '',
            ]"
            :style="{
              transform: `translateY(${vrow.start}px)`,
              height: `${vrow.size}px`,
              '--gutter-color': levelGutterVar(lineRowVirtual(vrow.index)?.level ?? 'unknown'),
              '--marker-row-colour': markerLineLookup.get(actualLineIndex(vrow.index))
                ? markerColourVar(markerLineLookup.get(actualLineIndex(vrow.index))!)
                : 'transparent',
            }"
            :data-marker-label="markerLineLookup.get(actualLineIndex(vrow.index))
              ? markerLabel(markerLineLookup.get(actualLineIndex(vrow.index))!)
              : null"
          >
            <span class="gutter" />
            <span
              class="idx idx-interactive"
              :title="tab.isBookmarked(actualLineIndex(vrow.index))
                ? 'Click to remove bookmark'
                : 'Click to add bookmark'"
              @click="onIdxClick(actualLineIndex(vrow.index), $event)"
              @contextmenu="onIdxContextMenu(actualLineIndex(vrow.index), $event)"
            >{{ actualLineIndex(vrow.index) + 1 }}</span>
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
    <div class="marker-rail" aria-hidden="false">
      <button
        v-for="m in markerVisuals"
        :key="`${m.kind}-${m.lineIdx}`"
        type="button"
        class="marker-triangle"
        :class="`marker-${m.kind}`"
        :style="{
          top: `${effectiveCount > 0 ? (m.virtualIdx / effectiveCount) * viewportHeightPx : 0}px`,
          borderLeftColor: markerColourVar(m.kind),
        }"
        :title="`${markerLabel(m.kind)} - line ${m.lineIdx + 1}`"
        @click="jumpToLine(m.lineIdx)"
      />
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
        <span
          v-if="minimapTooltip.error > 0 || minimapTooltip.warn > 0"
          class="heat"
        >{{ heatLine(minimapTooltip.error, minimapTooltip.warn) }}</span>
      </div>
    </div>
    <canvas
      v-if="speedRailVisible"
      ref="speedRailEl"
      class="speed-rail"
    />
    <button
      v-if="!tab.followTail.value && !atBottom"
      type="button"
      class="jump-bottom-floating"
      title="Jump to bottom and re-enable follow"
      aria-label="Jump to bottom"
      @click="toggleFollowTail"
    >&darr;</button>
  </div>
</template>

<style scoped>
.viewport-shell {
  flex: 1 1 auto;
  display: flex;
  flex-direction: row;
  min-height: 0;
  position: relative;
  overflow: hidden;

  .jump-bottom-floating {
    position: absolute;
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

.marker-rail {
  flex: 0 0 auto;
  width: 10px;
  position: relative;
  background: var(--bg-viewport);
  pointer-events: auto;

  .marker-triangle {
    position: absolute;
    left: 0;
    /* Right-pointing triangle: a zero-width box with a coloured left
       border and transparent top/bottom borders. The tip sits on the
       right edge so it aims at the minimap canvas. */
    width: 0;
    height: 0;
    padding: 0;
    background: transparent;
    border-style: solid;
    border-width: 4px 0 4px 6px;
    border-top-color: transparent;
    border-bottom-color: transparent;
    border-right-color: transparent;
    /* border-left-color is set inline per marker kind. */
    transform: translateY(-4px);
    cursor: pointer;
    opacity: 0.85;
    transition: opacity 120ms ease-out, filter 120ms ease-out;
  }

  .marker-triangle:hover,
  .marker-triangle:focus-visible {
    opacity: 1;
    filter: drop-shadow(0 0 2px rgba(255, 255, 255, 0.4));
    outline: none;
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

  /* Lozenge grip inside the window handle - inset so the surrounding
     level colours remain visible around it, signalling "draggable". */
  .minimap-indicator::after {
    content: '';
    position: absolute;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    width: 8px;
    height: 50%;
    max-height: 24px;
    min-height: 6px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.7);
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.55);
  }

  &:hover .minimap-indicator {
    background: rgba(255, 255, 255, 0.32);
    border-color: var(--fg-default);
  }

  &:hover .minimap-indicator::after {
    background: var(--fg-default);
  }
}

.minimap-tooltip {
  position: fixed;
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
  .heat { color: var(--hl-search-fg); font-weight: 600; }

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
  /* Horizontal track always reserved so a long line scrolling into view
     doesn't squash the viewport by 10px, which would resize the minimap
     canvas and make it jump mid-scroll. */
  overflow-x: scroll;
  overflow-y: auto;
  /* Do not set `scrollbar-width` / `scrollbar-color` here -- they would
     override the ::-webkit-scrollbar rules in Chromium (Webview2) and
     drop the accent hover state. The per-axis hide below suppresses the
     vertical track; the minimap is the vertical affordance. */
  font-family: var(--font-mono);
  font-size: var(--font-size-base);
  line-height: var(--row-height);
  background-color: var(--bg-viewport);

  /* width = vertical scrollbar width (0 -> hidden, replaced by minimap);
     height = horizontal scrollbar height (inherits the app-wide look). */
  &::-webkit-scrollbar { width: 0; height: 10px; }

  .total {
    position: relative;
    width: 100%;
    background-image:
      linear-gradient(
        to bottom,
        var(--bg-skeleton-gutter) 0,
        var(--bg-skeleton-gutter) 100%
      ),
      linear-gradient(
        to bottom,
        transparent 0,
        transparent 5px,
        var(--bg-skeleton-num) 5px,
        var(--bg-skeleton-num) 13px,
        transparent 13px
      ),
      linear-gradient(
        to bottom,
        transparent 0,
        transparent 5px,
        var(--bg-skeleton) 5px,
        var(--bg-skeleton) 13px,
        transparent 13px
      ),
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
    /* Width is driven by content so the viewport's horizontal scrollbar
       reflects the widest currently-rendered line. min-width keeps short
       rows full-bleed for the level-row gradient backgrounds. */
    min-width: 100%;
    width: max-content;
    z-index: 1;
    display: grid;
    grid-template-columns: var(--gutter-width) var(--line-num-width) max-content;
    align-items: center;
    white-space: pre;
    color: var(--fg-row);
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
      position: relative;

      /* Bookmark marker - a small accent-coloured pin on the left edge of
         the line-number cell. Hidden by default; faint on hover for any
         interactive idx; solid when the row is bookmarked. */
      &.idx-interactive {
        cursor: pointer;

        &::before {
          content: '';
          position: absolute;
          left: 4px;
          top: 50%;
          width: 6px;
          height: 8px;
          transform: translateY(-50%);
          background: var(--accent);
          clip-path: polygon(0 0, 100% 0, 100% 100%, 50% 70%, 0 100%);
          opacity: 0;
          transition: opacity 80ms ease-out;
          pointer-events: none;
        }

        &:hover::before { opacity: 0.35; }
      }
    }


    .txt {
      padding-right: 0.6rem;
    }

    &.is-continuation .txt {
      padding-left: var(--continuation-indent);
      color: var(--fg-message);
    }

    .s-level { font-weight: 600; }
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

    &.level-row-warn {
      background-image: linear-gradient(
        to right,
        color-mix(in srgb, var(--level-warn) 10%, transparent),
        transparent 25%
      );
    }
    &.level-row-error {
      background-image: linear-gradient(
        to right,
        color-mix(in srgb, var(--level-error) 10%, transparent),
        transparent 50%
      );
    }
    &.level-row-fatal {
      background-image: linear-gradient(
        to right,
        color-mix(in srgb, var(--level-fatal) 10%, transparent),
        transparent 75%
      );
    }

    &.is-current-hit {
      background-image: linear-gradient(
        color-mix(in srgb, var(--hl-search-bg) 22%, transparent),
        color-mix(in srgb, var(--hl-search-bg) 22%, transparent)
      );
      box-shadow:
        inset 0 1px 0 var(--hl-search-bg),
        inset 0 -1px 0 var(--hl-search-bg);
    }

    /* Placed last so the bookmark gradient overrules any level-row-* or
       is-current-hit gradient set above. */
    &.is-bookmarked .idx::before { opacity: 1 !important; }
    &.is-bookmarked .idx { color: var(--accent); }
    &.is-bookmarked {
      background-image: linear-gradient(
        to right,
        color-mix(in srgb, var(--accent) 12%, transparent),
        transparent 60%
      );
    }

    /* Marker rows: a thin coloured top border plus a faint horizontal
       wash + an inline label tag floated against the right edge. Doesn't
       displace any text since marker rows are themselves the record's
       first line. --marker-row-colour is set inline via :style. */
    &.is-marker {
      border-top: 1px solid var(--marker-row-colour);
      box-shadow: inset 0 1px 0 var(--marker-row-colour);
      background-image: linear-gradient(
        to right,
        color-mix(in srgb, var(--marker-row-colour) 14%, transparent),
        color-mix(in srgb, var(--marker-row-colour) 4%, transparent) 60%,
        transparent
      );
    }

    &.is-marker::after {
      content: attr(data-marker-label);
      position: sticky;
      right: 0.4rem;
      margin-left: auto;
      padding: 0 0.4rem;
      font-size: 0.7rem;
      font-weight: 600;
      letter-spacing: 0.04em;
      text-transform: uppercase;
      color: var(--marker-row-colour);
      background: color-mix(in srgb, var(--bg-viewport) 85%, transparent);
      border: 1px solid color-mix(in srgb, var(--marker-row-colour) 45%, transparent);
      border-radius: 3px;
      align-self: center;
      pointer-events: none;
      flex: 0 0 auto;
    }
  }

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
      min-width: 100%;
      width: max-content;
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
      &:focus-visible { outline: 1px solid var(--accent); outline-offset: -1px; }
    }
  }
}

.speed-rail {
  flex: 0 0 auto;
  width: 4px;
  display: block;
  cursor: pointer;
  image-rendering: pixelated;
}
</style>
