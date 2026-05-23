<script setup lang="ts">
/**
 * Shared modal scaffold: backdrop, frame, header bar with title + close.
 * Modal body lives in the default slot. Backdrop click closes; the close
 * button emits the same event.
 */

defineProps<{
  title: string
  ariaLabel?: string
  modalClass?: string
}>()

const emit = defineEmits<{
  (e: 'close'): void
}>()
</script>

<template>
  <div class="modal-backdrop" @click.self="emit('close')">
    <div class="modal" :class="modalClass" role="dialog" :aria-label="ariaLabel ?? title">
      <header class="modal-head">
        <h2>{{ title }}</h2>
        <button type="button" class="modal-close" :aria-label="`Close ${title.toLowerCase()}`" @click="emit('close')">&times;</button>
      </header>
      <section class="modal-body">
        <slot />
      </section>
    </div>
  </div>
</template>

<style scoped>
.modal-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 200;
}

.modal {
  width: min(720px, 92vw);
  max-height: 88vh;
  background: var(--bg-elevated);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
  display: flex;
  flex-direction: column;
  overflow: hidden;

  .modal-head {
    display: flex;
    align-items: center;
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--border-default);
    background: var(--bg-app);

    h2 { margin: 0; font-size: 1rem; }
  }

  .modal-close {
    margin-left: auto;
    background: transparent;
    color: var(--fg-default);
    border: 0;
    font-size: 1.4rem;
    line-height: 1;
    cursor: pointer;
  }

  .modal-body {
    padding: 1rem 1.2rem 1.5rem;
    overflow-y: auto;
  }
}
</style>
