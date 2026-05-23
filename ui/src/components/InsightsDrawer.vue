<script setup lang="ts">
/**
 * Right-side collapsible drawer hosting the slow-request insights for
 * the active tab. Entry table, threshold editor, and status chip are
 * added in subsequent tasks; this scaffold just renders the header +
 * empty body + close button.
 */
import { computed } from 'vue'
import type { Tab } from '../tab'

const props = defineProps<{ tab: Tab }>()

const emit = defineEmits<{
  (e: 'close'): void
}>()

const totals = computed(() => {
  const s = props.tab.slowRequestSummary.value
  if (!s) return 'Loading...'
  if (s.total_hits === 0) return 'No slow requests detected.'
  return `${s.total_hits} hits across ${s.entries.length} endpoints, ${s.deduped} dedupes`
})
</script>

<template>
  <aside class="insights-drawer">
    <header class="drawer-head">
      <span class="title">Slow requests</span>
      <button type="button" class="close-btn" aria-label="Close" @click="emit('close')">
        x
      </button>
    </header>
    <div class="drawer-totals">{{ totals }}</div>
    <div class="drawer-body">
      <!-- Body lands in Task 13. -->
    </div>
  </aside>
</template>

<style scoped>
.insights-drawer {
  flex: 0 0 auto;
  width: 360px;
  display: flex;
  flex-direction: column;
  background: var(--bg-elevated);
  border-left: 1px solid var(--border-default);
  min-height: 0;
}

.drawer-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0.4rem 0.6rem;
  border-bottom: 1px solid var(--border-default);

  & .title {
    font-weight: 600;
  }

  & .close-btn {
    background: transparent;
    border: 1px solid transparent;
    color: var(--fg-default);
    cursor: pointer;
    padding: 0.1rem 0.4rem;
    border-radius: 3px;

    &:hover {
      background: var(--bg-button-hover);
      border-color: var(--border-button);
    }
  }
}

.drawer-totals {
  padding: 0.4rem 0.6rem;
  color: var(--fg-muted);
  font-size: 0.85rem;
}

.drawer-body {
  flex: 1 1 auto;
  overflow-y: auto;
  padding: 0 0.6rem 0.6rem;
}
</style>
