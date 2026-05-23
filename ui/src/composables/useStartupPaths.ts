/**
 * CLI argv + single-instance forward handler.
 *
 * On first boot, drain `take_startup_paths` IPC for any file paths the
 * binary was launched with. While running, listen to `single-instance-paths`
 * events emitted by the tauri-plugin-single-instance callback so a second
 * invocation forwards its argv into this window as new tabs.
 *
 * Startup-path consumption is deliberately run AFTER session restore (the
 * caller controls the order) so a CLI file opens as an additional tab
 * rather than racing the restore.
 */

import { onBeforeUnmount } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

import type { Tab } from '../tab'

export function useStartupPaths(openPath: (path: string) => Promise<Tab | null>) {
  let unlistenSingleInstance: UnlistenFn | null = null

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

  onBeforeUnmount(() => {
    if (unlistenSingleInstance) { unlistenSingleInstance(); unlistenSingleInstance = null }
  })

  return { consumeStartupPaths, bindSingleInstance }
}
