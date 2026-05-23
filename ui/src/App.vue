<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, useTemplateRef, watch } from 'vue'
import { Channel, invoke } from '@tauri-apps/api/core'
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
}

interface LineRow {
  record_idx: number
  line_within_record: number
  level: string
  fields: HeaderFields | null
  text: string
}

interface LinesPayload {
  start_line: number
  lines: LineRow[]
}

interface PatternTestPayload {
  score: number
  sample_size: number
}

interface ApplyPatternPayload {
  record_count: number
  pattern_source: string
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

// Tail state.
const tailing = ref(false)
const followTail = ref(true)
const tailPulse = ref(false)
const rotationToast = ref<string | null>(null)
let tailPulseTimer: number | null = null
let rotationToastTimer: number | null = null
let lastTailLineCount = 0

const scrollEl = useTemplateRef<HTMLDivElement>('scrollEl')
const minimapEl = useTemplateRef<HTMLCanvasElement>('minimapEl')

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

const virtualizer = useVirtualizer(
  computed(() => ({
    count: file.value?.line_count ?? 0,
    getScrollElement: () => scrollEl.value ?? null,
    estimateSize: () => ROW_HEIGHT,
    overscan: OVERSCAN,
  })),
)

const virtualRows = computed(() => virtualizer.value.getVirtualItems())
const totalSize = computed(() => virtualizer.value.getTotalSize())

function basename(p: string): string {
  const m = p.match(/[^\\/]+$/)
  return m ? m[0] : p
}

function formatCount(n: number): string {
  return n.toLocaleString('en-GB')
}

function lineRow(index: number): LineRow | null {
  const pageIdx = Math.floor(index / PAGE_SIZE)
  const page = pages.value.get(pageIdx)
  if (!page) return null
  return page[index % PAGE_SIZE] ?? null
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
  for (const r of rows) wanted.add(Math.floor(r.index / PAGE_SIZE))
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
  const total = file.value.line_count
  if (total === 0) return null
  const topIdx = Math.min(total - 1, Math.floor(viewportScrollTop.value / ROW_HEIGHT))
  const data = lineRow(topIdx)
  if (!data) return null
  if (data.line_within_record === 0) return null
  // Walk backward to the header row (line_within_record == 0) of the
  // same record. Bounded by the record's first line, so this is cheap.
  for (let i = topIdx - 1; i >= 0; i--) {
    const candidate = lineRow(i)
    if (!candidate) {
      // Force-fetch the page so the header becomes available next frame.
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
  // Set scrollTop directly so the resulting position is exactly the
  // record's header row. `scrollToIndex` route went via the virtualizer
  // and consistently overshot by one row -- likely a sub-pixel offset
  // that my row-snap handler then rounded forward.
  el.scrollTop = sticky.lineIndex * ROW_HEIGHT
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

  // The previously-cached final page (which may have been partial because
  // line_count grew past its end) needs a force-refetch to pick up the new
  // entries. Force is used so the in-place swap keeps the visible rows
  // rendered with the old data until the new payload arrives -- no blank
  // flash during the round trip.
  if (lastTailLineCount > 0) {
    const lastPage = Math.floor((lastTailLineCount - 1) / PAGE_SIZE)
    fetchPage(lastPage, true)
  }
  lastTailLineCount = delta.line_count
  scheduleMinimapFetch()

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
// Viewport background -- matches --bg-viewport (slate-950) so the canvas
// composites correctly without having to read it from computed styles each
// repaint.
const MINIMAP_BG = '#0f131a'

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
  // When the file is shorter than the viewport, cap buckets at the actual
  // content pixel height (one bucket per row) so the minimap aligns to the
  // top instead of stretching a handful of records across the full column.
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
  ctx.fillStyle = MINIMAP_BG
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
  if (!canvas || !file.value || file.value.line_count === 0) return null
  const rect = canvas.getBoundingClientRect()
  if (rect.height <= 0) return null
  const ratio = Math.max(0, Math.min(1, (clientY - rect.top) / rect.height))
  const idx = Math.min(file.value.line_count - 1, Math.floor(ratio * file.value.line_count))
  return idx
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
  if (!file.value || file.value.line_count === 0) return
  // Defer to the next frame so any virtualizer resize from the count bump
  // has settled before we ask for a scroll target.
  requestAnimationFrame(() => {
    if (!file.value || file.value.line_count === 0) return
    virtualizer.value.scrollToIndex(file.value.line_count - 1, { align: 'end' })
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

onMounted(() => {
  globalThis.addEventListener('keydown', suppressBrowserFind, { capture: true })
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
  globalThis.removeEventListener('keydown', suppressBrowserFind, { capture: true })
  if (resizeObserver) {
    resizeObserver.disconnect()
    resizeObserver = null
  }
  if (tailPulseTimer !== null) globalThis.clearTimeout(tailPulseTimer)
  if (rotationToastTimer !== null) globalThis.clearTimeout(rotationToastTimer)
  if (file.value) {
    const id = file.value.file_id
    invoke('stop_tail', { fileId: id }).catch(() => {})
    invoke('close_file', { fileId: id }).catch(() => {})
  }
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
function renderLine(row: LineRow): LeafSpan[] {
  if (row.fields) {
    const base = headerBaseSpans(row.text, row.fields)
    const axis2 = highlightsFor(row.text)
    const leaves = overlay(row.text, base, axis2)
    return decorateLevels(leaves, row.level)
  }
  const base = [{ start: 0, end: row.text.length, cls: 'message' }]
  const axis2 = highlightsFor(row.text)
  return overlay(row.text, base, axis2)
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
    <header class="bar">
      <h1>Clog</h1>
      <button :disabled="busy" @click="pickFile">
        {{ busy ? 'Reading...' : 'Open file...' }}
      </button>
      <span v-if="file" class="meta">
        <strong>{{ basename(file.path) }}</strong>
        <span class="sep">--</span>
        {{ formatCount(file.record_count) }} records
        <span class="sep">--</span>
        {{ formatCount(file.line_count) }} lines
        <span class="sep">--</span>
        {{ formatCount(file.size_bytes) }} bytes
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
          {{ followTail ? 'Following' : 'Detached' }}
        </button>
        <button
          v-if="!followTail"
          type="button"
          class="jump-bottom"
          title="Jump to bottom and re-enable follow"
          @click="toggleFollowTail"
        >
          Jump to bottom
        </button>
      </span>
    </header>

    <section v-if="file" class="pattern-bar">
      <label class="kind">
        Pattern:
        <select v-model="patternMode">
          <option value="pattern">PatternLayout</option>
          <option value="regex">Regex</option>
        </select>
      </label>
      <input
        v-model="patternInput"
        class="pat-input"
        :placeholder="patternMode === 'pattern'
          ? '[%-5level] %d{yyyy-MM-dd HH:mm:ss.SSS} [%t] %c{1} - %msg%n'
          : '^(?P<timestamp>\\d{4}-...) (?P<level>INFO|WARN|ERROR) ...'"
        spellcheck="false"
      />
      <button type="button" @click="testPattern">Test</button>
      <button type="button" @click="applyPattern">Apply</button>
      <span v-if="patternScore !== null" class="score">
        match score: <strong>{{ (patternScore * 100).toFixed(1) }}%</strong>
        <span v-if="patternSampleSize > 0" class="muted"> of {{ patternSampleSize }} lines</span>
      </span>
      <span v-if="file.pattern_name" class="auto">
        auto-detected: <strong>{{ file.pattern_name }}</strong>
      </span>
      <span v-if="patternError" class="pat-error">{{ patternError }}</span>
    </section>

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
          v-if="lineRow(vrow.index)"
          class="row"
          :class="[
            {
              'is-header': lineRow(vrow.index)?.line_within_record === 0,
              'is-continuation': (lineRow(vrow.index)?.line_within_record ?? 0) > 0,
            },
            'level-row-' + (lineRow(vrow.index)?.level ?? 'unknown'),
          ]"
          :style="{
            transform: `translateY(${vrow.start}px)`,
            height: `${vrow.size}px`,
            '--gutter-color': levelGutterVar(lineRow(vrow.index)?.level ?? 'unknown'),
          }"
        >
          <span class="gutter" />
          <span class="idx">{{ vrow.index + 1 }}</span>
          <span class="txt">
              <span
                v-for="(span, si) in renderLine(lineRow(vrow.index)!)"
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
    </div>
    <p v-else class="placeholder">No file open. Click <em>Open file...</em> to pick one.</p>

    <footer class="status-bar">
      <span class="slot left" />
      <span class="slot right" />
    </footer>
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

    .follow-toggle, .jump-bottom {
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

  .slot.right { margin-left: auto; }
}

.viewport-shell {
  flex: 1 1 auto;
  display: flex;
  flex-direction: row;
  min-height: 0;
  /* The minimap tooltip is positioned absolutely with `right` so it
     extends leftward past the .minimap container; without clipping here
     it pushes the shell horizontally and pulls in a page-level scrollbar.
     The shell already bounds the viewport vertically, so clipping is
     safe -- the tooltip stays well within it. */
  overflow: hidden;
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
      overflow: hidden;
      text-overflow: ellipsis;
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
  }

  /* Zero-height sticky anchor: keeps the sticky header pinned to the
     viewport's top edge without ever displacing `.total`. The visible
     row inside is absolutely positioned relative to this anchor, so
     toggling stickiness on/off costs nothing in layout terms -- no
     flicker between adjacent multi-line records AND no empty band
     above row 1 when the file is scrolled to the very top. */
  .sticky-shell {
    position: sticky;
    top: 1px;
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
