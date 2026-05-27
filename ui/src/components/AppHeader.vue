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
  (e: 'error', msg: string): void
}>()

const { windowMaximized, minimizeWindow, toggleMaximizeWindow, closeWindow } = useWindowChrome(
  (msg) => emit('error', msg),
)
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
    <button :disabled="busy" @click="emit('pick-file')">
      {{ busy ? 'Reading...' : 'Open file...' }}
    </button>
    <button
      type="button"
      class="settings-btn"
      title="Settings"
      aria-label="Open settings"
      @click="emit('open-settings')"
    >&#9881;</button>
    <span class="window-controls" :class="{ 'no-file': !hasFile }">
      <button type="button" class="win-btn" title="Minimise" aria-label="Minimise" @click="minimizeWindow">
        <svg class="win-glyph" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
          <path d="M4 8 L12 8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" fill="none" />
        </svg>
      </button>
      <button
        type="button"
        class="win-btn"
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
      <button type="button" class="win-btn close" title="Close" aria-label="Close" @click="closeWindow">
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
  gap: 0.6rem;
  padding: 0.2rem 0.75rem;
  border-bottom: 1px solid var(--border-default);
  flex-wrap: wrap;

  h1 {
    margin: 0;
    font-size: 1rem;
    letter-spacing: 0.02em;
    display: inline-flex;
    align-items: center;
  }

  .app-icon {
    display: block;
    height: 18px;
    width: 18px;
    image-rendering: auto;
    object-fit: contain;
    pointer-events: none;
  }

  button {
    background: var(--bg-button);
    color: var(--fg-default);
    border: 1px solid var(--border-button);
    padding: 0.15rem 0.7rem;
    border-radius: var(--radius-sm);
    font-size: 0.85rem;
    line-height: 1.2;
    cursor: pointer;

    &:hover:not(:disabled) { background: var(--bg-button-hover); }
    &:disabled { opacity: 0.6; cursor: progress; }
  }

  .settings-btn {
    margin-left: 0.2rem;
    padding: 0.15rem 0.45rem;
    font-size: 0.95rem;
    line-height: 1;
  }

  .window-controls {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 0.15rem;
    -webkit-app-region: no-drag;

    .win-btn {
      /* Fixed-size boxes so the minimise dash, maximise square and close
         cross all sit in identical bounds regardless of glyph width. The
         glyph is flex-centred inside. Sized to feel comfortable as a
         click target without dominating the bar -- roughly Windows
         title-bar proportions, scaled down. */
      background: transparent;
      color: var(--fg-muted);
      border: 0;
      padding: 0;
      width: 2.2rem;
      height: 1.6rem;
      font-size: 0.95rem;
      line-height: 1;
      cursor: pointer;
      border-radius: var(--radius-sm);
      display: inline-flex;
      align-items: center;
      justify-content: center;
      font-family: var(--font-sans);

      &:hover { background: var(--bg-button-hover); color: var(--fg-default); }
      &.close:hover { background: var(--level-error); color: var(--fg-on-accent); }
      &:focus-visible { outline: 1px solid var(--accent); outline-offset: -1px; }

      .win-glyph {
        width: 12px;
        height: 12px;
        display: block;
      }
    }
  }
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
