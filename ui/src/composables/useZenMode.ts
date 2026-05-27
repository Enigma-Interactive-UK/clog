/**
 * Zen mode - hides the app chrome so the log records own the viewport.
 *
 * State is in-memory only (no persistence; resets on app mount).
 * F11 toggles. Esc exits (when no input/textarea/contenteditable is focused).
 * The exit pill component calls toggle() directly.
 *
 * The decision logic lives in a pure helper so it can be unit-tested
 * without a DOM (vitest runs in node env in this project).
 */

import { onBeforeUnmount, onMounted, ref } from 'vue'

export type ZenKeyAction = 'toggle' | 'exit' | 'noop'

export interface DecideZenKeyArgs {
  key: string
  zen: boolean
  inputFocused: boolean
}

export function decideZenKeyAction(args: DecideZenKeyArgs): ZenKeyAction {
  if (args.key === 'F11') return 'toggle'
  if (args.key === 'Escape' && args.zen && !args.inputFocused) return 'exit'
  return 'noop'
}

function isTextInputFocused(): boolean {
  const el = document.activeElement as HTMLElement | null
  if (!el) return false
  const tag = el.tagName
  if (tag === 'INPUT' || tag === 'TEXTAREA') return true
  if (el.isContentEditable) return true
  return false
}

export function useZenMode() {
  const zen = ref(false)

  function enter() { zen.value = true }
  function exit() { zen.value = false }
  function toggle() { zen.value = !zen.value }

  function onKey(ev: KeyboardEvent) {
    const action = decideZenKeyAction({
      key: ev.key,
      zen: zen.value,
      inputFocused: isTextInputFocused(),
    })
    if (action === 'noop') return
    // Suppress any default the webview might attach to F11 (fullscreen
    // in some contexts) and any propagation to per-component listeners.
    ev.preventDefault()
    ev.stopPropagation()
    if (action === 'toggle') toggle()
    else if (action === 'exit') exit()
  }

  onMounted(() => {
    globalThis.addEventListener('keydown', onKey, { capture: true })
  })
  onBeforeUnmount(() => {
    globalThis.removeEventListener('keydown', onKey, { capture: true })
  })

  return { zen, enter, exit, toggle }
}
