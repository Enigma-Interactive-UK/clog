<script setup lang="ts">
/**
 * App orchestrator. Owns the tab list, the active tab pointer, the global
 * modals (settings, about, pattern), the window chrome controls, the
 * drag-drop overlay, single-instance + CLI startup-path handling, and the
 * multi-tab session save/restore.
 *
 * Per-tab state lives in `Tab` objects (see ./tab.ts) -- the viewport, the
 * search bar, the tail loop and the per-file pages all flow through those.
 * Tail/search channels close over their tab via the factory, so deltas
 * always update the originating tab even when it is not the visible one.
 */
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, useTemplateRef, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getName, getTauriVersion, getVersion } from '@tauri-apps/api/app'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { open } from '@tauri-apps/plugin-dialog'
import { openUrl } from '@tauri-apps/plugin-opener'

import defaultRulesFile from './highlight/default-rules.json'
import { setRules, type HighlightRulesFile } from './highlight/engine'

import { createTab, type Tab } from './tab'
import { useSettings } from './composables/useSettings'
import LogViewport from './components/LogViewport.vue'
import SearchBar from './components/SearchBar.vue'
import TabStrip from './components/TabStrip.vue'
import type { IpcError, OpenedFile, Session, RestoredFile } from './types'

setRules((defaultRulesFile as HighlightRulesFile).rules)

// --- Tab management -------------------------------------------------------

// shallowRef keeps the Tab objects' internal `Ref` fields as Refs.
// A deep ref would auto-unwrap them, breaking `tab.file.value` access.
const tabs = shallowRef<Tab[]>([])
const activeTabId = ref<number | null>(null)
let nextLocalTabId = 0

const currentTab = computed<Tab | null>(() => {
  const id = activeTabId.value
  if (id === null) return null
  return tabs.value.find((t) => t.localId === id) ?? null
})

function tabByPath(path: string): Tab | null {
  return tabs.value.find((t) => t.file.value.path === path) ?? null
}

// --- Global UI state ------------------------------------------------------

const error = ref<string | null>(null)
const busy = ref(false)
const settingsOpen = ref(false)
const aboutOpen = ref(false)
const aboutInfo = ref<{ name: string; version: string; tauri: string } | null>(null)
const patternOpen = ref(false)
const dragHover = ref(false)
const sessionRestoreInFlight = ref(false)
const sessionSaveTimer = ref<number | null>(null)

// Refs into LogViewport so the search bar can drive prev/next-hit scrolls.
const viewportRef = useTemplateRef<InstanceType<typeof LogViewport>>('viewportRef')

const {
  settings,
  dataDir,
  themeToggleGlyph,
  THEME_LABEL,
  loadSettings,
  updateSettings,
  cycleTheme,
  bumpFontSize,
  resetFontSize,
  handleFontShortcut,
  refreshDataDir,
  openDataFolder,
  forgetRecent,
  resetData,
} = useSettings()

// --- Window chrome -------------------------------------------------------

const windowMaximized = ref(false)
const appWindow = getCurrentWindow()
const appWebview = getCurrentWebview()
let unlistenWindow: (() => void) | null = null
let unlistenDragDrop: UnlistenFn | null = null
let unlistenSingleInstance: UnlistenFn | null = null

async function refreshMaximized() {
  try {
    windowMaximized.value = await appWindow.isMaximized()
  } catch {
    windowMaximized.value = false
  }
}

async function minimizeWindow() {
  try { await appWindow.minimize() } catch (e) { error.value = (e as IpcError).message ?? String(e) }
}
async function toggleMaximizeWindow() {
  try { await appWindow.toggleMaximize(); await refreshMaximized() } catch (e) { error.value = (e as IpcError).message ?? String(e) }
}
async function closeWindow() {
  try { await appWindow.close() } catch (e) { error.value = (e as IpcError).message ?? String(e) }
}

// --- Helpers --------------------------------------------------------------

function basename(p: string): string {
  const m = p.match(/[^\\/]+$/)
  return m ? m[0] : p
}

function formatCount(n: number): string {
  return n.toLocaleString('en-GB')
}

function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return `${n}`
  if (n < 1024) return `${n} B`
  const units = ['KiB', 'MiB', 'GiB', 'TiB']
  let value = n / 1024
  let i = 0
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024
    i++
  }
  const digits = value < 10 ? 2 : value < 100 ? 1 : 0
  return `${value.toFixed(digits)} ${units[i]}`
}

// --- Tab open/close/switch ------------------------------------------------

