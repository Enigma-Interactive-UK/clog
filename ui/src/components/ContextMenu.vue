<script setup lang="ts">
/**
 * Custom right-click context menu surface. Renders the items in
 * useContextMenu(); supports actions, separators, toggles, sliders and
 * one level of submenu. Closes on outside-click, Escape, or after an
 * action fires.
 *
 * Positioning uses native CSS anchor positioning: a 0x0 anchor element
 * is parked at the cursor, and `position-area` + `position-try-fallbacks`
 * keep the menu inside the viewport (which the `.shell` fills, so this
 * is equivalent to staying inside the app). Submenus use the parent
 * item as their anchor.
 */
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import {
  useContextMenu,
  type MenuItem,
  type MenuSlider,
  type MenuSubmenu,
  type MenuToggle,
} from '../composables/useContextMenu'

const { open, x, y, items, hide } = useContextMenu()

const rootEl = ref<HTMLElement | null>(null)
const subEl = ref<HTMLElement | null>(null)
const submenuOpenIdx = ref<number | null>(null)

let hoverTimer: ReturnType<typeof setTimeout> | null = null

function clearHoverTimer() {
  if (hoverTimer) {
    clearTimeout(hoverTimer)
    hoverTimer = null
  }
}

// The menu surface is mounted once and only its content is v-if'd on
// `open`, so component-local view state survives a hide/show cycle.
// Reset the open-submenu index (and any pending hover timer) every time
// the menu closes, otherwise a submenu left open when the menu lost
// focus reappears already-expanded on the next right-click. Fixes #6.
watch(open, (isOpen) => {
  if (!isOpen) {
    clearHoverTimer()
    submenuOpenIdx.value = null
  }
})

function isSubmenu(it: MenuItem): it is MenuSubmenu {
  return it.kind === 'submenu'
}

function onItemEnter(idx: number, it: MenuItem) {
  clearHoverTimer()
  if (isSubmenu(it) && !it.disabled) {
    hoverTimer = setTimeout(() => { submenuOpenIdx.value = idx }, 120)
  } else {
    submenuOpenIdx.value = null
  }
}

function onItemLeave() {
  clearHoverTimer()
}

function openSubmenu(idx: number) {
  submenuOpenIdx.value = idx
}

function selectAction(fn: () => void) {
  try { fn() } finally { hide() }
}

// Mutate the menu item itself (which is a reactive proxy because it
// lives in items.value) so the slider/checkbox re-renders, THEN fire
// the user callback that pushes the value through to the rest of the
// app. The captured reference in the App.vue closures is the plain
// object, so a reverse approach (mutate-in-callback) wouldn't trigger
// reactivity.
function onSlider(it: MenuSlider, ev: Event) {
  const next = Number((ev.target as HTMLInputElement).value)
  it.value = next
  it.onInput(next)
}

function onToggle(it: MenuToggle) {
  const next = !it.checked
  it.checked = next
  it.onChange(next)
}

function onDocPointerDown(ev: PointerEvent) {
  const root = rootEl.value
  if (!root) return
  const target = ev.target as Node | null
  if (target && (root.contains(target) || subEl.value?.contains(target))) return
  hide()
}

function onKey(ev: KeyboardEvent) {
  if (!open.value) return
  if (ev.key === 'Escape') {
    ev.preventDefault()
    hide()
  }
}

onMounted(() => {
  document.addEventListener('pointerdown', onDocPointerDown, true)
  document.addEventListener('keydown', onKey)
  window.addEventListener('blur', hide)
  window.addEventListener('resize', hide)
})

onBeforeUnmount(() => {
  document.removeEventListener('pointerdown', onDocPointerDown, true)
  document.removeEventListener('keydown', onKey)
  window.removeEventListener('blur', hide)
  window.removeEventListener('resize', hide)
  clearHoverTimer()
})

function fmt(it: { value: number; format?: (v: number) => string }): string {
  return it.format ? it.format(it.value) : String(it.value)
}
</script>

