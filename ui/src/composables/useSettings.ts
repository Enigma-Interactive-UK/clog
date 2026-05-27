/**
 * Global settings, theme handling, and font-size scaling. Owns the
 * settings.json round trip and applies the resolved theme + font-size
 * tokens to `<html>` so the CSS custom property cascade follows the
 * user's choice without each component re-implementing it.
 *
 * Returns the reactive `settings` ref plus a small surface of methods
 * the UI binds to: theme cycling, font-size bumps, recent-files mgmt,
 * data-folder operations.
 */

import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { DataDirPayload, IpcError, Settings } from '../types'

type Theme = 'system' | 'light' | 'dark'

const THEME_GLYPH: Record<'light' | 'dark', string> = {
  light: '☀', // sun
  dark: '☽', // crescent moon
}

const THEME_LABEL: Record<Theme, string> = {
  system: 'Theme: auto (follows OS)',
  light: 'Theme: light',
  dark: 'Theme: dark',
}

function defaultSettings(): Settings {
  return {
    schema: 1,
    theme: 'system',
    font_size: 13,
    recent_files: [],
    follow_tail_default: true,
    colour_blind: false,
    minimap_heatmap_blend: 0,
    minimap_background_opacity: 0.5,
    speed_rail_enabled: true,
  }
}

