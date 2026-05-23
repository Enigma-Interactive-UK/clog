/**
 * Per-tab state container. A Tab owns every reactive ref that was
 * previously a top-level singleton in App.vue: the open file handle, the
 * page cache, the pattern bar state, the search/filter/level mask state,
 * the tail status, the per-tab scrollTop, and the unread-data indicator
 * that drives the tab strip pulse.
 *
 * Methods that operate purely on tab state live here too. Methods that
 * need DOM access (the virtualizer, the scroll element, the canvas) stay
 * in App.vue and receive the tab as a parameter -- the DOM is a property
 * of the *visible* tab, not the tab itself.
 *
 * Tail and search channel callbacks close over their tab via the factory
 * closure, so deltas always update the originating tab even when it's not
 * the active one. App.vue's reactive bindings re-render the viewport
 * whenever the active tab's state changes.
 */

import { ref } from 'vue'
import { Channel, invoke } from '@tauri-apps/api/core'
import {
  LEVEL_BIT,
  LEVEL_KEYS,
  PAGE_SIZE,
  type ApplyPatternPayload,
  type HitRef,
  type IpcError,
  type LineRow,
  type LinesPayload,
  type LevelKey,
  type OpenedFile,
  type PatternTestPayload,
  type RecordRef,
  type RecordRefsPayload,
  type RestoredFile,
  type SearchDelta,
  type SearchMode,
  type TailDelta,
} from './types'

export interface TabDefaults {
  followTail: boolean
}

export interface TabHooks {
  /** App.vue passes a closure that performs DOM-bound scroll-to-bottom on the
   *  currently-active tab. The tab calls it after the tail delta lands so the
   *  visible viewport stays pinned. Inactive-tab tail deltas are not routed
   *  to this hook (it's a no-op for non-active tabs by construction). */
  onTailAppend?: (tab: Tab, delta: TailDelta) => void
  /** Called after a rotation delta lands so App.vue can drop minimap state. */
  onTailRotate?: (tab: Tab, delta: TailDelta) => void
  /** Surface fatal-but-recoverable errors (failed open, failed search start)
   *  to the global error slot. */
  onError?: (message: string) => void
}

export type Tab = ReturnType<typeof createTab>

const FILTER_MODE_DEFAULT = false

/**
 * Build the level-mask bitmap from the tab's per-level allow flags. Layout
 * must match `clog_core::search::level_bit` so the backend search applies
 * the same narrowing.
 */
export function buildLevelMaskFromAllow(allow: Record<string, boolean>): number {
  let mask = 0
  for (const [k, v] of Object.entries(allow)) {
    if (v) mask |= LEVEL_BIT[k] ?? 0
  }
  return mask
}

export function isFullLevelMask(allow: Record<string, boolean>): boolean {
  for (const lvl of LEVEL_KEYS) {
    if (!allow[lvl]) return false
  }
  return true
}

export function defaultLevelAllow(): Record<string, boolean> {
  return {
    trace: true,
    debug: true,
    info: true,
    warn: true,
    error: true,
    fatal: true,
    off: true,
    all: true,
    unknown: true,
  }
}

export function applyMaskToAllow(mask: number): Record<string, boolean> {
  const allow = defaultLevelAllow()
  for (const lvl of LEVEL_KEYS) {
    allow[lvl] = (mask & LEVEL_BIT[lvl]) !== 0
  }
  return allow
}

