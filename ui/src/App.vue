<script setup lang="ts">
import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'

interface FileSummary {
  path: string
  size_bytes: number
  line_count: number
}

interface IpcError {
  kind: string
  message: string
  path?: string
}

const summary = ref<FileSummary | null>(null)
const error = ref<string | null>(null)
const busy = ref(false)

function basename(p: string): string {
  const m = p.match(/[^\\/]+$/)
  return m ? m[0] : p
}

function formatCount(n: number): string {
  return n.toLocaleString('en-GB')
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
    summary.value = await invoke<FileSummary>('open_file', { path: selected })
  } catch (e) {
    const err = e as IpcError | string
    error.value = typeof err === 'string' ? err : err.message
    summary.value = null
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <main class="shell">
    <header>
      <h1>Clog</h1>
      <p class="tagline">Core Log - viewer for log4j2 files</p>
    </header>

    <section class="actions">
      <button :disabled="busy" @click="pickFile">
        {{ busy ? 'Reading...' : 'Open file...' }}
      </button>
    </section>

    <section v-if="summary" class="summary">
      <strong>{{ basename(summary.path) }}</strong>
      <span class="sep">--</span>
      <span>{{ formatCount(summary.line_count) }} lines</span>
      <span class="sep">--</span>
      <span>{{ formatCount(summary.size_bytes) }} bytes</span>
      <div class="path">{{ summary.path }}</div>
    </section>

    <section v-if="error" class="error">{{ error }}</section>
  </main>
</template>

<style scoped>
.shell {
  font-family: ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif;
  color: #e6e9ef;
  background: #14181f;
  padding: 2rem;
  min-height: 100vh;
  box-sizing: border-box;
}
h1 { margin: 0 0 0.25rem; font-size: 1.8rem; }
.tagline { margin: 0 0 1.5rem; color: #8b94a3; font-size: 0.95rem; }
button {
  background: #2a3340;
  color: #e6e9ef;
  border: 1px solid #3a4554;
  padding: 0.55rem 1.2rem;
  border-radius: 6px;
  font-size: 0.95rem;
  cursor: pointer;
}
button:hover:not(:disabled) { background: #34404f; }
button:disabled { opacity: 0.6; cursor: progress; }
.summary {
  margin-top: 1.5rem;
  padding: 1rem 1.2rem;
  background: #1c2230;
  border: 1px solid #2a3340;
  border-radius: 6px;
  font-family: Consolas, ui-monospace, monospace;
}
.summary .sep { color: #5a6577; margin: 0 0.5rem; }
.summary .path { color: #6b7484; font-size: 0.8rem; margin-top: 0.6rem; word-break: break-all; }
.error {
  margin-top: 1.5rem;
  padding: 0.8rem 1rem;
  background: #3a1f22;
  border: 1px solid #5a2b30;
  border-radius: 6px;
  color: #ffb6bb;
}
</style>
