<script setup lang="ts">
/**
 * Drop-target overlay shown while the user drags files over the window.
 * Pointer-events: none so the overlay never swallows the drop.
 */

defineProps<{ visible: boolean }>()
</script>

<template>
  <div v-if="visible" class="drop-overlay">
    <div class="drop-hint">
      <span class="arrow">&darr;</span>
      Drop a log file to open it
    </div>
  </div>
</template>

<style scoped>
.drop-overlay {
  position: fixed;
  inset: 0;
  pointer-events: none;
  background: color-mix(in srgb, var(--accent) 18%, transparent);
  border: 3px dashed var(--accent);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 250;

  .drop-hint {
    background: var(--bg-elevated);
    color: var(--fg-default);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    padding: 1rem 1.6rem;
    font-size: 1rem;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.4);
    display: flex;
    align-items: center;
    gap: 0.6rem;

    .arrow { color: var(--accent); font-size: 1.4rem; }
  }
}
</style>
