<script setup lang="ts">
/**
 * App orchestrator. Composes the tab list, session save/restore,
 * single-instance forwarding, drag-drop handling, global shortcuts,
 * window chrome, and the rotation toast. Per-tab state lives in Tab
 * objects (see ./tab.ts) -- the viewport, search bar, tail and search
 * channels all flow through those.
 */
import { computed, onBeforeUnmount, onMounted, provide, ref, useTemplateRef } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWebview } from '@tauri-apps/api/webview'

import type { UserHighlightRule } from './types'

import AboutModal from './components/AboutModal.vue'
import AppHeader from './components/AppHeader.vue'
import ContextMenu from './components/ContextMenu.vue'
import DropOverlay from './components/DropOverlay.vue'
import LogViewport from './components/LogViewport.vue'
import PatternModal from './components/PatternModal.vue'
import SearchBar from './components/SearchBar.vue'
import SettingsModal from './components/SettingsModal.vue'
import StatusBar from './components/StatusBar.vue'
import TabStrip from './components/TabStrip.vue'

import { useContextMenu, type MenuItem, type MenuSlider, type MenuToggle } from './composables/useContextMenu'

import { useAppShortcuts } from './composables/useAppShortcuts'
import { useHighlightRules } from './composables/useHighlightRules'
import { useSession } from './composables/useSession'
import { useSettings } from './composables/useSettings'
import { useStartupPaths } from './composables/useStartupPaths'
import { useTabs } from './composables/useTabs'

const error = ref<string | null>(null)
const settingsOpen = ref(false)
const aboutOpen = ref(false)
const patternOpen = ref(false)
const dragHover = ref(false)

const viewportRef = useTemplateRef<InstanceType<typeof LogViewport>>('viewportRef')
const aboutRef = useTemplateRef<InstanceType<typeof AboutModal>>('aboutRef')

