<script setup lang="ts">
import { computed, onBeforeUnmount, ref, useTemplateRef, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { useVirtualizer } from '@tanstack/vue-virtual'

interface OpenedFile {
  file_id: number
  path: string
  size_bytes: number
  line_count: number
  record_count: number
}

interface IpcError {
  kind: string
  message: string
  path?: string
}

interface RecordHeader {
  byte_offset: number
  byte_len: number
  line_offset: number
  line_count: number
  level: string
}

interface RecordsPayload {
  start: number
  base_offset: number
  headers: RecordHeader[]
  text: string
}

const PAGE_SIZE = 256
const ROW_HEIGHT = 18
const OVERSCAN = 16

const file = ref<OpenedFile | null>(null)
const error = ref<string | null>(null)
const busy = ref(false)

// page_index -> array of record texts (length up to PAGE_SIZE)
const pages = ref(new Map<number, string[]>())
const inflight = new Set<number>()

const scrollEl = useTemplateRef<HTMLDivElement>('scrollEl')

const virtualizer = useVirtualizer(
  computed(() => ({
    count: file.value?.record_count ?? 0,
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

function recordText(index: number): string {
  const pageIdx = Math.floor(index / PAGE_SIZE)
  const page = pages.value.get(pageIdx)
  if (!page) return ''
  return page[index % PAGE_SIZE] ?? ''
}

async function fetchPage(pageIdx: number) {
  if (!file.value) return
  if (pages.value.has(pageIdx)) return
  if (inflight.has(pageIdx)) return
  const start = pageIdx * PAGE_SIZE
  const total = file.value.record_count
  if (start >= total) return
  const end = Math.min(start + PAGE_SIZE, total)
  inflight.add(pageIdx)
  try {
    const payload = await invoke<RecordsPayload>('get_records', {
      fileId: file.value.file_id,
      start,
      end,
    })
    const texts = sliceRecords(payload)
    pages.value.set(pageIdx, texts)
    // Force reactive read for any visible rows pointing into this page.
    pages.value = new Map(pages.value)
  } catch (e) {
    const err = e as IpcError | string
    error.value = typeof err === 'string' ? err : err.message
  } finally {
    inflight.delete(pageIdx)
  }
}

function sliceRecords(payload: RecordsPayload): string[] {
  const out: string[] = []
  for (const h of payload.headers) {
    const startInText = h.byte_offset - payload.base_offset
    const endInText = startInText + h.byte_len
    let chunk = payload.text.slice(startInText, endInText)
    // Drop trailing newline so the row renders flush.
    if (chunk.endsWith('\r\n')) chunk = chunk.slice(0, -2)
    else if (chunk.endsWith('\n')) chunk = chunk.slice(0, -1)
    out.push(chunk)
  }
  return out
}

watch(virtualRows, (rows) => {
  if (!file.value) return
  const wanted = new Set<number>()
  for (const r of rows) wanted.add(Math.floor(r.index / PAGE_SIZE))
  for (const p of wanted) fetchPage(p)
})

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
    // Close the previously open file so its memory is reclaimed.
    if (file.value) {
      const prev = file.value.file_id
      file.value = null
      pages.value = new Map()
      await invoke('close_file', { fileId: prev }).catch(() => {})
    }
    const opened = await invoke<OpenedFile>('open_file', { path: selected })
    file.value = opened
    // Kick off the first page so the user sees content immediately.
    fetchPage(0)
  } catch (e) {
    const err = e as IpcError | string
    error.value = typeof err === 'string' ? err : err.message
    file.value = null
  } finally {
    busy.value = false
  }
}

onBeforeUnmount(() => {
  if (file.value) {
    invoke('close_file', { fileId: file.value.file_id }).catch(() => {})
  }
})
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
    </header>

    <section v-if="error" class="error">{{ error }}</section>

    <div v-if="file" ref="scrollEl" class="viewport">
      <div class="total" :style="{ height: `${totalSize}px` }">
        <div
          v-for="row in virtualRows"
          :key="String(row.key)"
          class="row"
          :style="{ transform: `translateY(${row.start}px)`, height: `${row.size}px` }"
        >
          <span class="idx">{{ row.index + 1 }}</span>
          <span class="txt">{{ recordText(row.index) }}</span>
        </div>
      </div>
    </div>
    <p v-else class="placeholder">No file open. Click <em>Open file...</em> to pick one.</p>
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

.viewport {
  flex: 1 1 auto;
  overflow: auto;
  font-family: var(--font-mono);
  font-size: var(--font-size-base);
  line-height: var(--row-height);
  background: var(--bg-viewport);

  .total {
    position: relative;
    width: 100%;
  }

  .row {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    display: flex;
    align-items: center;
    white-space: pre;
    padding: 0 0.6rem;
    color: var(--fg-row);

    .idx {
      display: inline-block;
      width: 4.5em;
      color: var(--fg-gutter);
      text-align: right;
      margin-right: 0.8rem;
      flex: 0 0 auto;
      user-select: none;
    }

    .txt {
      flex: 1 1 auto;
      overflow: hidden;
      text-overflow: ellipsis;
    }
  }
}
</style>
