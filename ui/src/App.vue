<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, useTemplateRef, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { useVirtualizer } from '@tanstack/vue-virtual'

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

const PAGE_SIZE = 256
const ROW_HEIGHT = 18
const OVERSCAN = 32

const file = ref<OpenedFile | null>(null)
const error = ref<string | null>(null)
const busy = ref(false)

// page_index -> array of LineRow (length up to PAGE_SIZE).
const pages = ref(new Map<number, LineRow[]>())
const inflight = new Set<number>()

// Pattern-paste bar state.
const patternInput = ref<string>('')
const patternMode = ref<'pattern' | 'regex'>('pattern')
const patternScore = ref<number | null>(null)
const patternSampleSize = ref<number>(0)
const patternError = ref<string | null>(null)

const scrollEl = useTemplateRef<HTMLDivElement>('scrollEl')

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

async function fetchPage(pageIdx: number) {
  if (!file.value) return
  if (pages.value.has(pageIdx)) return
  if (inflight.has(pageIdx)) return
  const start = pageIdx * PAGE_SIZE
  const total = file.value.line_count
  if (start >= total) return
  const end = Math.min(start + PAGE_SIZE, total)
  inflight.add(pageIdx)
  try {
    const payload = await invoke<LinesPayload>('get_lines', {
      fileId: file.value.file_id,
      start,
      end,
    })
    pages.value.set(pageIdx, payload.lines)
    pages.value = new Map(pages.value)
  } catch (e) {
    const err = e as IpcError | string
    error.value = typeof err === 'string' ? err : err.message
  } finally {
    inflight.delete(pageIdx)
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
}

const stickyHeader = computed<StickyHeader | null>(() => {
  if (!file.value) return null
  const rows = virtualRows.value
  if (rows.length === 0) return null
  const first = rows[0]
  const data = lineRow(first.index)
  if (!data) return null
  if (data.line_within_record === 0) return null
  // Need to look up the header row (line_within_record == 0) of the same
  // record. Walk backward from the first visible row.
  for (let i = first.index - 1; i >= 0; i--) {
    const candidate = lineRow(i)
    if (!candidate) {
      // Force-fetch the page so the header becomes available next frame.
      fetchPage(Math.floor(i / PAGE_SIZE))
      return null
    }
    if (candidate.record_idx !== data.record_idx) return null
    if (candidate.line_within_record === 0) return { row: candidate }
  }
  return null
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
    if (file.value) {
      const prev = file.value.file_id
      file.value = null
      pages.value = new Map()
      await invoke('close_file', { fileId: prev }).catch(() => {})
    }
    const opened = await invoke<OpenedFile>('open_file', { path: selected })
    file.value = opened
    patternInput.value = opened.pattern_source
    patternMode.value = 'pattern'
    patternScore.value = opened.pattern_score
    patternError.value = null
    fetchPage(0)
  } catch (e) {
    const err = e as IpcError | string
    error.value = typeof err === 'string' ? err : err.message
    file.value = null
  } finally {
    busy.value = false
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
})

onBeforeUnmount(() => {
  globalThis.removeEventListener('keydown', suppressBrowserFind, { capture: true })
  if (file.value) {
    invoke('close_file', { fileId: file.value.file_id }).catch(() => {})
  }
})

// --- Header-line span slicing (axis-1). ---
interface Span {
  cls: string
  text: string
}

/**
 * Slice `text` into ordered, non-overlapping spans driven by `fields`. Any
 * gap between known fields is emitted as a `sep` span (the literal text
 * between two structural fields, e.g. brackets, dashes, spaces).
 */
function sliceHeader(text: string, fields: HeaderFields): Span[] {
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
  const out: Span[] = []
  let cursor = 0
  for (const m of marks) {
    if (m.start > cursor) out.push({ cls: 'sep', text: text.slice(cursor, m.start) })
    out.push({ cls: m.cls, text: text.slice(m.start, m.end) })
    cursor = m.end
  }
  if (cursor < text.length) out.push({ cls: 'sep', text: text.slice(cursor) })
  return out
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

    <div v-if="file" ref="scrollEl" class="viewport">
      <div
        v-if="stickyHeader"
        class="sticky-shell"
      >
        <div
          class="row is-header"
          :style="{ '--gutter-color': levelGutterVar(stickyHeader.row.level) }"
        >
          <span class="gutter" />
          <span class="idx muted-idx"></span>
          <span class="txt">
            <span
              v-for="(span, si) in sliceHeader(stickyHeader.row.text, stickyHeader.row.fields!)"
              :key="si"
              :class="['s-' + span.cls, span.cls === 'level' ? 'level-' + stickyHeader.row.level : '']"
            >{{ span.text }}</span>
          </span>
        </div>
      </div>
      <div class="total" :style="{ height: `${totalSize}px` }">
        <div
          v-for="vrow in virtualRows"
          :key="String(vrow.key)"
          class="row"
          :class="{
            'is-header': lineRow(vrow.index)?.line_within_record === 0,
            'is-continuation': (lineRow(vrow.index)?.line_within_record ?? 0) > 0,
          }"
          :style="{
            transform: `translateY(${vrow.start}px)`,
            height: `${vrow.size}px`,
            '--gutter-color': levelGutterVar(lineRow(vrow.index)?.level ?? 'unknown'),
          }"
        >
          <span class="gutter" />
          <span class="idx">{{ vrow.index + 1 }}</span>
          <span class="txt">
            <template v-if="lineRow(vrow.index)?.fields">
              <span
                v-for="(span, si) in sliceHeader(
                  lineRow(vrow.index)!.text,
                  lineRow(vrow.index)!.fields!,
                )"
                :key="si"
                :class="['s-' + span.cls, span.cls === 'level' ? 'level-' + (lineRow(vrow.index)?.level ?? 'unknown') : '']"
              >{{ span.text }}</span>
            </template>
            <template v-else>
              <span class="continuation">{{ lineRow(vrow.index)?.text ?? '' }}</span>
            </template>
          </span>
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
    display: grid;
    grid-template-columns: var(--gutter-width) var(--line-num-width) 1fr;
    align-items: center;
    white-space: pre;
    color: var(--fg-row);

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
  }

  .sticky-shell {
    position: sticky;
    top: 0;
    z-index: 2;
    background: var(--bg-sticky);
    backdrop-filter: blur(2px);
    border-bottom: 1px solid var(--border-sticky);
    height: var(--row-height);

    .row {
      position: relative;
      height: var(--row-height);
    }

    .muted-idx {
      visibility: hidden;
    }
  }
}
</style>
