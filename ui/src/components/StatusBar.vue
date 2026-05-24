<script setup lang="ts">
/**
 * Footer status bar: cache hint, record/line/byte stats for the current
 * tab, theme toggle, font-size hint, and the pattern label + Edit button.
 *
 * All visible state is read directly from props -- the bar owns no
 * reactive state of its own.
 */

import type { Settings } from '../types'
import type { Tab } from '../tab'

defineProps<{
  tab: Tab | null
  settings: Settings
  themeToggleGlyph: string
  themeLabel: Record<'system' | 'light' | 'dark', string>
}>()

const emit = defineEmits<{
  (e: 'cycle-theme'): void
  (e: 'open-pattern'): void
}>()

function formatCount(n: number): string {
  return n.toLocaleString('en-GB')
}

function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return `${n}`
  if (n < 1024) return `${n} B`
  const units = ['KiB', 'MiB', 'GiB', 'TiB']
  let value = n / 1024
  let i = 0
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024
    i++
  }
  const digits = value < 10 ? 2 : value < 100 ? 1 : 0
  return `${value.toFixed(digits)} ${units[i]}`
}
</script>

<template>
  <footer class="status-bar">
    <span class="slot left">
      <span v-if="tab?.file.value.cache_hit" class="cache-hint" title="Records loaded from the on-disk index cache">cached</span>
      <span
        v-if="tab?.tailing.value && tab?.followTail.value"
        class="follow-hint"
        title="Following the tail - new records will scroll into view automatically"
      >
        <span class="follow-dot" aria-hidden="true" />following
      </span>
    </span>
    <span class="slot right">
      <template v-if="tab">
        <span class="stat">{{ formatCount(tab.file.value.record_count) }} records</span>
        <span class="stat">{{ formatCount(tab.file.value.line_count) }} lines</span>
        <span class="stat" :title="`${formatCount(tab.file.value.size_bytes)} bytes`">{{ formatBytes(tab.file.value.size_bytes) }}</span>
      </template>
      <button
        type="button"
        class="theme-toggle"
        :class="{ 'is-auto': settings.theme === 'system' }"
        :title="themeLabel[settings.theme]"
        :aria-label="themeLabel[settings.theme]"
        @click="emit('cycle-theme')"
      >{{ themeToggleGlyph }}</button>
      <span class="font-size-hint" :title="`Base font size (Ctrl-+ / Ctrl-- / Ctrl-0)`">
        {{ settings.font_size }}px
      </span>
      <span v-if="tab" class="pattern-status">
        <span class="pattern-label" :title="tab.file.value.pattern_source">
          Pattern: <strong>{{ tab.file.value.pattern_name ?? 'custom' }}</strong>
        </span>
        <button
          type="button"
          class="pattern-edit-btn"
          title="Edit pattern"
          aria-label="Edit pattern"
          @click="emit('open-pattern')"
        >Edit</button>
      </span>
    </span>
  </footer>
</template>

<style scoped>
.status-bar {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 0.6rem;
  padding: 0.25rem 0.75rem;
  border-top: 1px solid var(--border-default);
  background: var(--bg-elevated);
  color: var(--fg-muted);
  font-family: var(--font-mono);
  font-size: 0.78rem;
  min-height: 1.6rem;

  .slot { display: flex; align-items: center; gap: 0.6rem; }
  .slot.right { margin-left: auto; gap: 1.5em; }
  .stat { color: var(--fg-muted); }

  .cache-hint {
    padding: 0.05rem 0.4rem;
    border-radius: var(--radius-sm);
    background: var(--bg-button);
    color: var(--fg-dim);
    font-size: 0.72rem;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .follow-hint {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.05rem 0.45rem;
    border-radius: var(--radius-sm);
    background: var(--bg-button);
    color: var(--fg-default);
    font-size: 0.72rem;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  .follow-dot {
    width: 0.55rem;
    height: 0.55rem;
    border-radius: 50%;
    background: #f59e0b;
    animation: follow-pulse 1.4s ease-in-out infinite;
  }
  .font-size-hint { color: var(--fg-dim); }

  .theme-toggle {
    background: transparent;
    color: var(--fg-muted);
    border: 1px solid var(--border-button);
    border-radius: var(--radius-sm);
    padding: 0.05rem 0.5rem;
    font-size: 0.85rem;
    line-height: 1.2;
    cursor: pointer;

    &.is-auto { opacity: 0.5; }
    &:hover { background: var(--bg-button-hover); color: var(--fg-default); opacity: 1; }
  }

  .pattern-status {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
  }
  .pattern-label strong { color: var(--fg-default); font-weight: 600; }
  .pattern-edit-btn {
    background: var(--bg-button);
    color: var(--fg-default);
    border: 1px solid var(--border-button);
    border-radius: var(--radius-sm);
    padding: 0.05rem 0.45rem;
    font-size: 0.72rem;
    line-height: 1.2;
    cursor: pointer;

    &:hover { background: var(--bg-button-hover); }
  }
}

@keyframes follow-pulse {
  0%, 100% {
    opacity: 0.45;
    transform: scale(0.7);
    box-shadow: 0 0 0 0 rgba(245, 158, 11, 0.55);
  }
  50% {
    opacity: 1;
    transform: scale(1);
    box-shadow: 0 0 0 0.35rem rgba(245, 158, 11, 0);
  }
}
</style>
