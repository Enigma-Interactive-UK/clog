<script setup lang="ts">
/**
 * Right-side collapsible drawer hosting the slow-request insights for
 * the active tab. Renders the totals header, mode/filter/sort toolbar,
 * and a sortable entry list with click-to-jump + expandable occurrence
 * rows. Threshold editor and speed grid land in later tasks.
 */
import { computed, onMounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { Tab } from '../tab'
import type { SlowRequestEntry, SlowRequestSummary } from '../types'

const props = defineProps<{ tab: Tab }>()

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'jump', line: number): void
}>()

const expanded = ref<Set<string>>(new Set())
const error = ref<string | null>(null)
const loading = ref(false)

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

onMounted(() => {
  void refresh()
})

watch(() => props.tab.slowRequestMode.value, refresh)
watch(
  () => props.tab.file.value.line_count,
  () => {
    if (props.tab.insightsOpen.value) void refresh()
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
  <aside class="insights-drawer">
    <header class="drawer-head">
      <span class="title">Slow requests</span>
      <button type="button" class="close-btn" aria-label="Close" @click="emit('close')">
        x
      </button>
    </header>
    <div class="drawer-totals">{{ totals }}</div>
    <div class="drawer-toolbar">
      <div class="mode-toggle">
        <button
          type="button"
          class="seg"
          :class="{ active: tab.slowRequestMode.value === 'normalised' }"
          @click="tab.slowRequestMode.value = 'normalised'"
        >Normalised</button>
        <button
          type="button"
          class="seg"
          :class="{ active: tab.slowRequestMode.value === 'raw' }"
          @click="tab.slowRequestMode.value = 'raw'"
        >Raw</button>
      </div>
      <input
        v-model="tab.slowRequestFilter.value"
        type="text"
        class="filter-input"
        placeholder="Filter path..."
      />
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
    <div class="drawer-body">
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

      <ul v-else class="entry-list">
        <li v-for="entry in filteredEntries" :key="entry.path" class="entry">
          <div class="entry-row" @click="toggleExpanded(entry.path)">
            <span class="entry-path" :title="entry.path" @click.stop="jumpTo(entry.longest_line)">
              {{ entry.path }}
            </span>
            <span class="entry-stats">
              {{ entry.count }} hits . total {{ formatMs(entry.total_ms) }} .
              avg {{ formatMs(entry.avg_ms) }} . p95 {{ formatMs(entry.p95_ms) }} .
              max {{ formatMs(entry.max_ms) }}
            </span>
            <span class="entry-expand" :class="{ open: expanded.has(entry.path) }">v</span>
          </div>
          <ul v-if="expanded.has(entry.path)" class="occurrence-list">
            <li
              v-for="occ in entry.occurrences"
              :key="`${entry.path}-${occ.line_index}`"
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
        </li>
      </ul>
    </div>
  </aside>
</template>

<style scoped>
.insights-drawer {
  flex: 0 0 auto;
  width: 360px;
  display: flex;
  flex-direction: column;
  background: var(--bg-elevated);
  border-left: 1px solid var(--border-default);
  min-height: 0;
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

.mode-toggle {
  display: flex;

  & .seg {
    background: transparent;
    border: 1px solid var(--border-button);
    color: var(--fg-default);
    padding: 0.15rem 0.5rem;
    cursor: pointer;

    &:first-child { border-radius: 3px 0 0 3px; }
    &:last-child  { border-radius: 0 3px 3px 0; border-left: none; }
    &.active {
      background: var(--bg-button-hover);
      color: var(--accent);
    }
  }
}

.filter-input {
  flex: 1 1 auto;
  background: var(--bg-viewport);
  border: 1px solid var(--border-button);
  color: var(--fg-default);
  padding: 0.15rem 0.4rem;
  border-radius: 3px;
  min-width: 60px;
}

.sort-select {
  background: var(--bg-viewport);
  border: 1px solid var(--border-button);
  color: var(--fg-default);
  padding: 0.15rem 0.3rem;
  border-radius: 3px;
}

.sort-dir { color: var(--fg-muted); font-size: 0.8rem; }

.entry-list { list-style: none; margin: 0; padding: 0; }

.entry {
  border-bottom: 1px solid var(--border-default);

  & .entry-row {
    display: grid;
    grid-template-columns: 1fr auto 16px;
    gap: 0.4rem;
    padding: 0.4rem 0;
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
  & .entry-expand { color: var(--fg-muted); transition: transform 120ms; }
  & .entry-expand.open { transform: rotate(180deg); }
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
</style>