async function openPath(path: string, restored: RestoredFile | null = null): Promise<Tab | null> {
  // If the file is already open in a tab, just activate it.
  const existing = tabByPath(path)
  if (existing) {
    activateTab(existing.localId)
    return existing
  }
  if (busy.value) return null
  busy.value = true
  error.value = null
  try {
    const opened = (await invoke('open_file', { path })) as OpenedFile
    const tab = createTab(
      ++nextLocalTabId,
      opened,
      { followTail: settings.value.follow_tail_default },
      {
        onError: (msg) => { error.value = msg },
      },
    )
    if (restored) tab.applyRestored(restored)
    tabs.value = [...tabs.value, tab]
    activeTabId.value = tab.localId
    // Kick off the tail loop. The handler closes over the tab so deltas
    // route correctly even after a tab switch.
    void tab.startTail()
    // If there's already a query (e.g. carried over from a restore), kick
    // off the search.
    if (tab.searchQuery.value.trim().length > 0) tab.scheduleSearch()
    return tab
  } catch (e) {
    error.value = (e as IpcError).message ?? String(e)
    return null
  } finally {
    busy.value = false
  }
}

function activateTab(localId: number) {
  if (activeTabId.value === localId) return
  // Capture the leaving tab's DOM scrollTop so the next time we switch
  // back the viewport restores it. LogViewport's onBeforeUnmount also
  // does this, but capturing here covers the race where the user
  // switches forwards/back quickly.
  activeTabId.value = localId
  const t = tabs.value.find((t) => t.localId === localId)
  if (t) t.unread.value = false
}

async function closeTab(localId: number) {
  const idx = tabs.value.findIndex((t) => t.localId === localId)
  if (idx < 0) return
  const tab = tabs.value[idx]
  await tab.teardown()
  const newTabs = tabs.value.slice()
  newTabs.splice(idx, 1)
  tabs.value = newTabs
  if (activeTabId.value === localId) {
    if (newTabs.length === 0) activeTabId.value = null
    else {
      // Activate the tab to the left, or the leftmost remaining.
      const replacement = newTabs[Math.max(0, idx - 1)]
      activeTabId.value = replacement?.localId ?? null
    }
  }
  scheduleSessionSave()
}

async function pickFile() {
  error.value = null
  const selected = await open({
    multiple: true,
    title: 'Open a log file',
    filters: [
      { name: 'Log files', extensions: ['log', 'out', 'txt'] },
      { name: 'All files', extensions: ['*'] },
    ],
  })
  if (!selected) return
  const paths = Array.isArray(selected) ? selected : [selected]
  for (const p of paths) {
    if (typeof p === 'string') await openPath(p)
  }
}

// --- Drag/drop into the window adds a new tab ----------------------------

async function onDragDropEvent(evt: { payload: { type: string; paths?: string[] } }) {
  const t = evt.payload.type
  if (t === 'enter' || t === 'over') {
    dragHover.value = true
  } else if (t === 'leave') {
    dragHover.value = false
  } else if (t === 'drop') {
    dragHover.value = false
    const paths = evt.payload.paths ?? []
    for (const p of paths) {
      if (typeof p === 'string' && p.length > 0) await openPath(p)
    }
  }
}

// --- Hit nav: SearchBar emits, we drive the viewport ---------------------

function onNextHit() { viewportRef.value?.scrollToCurrentHit() }
function onPrevHit() { viewportRef.value?.scrollToCurrentHit() }

// --- Modals ---------------------------------------------------------------

async function openAbout() {
  if (!aboutInfo.value) {
    try {
      const [name, version, tauri] = await Promise.all([
        getName(), getVersion(), getTauriVersion(),
      ])
      aboutInfo.value = { name, version, tauri }
    } catch {
      aboutInfo.value = { name: 'Clog', version: 'unknown', tauri: 'unknown' }
    }
  }
  aboutOpen.value = true
}

async function openSettings() {
  await refreshDataDir()
  settingsOpen.value = true
}

async function openRecent(path: string) {
  settingsOpen.value = false
  await openPath(path)
}

async function onForgetRecent(path: string) {
  const err = await forgetRecent(path)
  if (err) error.value = err
}

async function onUpdateSettings(patch: Partial<typeof settings.value>) {
  const err = await updateSettings(patch)
  if (err) error.value = err
}

async function onOpenDataFolder() {
  const err = await openDataFolder()
  if (err) error.value = err
}