const {
  settings,
  settingsVersion,
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

// Make the version counter available to the insights drawer (deep in the
// LogViewport tree) so it can refetch effective thresholds whenever any
// settings save fires - no prop drilling through LogViewport required.
provide('settingsVersion', settingsVersion)
// Same rationale for the settings ref itself: the minimap heatmap blend,
// minimap canvas opacity, and speed-rail enabled toggle all read it and
// repaint, no prop-drilling needed.
provide('settings', settings)

const {
  tabs,
  activeTabId,
  currentTab,
  busy,
  openPath,
  activateTab,
  closeTab,
  pickFile,
  teardownAll,
  reorderTab,
} = useTabs({
  settings,
  onError: (msg) => { error.value = msg },
})

const activePath = computed<string | null>(() => currentTab.value?.file.value.path ?? null)

const {
  globalRules,
  activePerFileRules,
  loadGlobal: loadGlobalRules,
  saveGlobal: saveGlobalRules,
  savePerFile: savePerFileRules,
  forgetPerFile: forgetPerFileRules,
  clearAll: clearAllRules,
} = useHighlightRules({ activePath })

async function onSaveGlobalRules(rules: UserHighlightRule[]) {
  try { await saveGlobalRules(rules) } catch (e) { error.value = String(e) }
}

async function onSavePerFileRules(path: string, rules: UserHighlightRule[]) {
  try { await savePerFileRules(path, rules) } catch (e) { error.value = String(e) }
}

async function onForgetPerFileRules(path: string) {
  try { await forgetPerFileRules(path) } catch (e) { error.value = String(e) }
}

async function onForgetPattern() {
  if (!currentTab.value) return
  const path = currentTab.value.file.value.path
  try {
    await invoke('forget_pattern_override', { path })
    patternOpen.value = false
  } catch (e) {
    error.value = String(e)
  }
}

const { restoreSession } = useSession({
  tabs,
  activeTabId,
  openPath,
  setActiveTabId: (id) => { activeTabId.value = id },
})

const { consumeStartupPaths, bindSingleInstance } = useStartupPaths(openPath)

useAppShortcuts({
  tabs,
  activeTabId,
  activateTab,
  closeTab,
  pickFile,
  handleFontShortcut,
})

// --- Modal triggers -------------------------------------------------------

async function openAbout() {
  aboutOpen.value = true
  await aboutRef.value?.ensureLoaded()
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

async function onResetData(scope: 'settings' | 'session' | 'patterns' | 'index' | 'highlight' | 'all') {
  const err = await resetData(scope)
  if (err) { error.value = err; return }
  // The IPC removed the JSON files on disk, but useHighlightRules still
  // holds the previously-loaded rules in memory. Synchronise the JS side
  // so the viewport actually reflects the wipe.
  if (scope === 'highlight' || scope === 'all') {
    await clearAllRules()
  }
}

// --- Custom right-click context menu -------------------------------------
//
// Replaces the default WebView2 menu everywhere in the window. Universal
// items (Recent files, Settings) always appear; minimap-specific items
// appear when the click lands on the minimap canvas or marker rail. The
// menu surface clamps itself to the viewport via CSS anchor positioning
// (see ContextMenu.vue), so submenus and the menu itself can't spill
// over the app edges.

const { show: showContextMenu } = useContextMenu()

function basenameOf(p: string): string {
  const i = Math.max(p.lastIndexOf('\\'), p.lastIndexOf('/'))
  return i >= 0 ? p.slice(i + 1) : p
}

function buildRecentFilesSubmenu(): MenuItem {
  const recents = settings.value.recent_files ?? []
  if (recents.length === 0) {
    return {
      kind: 'submenu',
      label: 'Recent files',
      children: [{ kind: 'action', label: '(no recent files)', onSelect: () => {}, disabled: true }],
    }
  }
  return {
    kind: 'submenu',
    label: 'Recent files',
    children: recents.map<MenuItem>((p) => ({
      kind: 'action',
      label: basenameOf(p),
      onSelect: () => { void openPath(p) },
    })),
  }
}

function buildUniversalItems(): MenuItem[] {
  return [
    { kind: 'action', label: 'Open file...', accel: 'Ctrl+O', onSelect: () => { void pickFile() } },
    buildRecentFilesSubmenu(),
    { kind: 'separator' },
    { kind: 'action', label: 'Settings...', onSelect: () => { void openSettings() } },
  ]
}

function buildMinimapItems(): MenuItem[] {
  const blendItem: MenuSlider = {
    kind: 'slider',
    label: 'Level heatmap blend',
    value: settings.value.minimap_heatmap_blend ?? 0,
    min: 0,
    max: 1,
    step: 0.01,
    format: (v) => `${Math.round(v * 100)}%`,
    onInput: (v) => {
      blendItem.value = v
      void onUpdateSettings({ minimap_heatmap_blend: v })
    },
  }
  const opacityItem: MenuSlider = {
    kind: 'slider',
    label: 'Opacity',
    value: settings.value.minimap_background_opacity ?? 0.5,
    min: 0,
    max: 1,
    step: 0.01,
    format: (v) => `${Math.round(v * 100)}%`,
    onInput: (v) => {
      opacityItem.value = v
      void onUpdateSettings({ minimap_background_opacity: v })
    },
  }
  const speedRailItem: MenuToggle = {
    kind: 'toggle',
    label: 'Show speed rail',
    checked: settings.value.speed_rail_enabled !== false,
    onChange: (next) => {
      speedRailItem.checked = next
      void onUpdateSettings({ speed_rail_enabled: next })
    },
  }
  return [blendItem, opacityItem, speedRailItem]
}

function onAppContextMenu(ev: MouseEvent) {
  // Inner elements that handle their own right-click (bookmark pin,
  // line-number gutter, cluster popover) call preventDefault + stop
  // propagation, so this listener never sees those events. For
  // everything else we replace the WebView2 default with our menu.
  ev.preventDefault()
  const target = ev.target as HTMLElement | null
  const inMinimap = !!target?.closest('.minimap, .marker-rail')

  const items: MenuItem[] = []
  if (inMinimap) {
    items.push(...buildMinimapItems(), { kind: 'separator' })
  }
  items.push(...buildUniversalItems())
  showContextMenu({ clientX: ev.clientX, clientY: ev.clientY }, items)
}

// --- Hit nav: SearchBar emits, we drive the viewport ---------------------

function onNextHit() { viewportRef.value?.scrollToCurrentHit() }
function onPrevHit() { viewportRef.value?.scrollToCurrentHit() }

function onToggleInsights() {
  const t = currentTab.value
  if (!t) return
  t.insightsOpen.value = !t.insightsOpen.value
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

// --- Lifecycle -----------------------------------------------------------

const appWebview = getCurrentWebview()
let unlistenDragDrop: UnlistenFn | null = null

onMounted(() => {
  void (async () => {
    await loadSettings()
    await loadGlobalRules()
    try {
      unlistenDragDrop = await appWebview.onDragDropEvent(onDragDropEvent)
    } catch {
      unlistenDragDrop = null
    }
    await bindSingleInstance()
    await restoreSession()
    // Startup-path argv handling happens AFTER session restore so a CLI
    // file opens as an additional tab rather than racing with the restore.
    await consumeStartupPaths()
  })()
})

onBeforeUnmount(() => {
  if (unlistenDragDrop) { unlistenDragDrop(); unlistenDragDrop = null }
  void teardownAll()
})
</script>

<template>
  <main class="shell" @contextmenu="onAppContextMenu">
    <AppHeader
      :busy="busy"
      :has-file="!!currentTab"
      @pick-file="pickFile"
      @open-settings="openSettings"
      @open-about="openAbout"
      @error="(msg) => (error = msg)"
    />

    <TabStrip
      :tabs="tabs"
      :active-tab-id="activeTabId"
      :insights-active="currentTab?.insightsOpen.value ?? false"
      :insights-available="!!currentTab"
      @switch="activateTab"
      @close="closeTab"
      @new-tab="pickFile"
      @reorder="reorderTab"
      @toggle-insights="onToggleInsights"
    />

    <section v-if="error" class="error" role="alert">
      <span class="error-msg">{{ error }}</span>
      <button
        type="button"
        class="btn-dismiss error-dismiss"
        aria-label="Dismiss error"
        title="Dismiss"
        @click="error = null"
      >
        <svg class="dismiss-glyph" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
          <path d="M4 4 L12 12 M12 4 L4 12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" fill="none" />
        </svg>
      </button>
    </section>

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

    <StatusBar
      :tab="currentTab"
      :settings="settings"
      :theme-toggle-glyph="themeToggleGlyph"
      :theme-label="THEME_LABEL"
      @cycle-theme="cycleTheme"
      @open-pattern="patternOpen = true"
    />

    <DropOverlay :visible="dragHover" />

    <PatternModal
      v-if="patternOpen && currentTab"
      :tab="currentTab"
      :per-file-rules="activePerFileRules"
      @close="patternOpen = false"
      @forget-pattern="onForgetPattern"
      @save-per-file-rules="onSavePerFileRules"
      @forget-per-file-rules="onForgetPerFileRules"
    />

    <SettingsModal
      v-if="settingsOpen"
      :settings="settings"
      :data-dir="dataDir"
      :global-rules="globalRules"
      @close="settingsOpen = false"
      @update="onUpdateSettings"
      @bump-font="bumpFontSize"
      @reset-font="resetFontSize"
      @open-recent="openRecent"
      @forget-recent="onForgetRecent"
      @open-data-folder="onOpenDataFolder"
      @reset-data="onResetData"
      @save-global-rules="onSaveGlobalRules"
    />

    <AboutModal
      ref="aboutRef"
      :open="aboutOpen"
      @close="aboutOpen = false"
    />

    <ContextMenu />
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
  padding: 0.5rem 0.5rem 0.5rem 0.8rem;
  background: var(--bg-error);
  border: 1px solid var(--border-error);
  border-radius: var(--radius-sm);
  color: var(--fg-error);
  display: flex;
  align-items: flex-start;
  gap: 0.6rem;

  .error-msg {
    flex: 1;
    min-width: 0;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .error-dismiss {
    flex: 0 0 auto;
    width: 1.6rem;
    height: 1.6rem;
    font-size: 1.1rem;
    /* Sit on the error palette: dim error-red at rest, brighten on hover.
       The .btn-dismiss base hover would land on --bg-button-hover which
       reads as "neutral" against the red banner; override here. */
    color: var(--fg-error);
    opacity: 0.7;

    &:hover {
      background: color-mix(in srgb, var(--level-error) 22%, transparent);
      color: var(--fg-error);
      opacity: 1;
    }
  }
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
</style>
