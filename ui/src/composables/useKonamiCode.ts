/**
 * Konami-code detector: up up down down left right left right b a.
 *
 * Listens at the document level (bubble phase, no preventDefault) so the
 * sequence is observed without disturbing anything else that handles
 * arrow keys -- log navigation, focused inputs, etc. The match is
 * tolerant of partial restarts: a mismatched key that happens to be the
 * sequence's first key starts a fresh attempt from index 1.
 */
import { onBeforeUnmount, onMounted } from 'vue'

const SEQUENCE = [
  'ArrowUp', 'ArrowUp', 'ArrowDown', 'ArrowDown',
  'ArrowLeft', 'ArrowRight', 'ArrowLeft', 'ArrowRight',
  'b', 'a',
] as const

export function useKonamiCode(onMatch: () => void) {
  let idx = 0

  function onKey(ev: KeyboardEvent) {
    // Single-character keys (letters) come through as their literal
    // character with Shift applied -- normalise to lowercase so the
    // letter portion of the sequence is case-insensitive. Named keys
    // like 'ArrowUp' stay as-is.
    const key = ev.key.length === 1 ? ev.key.toLowerCase() : ev.key
    if (key === SEQUENCE[idx]) {
      idx++
      if (idx === SEQUENCE.length) {
        idx = 0
        onMatch()
      }
    } else {
      idx = key === SEQUENCE[0] ? 1 : 0
    }
  }

  onMounted(() => { globalThis.addEventListener('keydown', onKey) })
  onBeforeUnmount(() => { globalThis.removeEventListener('keydown', onKey) })
}