async function onResetData(scope: 'settings' | 'session' | 'patterns' | 'index' | 'all') {
  const err = await resetData(scope)
  if (err) error.value = err
}

// --- Pattern modal (operates on current tab) -----------------------------

async function testPattern() {
  if (!currentTab.value) return
  await currentTab.value.testPattern()
}
async function applyPattern() {
  if (!currentTab.value) return
  await currentTab.value.applyPattern()
}

// --- Session save/restore (multi-tab) ------------------------------------

function captureSession(): Session {
  const snapshots = tabs.value.map((t) => t.snapshot())
  const idx = tabs.value.findIndex((t) => t.localId === activeTabId.value)
  return {
    schema: 1,
    last_file: snapshots[Math.max(0, idx)] ?? null,
    tabs: snapshots,
    active_tab: Math.max(0, idx),
  }
}

function scheduleSessionSave() {
  if (sessionSaveTimer.value !== null) globalThis.clearTimeout(sessionSaveTimer.value)
  sessionSaveTimer.value = globalThis.setTimeout(() => {
    sessionSaveTimer.value = null
    void invoke('save_session', { session: captureSession() }).catch(() => {})
  }, 400) as unknown as number
}

async function restoreSession() {
  sessionRestoreInFlight.value = true
  try {
    const sess = (await invoke('get_session')) as Session
    const stored = sess.tabs && sess.tabs.length > 0 ? sess.tabs : (sess.last_file ? [sess.last_file] : [])
    if (stored.length === 0) return
    for (const r of stored) {
      await openPath(r.path, r)
    }
    const active = Math.min(stored.length - 1, Math.max(0, sess.active_tab))
    if (tabs.value[active]) activeTabId.value = tabs.value[active].localId
  } catch {
    // Files gone / unreadable: drop to empty state but don't surface
    // a modal error -- the user's recent-files list still holds the
    // breadcrumbs.
  } finally {
    sessionRestoreInFlight.value = false
  }
}

// --- Single-instance forward + CLI startup paths -------------------------

async function consumeStartupPaths() {
  try {
    const paths = (await invoke('take_startup_paths')) as string[]
    for (const p of paths) await openPath(p)
  } catch {
    // Older binary / IPC missing: no-op.
  }
}

async function bindSingleInstance() {
  try {
    unlistenSingleInstance = await listen<string[]>('single-instance-paths', async (evt) => {
      const paths = evt.payload ?? []
      for (const p of paths) await openPath(p)
    })
  } catch {
    unlistenSingleInstance = null
  }
}

// --- Persist tab state changes -------------------------------------------

watch(
  // Watch a coarse fingerprint over every tab so any per-tab knob change
  // schedules a debounced session save without 30 separate watchers.
  () => tabs.value.map((t) => `${t.file.value.path}|${t.followTail.value}|${t.searchMode.value}|${t.searchQuery.value}|${t.searchCaseSensitive.value}|${t.filterMode.value}|${Object.entries(t.levelAllow.value).filter(([, v]) => v).map(([k]) => k).join(',')}|${t.scrollTop.value}`).join('||') + '#' + String(activeTabId.value),
  () => {
    if (sessionRestoreInFlight.value) return
    scheduleSessionSave()
  },
)

// --- Keyboard shortcuts --------------------------------------------------

function suppressBrowserFind(ev: KeyboardEvent) {
  if (!(ev.ctrlKey || ev.metaKey) || ev.altKey) return
  const k = ev.key.toLowerCase()
  if (k === 'f' || k === 'g') {
    ev.preventDefault()
    ev.stopPropagation()
  }
}

function handleTabShortcuts(ev: KeyboardEvent): boolean {
  if (!(ev.ctrlKey || ev.metaKey) || ev.altKey) return false
  const k = ev.key.toLowerCase()
  if (k === 'w') {
    // Ctrl+W -- close active tab.
    if (activeTabId.value !== null) {
      ev.preventDefault()
      void closeTab(activeTabId.value)
      return true
    }
  }
  if (k === 't') {
    // Ctrl+T -- new tab via file picker.
    ev.preventDefault()
    void pickFile()
    return true
  }
  if (ev.key === 'Tab') {
    ev.preventDefault()
    if (tabs.value.length < 2) return true
    const idx = tabs.value.findIndex((t) => t.localId === activeTabId.value)
    const step = ev.shiftKey ? -1 : 1
    const next = (idx + step + tabs.value.length) % tabs.value.length
    activateTab(tabs.value[next].localId)
    return true
  }
  return false
}

