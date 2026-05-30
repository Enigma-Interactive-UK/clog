/**
 * Multi-tab session save/restore + the autosave watcher.
 *
 * The autosave watcher fingerprints each tab's persisted knobs into a
 * single string so one watch covers every per-tab change without 30
 * separate watchers. A 400ms debounce coalesces rapid edits (scroll,
 * typing in the search box).
 *
 * `sessionRestoreInFlight` gates the autosave so the act of restoring
 * doesn't immediately overwrite the session with half-applied state.
 */

import { onBeforeUnmount, ref, watch, type Ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'

import type { Tab } from '../tab'
import type { Session } from '../types'

export interface UseSessionOptions {
  tabs: Ref<Tab[]>
  activeTabId: Ref<number | null>
  openPath: (path: string, restored?: import('../types').RestoredFile | null) => Promise<Tab | null>
  setActiveTabId: (id: number | null) => void
}

export function useSession({ tabs, activeTabId, openPath, setActiveTabId }: UseSessionOptions) {
  const sessionRestoreInFlight = ref(false)
  const sessionSaveTimer = ref<number | null>(null)

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
      let openedCount = 0
      for (const r of stored) {
        const tab = await openPath(r.path, r)
        if (tab) openedCount++
      }
      // Active tab index is from the original stored list; clamp it against
      // however many tabs actually opened (some paths may have been pruned
      // below because their files were moved or deleted).
      const active = Math.min(tabs.value.length - 1, Math.max(0, sess.active_tab))
      if (tabs.value[active]) setActiveTabId(tabs.value[active].localId)
      // If any stored path failed to open (file moved/deleted/locked), persist
      // the pruned tab list immediately so the failure doesn't recur next
      // launch. The autosave watcher is gated by sessionRestoreInFlight, so
      // the cleanup would otherwise wait for the next user-driven knob change.
      if (openedCount < stored.length) {
        await invoke('save_session', { session: captureSession() }).catch(() => {})
      }
    } catch {
      // Top-level read failure: drop to empty state silently. The
      // recent-files list still holds the breadcrumbs.
    } finally {
      sessionRestoreInFlight.value = false
    }
  }

  // Coarse fingerprint over every tab so any per-tab knob change schedules
  // a debounced save without N separate watchers.
  watch(
    () => tabs.value.map((t) => `${t.file.value.path}|${t.followTail.value}|${t.searchMode.value}|${t.searchQuery.value}|${t.searchCaseSensitive.value}|${t.filterMode.value}|${Object.entries(t.levelAllow.value).filter(([, v]) => v).map(([k]) => k).join(',')}|tg:${Object.entries(t.threadGroupAllow.value).filter(([, v]) => v).map(([k]) => k).join(',')}|${t.scrollTop.value}|bm:${t.bookmarks.value.size}|cm:${t.collapseMode.value}|me:${t.manuallyExpanded.value.size}|mc:${t.manuallyCollapsed.value.size}|tr:${t.truncateBefore.value}:${t.truncateAfter.value}`).join('||') + '#' + String(activeTabId.value),
    () => {
      if (sessionRestoreInFlight.value) return
      scheduleSessionSave()
    },
  )

  onBeforeUnmount(() => {
    if (sessionSaveTimer.value !== null) globalThis.clearTimeout(sessionSaveTimer.value)
  })

  return {
    sessionRestoreInFlight,
    captureSession,
    scheduleSessionSave,
    restoreSession,
  }
}
