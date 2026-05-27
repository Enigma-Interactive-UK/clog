<script setup lang="ts">
/**
 * Floating "Exit zen mode" pill. Rendered by App.vue only when zen is
 * on. Fades to half opacity at rest so it does not compete with the
 * logs; goes opaque on hover/focus.
 */
defineEmits<{ (e: 'click'): void }>()
</script>

<template>
  <button
    type="button"
    class="zen-exit-pill"
    aria-label="Exit zen mode"
    title="Exit zen mode (F11 or Esc)"
    @click="$emit('click')"
  >
    <svg class="glyph" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
      <path d="M4 4 L12 12 M12 4 L4 12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" fill="none" />
    </svg>
    <span class="label">Exit zen mode</span>
  </button>
</template>

<style scoped>
.zen-exit-pill {
  position: fixed;
  top: 0.5rem;
  right: 0.5rem;
  z-index: 20;
  display: inline-flex;
  align-items: center;
  gap: 0.4rem;
  padding: 0.35rem 0.7rem;
  font-family: var(--font-sans);
  font-size: 0.8rem;
  color: var(--fg-default);
  background: var(--bg-elevated);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  opacity: 0.5;
  cursor: pointer;
  transition: opacity 120ms ease;

  &:hover,
  &:focus-visible {
    opacity: 1;
  }

  &:focus-visible {
    outline: 2px solid var(--accent, currentColor);
    outline-offset: 2px;
  }
}

.glyph {
  width: 0.9rem;
  height: 0.9rem;
  flex: 0 0 auto;
}

.label {
  line-height: 1;
}
</style>