function onGlobalKey(ev: KeyboardEvent) {
  if (handleTabShortcuts(ev)) return
  if (handleFontShortcut(ev)) return
  suppressBrowserFind(ev)
}

// --- Lifecycle -----------------------------------------------------------

onMounted(() => {
  globalThis.addEventListener('keydown', onGlobalKey, { capture: true })
  void (async () => {
    await loadSettings()
    void refreshMaximized()
    try {
      const unlistenResize = await appWindow.onResized(() => void refreshMaximized())
      unlistenWindow = unlistenResize
    } catch {
      unlistenWindow = null
    }
    try {
      unlistenDragDrop = await appWebview.onDragDropEvent(onDragDropEvent)
    } catch {
      unlistenDragDrop = null
    }
    await bindSingleInstance()
    await restoreSession()
    // Startup-path argv handling happens AFTER session restore so a CLI
    // file opens as an additional tab rather than racing with the
    // restore.
    await consumeStartupPaths()
  })()
})

onBeforeUnmount(() => {
  globalThis.removeEventListener('keydown', onGlobalKey, { capture: true })
  if (unlistenWindow) { unlistenWindow(); unlistenWindow = null }
  if (unlistenDragDrop) { unlistenDragDrop(); unlistenDragDrop = null }
  if (unlistenSingleInstance) { unlistenSingleInstance(); unlistenSingleInstance = null }
  if (sessionSaveTimer.value !== null) globalThis.clearTimeout(sessionSaveTimer.value)
  for (const t of tabs.value) void t.teardown()
})
</script>

