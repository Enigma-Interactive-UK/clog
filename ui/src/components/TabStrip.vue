<script setup lang="ts">
/**
 * Tab strip across the top of the app. Lists open tabs with a tail status
 * dot per tab (pulses when a non-active tab receives data), a close
 * button per tab, and a "new tab" button that delegates to the parent's
 * file picker. Tabs can be reordered by dragging; the parent's
 * useTabs.reorderTab mutates the array and the autosave fingerprint
 * watcher picks up the new order (it joins tabs by array index).
 *
 * The reorder uses mouse events rather than the HTML5 drag-and-drop API
 * because Tauri windows have `dragDropEnabled: true` by default, which
 * intercepts all native drag events at the OS level so that file drops
 * from Explorer can be received via the Tauri IPC. The side effect is
 * that HTML5 drag-and-drop inside the page never fires. Switching to a
 * mouse-driven drag sidesteps that interception entirely.
 */
import { onBeforeUnmount, ref } from 'vue'
import type { Tab } from '../tab'

defineProps<{
  tabs: Tab[]
  activeTabId: number | null
  insightsActive?: boolean
  insightsAvailable?: boolean
}>()

const emit = defineEmits<{
  (e: 'switch', localId: number): void
  (e: 'close', localId: number): void
  (e: 'new-tab'): void
  (e: 'reorder', sourceId: number, targetId: number, placeBefore: boolean): void
  (e: 'toggle-insights'): void
}>()

const tabStripEl = ref<HTMLElement | null>(null)

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

// --- Reorder via mouse drag ----------------------------------------------
// Three-phase state machine:
//   1. mousedown on a tab arms `pending` with start position + source id.
//   2. mousemove past DRAG_THRESHOLD px promotes pending -> active drag;
//      from then on the live cursor X picks a drop target.
//   3. mouseup commits if a drop target was chosen, otherwise the original
//      click (tab switch) goes through untouched.
// The threshold matters: without it, every click would start a "drag" and
// the tab-switch click handler would never fire because mouseup would land
// in the drag-finish path. 4px is the standard browser-text-drag threshold.
const DRAG_THRESHOLD = 4

interface PendingDrag { sourceId: number; startX: number; startY: number }

const pendingDrag = ref<PendingDrag | null>(null)
const dragSourceId = ref<number | null>(null)
const dragOverId = ref<number | null>(null)
const dropBefore = ref<boolean>(true)

function onTabMouseDown(ev: MouseEvent, localId: number) {
  if (ev.button !== 0) return
  pendingDrag.value = { sourceId: localId, startX: ev.clientX, startY: ev.clientY }
  globalThis.addEventListener('mousemove', onDocMouseMove)
  globalThis.addEventListener('mouseup', onDocMouseUp)
}

function onDocMouseMove(ev: MouseEvent) {
  const pending = pendingDrag.value
  if (!pending) return
  if (dragSourceId.value === null) {
    // Still pending: only promote to a real drag once the cursor has
    // moved past the threshold. Below the threshold the user's intent is
    // ambiguous (could still be a click), so we leave the click chain
    // intact.
    const dx = ev.clientX - pending.startX
    const dy = ev.clientY - pending.startY
    if (dx * dx + dy * dy < DRAG_THRESHOLD * DRAG_THRESHOLD) return
    dragSourceId.value = pending.sourceId
  }
  updateDropTarget(ev.clientX)
}

function updateDropTarget(clientX: number) {
  // Walk the live tab DOM elements and find which one contains clientX.
  // Using the DOM rather than a coordinate table because the tab strip can
  // scroll horizontally on overflow and DOMRect already accounts for that.
  const strip = tabStripEl.value
  if (!strip) return
  const tabEls = strip.querySelectorAll<HTMLElement>('.tab')
  for (const el of tabEls) {
    const rect = el.getBoundingClientRect()
    if (clientX >= rect.left && clientX <= rect.right) {
      const id = Number(el.dataset.localId)
      dragOverId.value = id
      dropBefore.value = clientX < rect.left + rect.width / 2
      return
    }
  }
  // Past the last tab: drop after it. Before the first: drop before it.
  if (tabEls.length > 0) {
    const first = tabEls[0].getBoundingClientRect()
    const last = tabEls[tabEls.length - 1].getBoundingClientRect()
    if (clientX < first.left) {
      dragOverId.value = Number(tabEls[0].dataset.localId)
      dropBefore.value = true
    } else if (clientX > last.right) {
      dragOverId.value = Number(tabEls[tabEls.length - 1].dataset.localId)
      dropBefore.value = false
    }
  }
}

function onDocMouseUp() {
  globalThis.removeEventListener('mousemove', onDocMouseMove)
  globalThis.removeEventListener('mouseup', onDocMouseUp)
  const source = dragSourceId.value
  const target = dragOverId.value
  if (source !== null && target !== null && source !== target) {
    emit('reorder', source, target, dropBefore.value)
  }
  resetDragState()
}

function resetDragState() {
  pendingDrag.value = null
  dragSourceId.value = null
  dragOverId.value = null
  dropBefore.value = true
}

onBeforeUnmount(() => {
  // Defensive: if the component unmounts mid-drag, take the listeners with us.
  globalThis.removeEventListener('mousemove', onDocMouseMove)
  globalThis.removeEventListener('mouseup', onDocMouseUp)
})
</script>

