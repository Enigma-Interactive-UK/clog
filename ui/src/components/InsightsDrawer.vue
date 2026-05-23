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
import type { EffectiveThresholds, SlowRequestEntry, SlowRequestSummary, SlowRequestThresholds } from '../types'

const props = defineProps<{ tab: Tab }>()

const emit = defineEmits<{
  (e: 'close'): void
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
}

const fastInput = ref('')
const slowInput = ref('')

const validationError = computed<string | null>(() => {
  const fast = Number(fastInput.value)
  const slow = Number(slowInput.value)
  if (fastInput.value === '' && slowInput.value === '') return null
  if (fastInput.value === '' || slowInput.value === '') return 'Both fields required'
  if (Number.isNaN(fast) || Number.isNaN(slow)) return 'Numbers only'
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
    fastInput.value = String(payload.per_file?.fast_ms ?? '')
    slowInput.value = String(payload.per_file?.slow_ms ?? '')
  } catch {
    // non-fatal
  }
}

async function savePerFile() {
  if (validationError.value) return
  const fast = Number(fastInput.value)
  const slow = Number(slowInput.value)
  const t: SlowRequestThresholds | null =
    fastInput.value === '' && slowInput.value === ''
      ? null
      : { fast_ms: fast, slow_ms: slow }
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
  fastInput.value = ''
  slowInput.value = ''
  await savePerFile()
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
      <button type="button" class="close-btn" aria-label="Close" @click="emit('close')">
        x
      </button>
    </header>
    <div class="drawer-totals">{{ totals }}</div>
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
        <label>Fast (ms) <input v-model="fastInput" type="number" min="0" max="600000" /></label>
        <label>Slow (ms) <input v-model="slowInput" type="number" min="0" max="600000" /></label>
      </div>
      <div v-if="validationError" class="threshold-error">{{ validationError }}</div>
      <div class="threshold-actions">
        <button type="button" :disabled="!!validationError" @click="savePerFile">Save</button>
        <button type="button" class="muted" @click="clearPerFile">Clear override</button>
      </div>
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
              <span class="entry-stats">
                {{ filteredEntries[vRow.index].count }} hits . total {{ formatMs(filteredEntries[vRow.index].total_ms) }} .
                avg {{ formatMs(filteredEntries[vRow.index].avg_ms) }} . p95 {{ formatMs(filteredEntries[vRow.index].p95_ms) }} .
                max {{ formatMs(filteredEntries[vRow.index].max_ms) }}
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

  & .close-btn {
    background: transparent;
    border: 1px solid transparent;
    color: var(--fg-default);
    cursor: pointer;
    padding: 0.1rem 0.4rem;
    border-radius: 3px;

    &:hover {
      background: var(--bg-button-hover);
      border-color: var(--border-button);
    }
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
  gap: 0.6rem;
  margin: 0.4rem 0;

  & input {
    width: 6rem;
    background: var(--bg-viewport);
    border: 1px solid var(--border-button);
    color: var(--fg-default);
    padding: 0.1rem 0.3rem;
    border-radius: 3px;
  }
}

.threshold-error { color: var(--level-error); margin: 0.2rem 0; }

.threshold-actions {
  display: flex;
  gap: 0.4rem;

  & button {
    background: transparent;
    border: 1px solid var(--border-button);
    color: var(--fg-default);
    padding: 0.15rem 0.6rem;
    border-radius: 3px;
    cursor: pointer;

    &.muted { color: var(--fg-muted); }
    &:disabled { opacity: 0.4; cursor: not-allowed; }
  }
}
</style>