<template>
  <main class="shell">
    <header class="bar" data-tauri-drag-region>
      <h1 class="app-title">
        <button
          type="button"
          class="logo-btn"
          title="About Clog"
          @click="openAbout"
        >
          <img src="/clog-icon.png" alt="" class="app-icon" />
        </button>
      </h1>
      <button :disabled="busy" @click="pickFile">
        {{ busy ? 'Reading...' : 'Open file...' }}
      </button>
      <button
        type="button"
        class="settings-btn"
        title="Settings"
        aria-label="Open settings"
        @click="openSettings"
      >&#9881;</button>
      <span class="window-controls" :class="{ 'no-file': !currentTab }">
        <button type="button" class="win-btn" title="Minimize" aria-label="Minimize" @click="minimizeWindow">&#9472;</button>
        <button
          type="button"
          class="win-btn"
          :title="windowMaximized ? 'Restore' : 'Maximize'"
          :aria-label="windowMaximized ? 'Restore' : 'Maximize'"
          @click="toggleMaximizeWindow"
        >{{ windowMaximized ? '⧉' : '□' }}</button>
        <button type="button" class="win-btn close" title="Close" aria-label="Close" @click="closeWindow">&times;</button>
      </span>
    </header>

    <TabStrip
      :tabs="tabs"
      :active-tab-id="activeTabId"
      @switch="activateTab"
      @close="closeTab"
      @new-tab="pickFile"
    />

    <section v-if="error" class="error">{{ error }}</section>

    <div v-if="currentTab?.rotationToast.value" class="rotation-toast">{{ currentTab.rotationToast.value }}</div>

    <LogViewport
      v-if="currentTab"
      :key="currentTab.localId"
      ref="viewportRef"
      :tab="currentTab"
      @error="(msg: string) => (error = msg)"
    />
    <p v-else class="placeholder">No file open. Click <em>Open file...</em> to pick one.</p>

    <SearchBar
      v-if="currentTab"
      :key="`sb-${currentTab.localId}`"
      :tab="currentTab"
      @next-hit="onNextHit"
      @prev-hit="onPrevHit"
    />

    <footer class="status-bar">
      <span class="slot left">
        <span v-if="currentTab?.file.value.cache_hit" class="cache-hint" title="Records loaded from the on-disk index cache">cached</span>
      </span>
      <span class="slot right">
        <template v-if="currentTab">
          <span class="stat">{{ formatCount(currentTab.file.value.record_count) }} records</span>
          <span class="stat">{{ formatCount(currentTab.file.value.line_count) }} lines</span>
          <span class="stat" :title="`${formatCount(currentTab.file.value.size_bytes)} bytes`">{{ formatBytes(currentTab.file.value.size_bytes) }}</span>
        </template>
        <button
          type="button"
          class="theme-toggle"
          :class="{ 'is-auto': settings.theme === 'system' }"
          :title="THEME_LABEL[settings.theme]"
          :aria-label="THEME_LABEL[settings.theme]"
          @click="cycleTheme"
        >{{ themeToggleGlyph }}</button>
        <span class="font-size-hint" :title="`Base font size (Ctrl-+ / Ctrl-- / Ctrl-0)`">
          {{ settings.font_size }}px
        </span>
        <span v-if="currentTab" class="pattern-status">
          <span class="pattern-label" :title="currentTab.file.value.pattern_source">
            Pattern: <strong>{{ currentTab.file.value.pattern_name ?? 'custom' }}</strong>
          </span>
          <button
            type="button"
            class="pattern-edit-btn"
            title="Edit pattern"
            aria-label="Edit pattern"
            @click="patternOpen = true"
          >Edit</button>
        </span>
      </span>
    </footer>

    <div v-if="dragHover" class="drop-overlay">
      <div class="drop-hint">
        <span class="arrow">&darr;</span>
        Drop a log file to open it
      </div>
    </div>

    <div v-if="patternOpen && currentTab" class="modal-backdrop" @click.self="patternOpen = false">
      <div class="modal pattern-modal" role="dialog" aria-label="Pattern">
        <header class="modal-head">
          <h2>Pattern</h2>
          <button type="button" class="modal-close" aria-label="Close pattern editor" @click="patternOpen = false">&times;</button>
        </header>
        <section class="modal-body">
          <div class="row-grid">
            <label>Kind</label>
            <select v-model="currentTab.patternMode.value">
              <option value="pattern">PatternLayout</option>
              <option value="regex">Regex</option>
            </select>
          </div>
          <div class="pattern-input-row">
            <input
              v-model="currentTab.patternInput.value"
              class="pat-input"
              :placeholder="currentTab.patternMode.value === 'pattern'
                ? '[%-5level] %d{yyyy-MM-dd HH:mm:ss.SSS} [%t] %c{1} - %msg%n'
                : '^(?P&lt;timestamp&gt;\\d{4}-...) (?P&lt;level&gt;INFO|WARN|ERROR) ...'"
              spellcheck="false"
            />
            <button type="button" @click="testPattern">Test</button>
            <button type="button" @click="applyPattern">Apply</button>
          </div>
          <p v-if="currentTab.patternScore.value !== null" class="score">
            Match score: <strong>{{ (currentTab.patternScore.value * 100).toFixed(1) }}%</strong>
            <span v-if="currentTab.patternSampleSize.value > 0" class="muted"> of {{ currentTab.patternSampleSize.value }} lines</span>
          </p>
          <p v-if="currentTab.file.value.pattern_name" class="muted">
            Auto-detected: <strong>{{ currentTab.file.value.pattern_name }}</strong>
          </p>
          <p v-if="currentTab.patternError.value" class="pat-error">{{ currentTab.patternError.value }}</p>
          <p class="muted">
            Apply saves the pattern as a per-file override; the next time you open this file the same pattern is used automatically.
          </p>
        </section>
      </div>
    </div>

    <div v-if="settingsOpen" class="modal-backdrop" @click.self="settingsOpen = false">
      <div class="modal" role="dialog" aria-label="Settings">
        <header class="modal-head">
          <h2>Settings</h2>
          <button type="button" class="modal-close" aria-label="Close settings" @click="settingsOpen = false">&times;</button>
        </header>
        <section class="modal-body">
          <h3>Appearance</h3>
          <div class="row-grid">
            <span class="row-label">Theme</span>
            <span class="seg">
              <button
                v-for="opt in (['system', 'light', 'dark'] as const)"
                :key="opt"
                type="button"
                class="seg-btn"
                :class="{ 'is-on': settings.theme === opt }"
                @click="onUpdateSettings({ theme: opt })"
              >{{ opt[0].toUpperCase() + opt.slice(1) }}</button>
            </span>
          </div>
          <div class="row-grid">
            <span class="row-label">Font size</span>
            <span class="seg font-seg">
              <button type="button" class="seg-btn" @click="bumpFontSize(-1)" title="Decrease (Ctrl--)">&minus;</button>
              <button type="button" class="seg-btn font-val" @click="resetFontSize" title="Reset to default (Ctrl-0)">{{ settings.font_size }}px</button>
              <button type="button" class="seg-btn" @click="bumpFontSize(1)" title="Increase (Ctrl-+)">+</button>
            </span>
          </div>

          <h3>Behaviour</h3>
          <div class="row-grid">
            <label for="follow-tail-default">Follow tail by default</label>
            <span class="control-cell">
              <input
                id="follow-tail-default"
                type="checkbox"
                :checked="settings.follow_tail_default"
                @change="(e: Event) => onUpdateSettings({ follow_tail_default: (e.target as HTMLInputElement).checked })"
              />
            </span>
          </div>

          <h3>Recent files</h3>
          <ul v-if="settings.recent_files.length > 0" class="recent-list">
            <li v-for="p in settings.recent_files" :key="p">
              <button type="button" class="open-btn" @click="openRecent(p)">{{ basename(p) }}</button>
              <span class="path">{{ p }}</span>
              <button type="button" class="forget-btn" @click="onForgetRecent(p)" title="Remove from list">&times;</button>
            </li>
          </ul>
          <p v-else class="muted">No recent files yet. Open a log to populate this list.</p>

          <h3>Advanced</h3>
          <div class="row-grid">
            <span class="row-label">Data folder</span>
            <span class="control-cell data-cell">
              <code class="data-path">{{ dataDir?.path ?? '(loading...)' }}</code>
              <span v-if="dataDir?.portable" class="badge">portable</span>
              <button type="button" class="seg-btn" @click="onOpenDataFolder">Open folder</button>
            </span>
          </div>
          <div class="reset-grid">
            <div class="row-grid">
              <span class="row-label">Session state</span>
              <span class="control-cell"><button type="button" class="seg-btn" @click="onResetData('session')">Reset</button></span>
            </div>
            <div class="row-grid">
              <span class="row-label">Settings</span>
              <span class="control-cell"><button type="button" class="seg-btn" @click="onResetData('settings')">Reset</button></span>
            </div>
            <div class="row-grid">
              <span class="row-label">Pattern overrides</span>
              <span class="control-cell"><button type="button" class="seg-btn" @click="onResetData('patterns')">Reset</button></span>
            </div>
            <div class="row-grid">
              <span class="row-label">Index cache</span>
              <span class="control-cell"><button type="button" class="seg-btn" @click="onResetData('index')">Clear</button></span>
            </div>
            <div class="row-grid">
              <span class="row-label">Everything</span>
              <span class="control-cell"><button type="button" class="seg-btn danger" @click="onResetData('all')">Reset all data</button></span>
            </div>
          </div>

          <p class="footer-note muted">
            Custom highlighting rules and automatic update checks are planned for a later milestone.
            Built-in highlights cover Java exceptions, <code>Caused by:</code>, stack frames, file paths and URLs.
          </p>
        </section>
      </div>
    </div>

    <div v-if="aboutOpen" class="modal-backdrop" @click.self="aboutOpen = false">
      <div class="modal about-modal" role="dialog" aria-label="About Clog">
        <header class="modal-head">
          <h2>About Clog {{ aboutInfo?.version ?? '...' }}</h2>
          <button type="button" class="modal-close" aria-label="Close about" @click="aboutOpen = false">&times;</button>
        </header>
        <section class="modal-body">
          <div class="about-hero">
            <img src="/clog-icon.png" alt="" class="about-icon" />
            <div>
              <h3 class="about-name">{{ aboutInfo?.name ?? 'Clog' }}</h3>
              <p class="about-tag muted">The Core log viewer: tail, search and filter your logs.</p>
              <p class="about-version">Version <code>{{ aboutInfo?.version ?? '...' }}</code></p>
            </div>
          </div>

          <h3>Credits</h3>
          <p>
            Built by Lewis Lane. Source and issue tracker on
            <button type="button" class="link-btn" @click="openUrl('https://github.com/lewster32/clog')">GitHub</button>.
          </p>

          <h3>Built with</h3>
          <ul class="dep-list">
            <li><strong>Rust</strong>: the engine, parser, search and tail loop.</li>
            <li><strong>Tauri</strong> v{{ aboutInfo?.tauri ?? '2.x' }}: <button type="button" class="link-btn" @click="openUrl('https://tauri.app/')">tauri.app</button></li>
            <li><strong>Vue 3</strong> + <strong>Vite</strong> + <strong>TypeScript</strong>: the UI shell.</li>
            <li><strong>@tanstack/vue-virtual</strong>: virtualised line viewer.</li>
            <li><strong>rayon</strong>: parallel record search.</li>
          </ul>

          <p class="footer-note muted">
            Licensed under the MIT license. See the repository for full third-party notices.
          </p>
        </section>
      </div>
    </div>
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
  border: 1px solid var(--border-default);
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
    display: inline-flex;
    align-items: center;
  }

  .app-icon {
    display: block;
    height: 22px;
    width: 22px;
    image-rendering: auto;
    object-fit: contain;
    pointer-events: none;
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

  .settings-btn {
    margin-left: 0.2rem;
    padding: 0.35rem 0.55rem;
    font-size: 1rem;
    line-height: 1;
  }

  .window-controls {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 0.15rem;
    -webkit-app-region: no-drag;

    .win-btn {
      background: transparent;
      color: var(--fg-muted);
      border: 0;
      padding: 0.3rem 0.7rem;
      font-size: 0.95rem;
      line-height: 1;
      cursor: pointer;
      border-radius: var(--radius-sm);

      &:hover { background: var(--bg-button-hover); color: var(--fg-default); }
      &.close:hover { background: var(--level-error); color: var(--fg-on-accent); }
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
  height: 100%;
  display: flex;
  justify-content: center;
  align-items: center;
  text-align: center;
  gap: .25em;
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

  .slot { display: flex; align-items: center; gap: 0.6rem; }
  .slot.right { margin-left: auto; gap: 1.5em; }
  .stat { color: var(--fg-muted); }

  .cache-hint {
    padding: 0.05rem 0.4rem;
    border-radius: var(--radius-sm);
    background: var(--bg-button);
    color: var(--fg-dim);
    font-size: 0.72rem;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .font-size-hint { color: var(--fg-dim); }

  .theme-toggle {
    background: transparent;
    color: var(--fg-muted);
    border: 1px solid var(--border-button);
    border-radius: var(--radius-sm);
    padding: 0.05rem 0.5rem;
    font-size: 0.85rem;
    line-height: 1.2;
    cursor: pointer;

    &.is-auto { opacity: 0.5; }
    &:hover { background: var(--bg-button-hover); color: var(--fg-default); opacity: 1; }
  }

  .pattern-status {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
  }
  .pattern-label strong { color: var(--fg-default); font-weight: 600; }
  .pattern-edit-btn {
    background: var(--bg-button);
    color: var(--fg-default);
    border: 1px solid var(--border-button);
    border-radius: var(--radius-sm);
    padding: 0.05rem 0.45rem;
    font-size: 0.72rem;
    line-height: 1.2;
    cursor: pointer;

    &:hover { background: var(--bg-button-hover); }
  }
}

.drop-overlay {
  position: fixed;
  inset: 0;
  pointer-events: none;
  background: color-mix(in srgb, var(--level-info) 18%, transparent);
  border: 3px dashed var(--level-info);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 250;

  .drop-hint {
    background: var(--bg-elevated);
    color: var(--fg-default);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    padding: 1rem 1.6rem;
    font-size: 1rem;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.4);
    display: flex;
    align-items: center;
    gap: 0.6rem;

    .arrow { color: var(--level-info); font-size: 1.4rem; }
  }
}

.modal-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 200;
}

.modal {
  width: min(720px, 92vw);
  max-height: 88vh;
  background: var(--bg-elevated);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
  display: flex;
  flex-direction: column;
  overflow: hidden;

  .modal-head {
    display: flex;
    align-items: center;
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--border-default);
    background: var(--bg-app);

    h2 { margin: 0; font-size: 1rem; }
  }

  .modal-close {
    margin-left: auto;
    background: transparent;
    color: var(--fg-default);
    border: 0;
    font-size: 1.4rem;
    line-height: 1;
    cursor: pointer;
  }

  .modal-body {
    padding: 1rem 1.2rem 1.5rem;
    overflow-y: auto;

    h3 {
      margin: 1.2rem 0 0.4rem;
      font-size: 0.95rem;
      border-bottom: 1px solid var(--border-default);
      padding-bottom: 0.25rem;
    }
    h3:first-of-type { margin-top: 0; }

    p.muted { color: var(--fg-muted); font-size: 0.85rem; margin: 0.4rem 0; }
    code { background: var(--bg-button); padding: 0.05rem 0.3rem; border-radius: 3px; font-family: var(--font-mono); }

    .row-grid {
      display: grid;
      grid-template-columns: 10rem 1fr;
      align-items: center;
      gap: 0.8rem;
      margin: 0.35rem 0;
    }
    .row-label { color: var(--fg-muted); font-size: 0.85rem; }
    .control-cell { display: inline-flex; align-items: center; gap: 0.5rem; min-width: 0; }

    .seg { display: inline-flex; gap: 0.3rem; }
    .seg-btn {
      background: var(--bg-button);
      color: var(--fg-default);
      border: 1px solid var(--border-button);
      border-radius: var(--radius-sm);
      padding: 0.3rem 0.7rem;
      font-size: 0.85rem;
      cursor: pointer;

      &.is-on { background: var(--fg-default); color: var(--bg-app); border-color: var(--fg-default); }
      &.danger { color: var(--level-error); border-color: var(--level-error); }
    }
    .font-seg .font-val { font-family: var(--font-mono); min-width: 3.5rem; text-align: center; }

    .recent-list {
      list-style: none;
      padding: 0;
      margin: 0.3rem 0 0;
      max-height: 14rem;
      overflow-y: auto;
      border: 1px solid var(--border-default);
      border-radius: var(--radius-sm);

      li {
        display: flex;
        align-items: center;
        gap: 0.5rem;
        padding: 0.3rem 0.5rem;
        border-bottom: 1px dashed var(--border-default);
        font-size: 0.82rem;

        &:last-child { border-bottom: 0; }

        .open-btn {
          background: transparent;
          border: 0;
          color: var(--level-info);
          cursor: pointer;
          padding: 0;
          font-weight: 600;
          flex: 0 0 auto;

          &:hover { text-decoration: underline; }
        }

        .path {
          color: var(--fg-dim);
          font-family: var(--font-mono);
          overflow: hidden;
          text-overflow: ellipsis;
          white-space: nowrap;
          flex: 1;
          min-width: 0;
        }

        .forget-btn {
          background: transparent;
          border: 0;
          color: var(--fg-dim);
          cursor: pointer;
          font-size: 1rem;
          line-height: 1;

          &:hover { color: var(--level-error); }
        }
      }
    }

    .data-cell {
      flex-wrap: wrap;

      .data-path {
        background: var(--bg-button);
        padding: 0.2rem 0.45rem;
        border-radius: var(--radius-sm);
        font-family: var(--font-mono);
        font-size: 0.8rem;
        color: var(--fg-default);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        min-width: 0;
        flex: 1;
      }

      .badge {
        padding: 0.05rem 0.4rem;
        border-radius: var(--radius-sm);
        background: var(--level-info);
        color: var(--bg-app);
        font-size: 0.7rem;
        text-transform: uppercase;
        letter-spacing: 0.04em;
      }
    }

    .reset-grid { margin-top: 0.4rem; display: flex; flex-direction: column; gap: 0; }
    .footer-note { margin-top: 1.2rem; padding-top: 0.6rem; border-top: 1px solid var(--border-default); font-size: 0.8rem; }
  }
}

