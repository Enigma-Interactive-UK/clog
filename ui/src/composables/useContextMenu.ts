/**
 * Global custom context-menu state. One menu at a time; module-scoped
 * refs so any component can call `show()` / `hide()` without prop-drilling
 * a controller down the tree. App.vue mounts the single `<ContextMenu />`
 * surface and routes right-clicks into here.
 */
import { ref } from 'vue'

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
