<script setup lang="ts">
/**
 * Compact popover that surfaces both foreground and background palette
 * choices for a single highlight rule. Uses CSS anchor positioning so
 * the popover tracks its trigger across resize / scroll / layout shifts
 * without any JS reposition loop.
 *
 * Each instance gets a unique anchor name so multiple pickers on the
 * same screen don't collide. `position-try-fallbacks: flip-block`
 * lets the browser flip the popover above the trigger when there's no
 * room below.
 *
 * Closes on outside click and on Escape.
 */

import { onBeforeUnmount, onMounted, ref, useId } from 'vue'

import { USER_RULE_PALETTE } from '../types'

const props = defineProps<{
  /** Foreground palette key, or empty for "none". */
  colour: string
  /** Background palette key, or empty for "none". */
  background: string
}>()

const emit = defineEmits<{
  (e: 'update:colour', value: string): void
  (e: 'update:background', value: string): void
}>()

// Unique anchor name per instance. CSS dashed-ident form requires letters,
// digits, hyphens, underscores - `useId()` returns one of those alphabets.
const anchorName = `--cp-${useId().replace(/[^a-zA-Z0-9_-]/g, '-')}`

const open = ref(false)
const triggerRef = ref<HTMLElement | null>(null)
const popoverRef = ref<HTMLElement | null>(null)

function toggle() {
  open.value = !open.value
}

function pickFg(c: string) {
  emit('update:colour', c === props.colour ? '' : c)
}

function pickBg(c: string) {
  emit('update:background', c === props.background ? '' : c)
}

function onDocClick(e: MouseEvent) {
  if (!open.value) return
  const t = e.target as Node
  const inTrigger = triggerRef.value?.contains(t)
  const inPopover = popoverRef.value?.contains(t)
  if (!inTrigger && !inPopover) open.value = false
}

function onKey(e: KeyboardEvent) {
  if (e.key === 'Escape' && open.value) {
    open.value = false
    e.stopPropagation()
  }
}

onMounted(() => {
  document.addEventListener('mousedown', onDocClick, true)
  document.addEventListener('keydown', onKey, true)
})
onBeforeUnmount(() => {
  document.removeEventListener('mousedown', onDocClick, true)
  document.removeEventListener('keydown', onKey, true)
})
</script>

<template>
  <span class="cp-root">
    <button
      ref="triggerRef"
      type="button"
      class="cp-trigger"
      :style="{ anchorName }"
      :class="[
        colour ? `h-user-${colour}` : '',
        background ? `h-user-bg-${background}` : '',
      ]"
      :title="`fg: ${colour || 'none'} / bg: ${background || 'none'}`"
      @click="toggle"
    >Aa</button>

    <Teleport to="body">
      <div
        v-if="open"
        ref="popoverRef"
        class="cp-popover"
        aria-label="Colour picker"
        :style="{ positionAnchor: anchorName }"
      >
        <div class="cp-section">
          <span class="cp-palette">
            <button
              type="button"
              class="cp-swatch cp-none"
              :class="{ 'is-on': !colour }"
              title="No foreground colour"
              @click="emit('update:colour', '')"
            >&times;</button>
            <button
              v-for="c in USER_RULE_PALETTE"
              :key="`fg-${c}`"
              type="button"
              :title="c"
              :class="['cp-swatch', `h-user-${c}`, { 'is-on': colour === c }]"
              @click="pickFg(c)"
            >A</button>
          </span>
        </div>
        <div class="cp-section">
          <span class="cp-palette">
            <button
              type="button"
              class="cp-swatch cp-none"
              :class="{ 'is-on': !background }"
              title="No background"
              @click="emit('update:background', '')"
            >&times;</button>
            <button
              v-for="c in USER_RULE_PALETTE"
              :key="`bg-${c}`"
              type="button"
              :title="c"
              :class="['cp-swatch', `h-user-bg-${c}`, { 'is-on': background === c }]"
              @click="pickBg(c)"
            >&nbsp;</button>
          </span>
        </div>
      </div>
    </Teleport>
  </span>
</template>

<style scoped>
.cp-root {
  display: inline-block;
}

.cp-trigger {
  width: 2.2rem;
  height: 1.6rem;
  padding: 0;
  background: var(--bg-button);
  border: 1px solid var(--border-button);
  border-radius: var(--radius-sm);
  cursor: pointer;
  font-weight: 700;
  font-family: var(--font-mono);
  font-size: 0.85rem;
  line-height: 1;
  color: var(--fg-default);

  &:hover { background: var(--bg-button-hover); }
}
</style>

<!-- Unscoped: the popover is teleported to <body>, outside this
     component's scoped-CSS data attribute, and uses CSS anchor
     positioning which is referenced by the trigger's inline
     `anchor-name` style. -->
<style>
.cp-popover {
  /* Anchor positioning: pin to the trigger's bottom-left, flip above
     when there's no room below. The position-try-fallbacks lets the
     browser shift left/right too if the popover would overflow the
     viewport edge. */
  position: fixed;
  top: anchor(bottom);
  left: anchor(left);
  margin-top: 4px;
  position-try-fallbacks: flip-block, flip-inline, flip-block flip-inline;
  z-index: 300;

  display: flex;
  flex-direction: column;
  gap: 0.4rem;
  padding: 0.5rem 0.6rem;
  background: var(--bg-elevated);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
  min-width: 14rem;
}

.cp-section {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
}

.cp-palette {
  display: inline-flex;
  flex-wrap: wrap;
  gap: 0.25rem;

  .cp-swatch {
    width: 1.4rem;
    height: 1.4rem;
    padding: 0;
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    background: var(--bg-button);
    cursor: pointer;
    font-weight: 700;
    font-size: 0.75rem;
    line-height: 1;

    &.is-on { outline: 2px solid var(--accent); outline-offset: 1px; }
    &.cp-none { color: var(--fg-muted); }
  }
}
</style>
