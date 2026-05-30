/**
 * Global custom context-menu state. One menu at a time; module-scoped
 * refs so any component can call `show()` / `hide()` without prop-drilling
 * a controller down the tree. App.vue mounts the single `<ContextMenu />`
 * surface and routes right-clicks into here.
 */
import { ref } from 'vue'
// Native clipboard via Tauri (Rust/arboard), NOT the WebView Clipboard API.
// readText() through the web API triggers a permission prompt in WebView2;
// the plugin reads the system clipboard from the backend with no prompt.
import { readText, writeText } from '@tauri-apps/plugin-clipboard-manager'

export type MenuAction = {
  kind: 'action'
  label: string
  onSelect: () => void
  accel?: string
  disabled?: boolean
  danger?: boolean
}
export type MenuSeparator = { kind: 'separator' }
export type MenuSubmenu = {
  kind: 'submenu'
  label: string
  children: MenuItem[]
  disabled?: boolean
}
export type MenuToggle = {
  kind: 'toggle'
  label: string
  checked: boolean
  onChange: (next: boolean) => void
  disabled?: boolean
}
export type MenuSlider = {
  kind: 'slider'
  label: string
  value: number
  min: number
  max: number
  step: number
  format?: (v: number) => string
  onInput: (next: number) => void
}
export type MenuItem = MenuAction | MenuSeparator | MenuSubmenu | MenuToggle | MenuSlider

const open = ref(false)
const x = ref(0)
const y = ref(0)
// Plain `ref` (not shallowRef) so sliders and toggles can mutate their
// own `value`/`checked` field while the menu is open and the DOM picks
// the change up reactively.
const items = ref<MenuItem[]>([])

// --- Clipboard items ------------------------------------------------------
//
// Restores the Cut/Copy/Paste affordances the native WebView2 menu used to
// provide. Built per right-click from the event target + current selection,
// so the actions operate on whatever the user actually clicked.

// Input types whose selectionStart/selectionEnd are safe to read. Chromium
// throws on those properties for number/email/date inputs, so we only treat
// the text-like, selection-supporting types as editable here.
const SELECTABLE_INPUT_TYPES = new Set([
  '', 'text', 'search', 'url', 'tel', 'password',
])

function editableTarget(
  el: EventTarget | null,
): HTMLInputElement | HTMLTextAreaElement | null {
  if (el instanceof HTMLTextAreaElement) {
    return !el.disabled && !el.readOnly ? el : null
  }
  if (el instanceof HTMLInputElement) {
    const ok = !el.disabled && !el.readOnly && SELECTABLE_INPUT_TYPES.has(el.type)
    return ok ? el : null
  }
  return null
}

async function writeClipboard(text: string): Promise<void> {
  if (!text) return
  try {
    await writeText(text)
  } catch {
    // best-effort.
  }
}

function replaceInInput(
  el: HTMLInputElement | HTMLTextAreaElement,
  text: string,
  start: number,
  end: number,
): void {
  el.focus()
  el.setRangeText(text, start, end, 'end')
  // v-model bindings only update on a real input event.
  el.dispatchEvent(new Event('input', { bubbles: true }))
}

export function buildClipboardItems(ev: MouseEvent): MenuItem[] {
  const editable = editableTarget(ev.target)
  if (editable) {
    const start = editable.selectionStart ?? 0
    const end = editable.selectionEnd ?? 0
    const hasSel = start !== end
    const selected = editable.value.slice(start, end)
    return [
      {
        kind: 'action',
        label: 'Cut',
        accel: 'Ctrl+X',
        disabled: !hasSel,
        onSelect: () => {
          void writeClipboard(selected).then(() => replaceInInput(editable, '', start, end))
        },
      },
      {
        kind: 'action',
        label: 'Copy',
        accel: 'Ctrl+C',
        disabled: !hasSel,
        onSelect: () => { void writeClipboard(selected) },
      },
      {
        kind: 'action',
        label: 'Paste',
        accel: 'Ctrl+V',
        onSelect: () => {
          void (async () => {
            try {
              const text = await readText()
              if (text) replaceInInput(editable, text, start, end)
            } catch {
              // best-effort -- clipboard may be empty or non-text.
            }
          })()
        },
      },
    ]
  }

  // Non-editable target: offer Copy only when there is a real selection.
  const sel = window.getSelection()?.toString() ?? ''
  if (sel.length > 0) {
    return [{
      kind: 'action',
      label: 'Copy',
      accel: 'Ctrl+C',
      onSelect: () => { void writeClipboard(sel) },
    }]
  }
  return []
}

export function useContextMenu() {
  function show(ev: { clientX: number; clientY: number }, list: MenuItem[]) {
    if (list.length === 0) return
    x.value = ev.clientX
    y.value = ev.clientY
    items.value = list
    open.value = true
  }
  function hide() {
    open.value = false
    items.value = []
  }
  return { open, x, y, items, show, hide }
}
