<script setup lang="ts">
/**
 * Right-side collapsible drawer hosting the slow-request insights for
 * the active tab. Renders the totals header, mode/filter/sort toolbar,
 * and a sortable entry list with click-to-jump + expandable occurrence
 * rows. Threshold editor and speed grid land in later tasks.
 */
import { computed, inject, nextTick, onBeforeUnmount, onMounted, ref, useTemplateRef, watch, type Ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useVirtualizer } from '@tanstack/vue-virtual'
import type { Tab } from '../tab'
import type { EffectiveThresholds, SlowRequestEntry, SlowRequestSummary, SlowRequestThresholds, SpeedGrid } from '../types'

const props = defineProps<{ tab: Tab }>()

const emit = defineEmits<{
  (e: 'jump', line: number): void
  (e: 'thresholds-changed'): void
}>()

const expanded = ref<Set<string>>(new Set())
const error = ref<string | null>(null)
const loading = ref(false)

// Drawer width is user-resizable via the left-edge handle and persists
// across sessions in localStorage. Clamped to keep the toolbar usable
// and to stop the drawer eating the entire window.
const MIN_WIDTH = 240
const MAX_WIDTH = 800
const STORAGE_KEY = 'clog.insightsDrawer.width'
function loadWidth(): number {
  const raw = globalThis.localStorage?.getItem(STORAGE_KEY)
  const n = raw === null || raw === undefined ? Number.NaN : Number(raw)
  if (!Number.isFinite(n)) return 360
  return Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, n))
}
const drawerWidth = ref(loadWidth())
let resizeStartX = 0
let resizeStartWidth = 0

function onResizeDown(ev: PointerEvent) {
  resizeStartX = ev.clientX
  resizeStartWidth = drawerWidth.value
  ;(ev.currentTarget as Element).setPointerCapture(ev.pointerId)
  ev.preventDefault()
}

function onResizeMove(ev: PointerEvent) {
  const target = ev.currentTarget as Element
  if (!target.hasPointerCapture(ev.pointerId)) return
  // Handle sits on the left edge of a right-anchored drawer, so
  // dragging left (negative deltaX) grows the drawer.
  const next = resizeStartWidth - (ev.clientX - resizeStartX)
  drawerWidth.value = Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, next))
}

function onResizeUp(ev: PointerEvent) {
  const target = ev.currentTarget as Element
  if (target.hasPointerCapture(ev.pointerId)) {
    target.releasePointerCapture(ev.pointerId)
  }
  globalThis.localStorage?.setItem(STORAGE_KEY, String(drawerWidth.value))
}

async function refresh() {
  loading.value = true
  error.value = null
  try {
    const payload = await invoke<SlowRequestSummary>('get_slow_requests', {
      fileId: props.tab.file.value.file_id,
      mode: props.tab.slowRequestMode.value,
    })
    props.tab.slowRequestSummary.value = payload
  } catch (e) {
    error.value = String((e as { message?: string })?.message ?? e)
  } finally {
    loading.value = false
  }
  void fetchSpeedGrid()
}

// --- Horizontal speed chart ------------------------------------------------
//
// Per-bucket average request time across the file's line range, painted as
// a bar chart in the drawer header area. Clicking a bar jumps the log
// viewport to the line at the centre of that bucket. Backed by the same
// build_speed_grid IPC the vertical speed rail uses, so paint logic and
// auto/threshold colouring stay consistent.

const chartEl = useTemplateRef<HTMLCanvasElement>('chartEl')
const speedGrid = ref<SpeedGrid | null>(null)
const CHART_HEIGHT = 67
// Gutters reserved for axis labels (CSS px). LEFT_PAD fits a ms label
// like "12s" or "999ms"; BOTTOM_PAD fits a single line of x-axis labels
// with breathing room from the host's border.
const LEFT_PAD = 36
const RIGHT_PAD = 8
const TOP_PAD = 12
const BOTTOM_PAD = 20
const chartTooltip = ref<{ visible: boolean; x: number; text: string }>({
  visible: false,
  x: 0,
  text: '',
})
const chartCrosshair = ref<{ visible: boolean; x: number; y: number }>({
  visible: false,
  x: 0,
  y: 0,
})

