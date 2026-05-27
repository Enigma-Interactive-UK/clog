<script setup lang="ts">
/**
 * Title bar: app logo (opens About), Open button, Settings cog, and the
 * three window-control buttons (minimize / maximize-restore / close).
 *
 * Window-chrome state is owned by useWindowChrome so the resize listener
 * lives next to the maximize-tracking ref.
 */

import { useWindowChrome } from '../composables/useWindowChrome'

defineProps<{
  busy: boolean
  hasFile: boolean
}>()

const emit = defineEmits<{
  (e: 'pick-file'): void
  (e: 'open-settings'): void
  (e: 'open-about'): void
  (e: 'enter-zen'): void
  (e: 'error', msg: string): void
}>()

const { windowMaximized, minimizeWindow, toggleMaximizeWindow, maximizeWindow, closeWindow } = useWindowChrome(
  (msg) => emit('error', msg),
)

async function enterZenFullscreen() {
  // Maximise the OS window first, then ask the parent to turn zen on, so
  // the chrome unmounts onto a window that already fills the display.
  await maximizeWindow()
  emit('enter-zen')
}
</script>

<template>
  <header class="bar" data-tauri-drag-region>
    <h1 class="app-title">
      <button
        type="button"
        class="logo-btn"
        title="About Clog"
        @click="emit('open-about')"
      >
        <img src="/clog-icon.png" alt="" class="app-icon" />
      </button>
    </h1>
    <button
      type="button"
      class="icon-btn"
      :class="{ 'is-busy': busy }"
      :disabled="busy"
      :title="busy ? 'Reading...' : 'Open file...'"
      :aria-label="busy ? 'Reading file' : 'Open file'"
      @click="emit('pick-file')"
    >
      <svg class="win-glyph" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
        <path
          d="M2 4 L7 4 L8.5 5.5 L14 5.5 L14 12.5 L2 12.5 Z"
          stroke="currentColor"
          stroke-width="1.25"
          stroke-linejoin="round"
          fill="none"
        />
      </svg>
    </button>
    <button
      type="button"
      class="icon-btn"
      title="Settings"
      aria-label="Open settings"
      @click="emit('open-settings')"
    >
      <svg class="win-glyph" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
        <circle cx="3.5" cy="8" r="1.4" fill="currentColor" />
        <circle cx="8" cy="8" r="1.4" fill="currentColor" />
        <circle cx="12.5" cy="8" r="1.4" fill="currentColor" />
      </svg>
    </button>
    <span class="window-controls" :class="{ 'no-file': !hasFile }">
      <button
        type="button"
        class="icon-btn"
        title="Maximise and enter zen mode"
        aria-label="Maximise and enter zen mode"
        @click="enterZenFullscreen"
      >
        <svg class="win-glyph" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
          <path
            d="M3 6 L3 3 L6 3 M10 3 L13 3 L13 6 M13 10 L13 13 L10 13 M6 13 L3 13 L3 10"
            stroke="currentColor"
            stroke-width="1.5"
            stroke-linecap="round"
            stroke-linejoin="round"
            fill="none"
          />
        </svg>
      </button>
      <button type="button" class="icon-btn" title="Minimise" aria-label="Minimise" @click="minimizeWindow">
        <svg class="win-glyph" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
          <path d="M4 8 L12 8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" fill="none" />
        </svg>
      </button>
      <button
        type="button"
        class="icon-btn"
        :title="windowMaximized ? 'Restore' : 'Maximise'"
        :aria-label="windowMaximized ? 'Restore' : 'Maximise'"
        @click="toggleMaximizeWindow"
      >
        <svg v-if="windowMaximized" class="win-glyph" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
          <path d="M5 5 L5 3 L13 3 L13 11 L11 11" stroke="currentColor" stroke-width="1.25" fill="none" stroke-linejoin="miter" />
          <rect x="3" y="5" width="8" height="8" stroke="currentColor" stroke-width="1.25" fill="none" />
        </svg>
        <svg v-else class="win-glyph" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
          <rect x="4" y="4" width="8" height="8" stroke="currentColor" stroke-width="1.25" fill="none" />
        </svg>
      </button>
      <button type="button" class="icon-btn close" title="Close" aria-label="Close" @click="closeWindow">
        <svg class="dismiss-glyph" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
          <path d="M4 4 L12 12 M12 4 L4 12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" fill="none" />
        </svg>
      </button>
    </span>
  </header>
</template>

<style scoped>
.bar {
  display: flex;
  align-items: center;
  padding: 0.2rem 0.75rem;
  border-bottom: 1px solid var(--border-default);
  flex-wrap: wrap;

  h1 {
    margin: 0;
    font-size: 1rem;
    letter-spacing: 0.02em;
    display: inline-flex;
    align-items: center;
    margin-right: .5rem;
  }

  .app-icon {
    display: block;
    height: round(calc(var(--font-size-base) * 1.4), 1px);
    width: round(calc(var(--font-size-base) * 1.4), 1px);
    image-rendering: auto;
    object-fit: contain;
    pointer-events: none;
  }

  /* Shared icon-button look for everything in the bar (Open, Settings,
     fullscreen, minimise, maximise/restore, close). Fixed-size boxes so
     the glyphs all sit in identical bounds regardless of stroke shape.
     Sized to feel comfortable as a click target without dominating the
     bar -- roughly Windows title-bar proportions, scaled down. */
  .icon-btn {
    background: transparent;
    color: var(--fg-muted);
    border: 0;
    padding: 0;
    /* Sized off --font-size-base directly so the icon buttons (and the
       glyphs nested inside) track the user's font setting. The settings
       shortcut writes --font-size-base on the root, and these calc()s
       pick it up so everything grows in lockstep. */
    width: round(calc(var(--font-size-base) * 2.2), 1px);
    height: round(calc(var(--font-size-base) * 1.7), 1px);
    line-height: 1;
    cursor: pointer;
    border-radius: var(--radius-sm);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    -webkit-app-region: no-drag;

    &:hover:not(:disabled) { background: var(--bg-button-hover); color: var(--fg-default); }
    &.close:hover { background: var(--level-error); color: var(--fg-on-accent); }
    &:focus-visible { outline: 1px solid var(--accent); outline-offset: -1px; }
    &:disabled { cursor: progress; }
    &.is-busy .win-glyph { animation: icon-busy-pulse 1.1s ease-in-out infinite; }

    .win-glyph,
    .dismiss-glyph {
      /* Snap to whole pixels so the SVG's viewBox maps cleanly to the
         device grid -- prevents the fractional-pixel blur that creeps
         in when font-size scales the glyph to a fractional px size. */
      width: round(calc(var(--font-size-base) * 1.05), 1px);
      height: round(calc(var(--font-size-base) * 1.05), 1px);
      display: block;
      /* geometricPrecision keeps curves (folder, cog) smooth while
         still aligning straight strokes much better than the default. */
      shape-rendering: geometricPrecision;
    }
  }

  .window-controls {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
  }
}

@keyframes icon-busy-pulse {
  0%, 100% { opacity: 0.5; }
  50% { opacity: 1; }
}

.logo-btn {
  all: unset;
  border: none !important;
  background: transparent !important;
  padding: 0 !important;
  margin: 0 !important;
  -webkit-app-region: no-drag;

  &:hover .app-icon { filter: brightness(1.15); }
  &:focus-visible { outline: 1px solid var(--accent); outline-offset: 2px; }
}
</style>