export function createTab(localId: number, opened: OpenedFile, defaults: TabDefaults, hooks: TabHooks = {}) {
  // --- File handle + page cache ---
  const file = ref<OpenedFile>(opened)
  const pages = ref(new Map<number, LineRow[]>())
  const inflight = new Map<number, number>()
  let nextGen = 0

  // --- Pattern bar ---
  const patternInput = ref<string>(opened.pattern_source)
  const patternMode = ref<'pattern' | 'regex'>(
    opened.pattern_source.startsWith('regex:') ? 'regex' : 'pattern',
  )
  const patternScore = ref<number | null>(opened.pattern_score)
  const patternSampleSize = ref<number>(0)
  const patternError = ref<string | null>(null)

  // --- Search + filter ---
  const searchMode = ref<SearchMode>('smart')
  const searchQuery = ref('')
  const searchCaseSensitive = ref(false)
  const filterMode = ref(FILTER_MODE_DEFAULT)
  const searchError = ref<string | null>(null)
  const searchInflight = ref(false)
  const hits = ref(new Map<number, HitRef>())
  const hitOrder = ref<number[]>([])
  const currentHit = ref<number>(-1)
  const allowedRecords = ref<RecordRef[] | null>(null)
  const levelAllow = ref<Record<string, boolean>>(defaultLevelAllow())
  let runSearchGen = 0
  let pendingSearchTimer: number | null = null

  // --- Tail ---
  const tailing = ref(false)
  const followTail = ref(defaults.followTail)
  const tailPulse = ref(false)
  const rotationToast = ref<string | null>(null)
  let lastTailLineCount = opened.line_count
  let tailPulseTimer: number | null = null
  let rotationToastTimer: number | null = null

  // --- Scroll persistence + tab-strip unread indicator ---
  const scrollTop = ref(0)
  const unread = ref(false)

  // --- Helpers -------------------------------------------------------------

  function lineRow(index: number): LineRow | null {
    const pageIdx = Math.floor(index / PAGE_SIZE)
    const page = pages.value.get(pageIdx)
    if (!page) return null
    return page[index % PAGE_SIZE] ?? null
  }

  async function fetchPage(pageIdx: number, force = false): Promise<void> {
    if (!force && pages.value.has(pageIdx)) return
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
      if (inflight.get(pageIdx) !== myGen) return
      pages.value.set(pageIdx, payload.lines)
      pages.value = new Map(pages.value)
    } catch (e) {
      const err = e as IpcError | string
      hooks.onError?.(typeof err === 'string' ? err : err.message)
    } finally {
      if (inflight.get(pageIdx) === myGen) inflight.delete(pageIdx)
    }
  }

  // --- Tail ----------------------------------------------------------------

  async function startTail(): Promise<void> {
    const fileId = file.value.file_id
    const channel = new Channel<TailDelta>()
    channel.onmessage = handleTailDelta
    try {
      await invoke('start_tail', { fileId, onDelta: channel })
      tailing.value = true
    } catch (e) {
      const err = e as IpcError | string
      hooks.onError?.(typeof err === 'string' ? err : err.message)
      tailing.value = false
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

  function handleTailDelta(delta: TailDelta) {
    tailPulse.value = true
    if (tailPulseTimer !== null) globalThis.clearTimeout(tailPulseTimer)
    tailPulseTimer = globalThis.setTimeout(() => {
      tailPulse.value = false
      tailPulseTimer = null
    }, 250)

    if (delta.rotated) {
      pages.value = new Map()
      file.value = {
        ...file.value,
        line_count: delta.line_count,
        record_count: delta.record_count,
        size_bytes: delta.last_offset,
      }
      lastTailLineCount = delta.line_count
      showRotationToast()
      void fetchPage(0)
      if (!isFullLevelMask(levelAllow.value)) void refreshAllowedRecords()
      if (searchQuery.value.trim().length > 0) scheduleSearch()
      hooks.onTailRotate?.(api, delta)
      unread.value = true
      return
    }

    file.value = {
      ...file.value,
      line_count: delta.line_count,
      record_count: delta.record_count,
      size_bytes: delta.last_offset,
    }

    if (lastTailLineCount > 0 && delta.line_count > lastTailLineCount) {
      const oldLastPage = Math.floor((lastTailLineCount - 1) / PAGE_SIZE)
      const newLastPage = Math.floor((delta.line_count - 1) / PAGE_SIZE)
      for (let p = oldLastPage; p <= newLastPage; p++) {
        void fetchPage(p, true)
      }
    }
    lastTailLineCount = delta.line_count
    if (!isFullLevelMask(levelAllow.value)) void refreshAllowedRecords()
    if (searchQuery.value.trim().length > 0) scheduleSearch()
    hooks.onTailAppend?.(api, delta)
    unread.value = true
  }

  function syncLastTailLineCount() {
    lastTailLineCount = file.value.line_count
  }

  // --- Search --------------------------------------------------------------

  function scheduleSearch() {
    if (pendingSearchTimer !== null) globalThis.clearTimeout(pendingSearchTimer)
    pendingSearchTimer = globalThis.setTimeout(() => {
      pendingSearchTimer = null
      void runSearch()
    }, 50)
  }

  async function runSearch(): Promise<void> {
    const fileId = file.value.file_id
    const query = searchQuery.value
    const mask = buildLevelMaskFromAllow(levelAllow.value)
    const myGen = ++runSearchGen
    if (query.trim().length === 0) {
      try {
        await invoke('cancel_search', { fileId })
      } catch {
        // best effort
      }
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
    const local = new Map<number, HitRef>()
    const order: number[] = []
    channel.onmessage = (delta: SearchDelta) => {
      if (myGen !== runSearchGen) return
      for (const h of delta.hits) {
        if (!local.has(h.record_idx)) {
          local.set(h.record_idx, h)
          order.push(h.record_idx)
        }
      }
      hits.value = new Map(local)
      hitOrder.value = order.slice()
      if (delta.done) {
        searchInflight.value = false
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
      if (myGen !== runSearchGen) return
      const err = e as IpcError | string
      const message = typeof err === 'string' ? err : err.message
      searchError.value = message
      searchInflight.value = false
      hits.value = new Map()
      hitOrder.value = []
    }
  }

  function clearSearchState() {
    if (pendingSearchTimer !== null) {
      globalThis.clearTimeout(pendingSearchTimer)
      pendingSearchTimer = null
    }
    void invoke('cancel_search', { fileId: file.value.file_id }).catch(() => {})
    hits.value = new Map()
    hitOrder.value = []
    currentHit.value = -1
    searchInflight.value = false
  }

  function setSearchMode(mode: SearchMode) {
    if (searchMode.value === mode) return
    searchMode.value = mode
    scheduleSearch()
  }

  function nextHitIdx(): number | null {
    if (hitOrder.value.length === 0) return null
    currentHit.value = (currentHit.value + 1) % hitOrder.value.length
    return currentHit.value
  }

  function prevHitIdx(): number | null {
    if (hitOrder.value.length === 0) return null
    const n = hitOrder.value.length
    currentHit.value = (currentHit.value - 1 + n) % n
    return currentHit.value
  }

  function toggleLevel(level: LevelKey) {
    levelAllow.value = { ...levelAllow.value, [level]: !levelAllow.value[level] }
    void refreshAllowedRecords()
    if (searchQuery.value.trim().length > 0) scheduleSearch()
  }

  async function refreshAllowedRecords(): Promise<void> {
    if (isFullLevelMask(levelAllow.value)) {
      allowedRecords.value = null
      return
    }
    try {
      const payload = await invoke<RecordRefsPayload>('list_records_by_level', {
        fileId: file.value.file_id,
        levelMask: buildLevelMaskFromAllow(levelAllow.value),
      })
      allowedRecords.value = payload.refs
    } catch {
      // non-fatal -- keep previous list
    }
  }

  // --- Pattern -------------------------------------------------------------

  async function testPattern(): Promise<void> {
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

  async function applyPattern(): Promise<void> {
    patternError.value = null
    try {
      const args: Record<string, unknown> = { fileId: file.value.file_id }
      if (patternMode.value === 'pattern') args.pattern = patternInput.value
      else args.regex = patternInput.value
      const payload = await invoke<ApplyPatternPayload>('set_pattern', args)
      file.value = {
        ...file.value,
        record_count: payload.record_count,
        pattern_source: payload.pattern_source,
        pattern_name: null,
        loose: payload.loose,
      }
      pages.value = new Map()
      void fetchPage(0)
    } catch (e) {
      const err = e as IpcError | string
      patternError.value = typeof err === 'string' ? err : err.message
    }
  }

  // --- Restore + capture ---------------------------------------------------

  function applyRestored(r: RestoredFile) {
    levelAllow.value = applyMaskToAllow(r.level_mask)
    searchMode.value = r.search_mode === 'regex' ? 'regex' : 'smart'
    searchCaseSensitive.value = !!r.search_case_sensitive
    filterMode.value = !!r.filter_mode
    searchQuery.value = r.filter_text ?? ''
    followTail.value = !!r.follow_tail
    scrollTop.value = r.scroll_top
  }

  function snapshot(): RestoredFile {
    return {
      path: file.value.path,
      scroll_top: scrollTop.value,
      follow_tail: followTail.value,
      level_mask: buildLevelMaskFromAllow(levelAllow.value),
      filter_text: searchQuery.value,
      search_mode: searchMode.value,
      search_case_sensitive: searchCaseSensitive.value,
      filter_mode: filterMode.value,
    }
  }

  // --- Teardown ------------------------------------------------------------

  async function teardown(): Promise<void> {
    const id = file.value.file_id
    if (pendingSearchTimer !== null) {
      globalThis.clearTimeout(pendingSearchTimer)
      pendingSearchTimer = null
    }
    if (tailPulseTimer !== null) {
      globalThis.clearTimeout(tailPulseTimer)
      tailPulseTimer = null
    }
    if (rotationToastTimer !== null) {
      globalThis.clearTimeout(rotationToastTimer)
      rotationToastTimer = null
    }
    void invoke('cancel_search', { fileId: id }).catch(() => {})
    void invoke('stop_tail', { fileId: id }).catch(() => {})
    void invoke('close_file', { fileId: id }).catch(() => {})
  }

  const api = {
    localId,
    // state
    file,
    pages,
    inflight,
    patternInput,
    patternMode,
    patternScore,
    patternSampleSize,
    patternError,
    searchMode,
    searchQuery,
    searchCaseSensitive,
    filterMode,
    searchError,
    searchInflight,
    hits,
    hitOrder,
    currentHit,
    allowedRecords,
    levelAllow,
    tailing,
    followTail,
    tailPulse,
    rotationToast,
    scrollTop,
    unread,
    // methods
    lineRow,
    fetchPage,
    startTail,
    syncLastTailLineCount,
    scheduleSearch,
    runSearch,
    clearSearchState,
    setSearchMode,
    nextHitIdx,
    prevHitIdx,
    toggleLevel,
    refreshAllowedRecords,
    testPattern,
    applyPattern,
    applyRestored,
    snapshot,
    teardown,
  }
  return api
}
