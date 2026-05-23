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
import { highlightsFor, overlay, type LeafSpan } from '../highlight/engine'
import {
  OVERSCAN,
  PAGE_SIZE,
  ROW_HEIGHT,
  type HeaderFields,
  type LevelMinimapPayload,
  type LineRow,
  type RecordRef,
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

// --- Local UI state (per-mount) ---
const minimapBuckets = ref<string[]>([])
const viewportHeightPx = ref(0)
const viewportScrollTop = ref(0)
let minimapFetchPending = false
let lastMinimapLineCount = -1
let lastMinimapHeight = -1
const MINIMAP_WIDTH = 20

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

// Scroll-to-current-hit, defined here because it needs the virtualizer.
function scrollToCurrentHit() {
  const tab = props.tab
  if (tab.currentHit.value < 0) return
  const recIdx = tab.hitOrder.value[tab.currentHit.value]
  const hit = tab.hits.value.get(recIdx)
  if (!hit) return
  const filt = filteredLineIndices.value
  let targetVirtual: number
  if (filt) {
    const want = hit.record_first_line
    targetVirtual = filt.indexOf(want)
    if (targetVirtual < 0) return
  } else {
    targetVirtual = hit.record_first_line
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
  const txt = match.closest('.txt') as HTMLElement | null
  if (!txt) return false
  if (txt.scrollWidth <= txt.clientWidth) return true
  const matchRect = match.getBoundingClientRect()
  const txtRect = txt.getBoundingClientRect()
  const matchLeftInContent = matchRect.left - txtRect.left + txt.scrollLeft
  const targetScrollLeft = matchLeftInContent - txt.clientWidth / 2 + match.offsetWidth / 2
  const maxScrollLeft = txt.scrollWidth - txt.clientWidth
  txt.scrollLeft = Math.max(0, Math.min(maxScrollLeft, targetScrollLeft))
  return true
}

// --- Minimap ---
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
  const height = viewportHeightPx.value
  if (height <= 0) return
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
  if (props.tab.file.value.line_count === 0) {
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
  if (!canvas || effectiveCount.value === 0) return null
  const rect = canvas.getBoundingClientRect()
  if (rect.height <= 0) return null
  const ratio = Math.max(0, Math.min(1, (clientY - rect.top) / rect.height))
  const virtualIdx = Math.min(
    effectiveCount.value - 1,
    Math.floor(ratio * effectiveCount.value),
  )
  return actualLineIndex(virtualIdx)
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
  const idx = tooltipLineFromY(ev.clientY)
  if (idx === null) {
    minimapTooltip.value = { visible: false, top: 0, left: 0, lineIndex: 0, timestamp: null }
    return
  }
  const pageIdx = Math.floor(idx / PAGE_SIZE)
  if (!props.tab.pages.value.has(pageIdx)) void props.tab.fetchPage(pageIdx)
  const ts = timestampForLine(idx)
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

function onMinimapPointerEnter(ev: PointerEvent) {
  updateMinimapTooltip(ev)
}
function onMinimapPointerLeave() {
  minimapTooltip.value = { visible: false, top: 0, left: 0, lineIndex: 0, timestamp: null }
}
let minimapDragging = false
function onMinimapPointerDown(ev: PointerEvent) {
  props.tab.followTail.value = false
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
    right: 0;
    z-index: 1;
    display: grid;
    grid-template-columns: var(--gutter-width) var(--line-num-width) 1fr;
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
    }

    .txt {
      padding-right: 0.6rem;
      overflow-x: auto;
      overflow-y: hidden;
      scrollbar-width: none;

      &::-webkit-scrollbar { display: none; }
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
      &:focus-visible { outline: 1px solid var(--accent); outline-offset: -1px; }
    }
  }
}
</style>
