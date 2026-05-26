<script setup lang="ts">
/**
 * Non-modal banner that surfaces an available update. Sits at the bottom
 * of the shell, above the status bar. Two layouts:
 *   - installer mode: [What's new] [Update now] [Later] [x]
 *   - portable mode:  [Download]   [Later] [x]
 *
 * State is owned by useUpdateBanner; this component is presentation only.
 */

import { computed } from 'vue'
import type { UpdateStatus, BannerPhase } from '../composables/useUpdateBanner'

const props = defineProps<{
  status: UpdateStatus | null
  phase: BannerPhase
  errorMessage: string | null
}>()

const emit = defineEmits<{
  (e: 'install'): void
  (e: 'download'): void
  (e: 'open-notes'): void
  (e: 'snooze'): void
  (e: 'dismiss-error'): void
}>()

const version = computed(() => props.status?.available_version ?? '')
const isPortable = computed(() => props.status?.mode === 'portable')
const installing = computed(() => props.phase === 'installing')
const erroring = computed(() => props.phase === 'error')

const headline = computed(() => {
  if (erroring.value) {
    return `Update failed: ${props.errorMessage ?? 'unknown error'}`
  }
  if (installing.value) {
    return `Downloading Clog ${version.value}...`
  }
  if (props.status?.notes) {
    return `Clog ${version.value} is available - ${props.status.notes}`
  }
  return `Clog ${version.value} is available.`
})
</script>

<template>
  <output
    class="update-banner"
    :class="{ erroring }"
  >
    <span class="msg">{{ headline }}</span>
    <span class="actions">
      <template v-if="erroring">
        <button type="button" class="action" @click="emit('dismiss-error')">Dismiss</button>
      </template>
      <template v-else-if="installing">
        <span class="installing-indicator" aria-hidden="true">...</span>
      </template>
      <template v-else>
        <button
          v-if="!isPortable && status?.notes !== undefined"
          type="button"
          class="action subtle"
          title="Open the release notes"
          @click="emit('open-notes')"
        >What's new</button>
        <button
          v-if="isPortable"
          type="button"
          class="action primary"
          title="Open the GitHub release page"
          @click="emit('download')"
        >Download</button>
        <button
          v-else
          type="button"
          class="action primary"
          title="Download and install"
          @click="emit('install')"
        >Update now</button>
        <button
          type="button"
          class="action close"
          title="Snooze this version for 7 days"
          aria-label="Snooze"
          @click="emit('snooze')"
        >
          <svg viewBox="0 0 16 16" aria-hidden="true" focusable="false">
            <path d="M4 4 L12 12 M12 4 L4 12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" fill="none" />
          </svg>
        </button>
      </template>
    </span>
  </output>
</template>

<style scoped>
.update-banner {
  display: flex;
  align-items: center;
  gap: 0.8rem;
  padding: 0.45rem 1rem;
  background: var(--accent);
  border-top: 1px solid var(--accent-strong);
  color: var(--fg-on-accent);
  font-size: 0.85rem;

  &.erroring {
    border-top-color: var(--level-error);
    background: var(--bg-error);
    color: var(--fg-error);
  }

  .msg {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .actions {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    flex: 0 0 auto;
  }

  .action {
    background: color-mix(in srgb, var(--fg-on-accent) 18%, transparent);
    color: var(--fg-on-accent);
    border: 1px solid color-mix(in srgb, var(--fg-on-accent) 35%, transparent);
    padding: 0.25rem 0.7rem;
    border-radius: var(--radius-sm);
    font-size: 0.82rem;
    line-height: 1;
    cursor: pointer;
    font-family: var(--font-sans);

    &:hover { background: color-mix(in srgb, var(--fg-on-accent) 32%, transparent); }
    &:focus-visible { outline: 1px solid var(--fg-on-accent); outline-offset: 1px; }

    &.primary {
      background: var(--fg-on-accent);
      color: var(--accent-strong);
      border-color: var(--fg-on-accent);
      font-weight: 600;

      &:hover { filter: brightness(0.94); }
    }

    &.subtle {
      background: transparent;
      border-color: transparent;
      color: color-mix(in srgb, var(--fg-on-accent) 80%, transparent);

      &:hover {
        color: var(--fg-on-accent);
        background: color-mix(in srgb, var(--fg-on-accent) 18%, transparent);
      }
    }

    &.close {
      background: transparent;
      border-color: transparent;
      color: color-mix(in srgb, var(--fg-on-accent) 80%, transparent);
      padding: 0.2rem;
      width: 1.8rem;
      height: 1.8rem;
      display: inline-flex;
      align-items: center;
      justify-content: center;

      svg { width: 14px; height: 14px; }

      &:hover {
        background: color-mix(in srgb, var(--fg-on-accent) 18%, transparent);
        color: var(--fg-on-accent);
      }
    }
  }

  .installing-indicator {
    font-family: var(--font-mono);
    color: color-mix(in srgb, var(--fg-on-accent) 80%, transparent);
    font-size: 0.9rem;
  }
}
</style>