<template>
  <template v-if="open">
    <!-- 0x0 element parked at the cursor; the menu anchors to this. -->
    <div
      class="cm-cursor-anchor"
      aria-hidden="true"
      :style="{ left: `${x}px`, top: `${y}px` }"
    />

    <div
      ref="rootEl"
      class="cm"
      role="menu"
      @contextmenu.prevent.stop
    >
      <template v-for="(it, idx) in items" :key="idx">
        <hr v-if="it.kind === 'separator'" class="cm-sep" />

        <button
          v-else-if="it.kind === 'action'"
          type="button"
          class="cm-row cm-action"
          :class="{ 'is-disabled': it.disabled, 'is-danger': it.danger }"
          :disabled="it.disabled"
          role="menuitem"
          @mouseenter="onItemEnter(idx, it)"
          @mouseleave="onItemLeave"
          @click="selectAction(it.onSelect)"
        >
          <span class="cm-label">{{ it.label }}</span>
          <span v-if="it.accel" class="cm-accel">{{ it.accel }}</span>
        </button>

        <button
          v-else-if="it.kind === 'toggle'"
          type="button"
          class="cm-row cm-toggle"
          :class="{ 'is-disabled': it.disabled, 'is-checked': it.checked }"
          :disabled="it.disabled"
          role="menuitemcheckbox"
          :aria-checked="it.checked"
          @mouseenter="onItemEnter(idx, it)"
          @mouseleave="onItemLeave"
          @click="onToggle(it)"
        >
          <span class="cm-check" aria-hidden="true">
            <svg v-if="it.checked" viewBox="0 0 12 12">
              <path d="M2.5 6.5 L5 9 L9.5 3.5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" fill="none" />
            </svg>
          </span>
          <span class="cm-label">{{ it.label }}</span>
        </button>

        <div
          v-else-if="it.kind === 'slider'"
          class="cm-row cm-slider"
          @mouseenter="onItemEnter(idx, it)"
          @mouseleave="onItemLeave"
          @contextmenu.prevent.stop
        >
          <div class="cm-slider-head">
            <span class="cm-label">{{ it.label }}</span>
            <span class="cm-slider-val">{{ fmt(it) }}</span>
          </div>
          <input
            type="range"
            class="cm-slider-input"
            :min="it.min"
            :max="it.max"
            :step="it.step"
            :value="it.value"
            @input="(e) => onSlider(it, e)"
            @pointerdown.stop
            @click.stop
          />
        </div>

        <button
          v-else-if="it.kind === 'submenu'"
          type="button"
          class="cm-row cm-submenu"
          :class="{ 'is-disabled': it.disabled, 'is-open': submenuOpenIdx === idx }"
          :disabled="it.disabled"
          role="menuitem"
          :aria-haspopup="true"
          :aria-expanded="submenuOpenIdx === idx"
          :style="submenuOpenIdx === idx ? { 'anchor-name': '--cm-sub-anchor' } : undefined"
          @mouseenter="onItemEnter(idx, it)"
          @mouseleave="onItemLeave"
          @click="openSubmenu(idx)"
        >
          <span class="cm-label">{{ it.label }}</span>
          <span class="cm-arrow" aria-hidden="true">
            <svg viewBox="0 0 12 12">
              <path d="M4 3 L8 6 L4 9" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" fill="none" />
            </svg>
          </span>
        </button>
      </template>
    </div>

    <div
      v-if="submenuOpenIdx !== null && isSubmenu(items[submenuOpenIdx])"
      ref="subEl"
      class="cm cm-sub"
      role="menu"
      @contextmenu.prevent.stop
    >
      <template
        v-for="(child, cIdx) in (items[submenuOpenIdx] as MenuSubmenu).children"
        :key="cIdx"
      >
        <hr v-if="child.kind === 'separator'" class="cm-sep" />

        <button
          v-else-if="child.kind === 'action'"
          type="button"
          class="cm-row cm-action"
          :class="{ 'is-disabled': child.disabled, 'is-danger': child.danger }"
          :disabled="child.disabled"
          role="menuitem"
          @click="selectAction(child.onSelect)"
        >
          <span class="cm-label">{{ child.label }}</span>
          <span v-if="child.accel" class="cm-accel">{{ child.accel }}</span>
        </button>

        <button
          v-else-if="child.kind === 'toggle'"
          type="button"
          class="cm-row cm-toggle"
          :class="{ 'is-disabled': child.disabled, 'is-checked': child.checked }"
          :disabled="child.disabled"
          role="menuitemcheckbox"
          :aria-checked="child.checked"
          @click="onToggle(child)"
        >
          <span class="cm-check" aria-hidden="true">
            <svg v-if="child.checked" viewBox="0 0 12 12">
              <path d="M2.5 6.5 L5 9 L9.5 3.5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" fill="none" />
            </svg>
          </span>
          <span class="cm-label">{{ child.label }}</span>
        </button>

        <div
          v-else-if="child.kind === 'slider'"
          class="cm-row cm-slider"
          @contextmenu.prevent.stop
        >
          <div class="cm-slider-head">
            <span class="cm-label">{{ child.label }}</span>
            <span class="cm-slider-val">{{ fmt(child) }}</span>
          </div>
          <input
            type="range"
            class="cm-slider-input"
            :min="child.min"
            :max="child.max"
            :step="child.step"
            :value="child.value"
            @input="(e) => onSlider(child, e)"
            @pointerdown.stop
            @click.stop
          />
        </div>
      </template>
    </div>
  </template>
