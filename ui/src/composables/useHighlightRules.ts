/**
 * Global + per-file highlight rule loading and engine wiring.
 *
 * The engine module carries one active rule set at a time. We rebuild that
 * set whenever any of these change:
 *   - the active file path (per-file rules attach to it),
 *   - the persisted global rule set,
 *   - the persisted per-file rule cache for the active path.
 *
 * Per-file rules are loaded lazily and cached by path so a tab switch back
 * to a previously-active file doesn't have to round-trip the disk again.
 */

import { ref, watch, type Ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'

import defaultRulesFile from '../highlight/default-rules.json'
import { setRules, type HighlightRule, type HighlightRulesFile as EngineRulesFile } from '../highlight/engine'
import { composeEffectiveRules } from '../highlight/user-rule'
import type {
  HighlightRulesFile,
  PerFileRulesFile,
  UserHighlightRule,
} from '../types'

const DEFAULT_RULES: HighlightRule[] = (defaultRulesFile as EngineRulesFile).rules

export interface UseHighlightRulesOptions {
  /** Path of the active tab's file, or null when no tab is open. */
  activePath: Ref<string | null>
}

export function useHighlightRules({ activePath }: UseHighlightRulesOptions) {
  const globalRules = ref<UserHighlightRule[]>([])
  const perFileCache = new Map<string, UserHighlightRule[]>()
  const activePerFileRules = ref<UserHighlightRule[]>([])

  async function refreshEngine() {
    setRules(composeEffectiveRules(DEFAULT_RULES, globalRules.value, activePerFileRules.value))
  }

  async function loadGlobal() {
    try {
      const file = (await invoke('get_highlight_rules')) as HighlightRulesFile
      globalRules.value = file.rules ?? []
    } catch {
      globalRules.value = []
    }
    await refreshEngine()
  }

  async function saveGlobal(rules: UserHighlightRule[]) {
    globalRules.value = rules
    const file: HighlightRulesFile = { schema: 1, rules }
    await invoke('save_highlight_rules', { rules: file })
    await refreshEngine()
  }

  async function loadPerFile(path: string): Promise<UserHighlightRule[]> {
    if (perFileCache.has(path)) return perFileCache.get(path)!
    try {
      const file = (await invoke('get_per_file_rules', { path })) as PerFileRulesFile
      const rules = file.rules ?? []
      perFileCache.set(path, rules)
      return rules
    } catch {
      perFileCache.set(path, [])
      return []
    }
  }

  async function savePerFile(path: string, rules: UserHighlightRule[]) {
    perFileCache.set(path, rules)
    const file: PerFileRulesFile = { schema: 1, path, rules }
    await invoke('save_per_file_rules', { path, rules: file })
    if (activePath.value === path) {
      activePerFileRules.value = rules
      await refreshEngine()
    }
  }

  async function forgetPerFile(path: string) {
    perFileCache.delete(path)
    await invoke('forget_per_file_rules', { path })
    if (activePath.value === path) {
      activePerFileRules.value = []
      await refreshEngine()
    }
  }

  /**
   * Drop the in-memory rule state so the next engine refresh runs against
   * an empty user-rule set. Used after a `reset_data` IPC removes the
   * underlying JSON files - the disk side is cleared by the backend, this
   * synchronises the JS side so the viewport stops applying the old rules.
   */
  async function clearAll() {
    globalRules.value = []
    perFileCache.clear()
    activePerFileRules.value = []
    await refreshEngine()
  }

  // Re-target per-file rules when the active tab changes.
  watch(activePath, async (p) => {
    if (!p) {
      activePerFileRules.value = []
    } else {
      activePerFileRules.value = await loadPerFile(p)
    }
    await refreshEngine()
  }, { immediate: false })

  return {
    globalRules,
    activePerFileRules,
    loadGlobal,
    saveGlobal,
    loadPerFile,
    savePerFile,
    forgetPerFile,
    refreshEngine,
    clearAll,
  }
}
