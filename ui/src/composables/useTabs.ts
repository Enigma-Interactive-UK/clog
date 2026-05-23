/**
 * Tab list ownership: the reactive `tabs` array, the active tab pointer,
 * and the open/close/activate/pickFile lifecycle.
 *
 * Tabs are stored in a `shallowRef<Tab[]>` so each Tab's inner refs survive
 * (deep refs would auto-unwrap them; see .wolf/cerebrum.md). Path dedup on
 * openPath ensures multi-source opens (picker / drag-drop / CLI argv /
 * single-instance forward / recents) never spawn duplicates.
 */

import { computed, ref, shallowRef, type Ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { open as openDialog } from '@tauri-apps/plugin-dialog'

import { createTab, type Tab } from '../tab'
import type { IpcError, OpenedFile, RestoredFile, Settings } from '../types'

export interface UseTabsOptions {
  settings: Ref<Settings>
  onError: (msg: string) => void
}

export function useTabs({ settings, onError }: UseTabsOptions) {
  const tabs = shallowRef<Tab[]>([])
  const activeTabId = ref<number | null>(null)
  const busy = ref(false)
  let nextLocalTabId = 0

  const currentTab = computed<Tab | null>(() => {
    const id = activeTabId.value
    if (id === null) return null
    return tabs.value.find((t) => t.localId === id) ?? null
  })

  function tabByPath(path: string): Tab | null {
    return tabs.value.find((t) => t.file.value.path === path) ?? null
  }

  function activateTab(localId: number) {
    if (activeTabId.value === localId) return
    activeTabId.value = localId
    const t = tabs.value.find((t) => t.localId === localId)
    if (t) t.unread.value = false
  }

  async function openPath(path: string, restored: RestoredFile | null = null): Promise<Tab | null> {
    const existing = tabByPath(path)
    if (existing) {
      activateTab(existing.localId)
      return existing
    }
    if (busy.value) return null
    busy.value = true
    try {
      const opened = (await invoke('open_file', { path })) as OpenedFile
      const tab = createTab(
        ++nextLocalTabId,
        opened,
        { followTail: settings.value.follow_tail_default },
        { onError },
      )
      if (restored) tab.applyRestored(restored)
      tabs.value = [...tabs.value, tab]
      activeTabId.value = tab.localId
      void tab.startTail()
      if (tab.searchQuery.value.trim().length > 0) tab.scheduleSearch()
      return tab
    } catch (e) {
      onError((e as IpcError).message ?? String(e))
      return null
    } finally {
      busy.value = false
    }
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
        const replacement = newTabs[Math.max(0, idx - 1)]
        activeTabId.value = replacement?.localId ?? null
      }
    }
  }

  async function pickFile() {
    const selected = await openDialog({
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

  async function teardownAll() {
    for (const t of tabs.value) await t.teardown()
  }

  /**
   * Move `sourceId` so it sits immediately before (or after) `targetId`.
   * No-op if the move would leave the order unchanged, so the autosave
   * fingerprint doesn't fire on identity drops.
   */
  function reorderTab(sourceId: number, targetId: number, placeBefore: boolean) {
    if (sourceId === targetId) return
    const list = tabs.value.slice()
    const fromIdx = list.findIndex((t) => t.localId === sourceId)
    if (fromIdx < 0) return
    const [moved] = list.splice(fromIdx, 1)
    let toIdx = list.findIndex((t) => t.localId === targetId)
    if (toIdx < 0) return
    if (!placeBefore) toIdx += 1
    // Compare against the original order: if the source already sat where
    // the user is "dropping" it (same index after removal), bail to avoid a
    // spurious session save.
    if (toIdx === fromIdx) return
    list.splice(toIdx, 0, moved)
    tabs.value = list
  }

  return {
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
  }
}