<template>
  <nav v-if="tabs.length > 0" ref="tabStripEl" class="tab-strip" aria-label="Open files">
    <ul class="tabs">
      <li
        v-for="t in tabs"
        :key="t.localId"
        class="tab"
        :class="{
          'is-active': t.localId === activeTabId,
          'has-unread': t.unread.value && t.localId !== activeTabId,
          'is-dragging': dragSourceId === t.localId,
          'drop-before': dragOverId === t.localId && dropBefore && dragSourceId !== t.localId,
          'drop-after': dragOverId === t.localId && !dropBefore && dragSourceId !== t.localId,
        }"
        :title="t.file.value.path"
        :data-local-id="t.localId"
        @mousedown="onMiddleClick($event, t.localId); onTabMouseDown($event, t.localId)"
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
          class="btn-dismiss tab-close"
          :title="`Close ${basename(t.file.value.path)}`"
          aria-label="Close tab"
          @click.stop="emit('close', t.localId)"
          @mousedown.stop
        >
          <svg class="dismiss-glyph" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
            <path d="M4 4 L12 12 M12 4 L4 12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" fill="none" />
          </svg>
        </button>
      </li>
    </ul>
    <button
      type="button"
      class="tab-new"
      title="Open a file in a new tab"
      aria-label="New tab"
      @click="emit('new-tab')"
    >+</button>
    <button
      v-if="insightsAvailable"
      type="button"
      class="insights-pull"
      :class="{ 'is-active': insightsActive }"
      :title="insightsActive ? 'Hide slow-request insights (Ctrl+I)' : 'Show slow-request insights (Ctrl+I)'"
      :aria-label="insightsActive ? 'Hide insights drawer' : 'Show insights drawer'"
      :aria-pressed="insightsActive"
      @click="emit('toggle-insights')"
    >
      <svg class="insights-glyph" viewBox="0 0 24 24" aria-hidden="true">
        <rect x="3" y="13" width="4" height="8" fill="currentColor" />
        <rect x="10" y="8" width="4" height="13" fill="currentColor" />
        <rect x="17" y="3" width="4" height="18" fill="currentColor" />
      </svg>
    </button>
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
  cursor: grab;

  &:active { cursor: grabbing; }

  /* Drag-and-drop visual state.
     - .is-dragging fades the source so the user sees what's being moved.
     - .drop-before / .drop-after paint a vertical accent bar on the edge
       where the dragged tab will land. Positioned absolutely so the bar
       doesn't reflow neighbouring tabs (which would jitter the drop
       target out from under the cursor). */
  &.is-dragging { opacity: 0.4; }

  &.drop-before::before,
  &.drop-after::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    width: 2px;
    background: var(--accent);
    pointer-events: none;
  }
  &.drop-before::before { left: -2px; }
  &.drop-after::after { right: -2px; }

  &.is-active {
    /* Match the viewport surface so the tab visually fuses with the
       body below. The 2px accent strip across the top picks up the
       level-info colour so the user can spot the active tab at a
       glance, and the box-shadow erases the strip's bottom border. */
    background: var(--bg-viewport);
    color: var(--fg-default);
    border-color: var(--border-default);
    border-top-color: var(--accent);
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
      outline: 1px solid var(--accent);
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
    /* Three states:
       - idle    (no .is-active):  grey, the tail task is not running
                                    (start_tail failed, or torn down).
       - tailing (.is-active):     brand accent orange, tail is alive on
                                    the backend and watching the file.
       - pulsing (.is-pulsing):    brief green flash overlaid on the
                                    tailing state when a delta lands, so
                                    the user sees "log just updated".
                                    Cleared 250ms later by tab.ts. */
    width: 0.5rem;
    height: 0.5rem;
    border-radius: 50%;
    background: var(--fg-dim);
    flex: 0 0 auto;
    transition: background 0.15s ease, box-shadow 0.15s ease;

    &.is-active { background: var(--accent); }
    &.is-pulsing {
      background: var(--level-all);
      box-shadow: 0 0 6px var(--level-all);
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
    /* Pill stretched to tab height so the hit area covers the full
       right edge -- the dismiss base handles colour + hover + focus. */
    padding: 0 0.45rem;
    align-self: stretch;
    border-radius: 0;
    font-size: 1rem;
  }

  &.has-unread:not(.is-active) .tab-name {
    color: var(--fg-default);
    font-weight: 600;
  }
}

/* Drawer pullout tab anchored to the right edge of the strip, above the
   minimap. margin-left: auto pushes it past the new-tab button to the
   far right; rounded only on the top-left corner so it reads as a tab
   pulling out from the right edge. */
.insights-pull {
  margin-left: auto;
  /* Counter the strip's 0.4rem right padding so the pullout sits flush
     against the window edge, aligning with the minimap below. */
  margin-right: -0.4rem;
  align-self: stretch;
  margin-top: 0.25rem;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0 0.7rem;
  background: var(--bg-app);
  color: var(--fg-muted);
  border: 1px solid var(--border-default);
  border-right: none;
  border-bottom: none;
  border-top: 2px solid transparent;
  border-radius: var(--radius-sm) 0 0 0;
  cursor: pointer;
  line-height: 1;

  &:hover {
    background: var(--bg-button-hover);
    color: var(--fg-default);
  }

  &.is-active {
    background: var(--bg-viewport);
    color: var(--fg-default);
    border-color: var(--border-default);
    border-right: none;
    border-top-color: var(--accent);
    box-shadow: 0 1px 0 0 var(--bg-viewport);
    z-index: 1;
  }

  &:focus-visible {
    outline: 1px solid var(--accent);
    outline-offset: -1px;
  }

  .insights-glyph {
    /* Snap to whole pixels so the bar chart strokes land on the device
       grid -- matches the icon-sizing approach used by AppHeader so the
       glyph tracks the user's font-size setting. */
    width: round(calc(var(--font-size-base) * 1.1), 1px);
    height: round(calc(var(--font-size-base) * 1.1), 1px);
    display: block;
    shape-rendering: crispEdges;
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
    border-top-color: var(--accent);
  }
}
</style>