async function fetchSpeedGrid() {
  const canvas = chartEl.value
  if (!canvas) return
  const w = canvas.clientWidth
  if (w <= 0) return
  // One bucket per 4 CSS px across the plotting area (excluding axis
  // gutters) gives a readable bar chart without sending an absurd
  // payload for narrow drawers. Clamped to >=1 for safety.
  const plotW = Math.max(1, w - LEFT_PAD - RIGHT_PAD)
  const bucketCount = Math.max(1, Math.floor(plotW / 4))
  try {
    const payload = await invoke<SpeedGrid>('get_slow_request_speeds', {
      fileId: props.tab.file.value.file_id,
      bucketCount,
    })
    speedGrid.value = payload
    paintChart()
  } catch {
    // non-fatal; chart simply stays blank
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

function colourFor(avgMs: number, maxMs: number, fast: number, slow: number, auto: boolean): string {
  const fastC = readCssColour('--speed-fast')
  const midC = readCssColour('--speed-mid')
  const slowC = readCssColour('--speed-slow')
  if (auto) {
    const score = (avgMs + maxMs) / 2
    if (slow <= 0 || score >= slow) return slowC
    return lerpColour(midC, slowC, Math.max(0, score) / slow)
  }
  if (avgMs <= fast || slow <= fast) return fastC
  if (avgMs >= slow) return slowC
  const t = (avgMs - fast) / (slow - fast)
  if (t < 0.5) return lerpColour(fastC, midC, t * 2)
  return lerpColour(midC, slowC, (t - 0.5) * 2)
}

function niceCeil(value: number): number {
  if (value <= 0) return 1
  const exp = Math.pow(10, Math.floor(Math.log10(value)))
  const norm = value / exp
  let nice: number
  if (norm <= 1) nice = 1
  else if (norm <= 2) nice = 2
  else if (norm <= 5) nice = 5
  else nice = 10
  return nice * exp
}

function formatLine(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${Math.round(n / 1_000)}k`
  return String(n)
}

function paintChart() {
  const canvas = chartEl.value
  const grid = speedGrid.value
  if (!canvas || !grid) return
  const w = canvas.clientWidth
  const h = CHART_HEIGHT
  if (w <= 0) return
  const dpr = globalThis.devicePixelRatio || 1
  canvas.width = Math.floor(w * dpr)
  canvas.height = Math.floor(h * dpr)
  canvas.style.height = `${h}px`
  const ctx = canvas.getContext('2d')
  if (!ctx) return
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
  ctx.clearRect(0, 0, w, h)

  const plotX = LEFT_PAD
  const plotY = TOP_PAD
  const plotW = Math.max(1, w - LEFT_PAD - RIGHT_PAD)
  const plotH = Math.max(1, h - TOP_PAD - BOTTOM_PAD)

  const axisColour = readCssColour('--border-default')
  const labelColour = readCssColour('--fg-muted')
  ctx.font = '9px var(--font-sans, sans-serif)'
  ctx.textBaseline = 'middle'

  // Axes.
  ctx.fillStyle = axisColour
  ctx.fillRect(plotX, plotY + plotH, plotW, 1)          // x-axis
  ctx.fillRect(plotX - 1, plotY, 1, plotH + 1)          // y-axis

  const bc = grid.buckets.length
  const maxAvgRaw = grid.max_avg_ms
  const yMax = niceCeil(Math.max(1, maxAvgRaw))
  const eff = props.tab.slowRequestThresholds.value?.effective
  const fast = eff ? eff.fast_ms : 0
  const slow = eff ? Math.max(eff.slow_ms, fast + 1) : 6000
  const auto = (props.tab.slowRequestThresholds.value?.source ?? 'auto') === 'auto'

  // Y-axis ticks at 0, mid, max.
  ctx.fillStyle = labelColour
  ctx.textAlign = 'right'
  const yTicks: number[] = [0, yMax / 2, yMax]
  for (const t of yTicks) {
    const ty = plotY + plotH - (t / yMax) * plotH
    ctx.fillStyle = axisColour
    ctx.fillRect(plotX - 3, Math.round(ty), 3, 1)
    ctx.fillStyle = labelColour
    ctx.fillText(formatMs(Math.round(t)), plotX - 5, ty)
  }

  // X-axis ticks: start, middle, end of file (line count).
  const lc = props.tab.file.value.line_count
  if (lc > 0) {
    const xTicks: Array<{ frac: number; align: CanvasTextAlign; line: number }> = [
      { frac: 0, align: 'left', line: 1 },
      { frac: 0.5, align: 'center', line: Math.max(1, Math.round(lc / 2)) },
      { frac: 1, align: 'right', line: lc },
    ]
    for (const t of xTicks) {
      const tx = plotX + t.frac * plotW
      ctx.fillStyle = axisColour
      ctx.fillRect(Math.round(tx), plotY + plotH + 1, 1, 3)
      ctx.fillStyle = labelColour
      ctx.textAlign = t.align
      ctx.fillText(formatLine(t.line), tx, plotY + plotH + 9)
    }
  }

  // Bars.
  if (bc > 0) {
    for (let i = 0; i < bc; i++) {
      const b = grid.buckets[i]
      if (b.count === 0) continue
      const heightFrac = Math.min(1, b.avg_ms / yMax)
      const barH = Math.max(1, Math.round(heightFrac * plotH))
      const x = plotX + Math.floor((i * plotW) / bc)
      const xNext = plotX + Math.floor(((i + 1) * plotW) / bc)
      const rectW = Math.max(1, xNext - x)
      ctx.fillStyle = colourFor(b.avg_ms, b.max_ms, fast, slow, auto)
      ctx.fillRect(x, plotY + plotH - barH, rectW, barH)
    }
  }
}

function bucketAtX(clientX: number): { index: number; line: number } | null {
  const canvas = chartEl.value
  const grid = speedGrid.value
  if (!canvas || !grid || grid.buckets.length === 0) return null
  const rect = canvas.getBoundingClientRect()
  const plotW = Math.max(1, rect.width - LEFT_PAD - RIGHT_PAD)
  const x = clientX - rect.left - LEFT_PAD
  if (x < 0 || x > plotW) return null
  const bc = grid.buckets.length
  const idx = Math.max(0, Math.min(bc - 1, Math.floor((x / plotW) * bc)))
  const lc = props.tab.file.value.line_count
  const line = lc > 0 ? Math.floor(((idx + 0.5) * lc) / bc) : 0
  return { index: idx, line }
}

function onChartMove(ev: PointerEvent) {
  const hit = bucketAtX(ev.clientX)
  const grid = speedGrid.value
  const canvas = chartEl.value
  if (!hit || !grid || !canvas) {
    chartTooltip.value = { visible: false, x: 0, text: '' }
    chartCrosshair.value = { visible: false, x: 0, y: 0 }
    return
  }
  const rect = canvas.getBoundingClientRect()
  const localX = ev.clientX - rect.left
  const localY = ev.clientY - rect.top
  // Clamp crosshair to the plot region so the lines never paint over the
  // axis gutters.
  const plotLeft = LEFT_PAD
  const plotRight = rect.width - RIGHT_PAD
  const plotTop = TOP_PAD
  const plotBottom = rect.height - BOTTOM_PAD
  const cx = Math.max(plotLeft, Math.min(plotRight, localX))
  const cy = Math.max(plotTop, Math.min(plotBottom, localY))
  chartCrosshair.value = { visible: true, x: cx, y: cy }

  const b = grid.buckets[hit.index]
  const text = b.count === 0
    ? `line ${hit.line + 1} - no slow requests in this slice`
    : `line ${hit.line + 1} - avg ${formatMs(b.avg_ms)} (max ${formatMs(b.max_ms)}, ${b.count} hits)`
  chartTooltip.value = { visible: true, x: localX, text }
}

function onChartLeave() {
  chartTooltip.value = { visible: false, x: 0, text: '' }
  chartCrosshair.value = { visible: false, x: 0, y: 0 }
}

function onChartClick(ev: PointerEvent) {
  const hit = bucketAtX(ev.clientX)
  if (!hit) return
  emit('jump', hit.line)
}

const fastInput = ref('')
const slowInput = ref('')

// Defaults presented when no per-file override is set: whatever's currently
// in effect (auto or global). A blank input on save falls back to these
// so the user can tweak one side without typing the other.
function effectiveFastDefault(): number {
  return props.tab.slowRequestThresholds.value?.effective.fast_ms ?? 2000
}
function effectiveSlowDefault(): number {
  return props.tab.slowRequestThresholds.value?.effective.slow_ms ?? 10000
}

function resolvedInput(input: string, fallback: number): number | null {
  if (input === '') return fallback
  const n = Number(input)
  if (Number.isNaN(n)) return null
  return n
}

const validationError = computed<string | null>(() => {
  const fast = resolvedInput(fastInput.value, effectiveFastDefault())
  const slow = resolvedInput(slowInput.value, effectiveSlowDefault())
  if (fast === null || slow === null) return 'Numbers only'
  if (fast >= slow) return 'fast must be less than slow'
  if (slow > 600_000) return 'slow capped at 600,000 (10 min)'
  return null
})

async function refreshThresholds() {
  try {
    const payload = await invoke<EffectiveThresholds>('get_slow_request_thresholds', {
      fileId: props.tab.file.value.file_id,
    })
    props.tab.slowRequestThresholds.value = payload
    // Pre-populate the inputs with whatever is currently in effect (either
    // the existing per-file override, or the auto/global default) so the
    // user can immediately tweak.
    fastInput.value = String(payload.per_file?.fast_ms ?? payload.effective.fast_ms)
    slowInput.value = String(payload.per_file?.slow_ms ?? payload.effective.slow_ms)
  } catch {
    // non-fatal
  }
}

async function savePerFile() {
  if (validationError.value) return
  const fast = resolvedInput(fastInput.value, effectiveFastDefault()) ?? effectiveFastDefault()
  const slow = resolvedInput(slowInput.value, effectiveSlowDefault()) ?? effectiveSlowDefault()
  const t: SlowRequestThresholds = { fast_ms: fast, slow_ms: slow }
  try {
    await invoke<void>('save_slow_request_thresholds', {
      fileId: props.tab.file.value.file_id,
      thresholds: t,
    })
    await refreshThresholds()
    emit('thresholds-changed')
  } catch (e) {
    error.value = String((e as { message?: string })?.message ?? e)
  }
}

async function clearPerFile() {
  // Wipe the per-file override server-side; the next refreshThresholds
  // will repopulate the inputs from the auto/global fallback.
  try {
    await invoke<void>('save_slow_request_thresholds', {
      fileId: props.tab.file.value.file_id,
      thresholds: null,
    })
    await refreshThresholds()
    emit('thresholds-changed')
  } catch (e) {
    error.value = String((e as { message?: string })?.message ?? e)
  }
}

onMounted(() => {
  void refresh()
  void refreshThresholds()
})

// When global settings change (e.g. Settings modal saves new thresholds),
// refetch the effective-thresholds payload so the chip + speed rail
// repaint without requiring a reload.
const settingsVersion = inject<Ref<number> | null>('settingsVersion', null)
if (settingsVersion) {
  watch(settingsVersion, () => {
    void refreshThresholds()
    emit('thresholds-changed')
  })
}

watch(() => props.tab.slowRequestMode.value, refresh)

// Repaint the chart when the effective thresholds change (auto/global/
// per-file) so the colour bands update without a full re-fetch.
watch(() => props.tab.slowRequestThresholds.value, () => {
  paintChart()
}, { deep: true })

// Drawer resize changes bucket count; refetch with a small debounce so
// dragging doesn't fire an IPC every frame.
let widthRefetchTimer: ReturnType<typeof setTimeout> | null = null
watch(drawerWidth, () => {
  if (widthRefetchTimer !== null) clearTimeout(widthRefetchTimer)
  widthRefetchTimer = setTimeout(() => {
    widthRefetchTimer = null
    void fetchSpeedGrid()
  }, 150)
})

// Tail-driven refreshes are debounced. A tailing file emits a line_count
// change roughly every 250 ms; refreshing the drawer on each tick means
// four full get_slow_requests IPCs per second plus four full
// re-renders of the entry table. The aggregator is also one of the
// heaviest IPCs we expose because it walks every occurrence. 500 ms
// coalesces those bursts without making the data feel stale.
let refreshDebounceTimer: ReturnType<typeof setTimeout> | null = null
function scheduleRefresh() {
  if (refreshDebounceTimer !== null) return
  refreshDebounceTimer = setTimeout(() => {
    refreshDebounceTimer = null
    if (props.tab.insightsOpen.value) void refresh()
  }, 500)
}
onBeforeUnmount(() => {
  if (refreshDebounceTimer !== null) {
    clearTimeout(refreshDebounceTimer)
    refreshDebounceTimer = null
  }
})
watch(
  () => props.tab.file.value.line_count,
  () => {
    if (props.tab.insightsOpen.value) scheduleRefresh()
  },
)

const totals = computed(() => {
  const s = props.tab.slowRequestSummary.value
  if (!s) return 'Loading...'
  if (s.total_hits === 0) return 'No slow requests detected.'
  return `${s.total_hits} hits across ${s.entries.length} endpoints, ${s.deduped} dedupes`
})

const filteredEntries = computed<SlowRequestEntry[]>(() => {
  const s = props.tab.slowRequestSummary.value
  if (!s) return []
  const filter = props.tab.slowRequestFilter.value.trim().toLowerCase()
  const filtered = filter
    ? s.entries.filter((e) => e.path.toLowerCase().includes(filter))
    : s.entries.slice()
  const { field, dir } = props.tab.slowRequestSort.value
  const sign = dir === 'asc' ? 1 : -1
  const key = (e: SlowRequestEntry): number | string => {
    switch (field) {
      case 'total': return e.total_ms
      case 'count': return e.count
      case 'max':   return e.max_ms
      case 'p95':   return e.p95_ms
      case 'avg':   return e.avg_ms
      case 'path':  return e.path
    }
  }
  filtered.sort((a, b) => {
    const ka = key(a)
    const kb = key(b)
    if (typeof ka === 'number' && typeof kb === 'number') {
      return sign * (ka - kb)
    }
    return sign * String(ka).localeCompare(String(kb))
  })
  return filtered
})

// Per-entry "slowness" score in 0..1, normalised across the currently
// visible entries (so filtering recalibrates). Weighted blend: total
// time dominates because it captures both volume and per-hit slowness,
// p95 surfaces tail latency, count surfaces noisy-but-quick endpoints.
const slownessScores = computed<Map<string, number>>(() => {
  const list = filteredEntries.value
  const map = new Map<string, number>()
  if (list.length === 0) return map
  let maxCount = 0
  let maxTotal = 0
  let maxP95 = 0
  for (const e of list) {
    if (e.count > maxCount) maxCount = e.count
    if (e.total_ms > maxTotal) maxTotal = e.total_ms
    if (e.p95_ms > maxP95) maxP95 = e.p95_ms
  }
  for (const e of list) {
    const cN = maxCount > 0 ? e.count / maxCount : 0
    const tN = maxTotal > 0 ? e.total_ms / maxTotal : 0
    const pN = maxP95 > 0 ? e.p95_ms / maxP95 : 0
    map.set(e.path, 0.5 * tN + 0.25 * pN + 0.25 * cN)
  }
  return map
})

// --- Virtualised entry list ------------------------------------------------
//
// Without virtualisation the drawer renders every entry into the DOM.
// In Raw mode on a busy file that can be hundreds of <li>s each with a
// ::before slowness bar - every scroll tick repaints the whole list.
// useVirtualizer keeps only the visible window plus a small overscan
// mounted. Row heights are dynamic: collapsed rows are small, expanded
// rows grow with the occurrence list. estimateSize gives the initial
// guess; the library's ResizeObserver (auto-attached by measureElement)
// corrects each row's real size on mount, and toggleExpanded calls
// virtualizer.measure() after the DOM updates to invalidate the cache.

const bodyEl = useTemplateRef<HTMLDivElement>('bodyEl')
const ENTRY_COLLAPSED_HEIGHT = 34
const OCC_HEIGHT = 22

const virtualizer = useVirtualizer(
  computed(() => ({
    count: filteredEntries.value.length,
    getScrollElement: () => bodyEl.value ?? null,
    estimateSize: (i: number) => {
      const e = filteredEntries.value[i]
      if (!e) return ENTRY_COLLAPSED_HEIGHT
      if (!expanded.value.has(e.path)) return ENTRY_COLLAPSED_HEIGHT
      return ENTRY_COLLAPSED_HEIGHT + e.occurrences.length * OCC_HEIGHT
    },
    overscan: 6,
    getItemKey: (i: number) => filteredEntries.value[i]?.path ?? i,
  })),
)

const virtualRows = computed(() => virtualizer.value.getVirtualItems())
const totalSize = computed(() => virtualizer.value.getTotalSize())

function measureVirtualRow(el: Element | null) {
  if (el) virtualizer.value.measureElement(el)
}

const originalToggle = toggleExpanded
function toggleExpandedVirtual(path: string) {
  originalToggle(path)
  void nextTick(() => virtualizer.value.measure())
}

function formatMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`
  return `${(ms / 60_000).toFixed(1)}m`
}

// Compact label for the currently selected sort stat, shown next to the
// hit count on each row. Returns null when the sort field is itself the
// hit count (no point repeating) or when sorting by path (no stat).
function sortStatLabel(e: SlowRequestEntry): string | null {
  switch (props.tab.slowRequestSort.value.field) {
    case 'total': return `total ${formatMs(e.total_ms)}`
    case 'avg':   return `avg ${formatMs(e.avg_ms)}`
    case 'p95':   return `p95 ${formatMs(e.p95_ms)}`
    case 'max':   return `max ${formatMs(e.max_ms)}`
    case 'count':
    case 'path':
    default:      return null
  }
}

function entryStatTitle(e: SlowRequestEntry): string {
  return `${e.count} hits . total ${formatMs(e.total_ms)} . avg ${formatMs(e.avg_ms)} . p95 ${formatMs(e.p95_ms)} . max ${formatMs(e.max_ms)}`
}

function toggleExpanded(path: string) {
  const s = expanded.value
  if (s.has(path)) s.delete(path)
  else s.add(path)
  expanded.value = new Set(s)
}

function onSortChange(ev: Event) {
  const t = (ev.target as HTMLSelectElement).value as
    | 'total' | 'count' | 'max' | 'p95' | 'avg' | 'path'
  const cur = props.tab.slowRequestSort.value
  if (cur.field === t) {
    props.tab.slowRequestSort.value = { field: t, dir: cur.dir === 'desc' ? 'asc' : 'desc' }
  } else {
    props.tab.slowRequestSort.value = { field: t, dir: 'desc' }
  }
}

function jumpTo(line: number) {
  emit('jump', line)
}
</script>

<template>
  <aside class="insights-drawer" :style="{ width: drawerWidth + 'px' }">
    <div
      class="resize-handle"
      aria-label="Resize insights drawer"
      @pointerdown="onResizeDown"
      @pointermove="onResizeMove"
      @pointerup="onResizeUp"
      @pointercancel="onResizeUp"
    />
    <header class="drawer-head">
      <span class="title">Slow requests</span>
    </header>
    <div class="drawer-totals">{{ totals }}</div>
    <div class="chart-host">
      <canvas
        ref="chartEl"
        class="chart-canvas"
        @pointermove="onChartMove"
        @pointerleave="onChartLeave"
        @click="onChartClick"
      />
      <div
        v-show="chartCrosshair.visible"
        class="chart-crosshair-v"
        :style="{ left: chartCrosshair.x + 'px' }"
      />
      <div
        v-show="chartCrosshair.visible"
        class="chart-crosshair-h"
        :style="{ top: chartCrosshair.y + 'px' }"
      />
      <div
        v-if="chartTooltip.visible"
        class="chart-tooltip"
        :style="{ left: chartTooltip.x + 'px' }"
      >{{ chartTooltip.text }}</div>
    </div>
    <div class="threshold-row">
      <span
        class="threshold-chip"
        :class="`source-${tab.slowRequestThresholds.value?.source ?? 'auto'}`"
      >
        {{ tab.slowRequestThresholds.value?.source === 'per_file' ? 'Per-file'
          : tab.slowRequestThresholds.value?.source === 'global' ? 'Global'
          : 'Auto' }}
      </span>
      <span class="threshold-current">
        Fast {{ tab.slowRequestThresholds.value?.effective.fast_ms ?? '-' }}ms,
        Slow {{ tab.slowRequestThresholds.value?.effective.slow_ms ?? '-' }}ms
      </span>
    </div>
    <details class="threshold-editor">
      <summary>Override for this file</summary>
      <div class="threshold-fields">
        <label class="field">
          <span class="field-label">Fast (ms)</span>
          <input
            v-model="fastInput"
            type="number"
            min="0"
            max="600000"
            step="100"
            :placeholder="String(tab.slowRequestThresholds.value?.effective.fast_ms ?? 2000)"
          />
        </label>
        <label class="field">
          <span class="field-label">Slow (ms)</span>
          <input
            v-model="slowInput"
            type="number"
            min="0"
            max="600000"
            step="100"
            :placeholder="String(tab.slowRequestThresholds.value?.effective.slow_ms ?? 10000)"
          />
        </label>
        <button
          type="button"
          class="save-btn"
          :disabled="!!validationError"
          @click="savePerFile"
        >Save</button>
        <button
          type="button"
          class="btn-dismiss clear-override"
          title="Clear override"
          aria-label="Clear override"
          @click="clearPerFile"
        >
          <svg class="dismiss-glyph" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
            <path d="M4 4 L12 12 M12 4 L4 12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" fill="none" />
          </svg>
        </button>
      </div>
      <div v-if="validationError" class="threshold-error">{{ validationError }}</div>
    </details>
    <div class="drawer-toolbar">
      <fieldset class="mode-toggle">
        <legend class="sr-only">Path aggregation mode</legend>
        <button
          type="button"
          class="mode-btn"
          :class="{ 'is-on': tab.slowRequestMode.value === 'normalised' }"
          :aria-pressed="tab.slowRequestMode.value === 'normalised'"
          title="Collapse numeric / UUID / long-hex path segments to {id}"
          @click="tab.slowRequestMode.value = 'normalised'"
        >Normalised</button>
        <button
          type="button"
          class="mode-btn"
          :class="{ 'is-on': tab.slowRequestMode.value === 'raw' }"
          :aria-pressed="tab.slowRequestMode.value === 'raw'"
          title="Keep every observed raw path distinct"
          @click="tab.slowRequestMode.value = 'raw'"
        >Raw</button>
      </fieldset>
      <span class="filter-input-wrap">
        <input
          v-model="tab.slowRequestFilter.value"
          type="text"
          class="filter-input"
          placeholder="Filter path..."
          spellcheck="false"
          @keydown.esc.prevent="tab.slowRequestFilter.value = ''"
        />
        <button
          v-if="tab.slowRequestFilter.value.length > 0"
          type="button"
          class="btn-dismiss clear-filter"
          title="Clear filter (Esc)"
          aria-label="Clear filter"
          @click="tab.slowRequestFilter.value = ''"
        >
          <svg class="dismiss-glyph" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
            <path d="M4 4 L12 12 M12 4 L4 12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" fill="none" />
          </svg>
        </button>
      </span>
      <select class="sort-select" :value="tab.slowRequestSort.value.field" @change="onSortChange">
        <option value="total">Total time</option>
        <option value="count">Count</option>
        <option value="max">Max</option>
        <option value="p95">p95</option>
        <option value="avg">Avg</option>
        <option value="path">Path</option>
      </select>
      <span class="sort-dir">{{ tab.slowRequestSort.value.dir === 'desc' ? 'desc' : 'asc' }}</span>
    </div>
    <div ref="bodyEl" class="drawer-body">
      <div v-if="error" class="drawer-error">
        {{ error }}
        <button type="button" @click="refresh">Retry</button>
      </div>

      <div v-else-if="loading && !tab.slowRequestSummary.value" class="drawer-loading">
        Loading...
      </div>

      <div v-else-if="filteredEntries.length === 0" class="drawer-empty">
        No slow requests match the current filter.
      </div>

      <div v-else class="entry-list-host" :style="{ height: totalSize + 'px' }">
        <div
          v-for="vRow in virtualRows"
          :key="String(vRow.key)"
          :ref="(el) => measureVirtualRow(el as Element | null)"
          :data-index="vRow.index"
          class="entry"
          :style="{ transform: `translateY(${vRow.start}px)` }"
        >
          <template v-if="filteredEntries[vRow.index]">
            <div
              class="entry-row"
              :style="{ '--score': slownessScores.get(filteredEntries[vRow.index].path) ?? 0 }"
              @click="toggleExpandedVirtual(filteredEntries[vRow.index].path)"
            >
              <span class="entry-path" :title="filteredEntries[vRow.index].path" @click.stop="jumpTo(filteredEntries[vRow.index].longest_line)">
                {{ filteredEntries[vRow.index].path }}
              </span>
              <span class="entry-stats" :title="entryStatTitle(filteredEntries[vRow.index])">
                {{ filteredEntries[vRow.index].count }} hits<template v-if="sortStatLabel(filteredEntries[vRow.index])"> . {{ sortStatLabel(filteredEntries[vRow.index]) }}</template>
              </span>
              <span class="entry-expand" :class="{ open: expanded.has(filteredEntries[vRow.index].path) }" aria-hidden="true">
                <svg viewBox="0 0 16 16" focusable="false">
                  <path d="M3 8 H13" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
                  <path class="vbar" d="M8 3 V13" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
                </svg>
              </span>
            </div>
            <ul v-if="expanded.has(filteredEntries[vRow.index].path)" class="occurrence-list">
              <li
                v-for="occ in filteredEntries[vRow.index].occurrences"
                :key="`${filteredEntries[vRow.index].path}-${occ.line_index}`"
                class="occurrence"
                @click="jumpTo(occ.line_index)"
              >
                <span class="occ-ts">
                  {{ occ.timestamp_ms !== null ? new Date(occ.timestamp_ms).toISOString().slice(0, 23).replace('T', ' ') : 'no ts' }}
                </span>
                <span class="occ-dur">{{ formatMs(occ.duration_ms) }}</span>
                <span class="occ-line">line {{ occ.line_index + 1 }}</span>
                <span v-if="occ.dup_count > 1" class="occ-dup">x{{ occ.dup_count }}</span>
              </li>
            </ul>
          </template>
        </div>
      </div>
    </div>
  </aside>
</template>

<style scoped>
.insights-drawer {
  flex: 0 0 auto;
  display: flex;
  flex-direction: column;
  background: var(--bg-elevated);
  border-left: 1px solid var(--border-default);
  min-height: 0;
  position: relative;
}

.resize-handle {
  position: absolute;
  top: 0;
  left: -3px;
  width: 6px;
  height: 100%;
  cursor: ew-resize;
  z-index: 2;
  background: transparent;
  transition: background 120ms;
  touch-action: none;

  &:hover, &:active {
    background: var(--accent);
    opacity: 0.4;
  }
}

.drawer-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0.4rem 0.6rem;
  border-bottom: 1px solid var(--border-default);

  & .title {
    font-weight: 600;
  }

}

.drawer-totals {
  padding: 0.4rem 0.6rem;
  color: var(--fg-muted);
  font-size: 0.85rem;
}

.drawer-body {
  flex: 1 1 auto;
  overflow-y: auto;
  padding: 0 0.6rem 0.6rem;
}

.drawer-toolbar {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  padding: 0.4rem 0.6rem;
  border-bottom: 1px solid var(--border-default);
}

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

.mode-toggle {
  display: inline-flex;
  align-items: center;
  border: none;
  padding: 0;
  margin: 0;

  & .mode-btn {
    background: var(--bg-button);
    color: var(--fg-muted);
    border: 1px solid var(--border-button);
    padding: 0.25rem 0.7rem;
    font-size: 0.8rem;
    font-family: var(--font-mono);
    cursor: pointer;

    &:first-of-type {
      border-radius: var(--radius-sm) 0 0 var(--radius-sm);
      border-right-width: 0;
    }
    &:last-of-type {
      border-radius: 0 var(--radius-sm) var(--radius-sm) 0;
    }

    &:hover:not(.is-on) { background: var(--bg-button-hover); }

    &.is-on {
      background: var(--accent);
      color: var(--fg-on-accent);
      border-color: var(--accent);
      font-weight: 600;
    }
  }
}

.filter-input-wrap {
  flex: 1 1 auto;
  position: relative;
  min-width: 60px;
  display: flex;
}

.filter-input {
  flex: 1 1 auto;
  background: var(--bg-viewport);
  border: 1px solid var(--border-button);
  color: var(--fg-default);
  padding: 0.15rem 1.6rem 0.15rem 0.4rem;
  border-radius: 3px;
  min-width: 0;
}

.clear-filter {
  position: absolute;
  top: 50%;
  right: 0.25rem;
  transform: translateY(-50%);
  width: 1.2rem;
  height: 1.2rem;
  padding: 0;
  border: 0;
  background: transparent;
  border-radius: 50%;
}

.sort-select {
  background: var(--bg-viewport);
  border: 1px solid var(--border-button);
  color: var(--fg-default);
  padding: 0.15rem 0.3rem;
  border-radius: 3px;
}

.sort-dir { color: var(--fg-muted); font-size: 0.8rem; }

.entry-list-host {
  position: relative;
  width: 100%;
}

.entry {
  position: absolute;
  top: 0;
  left: 0;
  width: 100%;
  border-bottom: 1px solid var(--border-default);

  & .entry-row {
    --score: 0;
    position: relative;
    display: grid;
    grid-template-columns: 1fr auto 16px;
    gap: 0.4rem;
    padding: 0.4rem 0.4rem 0.4rem 0;

    /* Slowness bar: scaleX from 0..1 driven by the per-row --score var.
       Painted behind the row contents (z-index -1 against the relative
       parent) with a faint warm tint that picks up the speed palette so
       the drawer reads consistently with the speed rail. */
    &::before {
      content: '';
      position: absolute;
      inset: 0;
      background: linear-gradient(
        to right,
        transparent,
        color-mix(in srgb, var(--speed-mid) 22%, transparent)
      );
      opacity: 0.5;
      transform: scaleX(var(--score));
      transform-origin: left center;
      z-index: 0;
      pointer-events: none;
      transition: transform 180ms ease;
    }

    & > * { position: relative; z-index: 1; }
    align-items: center;
    cursor: pointer;
  }

  & .entry-path {
    color: var(--accent);
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  & .entry-stats { color: var(--fg-muted); font-size: 0.8rem; white-space: nowrap; }
  & .entry-expand {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    color: var(--fg-muted);
    border: 1px solid var(--border-button);
    border-radius: 3px;
    background: var(--bg-viewport);

    & svg {
      width: 12px;
      height: 12px;
      display: block;
    }

    & .vbar {
      transform-origin: center;
      transition: transform 140ms ease;
    }

    &.open {
      color: var(--accent);
      border-color: var(--accent);

      & .vbar { transform: scaleY(0); }
    }
  }
}

.occurrence-list {
  list-style: none;
  margin: 0 0 0.4rem 0;
  padding: 0;
}

.occurrence {
  display: grid;
  grid-template-columns: 11rem auto 1fr auto;
  gap: 0.4rem;
  padding: 0.15rem 0.4rem;
  cursor: pointer;
  font-size: 0.8rem;
  color: var(--fg-muted);

  &:hover { background: var(--bg-button-hover); color: var(--fg-default); }

  & .occ-dur { color: var(--fg-default); font-weight: 600; }
  & .occ-dup {
    color: var(--accent);
    font-size: 0.7rem;
    letter-spacing: 0.05em;
  }
}

.drawer-error, .drawer-loading, .drawer-empty {
  padding: 0.6rem;
  color: var(--fg-muted);
}

.drawer-error {
  color: var(--level-error);

  & button {
    margin-left: 0.6rem;
    background: transparent;
    border: 1px solid var(--border-button);
    color: var(--fg-default);
    padding: 0.1rem 0.4rem;
    border-radius: 3px;
    cursor: pointer;
  }
}

.chart-host {
  position: relative;
  flex: 0 0 auto;
  height: 67px;
  margin: 0 0.6rem 0.4rem;
  background: var(--bg-viewport);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  overflow: hidden;
}

.chart-canvas {
  display: block;
  width: 100%;
  height: 100%;
  cursor: crosshair;
}

.chart-crosshair-v,
.chart-crosshair-h {
  position: absolute;
  pointer-events: none;
  background: var(--fg-default);
  opacity: 0.25;
  z-index: 2;
}

.chart-crosshair-v {
  top: 0;
  bottom: 0;
  width: 1px;
}

.chart-crosshair-h {
  left: 0;
  right: 0;
  height: 1px;
}

.chart-tooltip {
  position: absolute;
  bottom: calc(100% + 4px);
  transform: translateX(-50%);
  background: var(--bg-elevated);
  border: 1px solid var(--border-default);
  color: var(--fg-default);
  font-size: 0.75rem;
  padding: 0.15rem 0.4rem;
  border-radius: 3px;
  white-space: nowrap;
  pointer-events: none;
  z-index: 3;
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.3);
}

.threshold-row {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0 0.6rem 0.4rem;
}

.threshold-chip {
  font-size: 0.7rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  padding: 0.1rem 0.4rem;
  border-radius: 3px;
  border: 1px solid var(--border-button);

  &.source-auto    { color: var(--level-info);  }
  &.source-global  { color: var(--level-warn);  }
  &.source-per_file { color: var(--accent); border-color: var(--accent); }
}

.threshold-current { color: var(--fg-muted); font-size: 0.8rem; }

.threshold-editor {
  margin: 0 0.6rem 0.6rem;
  font-size: 0.8rem;

  & summary { cursor: pointer; color: var(--fg-muted); }
}

.threshold-fields {
  display: flex;
  align-items: flex-end;
  gap: 0.4rem;
  margin: 0.4rem 0 0.2rem;

  & .field {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
  }

  & .field-label {
    font-size: 0.7rem;
    color: var(--fg-muted);
  }

  & input {
    width: 5rem;
    background: var(--bg-viewport);
    border: 1px solid var(--border-button);
    color: var(--fg-default);
    padding: 0.1rem 0.3rem;
    border-radius: 3px;
  }

  & .save-btn {
    background: transparent;
    border: 1px solid var(--border-button);
    color: var(--fg-default);
    padding: 0.15rem 0.6rem;
    border-radius: 3px;
    cursor: pointer;

    &:disabled { opacity: 0.4; cursor: not-allowed; }
  }

  & .clear-override {
    width: 1.6rem;
    height: 1.6rem;
    padding: 0;
  }
}

.threshold-error { color: var(--level-error); margin: 0.2rem 0; font-size: 0.75rem; }
</style>