.logo-btn {
  all: unset;
  border: none !important;
  background: transparent !important;
  padding: 0 !important;
  margin: 0 !important;
  -webkit-app-region: no-drag;

  &:hover .app-icon { filter: brightness(1.15); }
  &:focus-visible { outline: 1px solid var(--level-info); outline-offset: 2px; }
}

.about-modal {
  max-width: 500px;

  .about-hero {
    display: flex;
    gap: 1rem;
    align-items: center;
    margin-bottom: 0.4rem;
  }
  .about-icon { width: 64px; height: 64px; object-fit: contain; flex: 0 0 auto; }
  .about-name { margin: 0; font-size: 1.2rem; border: 0; padding: 0; }
  .about-tag { margin: 0.2rem 0 0.3rem; font-size: 0.9rem; }
  .about-version { margin: 0; font-size: 0.85rem; color: var(--fg-muted); code { color: var(--fg-default); font-family: var(--font-mono); } }
  .dep-list { padding-left: 1.2rem; font-size: 0.85rem; line-height: 1.5; }
  .link-btn {
    background: transparent;
    border: 0;
    color: var(--level-info);
    cursor: pointer;
    padding: 0;
    font: inherit;
    text-decoration: underline;
    text-underline-offset: 2px;

    &:hover { color: var(--fg-default); }
  }
}

.pattern-modal {
  width: min(720px, 92vw);

  .pattern-input-row {
    display: flex;
    gap: 0.4rem;
    margin: 0.6rem 0;

    .pat-input {
      flex: 1;
      min-width: 0;
      background: var(--bg-viewport);
      color: var(--fg-default);
      border: 1px solid var(--border-default);
      border-radius: var(--radius-sm);
      padding: 0.4rem 0.6rem;
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
  }

  .score, .muted, .pat-error { font-size: 0.85rem; margin: 0.3rem 0; }
  .muted { color: var(--fg-muted); }
  .pat-error { color: var(--fg-error); font-family: var(--font-mono); }
}
</style>
