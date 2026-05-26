/**
 * Update-banner state machine. Talks to the Rust shim (`check_for_update`,
 * `install_update_now`, `snooze_update`) and owns the banner's visibility
 * and progress fields.
 *
 * The banner mounts at the bottom of the window above the status bar and
 * has two flavours driven by `status.mode`:
 *   - `"installer"`: in-app install via the Tauri updater plugin.
 *   - `"portable"`: opens the GitHub release page in the default browser.
 *
 * State transitions:
 *   idle -> checking -> available | up-to-date | error
 *   available -> installing -> (process restarts) | error
 *   available -> dismissed (Later: this session only; x: snoozed via Rust)
 */

import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { openUrl } from '@tauri-apps/plugin-opener'

export interface UpdateStatus {
  available: boolean
  current_version: string
  available_version: string | null
  notes: string | null
  mode: 'installer' | 'portable'
  skipped_by_cadence: boolean
  snoozed: boolean
}

export type BannerPhase =
  | 'hidden'
  | 'available'
  | 'installing'
  | 'error'

const RELEASES_URL = 'https://github.com/Enigma-Interactive-UK/clog/releases/latest'

export function useUpdateBanner() {
  const status = ref<UpdateStatus | null>(null)
  const phase = ref<BannerPhase>('hidden')
  const errorMessage = ref<string | null>(null)
  const toast = ref<string | null>(null)

  const visible = computed(() => phase.value !== 'hidden')

  function clearToast() {
    toast.value = null
  }

  /**
   * Run a check. `force = false` is the on-launch silent check (subject to
   * the 24h cadence + 7d snooze in Rust); `force = true` is the user-driven
   * "Check for updates" action which bypasses both and surfaces a toast on
   * either outcome.
   */
  async function check(force: boolean) {
    try {
      const result = await invoke<UpdateStatus>('check_for_update', { force })

      // A silent check the Rust shim skipped (cadence or per-version
      // snooze) carries no fresh signal - it must not stomp on a banner
      // a prior forced check already raised.
      if (!force && result.skipped_by_cadence) {
        return
      }

      status.value = result

      if (result.available && result.available_version) {
        phase.value = 'available'
        errorMessage.value = null
      } else if (force) {
        // Only hide when this call actually contradicts what the banner
        // is showing - i.e. a forced check that came back "up to date".
        phase.value = 'hidden'
        toast.value = `You're on the latest version (${result.current_version}).`
      }
    } catch (e) {
      const msg = typeof e === 'string' ? e : (e as { message?: string })?.message ?? String(e)
      if (force) {
        toast.value = `Update check failed - see logs.`
        phase.value = 'hidden'
      }
      errorMessage.value = msg
    }
  }

  async function installNow() {
    if (!status.value?.available_version) return
    if (status.value.mode === 'portable') {
      await openReleasePage()
      return
    }
    phase.value = 'installing'
    errorMessage.value = null
    try {
      await invoke('install_update_now')
      // On success the Tauri plugin replaces the binary and triggers a
      // relaunch; this line is unlikely to ever run. If it does (e.g. the
      // user declined the installer's UAC equivalent), drop back to the
      // available banner so they can retry.
      phase.value = 'available'
    } catch (e) {
      const msg = typeof e === 'string' ? e : (e as { message?: string })?.message ?? String(e)
      errorMessage.value = msg
      phase.value = 'error'
    }
  }

  async function openReleasePage() {
    try {
      await openUrl(RELEASES_URL)
    } catch {
      // Browser opener failure is non-fatal: leave the banner up so the
      // user can copy the URL manually if needed.
    }
  }

  /** "x" button: persist a 7-day snooze on this specific version. */
  async function snoozeVersion() {
    const v = status.value?.available_version
    if (!v) {
      phase.value = 'hidden'
      return
    }
    try {
      await invoke('snooze_update', { version: v })
    } catch {
      // Snooze persistence failure is non-fatal; the dismissal still
      // takes effect for this session.
    }
    phase.value = 'hidden'
  }

  function dismissError() {
    phase.value = 'hidden'
    errorMessage.value = null
  }

  return {
    status,
    phase,
    visible,
    errorMessage,
    toast,
    clearToast,
    check,
    installNow,
    openReleasePage,
    snoozeVersion,
    dismissError,
    RELEASES_URL,
  }
}