</template>

<style scoped>
/* Cursor anchor: 0x0 element placed at the click point. The menu uses
   it as its position-anchor; `position-area` + `position-try-fallbacks`
   below handle clamping the menu inside the viewport so it never spills
   over the app edges. */
.cm-cursor-anchor {
  position: fixed;
  width: 0;
  height: 0;
  pointer-events: none;
  anchor-name: --cm-cursor;
}

.cm {
  position: fixed;
  z-index: 200;
  min-width: 13rem;
  max-width: 22rem;
  background: var(--bg-elevated);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  padding: 0.25rem;
  box-shadow: 0 8px 22px rgba(0, 0, 0, 0.45);
  font-size: 0.85rem;
  color: var(--fg-default);
  user-select: none;

  /* Anchor to the cursor. Default: open down-right; flip up / left /
     both as needed to stay inside the viewport. The 4px margin keeps
     the menu off the app border. */
  position-anchor: --cm-cursor;
  position-area: span-bottom span-right;
  margin: 4px;
  position-try-fallbacks:
    flip-block,
    flip-inline,
    flip-block flip-inline;
}

/* Submenu: anchored to the active parent item (which gets
   `anchor-name: --cm-sub-anchor` inline). Opens to the right of the
   item, top-aligned. Flips to the left first if it would spill, then
   flips block direction if it would spill vertically. */
.cm-sub {
  position-anchor: --cm-sub-anchor;
  position-area: span-bottom right;
  margin: 0 2px;
  position-try-fallbacks:
    flip-inline,
    flip-block,
    flip-inline flip-block;
}

.cm-sep {
  height: 1px;
  border: 0;
  background: var(--border-default);
  margin: 0.25rem 0.1rem;
}

.cm-row {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  width: 100%;
  padding: 0.3rem 0.55rem;
  background: transparent;
  border: 0;
  border-radius: var(--radius-sm);
  color: var(--fg-default);
  font: inherit;
  text-align: left;
  cursor: default;

  &:hover:not(.is-disabled):not(.cm-slider) {
    background: var(--bg-button-hover);
  }

  &.is-disabled {
    opacity: 0.45;
    cursor: default;
  }

  &.is-danger {
    color: var(--level-error);
  }
}

.cm-label {
  flex: 1;
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.cm-accel {
  flex: 0 0 auto;
  font-family: var(--font-mono);
  font-size: 0.75rem;
  color: var(--fg-muted);
}

.cm-arrow {
  flex: 0 0 auto;
  display: inline-flex;
  width: 0.85rem;
  height: 0.85rem;
  color: var(--fg-muted);

  svg { width: 100%; height: 100%; }
}

.cm-submenu.is-open {
  background: var(--bg-button-hover);
}

.cm-check {
  flex: 0 0 auto;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 0.95rem;
  height: 0.95rem;
  color: var(--accent);

  svg { width: 100%; height: 100%; }
}

.cm-slider {
  flex-direction: column;
  align-items: stretch;
  gap: 0.25rem;
  padding: 0.4rem 0.55rem 0.5rem;
  cursor: default;

  &:hover { background: transparent; }
}

.cm-slider-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 0.5rem;
}

.cm-slider-val {
  font-family: var(--font-mono);
  font-size: 0.75rem;
  color: var(--fg-muted);
}

.cm-slider-input {
  width: 100%;
  accent-color: var(--accent);
}
</style>
