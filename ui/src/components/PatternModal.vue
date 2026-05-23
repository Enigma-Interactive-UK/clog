<script setup lang="ts">
/**
 * Pattern editor modal. Operates directly on the current tab's pattern
 * refs; Test and Apply call into Tab's own per-pattern methods (which
 * round-trip through the backend and persist the override).
 */

import BaseModal from './BaseModal.vue'
import type { Tab } from '../tab'

defineProps<{ tab: Tab }>()

const emit = defineEmits<{ (e: 'close'): void }>()
</script>

<template>
  <BaseModal title="Pattern" modal-class="pattern-modal" @close="emit('close')">
    <div class="row-grid">
      <label>Kind</label>
      <select v-model="tab.patternMode.value">
        <option value="pattern">PatternLayout</option>
        <option value="regex">Regex</option>
      </select>
    </div>
    <div class="pattern-input-row">
      <input
        v-model="tab.patternInput.value"
        class="pat-input"
        :placeholder="tab.patternMode.value === 'pattern'
          ? '[%-5level] %d{yyyy-MM-dd HH:mm:ss.SSS} [%t] %c{1} - %msg%n'
          : '^(?P&lt;timestamp&gt;\\d{4}-...) (?P&lt;level&gt;INFO|WARN|ERROR) ...'"
        spellcheck="false"
      />
      <button type="button" @click="tab.testPattern()">Test</button>
      <button type="button" @click="tab.applyPattern()">Apply</button>
    </div>
    <p v-if="tab.patternScore.value !== null" class="score">
      Match score: <strong>{{ (tab.patternScore.value * 100).toFixed(1) }}%</strong>
      <span v-if="tab.patternSampleSize.value > 0" class="muted"> of {{ tab.patternSampleSize.value }} lines</span>
    </p>
    <p v-if="tab.file.value.pattern_name" class="muted">
      Auto-detected: <strong>{{ tab.file.value.pattern_name }}</strong>
    </p>
    <p v-if="tab.patternError.value" class="pat-error">{{ tab.patternError.value }}</p>
    <p class="muted">
      Apply saves the pattern as a per-file override; the next time you open this file the same pattern is used automatically.
    </p>
  </BaseModal>
</template>

<style scoped>
:deep(.pattern-modal) { width: min(720px, 92vw); }

.row-grid {
  display: grid;
  grid-template-columns: 10rem 1fr;
  align-items: center;
  gap: 0.8rem;
  margin: 0.35rem 0;
}

.pattern-input-row {
  display: flex;
  gap: 0.4rem;
  margin: 0.6rem 0;

  .pat-input {
    flex: 1;
    min-width: 0;
    background: var(--bg-viewport);
    color: var(--fg-default);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    padding: 0.4rem 0.6rem;
    font-family: var(--font-mono);
    font-size: 0.85rem;
  }

  button {
    background: var(--bg-button);
    color: var(--fg-default);
    border: 1px solid var(--border-button);
    border-radius: var(--radius-sm);
    padding: 0.3rem 0.8rem;
    font-size: 0.85rem;
    cursor: pointer;

    &:hover { background: var(--bg-button-hover); }
  }
}

.score, .muted, .pat-error { font-size: 0.85rem; margin: 0.3rem 0; }
.muted { color: var(--fg-muted); }
.pat-error { color: var(--fg-error); font-family: var(--font-mono); }
</style>
