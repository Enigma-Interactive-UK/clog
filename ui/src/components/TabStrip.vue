<script setup lang="ts">
/**
 * Tab strip across the top of the app. Lists open tabs with a tail status
 * dot per tab (pulses when a non-active tab receives data), a close
 * button per tab, and a "new tab" button that delegates to the parent's
 * file picker. Tabs are presented in their order in the array; this
 * component does not handle reordering -- that's deferred polish.
 */
import type { Tab } from '../tab'

defineProps<{
  tabs: Tab[]
  activeTabId: number | null
}>()

const emit = defineEmits<{
  (e: 'switch', localId: number): void
  (e: 'close', localId: number): void
  (e: 'new-tab'): void
}>()

function basename(p: string): string {
  const m = p.match(/[^\\/]+$/)
  return m ? m[0] : p
}

function onMiddleClick(ev: MouseEvent, localId: number) {
  // Middle-click closes the tab, browser-tab convention.
  if (ev.button === 1) {
    ev.preventDefault()
    emit('close', localId)
  }
}
</script>

<template>
  <nav v-if="tabs.length > 0" class="tab-strip" aria-label="Open files">
    <ul class="tabs">
      <li
        v-for="t in tabs"
        :key="t.localId"
        class="tab"
        :class="{
          'is-active': t.localId === activeTabId,
          'has-unread': t.unread.value && t.localId !== activeTabId,
        }"
        :title="t.file.value.path"
        @mousedown="onMiddleClick($event, t.localId)"
      >
        <button
          type="button"
          class="tab-body"
          :aria-pressed="t.localId === activeTabId"
          @click="emit('switch', t.localId)"
        >
          <span
            class="tail-dot"
            :class="{
              'is-active': t.tailing.value,
              'is-pulsing': t.tailPulse.value,
            }"
            :aria-label="t.tailing.value ? 'Tailing' : 'Idle'"
          />
          <span class="tab-name">{{ basename(t.file.value.path) }}</span>
          <span v-if="t.unread.value && t.localId !== activeTabId" class="unread-dot" aria-hidden="true" />
        </button>
        <button
          type="button"
          class="tab-close"
          :title="`Close ${basename(t.file.value.path)}`"
          aria-label="Close tab"
          @click.stop="emit('close', t.localId)"
        >&times;</button>
      </li>
    </ul>
    <button
      type="button"
      class="tab-new"
      title="Open a file in a new tab"
      aria-label="New tab"
      @click="emit('new-tab')"
    >+</button>
  </nav>
</template>

<style scoped>
.tab-strip {
  display: flex;
  flex: 0 0 auto;
  align-items: stretch;
  background: var(--bg-elevated);
  border-bottom: 1px solid var(--border-default);
  padding: 0 0.4rem;
  gap: 0.2rem;
  min-height: 2rem;
  overflow-x: auto;
  scrollbar-width: thin;
}

.tabs {
  display: flex;
  align-items: stretch;
  margin: 0;
  padding: 0;
  list-style: none;
  gap: 0.2rem;
}

.tab {
  display: inline-flex;
  align-items: stretch;
  border: 1px solid var(--border-default);
  border-bottom: none;
  /* Reserve space for an accent strip so the active state can swap it in
     without the row jumping by 2px. Transparent on inactive tabs. */
  border-top: 2px solid transparent;
  border-radius: var(--radius-sm) var(--radius-sm) 0 0;
  background: var(--bg-app);
  color: var(--fg-muted);
  font-family: var(--font-mono);
  font-size: 0.8rem;
  max-width: 24rem;
  margin-top: 0.25rem;
  position: relative;

  &.is-active {
    /* Match the viewport surface so the tab visually fuses with the
       body below. The 2px accent strip across the top picks up the
       level-info colour so the user can spot the active tab at a
       glance, and the box-shadow erases the strip's bottom border. */
    background: var(--bg-viewport);
    color: var(--fg-default);
    border-color: var(--border-default);
    border-top-color: var(--level-info);
    box-shadow: 0 1px 0 0 var(--bg-viewport);
    z-index: 1;

    .tab-name { font-weight: 600; }
  }

  &:hover:not(.is-active) {
    background: var(--bg-button-hover);
  }

  .tab-body {
    background: transparent;
    border: none;
    padding: 0.3rem 0.5rem 0.3rem 0.6rem;
    cursor: pointer;
    color: inherit;
    font: inherit;
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 0;

    &:focus-visible {
      outline: 1px solid var(--level-info);
      outline-offset: -1px;
    }
  }

  .tab-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .tail-dot {
    width: 0.5rem;
    height: 0.5rem;
    border-radius: 50%;
    background: var(--fg-dim);
    flex: 0 0 auto;
    transition: background 0.15s ease;

    &.is-active { background: var(--level-info); }
    &.is-pulsing {
      background: var(--level-warn);
      box-shadow: 0 0 6px var(--level-warn);
    }
  }

  .unread-dot {
    width: 0.4rem;
    height: 0.4rem;
    border-radius: 50%;
    background: var(--level-warn);
    flex: 0 0 auto;
    margin-left: 0.1rem;
  }

  .tab-close {
    background: transparent;
    border: none;
    color: var(--fg-dim);
    padding: 0 0.45rem;
    cursor: pointer;
    font-size: 1rem;
    line-height: 1;

    &:hover {
      color: var(--fg-default);
      background: var(--bg-button-hover);
    }
  }

  &.has-unread:not(.is-active) .tab-name {
    color: var(--fg-default);
    font-weight: 600;
  }
}

/* Shaped like a tab and seated alongside them so it visually reads as
   "one more slot at the end". Border-bottom is omitted (same as .tab) so
   it attaches to the strip's bottom border. */
.tab-new {
  background: transparent;
  border: 1px dashed var(--border-button);
  border-bottom: none;
  /* Match .tab's 2px top reserve so the bottoms line up. */
  border-top: 1px dashed var(--border-button);
  border-radius: var(--radius-sm) var(--radius-sm) 0 0;
  color: var(--fg-muted);
  padding: 0 0.7rem;
  margin-top: 0.25rem;
  font-size: 1rem;
  line-height: 1;
  cursor: pointer;
  align-self: stretch;

  &:hover {
    background: var(--bg-button-hover);
    color: var(--fg-default);
    border-style: solid;
    border-top-color: var(--level-info);
  }
}
</style>
