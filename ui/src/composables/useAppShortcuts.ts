/**
 * Global keyboard shortcuts wired to the document in capture phase.
 *
 *   Ctrl+T          new tab via file picker
 *   Ctrl+O          new tab via file picker (alias, matches context menu)
 *   Ctrl+W          close active tab
 *   Ctrl+Tab        cycle forward through tabs
 *   Ctrl+Shift+Tab  cycle backward through tabs
 *   Ctrl + / -      font size bump (delegated to useSettings)
 *   Ctrl 0          font size reset (delegated to useSettings)
 *   Ctrl I          toggle slow-request insights drawer for the active tab
 *   Ctrl F / Ctrl G suppressed (we own our own search bar)
 *
 * Capture phase is used so the shortcuts win against focused inputs
 * (the search box, the pattern editor).
 */

import { onBeforeUnmount, onMounted, type Ref } from 'vue'

import type { Tab } from '../tab'

export interface UseAppShortcutsOptions {
  tabs: Ref<Tab[]>
  activeTabId: Ref<number | null>
  activateTab: (id: number) => void
  closeTab: (id: number) => Promise<void>
  pickFile: () => Promise<void>
  handleFontShortcut: (ev: KeyboardEvent) => boolean
  toggleInsights: () => void
}

export function useAppShortcuts(opts: UseAppShortcutsOptions) {
  const { tabs, activeTabId, activateTab, closeTab, pickFile, handleFontShortcut, toggleInsights } = opts

  function suppressBrowserFind(ev: KeyboardEvent) {
    // F3 (and Shift+F3) trigger the webview "find next/previous" overlay on
    // their own, no modifier required.
    if (ev.key === 'F3') {
      ev.preventDefault()
      ev.stopPropagation()
      return
    }
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
      if (activeTabId.value !== null) {
        ev.preventDefault()
        void closeTab(activeTabId.value)
        return true
      }
    }
    if (k === 't' || k === 'o') {
      ev.preventDefault()
      void pickFile()
      return true
    }
    if (k === 'i' && !ev.shiftKey) {
      // Ctrl+I toggles the slow-request insights drawer. Needed because
      // the button lives in TabStrip, which is hidden in zen mode.
      ev.preventDefault()
      toggleInsights()
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

  onMounted(() => {
    globalThis.addEventListener('keydown', onGlobalKey, { capture: true })
  })
  onBeforeUnmount(() => {
    globalThis.removeEventListener('keydown', onGlobalKey, { capture: true })
  })
}