export function useSettings() {
  const settings = ref<Settings>(defaultSettings())
  const dataDir = ref<DataDirPayload | null>(null)
  /** Bumped after every successful settings save so observers (e.g. the
   *  insights drawer) can refetch derived state without prop-drilling
   *  every individual field. */
  const settingsVersion = ref(0)

  const systemDarkMql =
    typeof globalThis !== 'undefined' && typeof globalThis.matchMedia === 'function'
      ? globalThis.matchMedia('(prefers-color-scheme: dark)')
      : null
  const systemPrefersDark = ref(!!systemDarkMql?.matches)

  function applyTheme(theme: Theme) {
    let wantDark = !!systemDarkMql?.matches
    if (theme === 'dark') wantDark = true
    else if (theme === 'light') wantDark = false
    document.documentElement.setAttribute('data-theme', wantDark ? 'dark' : 'light')
  }

  function applyColourBlind(on: boolean) {
    if (on) document.documentElement.dataset.colourBlind = 'on'
    else delete document.documentElement.dataset.colourBlind
  }

  function applyFontSize(px: number) {
    const clamped = Math.max(9, Math.min(24, Math.round(px)))
    document.documentElement.style.setProperty('--font-size-base', `${clamped}px`)
    // Row height tracks font size so larger sizes don't overflow their row.
    // 1.4x ratio lands at 18px for the default 13px font (matching the
    // historic constant) and at ~34px at 24px. Keep this formula in sync
    // with `rowHeight` in LogViewport.vue.
    document.documentElement.style.setProperty('--row-height', `${rowHeightForFontSize(clamped)}px`)
  }

  /** Single source of truth for the font-size -> row-height mapping. */
  function rowHeightForFontSize(fontSize: number): number {
    return Math.round(fontSize * 1.4)
  }

  const MONO_FONT_FALLBACK = 'Consolas, ui-monospace, monospace'
  function applyMonoFont(family?: string | null) {
    const name = (family ?? '').trim()
    if (name) {
      // Quote the family name so multi-word families ("JetBrains Mono")
      // parse as one token; escape embedded double-quotes defensively.
      const escaped = name.replaceAll('"', String.raw`\"`)
      const quoted = `"${escaped}"`
      document.documentElement.style.setProperty(
        '--font-mono',
        `${quoted}, ${MONO_FONT_FALLBACK}`,
      )
    } else {
      document.documentElement.style.removeProperty('--font-mono')
    }
  }

  async function loadSettings(): Promise<void> {
    try {
      const s = (await invoke('get_settings')) as Settings
      settings.value = s
      applyTheme(s.theme)
      applyFontSize(s.font_size)
      applyColourBlind(!!s.colour_blind)
      applyMonoFont(s.mono_font_family)
    } catch {
      applyTheme('system')
      applyFontSize(13)
      applyColourBlind(false)
      applyMonoFont(null)
    }
  }

  async function updateSettings(patch: Partial<Settings>): Promise<string | null> {
    try {
      const s = (await invoke('update_settings', { patch })) as Settings
      settings.value = s
      settingsVersion.value++
      if (patch.theme !== undefined) applyTheme(s.theme)
      if (patch.font_size !== undefined) applyFontSize(s.font_size)
      if (patch.colour_blind !== undefined) applyColourBlind(!!s.colour_blind)
      if (patch.mono_font_family !== undefined) applyMonoFont(s.mono_font_family)
      return null
    } catch (e) {
      return (e as IpcError).message ?? String(e)
    }
  }

  function onSystemThemeChange() {
    systemPrefersDark.value = !!systemDarkMql?.matches
    if (settings.value.theme === 'system') applyTheme('system')
  }

  function bumpFontSize(delta: number) {
    const target = settings.value.font_size + delta
    void updateSettings({ font_size: target })
  }

  function resetFontSize() {
    void updateSettings({ font_size: 13 })
  }

  const themeToggleGlyph = computed(() => {
    if (settings.value.theme === 'light') return THEME_GLYPH.light
    if (settings.value.theme === 'dark') return THEME_GLYPH.dark
    return systemPrefersDark.value ? THEME_GLYPH.dark : THEME_GLYPH.light
  })

  function cycleTheme() {
    // The status-bar button must always produce a visible change. Decide
    // by resolved appearance, not by stored value: if the current theme
    // renders the same as the OS preference, jump to the opposite
    // explicit theme; otherwise return to 'system'. This avoids the
    // dead-click cases where the stored value differs but the pixels
    // don't (e.g. theme='light' on a light OS, or theme='system' on a
    // light OS both resolve to light). See #5.
    const cur = settings.value.theme
    const sysIsDark = systemPrefersDark.value
    const resolvedDark = cur === 'dark' || (cur === 'system' && sysIsDark)
    const sysMatchesResolved = resolvedDark === sysIsDark
    const opposite: Theme = sysIsDark ? 'light' : 'dark'
    const next: Theme = sysMatchesResolved ? opposite : 'system'
    void updateSettings({ theme: next })
  }

  /** Returns true iff a font shortcut handled the event. */
  function handleFontShortcut(ev: KeyboardEvent): boolean {
    if (!(ev.ctrlKey || ev.metaKey) || ev.altKey || ev.shiftKey) return false
    if (ev.key === '+' || ev.key === '=') {
      ev.preventDefault()
      bumpFontSize(1)
      return true
    }
    if (ev.key === '-' || ev.key === '_') {
      ev.preventDefault()
      bumpFontSize(-1)
      return true
    }
    if (ev.key === '0') {
      ev.preventDefault()
      resetFontSize()
      return true
    }
    return false
  }

  async function refreshDataDir(): Promise<void> {
    try {
      dataDir.value = (await invoke('get_data_dir')) as DataDirPayload
    } catch {
      dataDir.value = null
    }
  }

  async function openDataFolder(): Promise<string | null> {
    try {
      await invoke('open_data_dir')
      return null
    } catch (e) {
      return (e as IpcError).message ?? String(e)
    }
  }

  async function forgetRecent(path: string): Promise<string | null> {
    try {
      settings.value = (await invoke('forget_recent', { path })) as Settings
      return null
    } catch (e) {
      return (e as IpcError).message ?? String(e)
    }
  }

  async function resetData(
    scope: 'settings' | 'session' | 'patterns' | 'index' | 'highlight' | 'all',
  ): Promise<string | null> {
    try {
      await invoke('reset_data', { req: { scope } })
      if (scope === 'settings' || scope === 'all') {
        await loadSettings()
      }
      return null
    } catch (e) {
      return (e as IpcError).message ?? String(e)
    }
  }

  onMounted(() => {
    systemDarkMql?.addEventListener?.('change', onSystemThemeChange)
  })
  onBeforeUnmount(() => {
    systemDarkMql?.removeEventListener?.('change', onSystemThemeChange)
  })

  return {
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
  }
}
