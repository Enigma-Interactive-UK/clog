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

import { ref, shallowRef } from 'vue'
import { Channel, invoke } from '@tauri-apps/api/core'
import {
  FULL_THREAD_GROUP_MASK,
  LEVEL_BIT,
  LEVEL_KEYS,
  PAGE_SIZE,
  THREAD_GROUP_BIT,
  THREAD_GROUP_KEYS,
  type ApplyPatternPayload,
  type EffectiveThresholds,
  type HitRef,
  type IpcError,
  type LineRow,
  type LinesPayload,
  type LineWindow,
  type LineWindowPayload,
  type LevelKey,
  type OpenedFile,
  type PatternTestPayload,
  type CollapseMode,
  type RecordRef,
  type RecordRefsPayload,
  type RestoredFile,
  type SearchDelta,
  type SearchMode,
  type SetTruncatePayload,
  type SlowRequestPathMode,
  type SlowRequestSummary,
  type TailDelta,
  type ThreadGroupKey,
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

export function buildThreadGroupMaskFromAllow(allow: Record<string, boolean>): number {
  let mask = 0
  for (const [k, v] of Object.entries(allow)) {
    if (v) mask |= THREAD_GROUP_BIT[k as ThreadGroupKey] ?? 0
  }
  return mask
}

export function isFullThreadGroupMask(allow: Record<string, boolean>): boolean {
  for (const k of THREAD_GROUP_KEYS) {
    if (!allow[k]) return false
  }
  return true
}

export function defaultThreadGroupAllow(): Record<ThreadGroupKey, boolean> {
  return {
    requests: true,
    jobs: true,
    scheduler: true,
    system: true,
    infra: true,
    other: true,
  }
}

export function applyThreadGroupMaskToAllow(mask: number): Record<ThreadGroupKey, boolean> {
  const allow = defaultThreadGroupAllow()
  for (const k of THREAD_GROUP_KEYS) {
    allow[k] = (mask & THREAD_GROUP_BIT[k]) !== 0
  }
  return allow
}

// Single definition of the truncate-window out-of-range pruning rule, shared
// by snapshot, restore and the test so the three sites cannot drift apart. A
// before-cut keeps lines >= it, so it must be a valid line index (0..lineCount);
// an after-cut keeps lines < it, so it may sit exactly at lineCount but must be
// strictly positive (a cut at 0 hides everything and is meaningless).
export function pruneTruncateBefore(value: number | null, lineCount: number): number | null {
  return value !== null && value >= 0 && value < lineCount ? value : null
}

export function pruneTruncateAfter(value: number | null, lineCount: number): number | null {
  return value !== null && value > 0 && value <= lineCount ? value : null
}

export function createTab(localId: number, opened: OpenedFile, defaults: TabDefaults, hooks: TabHooks = {}) {
  // --- File handle + page cache ---
  const file = ref<OpenedFile>(opened)
  const pages = ref(new Map<number, LineRow[]>())
  const inflight = new Map<number, number>()
  let nextGen = 0

  // --- Windowed slices of monster lines ---
  // A search match can sit past the per-line transport cap of a multi-MB
  // line. For such lines we fetch a small slice centred on the match
  // (`get_line_window`) and the renderer draws it with ellipsis markers, so
  // every hit stays visible inline without dragging the whole line over IPC.
  // Keyed by physical line index. Cleared whenever the hit set changes.
  const lineWindows = ref(new Map<number, LineWindow>())
  const windowInflight = new Set<number>()

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
  const threadGroupAllow = ref<Record<ThreadGroupKey, boolean>>(defaultThreadGroupAllow())
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

  // --- Slow request insights drawer ---
  const insightsOpen = ref<boolean>(false)
  const slowRequestMode = ref<SlowRequestPathMode>('normalised')
  const slowRequestSort = ref<{
    field: 'total' | 'count' | 'max' | 'p95' | 'avg' | 'path'
    dir: 'asc' | 'desc'
  }>({ field: 'total', dir: 'desc' })
  const slowRequestFilter = ref<string>('')
  const slowRequestSummary = shallowRef<SlowRequestSummary | null>(null)
  const slowRequestThresholds = ref<EffectiveThresholds | null>(null)

  // --- Bookmarks (physical line indices) ---
  // Sorted-on-write via snapshot(); kept as a Set for O(1) toggle/lookup.
  const bookmarks = ref<Set<number>>(new Set<number>())

  // --- Collapse records ---
  // Per-file mode. 'inherit' follows settings.collapse_records_default.
  const collapseMode = ref<CollapseMode>('inherit')
  // Header-row physical line indices the user forced open / closed against
  // the mode. Persisted. Kept as Sets for O(1) toggle/lookup; sorted+deduped
  // on snapshot().
  const manuallyExpanded = ref<Set<number>>(new Set<number>())
  const manuallyCollapsed = ref<Set<number>>(new Set<number>())
  // Header-row line indices auto-expanded by intent navigation. In-memory
  // only - navigation crumbs, not preferences.
  const transientlyExpanded = ref<Set<number>>(new Set<number>())

  // --- Record map (collapse + minimap need every record's span/level) ---
  // The full (first_line, line_count, level) list, fetched with full masks.
  // Refreshed on open, tail growth, rotation and pattern apply.
  const recordIndex = ref<RecordRef[]>([])

  // --- Truncate window (physical line bounds; record-boundary snapped) ---
  // Persisted per file. null = no cut on that side.
  const truncateBefore = ref<number | null>(null)
  const truncateAfter = ref<number | null>(null)

  function isBookmarked(lineIdx: number): boolean {
    return bookmarks.value.has(lineIdx)
  }

  function toggleBookmark(lineIdx: number) {
    if (lineIdx < 0 || lineIdx >= file.value.line_count) return
    const next = new Set(bookmarks.value)
    if (next.has(lineIdx)) next.delete(lineIdx)
    else next.add(lineIdx)
    bookmarks.value = next
  }

  function removeBookmark(lineIdx: number) {
    if (!bookmarks.value.has(lineIdx)) return
    const next = new Set(bookmarks.value)
    next.delete(lineIdx)
    bookmarks.value = next
  }

  function clearBookmarks() {
    if (bookmarks.value.size === 0) return
    bookmarks.value = new Set()
  }

  // Drop bookmarks that point past the current line_count (file shrank,
  // rotated, or restored against a smaller file). Sorted ascending.
  function prunedBookmarks(): number[] {
    const out: number[] = []
    const limit = file.value.line_count
    for (const idx of bookmarks.value) {
      if (idx >= 0 && idx < limit) out.push(idx)
    }
    out.sort((a, b) => a - b)
    return out
  }

  // Drop manual-set entries pointing past the current line_count (file
  // shrank, rotated, or restored against a smaller file). Sorted ascending.
  function prunedManualSet(set: Set<number>): number[] {
    const out: number[] = []
    const limit = file.value.line_count
    for (const idx of set) {
      if (idx >= 0 && idx < limit) out.push(idx)
    }
    out.sort((a, b) => a - b)
    return out
  }

  // Clear all collapse override state. Called on rotation and on mode change.
  function clearCollapseOverrides() {
    if (manuallyExpanded.value.size > 0) manuallyExpanded.value = new Set()
    if (manuallyCollapsed.value.size > 0) manuallyCollapsed.value = new Set()
    if (transientlyExpanded.value.size > 0) transientlyExpanded.value = new Set()
  }

  // Set the per-file mode. The mode is the new rule, so sticky overrides from
  // the previous regime are cleared (design spec: "Mode reset on per-file
  // mode change").
  function setCollapseMode(mode: CollapseMode) {
    collapseMode.value = mode
    clearCollapseOverrides()
  }

  // Fetch the full record map with full masks so collapse has every record's
  // span + level regardless of the active filter. Non-fatal on error.
  async function refreshRecordIndex(): Promise<void> {
    try {
      const payload = await invoke<RecordRefsPayload>('list_records_by_filters', {
        fileId: file.value.file_id,
        levelMask: 0xffffffff,
        threadGroupMask: 0x3f,
      })
      recordIndex.value = payload.refs
    } catch {
      // non-fatal -- keep the previous map; collapse falls back to identity
    }
  }

  // --- Truncate ------------------------------------------------------------

  // Push a new truncate window to the backend, then refresh the windowed
  // views. Setting an "after" cut disengages follow-tail (lines below the cut
  // are not visible, so following them is meaningless). Non-fatal on error.
  async function setTruncate(before: number | null, after: number | null): Promise<void> {
    try {
      const payload = await invoke<SetTruncatePayload>('set_truncate', {
        fileId: file.value.file_id,
        before,
        after,
      })
      truncateBefore.value = payload.before
      truncateAfter.value = payload.after
      if (after !== null) followTail.value = false
      await refreshRecordIndex()
      if (!isFullLevelMask(levelAllow.value) || !isFullThreadGroupMask(threadGroupAllow.value)) {
        void refreshAllowedRecords()
      }
      if (searchQuery.value.trim().length > 0) scheduleSearch()
    } catch (e) {
      const err = e as IpcError | string
      hooks.onError?.(typeof err === 'string' ? err : err.message)
    }
  }

  // Clear both cuts locally and on the backend, without the refresh cascade
  // (callers that already re-index, e.g. rotation/pattern-apply, use this).
  function resetTruncateState() {
    truncateBefore.value = null
    truncateAfter.value = null
    void invoke('set_truncate', {
      fileId: file.value.file_id,
      before: null,
      after: null,
    }).catch(() => {})
  }

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

  // Fetch a slice of physical line `lineIndex` centred on `center` (a
  // line-local char offset), extending `radius` each way, and cache it for the
  // renderer. One window per line (the first hit that needs it); cleared when
  // the hit set changes. Non-fatal on error.
  async function fetchLineWindow(lineIndex: number, center: number, radius: number): Promise<void> {
    if (lineWindows.value.has(lineIndex) || windowInflight.has(lineIndex)) return
    windowInflight.add(lineIndex)
    try {
      const payload = await invoke<LineWindowPayload>('get_line_window', {
        fileId: file.value.file_id,
        lineIndex,
        center,
        radius,
      })
      const next = new Map(lineWindows.value)
      next.set(lineIndex, { text: payload.text, start: payload.start, fullLen: payload.full_len })
      lineWindows.value = next
    } catch {
      // non-fatal -- the line just shows its head with the truncation marker
    } finally {
      windowInflight.delete(lineIndex)
    }
  }

  function clearLineWindows() {
    windowInflight.clear()
    if (lineWindows.value.size > 0) lineWindows.value = new Map()
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
      clearLineWindows()
      file.value = {
        ...file.value,
        line_count: delta.line_count,
        record_count: delta.record_count,
        size_bytes: delta.last_offset,
      }
      lastTailLineCount = delta.line_count
      // Line numbers no longer mean what they meant before rotation;
      // existing bookmarks would point at unrelated content. Drop them
      // silently per the feature spec.
      clearBookmarks()
      clearCollapseOverrides()
      resetTruncateState()
      void refreshRecordIndex()
      showRotationToast()
      void fetchPage(0)
      if (!isFullLevelMask(levelAllow.value) || !isFullThreadGroupMask(threadGroupAllow.value)) void refreshAllowedRecords()
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
    void refreshRecordIndex()
    if (!isFullLevelMask(levelAllow.value) || !isFullThreadGroupMask(threadGroupAllow.value)) void refreshAllowedRecords()
    if (searchQuery.value.trim().length > 0) scheduleSearch()
    hooks.onTailAppend?.(api, delta)
    unread.value = true
  }

  function syncLastTailLineCount() {
    lastTailLineCount = file.value.line_count
  }

  // --- Search --------------------------------------------------------------

  // 50ms barely debounced anything - a fast typist fires a search per
  // keystroke. 200ms lets a normal typing burst settle before the
  // backend is asked to run, but still feels live when you pause.
  const SEARCH_DEBOUNCE_MS = 200

  function scheduleSearch() {
    if (pendingSearchTimer !== null) globalThis.clearTimeout(pendingSearchTimer)
    pendingSearchTimer = globalThis.setTimeout(() => {
      pendingSearchTimer = null
      void runSearch()
    }, SEARCH_DEBOUNCE_MS)
  }

  async function runSearch(): Promise<void> {
    const fileId = file.value.file_id
    const query = searchQuery.value
    const mask = buildLevelMaskFromAllow(levelAllow.value)
    const tgMask = buildThreadGroupMaskFromAllow(threadGroupAllow.value)
    const myGen = ++runSearchGen
    // Hit positions are about to change; drop windows centred on the old ones.
    clearLineWindows()
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
          thread_group_mask: tgMask,
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
    // Highlights are gone -> windowed monster lines revert to head truncation.
    clearLineWindows()
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

  function toggleThreadGroup(group: ThreadGroupKey) {
    threadGroupAllow.value = { ...threadGroupAllow.value, [group]: !threadGroupAllow.value[group] }
    void refreshAllowedRecords()
    if (searchQuery.value.trim().length > 0) scheduleSearch()
  }

  async function refreshAllowedRecords(): Promise<void> {
    if (isFullLevelMask(levelAllow.value) && isFullThreadGroupMask(threadGroupAllow.value)) {
      allowedRecords.value = null
      return
    }
    try {
      const payload = await invoke<RecordRefsPayload>('list_records_by_filters', {
        fileId: file.value.file_id,
        levelMask: buildLevelMaskFromAllow(levelAllow.value),
        threadGroupMask: buildThreadGroupMaskFromAllow(threadGroupAllow.value),
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
      clearLineWindows()
      void fetchPage(0)
      void refreshRecordIndex()
      resetTruncateState()
    } catch (e) {
      const err = e as IpcError | string
      patternError.value = typeof err === 'string' ? err : err.message
    }
  }

  // --- Restore + capture ---------------------------------------------------

  function applyRestored(r: RestoredFile) {
    levelAllow.value = applyMaskToAllow(r.level_mask)
    threadGroupAllow.value = applyThreadGroupMaskToAllow(r.thread_group_mask ?? FULL_THREAD_GROUP_MASK)
    searchMode.value = r.search_mode === 'regex' ? 'regex' : 'smart'
    searchCaseSensitive.value = !!r.search_case_sensitive
    filterMode.value = !!r.filter_mode
    searchQuery.value = r.filter_text ?? ''
    followTail.value = !!r.follow_tail
    scrollTop.value = r.scroll_top
    if (Array.isArray(r.bookmarks) && r.bookmarks.length > 0) {
      const limit = file.value.line_count
      const next = new Set<number>()
      for (const idx of r.bookmarks) {
        if (Number.isFinite(idx) && idx >= 0 && idx < limit) next.add(idx)
      }
      bookmarks.value = next
    } else {
      bookmarks.value = new Set()
    }
    collapseMode.value = r.collapse_mode ?? 'inherit'
    const limit = file.value.line_count
    const pruneIn = (arr: number[] | undefined): Set<number> => {
      const next = new Set<number>()
      if (Array.isArray(arr)) {
        for (const idx of arr) {
          if (Number.isFinite(idx) && idx >= 0 && idx < limit) next.add(idx)
        }
      }
      return next
    }
    manuallyExpanded.value = pruneIn(r.manually_expanded)
    manuallyCollapsed.value = pruneIn(r.manually_collapsed)
    transientlyExpanded.value = new Set() // never restored
    const tb = r.truncate_before
    const ta = r.truncate_after
    truncateBefore.value = pruneTruncateBefore(typeof tb === 'number' ? tb : null, limit)
    truncateAfter.value = pruneTruncateAfter(typeof ta === 'number' ? ta : null, limit)
    if (truncateBefore.value !== null || truncateAfter.value !== null) {
      void invoke('set_truncate', {
        fileId: file.value.file_id,
        before: truncateBefore.value,
        after: truncateAfter.value,
      })
        .then(() => refreshRecordIndex())
        .catch(() => {})
    }
  }

  function snapshot(): RestoredFile {
    return {
      path: file.value.path,
      scroll_top: scrollTop.value,
      follow_tail: followTail.value,
      level_mask: buildLevelMaskFromAllow(levelAllow.value),
      thread_group_mask: buildThreadGroupMaskFromAllow(threadGroupAllow.value),
      filter_text: searchQuery.value,
      search_mode: searchMode.value,
      search_case_sensitive: searchCaseSensitive.value,
      filter_mode: filterMode.value,
      bookmarks: prunedBookmarks(),
      collapse_mode: collapseMode.value,
      manually_expanded: prunedManualSet(manuallyExpanded.value),
      manually_collapsed: prunedManualSet(manuallyCollapsed.value),
      truncate_before: pruneTruncateBefore(truncateBefore.value, file.value.line_count),
      truncate_after: pruneTruncateAfter(truncateAfter.value, file.value.line_count),
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
    threadGroupAllow,
    tailing,
    followTail,
    tailPulse,
    rotationToast,
    scrollTop,
    unread,
    bookmarks,
    collapseMode,
    manuallyExpanded,
    manuallyCollapsed,
    transientlyExpanded,
    recordIndex,
    truncateBefore,
    truncateAfter,
    insightsOpen,
    slowRequestMode,
    slowRequestSort,
    slowRequestFilter,
    slowRequestSummary,
    slowRequestThresholds,
    // methods
    isBookmarked,
    toggleBookmark,
    removeBookmark,
    clearBookmarks,
    lineRow,
    fetchPage,
    lineWindows,
    fetchLineWindow,
    clearLineWindows,
    startTail,
    syncLastTailLineCount,
    scheduleSearch,
    runSearch,
    clearSearchState,
    setSearchMode,
    nextHitIdx,
    prevHitIdx,
    toggleLevel,
    toggleThreadGroup,
    refreshAllowedRecords,
    setCollapseMode,
    clearCollapseOverrides,
    refreshRecordIndex,
    setTruncate,
    resetTruncateState,
    testPattern,
    applyPattern,
    applyRestored,
    snapshot,
    teardown,
  }
  return api
}
