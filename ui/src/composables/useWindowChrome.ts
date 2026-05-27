/**
 * Window chrome: maximize/restore tracking + the three title-bar buttons.
 * Owns the resize listener so the Maximize/Restore glyph stays in sync
 * with the OS window state.
 */

import { onBeforeUnmount, onMounted, ref } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import type { IpcError } from '../types'

export function useWindowChrome(onError: (msg: string) => void) {
  const appWindow = getCurrentWindow()
  const windowMaximized = ref(false)
  let unlistenResize: (() => void) | null = null

  async function refreshMaximized() {
    try {
      windowMaximized.value = await appWindow.isMaximized()
    } catch {
      windowMaximized.value = false
    }
  }

  function reportError(e: unknown) {
    onError((e as IpcError).message ?? String(e))
  }

  async function minimizeWindow() {
    try { await appWindow.minimize() } catch (e) { reportError(e) }
  }
  async function toggleMaximizeWindow() {
    try { await appWindow.toggleMaximize(); await refreshMaximized() } catch (e) { reportError(e) }
  }
  async function maximizeWindow() {
    try { await appWindow.maximize(); await refreshMaximized() } catch (e) { reportError(e) }
  }
  async function closeWindow() {
    try { await appWindow.close() } catch (e) { reportError(e) }
  }

  onMounted(async () => {
    void refreshMaximized()
    try {
      unlistenResize = await appWindow.onResized(() => void refreshMaximized())
    } catch {
      unlistenResize = null
    }
  })

  onBeforeUnmount(() => {
    if (unlistenResize) { unlistenResize(); unlistenResize = null }
  })

  return {
    windowMaximized,
    minimizeWindow,
    toggleMaximizeWindow,
    maximizeWindow,
    closeWindow,
  }
}
