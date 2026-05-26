<script setup lang="ts">
/**
 * Full-record viewer modal. Shows the raw text of a single log record so
 * the user can read the whole thing (including stack-trace continuation
 * lines) without scrolling the viewport, select text and copy it. Lines
 * are rendered with the same axis-1/axis-2 syntax highlighting the main
 * viewport uses (handed in pre-rendered by the caller). Soft wrap is
 * user-toggleable. The dialog is resizable via the native CSS resize
 * handle on the bottom-right corner.
 */
import { ref, watch } from 'vue'
import BaseModal from './BaseModal.vue'
import type { LeafSpan } from '../highlight/engine'

export interface RecordRenderedLine {
  level: string
  isHeader: boolean
  spans: LeafSpan[]
  text: string
}

const props = defineProps<{
  recordIdx: number
  lines: RecordRenderedLine[]
  rawText: string
  loading: boolean
  error: string | null
}>()

const emit = defineEmits<{
  (e: 'close'): void
}>()

const wrap = ref(true)
const copied = ref(false)
let copyTimer: ReturnType<typeof setTimeout> | null = null

watch(() => props.recordIdx, () => {
  copied.value = false
  if (copyTimer) { clearTimeout(copyTimer); copyTimer = null }
})

async function copyText() {
  if (!props.rawText) return
  try {
    await navigator.clipboard.writeText(props.rawText)
    copied.value = true
    if (copyTimer) clearTimeout(copyTimer)
    copyTimer = setTimeout(() => { copied.value = false; copyTimer = null }, 1400)
  } catch {
    // best-effort
  }
}
</script>

<template>
  <BaseModal
    :title="`Record ${recordIdx + 1}`"
    aria-label="Full record"
    modal-class="record-modal"
    @close="emit('close')"
  >
    <div class="record-toolbar">
      <label class="wrap-toggle">
        <input type="checkbox" v-model="wrap" />
        <span>Wrap</span>
      </label>
      <button
        type="button"
        class="copy-btn"
        :disabled="loading || !!error || !rawText"
        @click="copyText"
      >{{ copied ? 'Copied' : 'Copy' }}</button>
    </div>
    <div v-if="loading" class="record-status">Loading...</div>
    <div v-else-if="error" class="record-status record-error">{{ error }}</div>
    <div
      v-else
      class="record-pre user-selectable"
      :class="{ wrap }"
    >
      <div
        v-for="(ln, li) in lines"
        :key="li"
        class="record-line"
        :class="ln.isHeader ? 'is-header' : 'is-continuation'"
      >
        <span
          v-for="(span, si) in ln.spans"
          :key="si"
          :class="span.cls"
        >{{ span.text }}</span>
      </div>
    </div>
  </BaseModal>
</template>

<style scoped>
:deep(.record-modal) {
  width: min(960px, 92vw);
  height: min(720px, 88vh);
  max-height: 92vh;
  max-width: 96vw;
  min-width: 320px;
  min-height: 240px;
  resize: both;
  overflow: hidden;
}

:deep(.record-modal) .modal-body {
  display: flex;
  flex-direction: column;
  min-height: 0;
  flex: 1 1 auto;
}

.record-toolbar {
  display: flex;
  align-items: center;
  gap: 0.8rem;
  margin: -0.2rem 0 0.6rem;
  flex: 0 0 auto;
}

.wrap-toggle {
  display: inline-flex;
  align-items: center;
  gap: 0.4rem;
  font-size: 0.85rem;
  color: var(--fg-default);
  cursor: pointer;
}

.copy-btn {
  margin-left: auto;
  padding: 0.25rem 0.7rem;
  background: var(--bg-button);
  color: var(--fg-default);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  cursor: pointer;
  font-size: 0.85rem;

  &:hover:not(:disabled) { background: var(--bg-button-hover); }
  &:disabled { opacity: 0.5; cursor: default; }
}

.record-pre {
  margin: 0;
  padding: 0.6rem 0.8rem;
  background: var(--bg-app);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  font-family: var(--font-mono);
  font-size: var(--font-size-base);
  line-height: 1.45;
  color: var(--fg-default);
  flex: 1 1 auto;
  min-height: 0;
  overflow: auto;

  .record-line { white-space: pre; }
  &.wrap .record-line {
    white-space: pre-wrap;
    word-break: break-word;
    overflow-wrap: anywhere;
  }
  .record-line.is-continuation { color: var(--fg-message); }
}

.record-status {
  padding: 0.6rem 0.8rem;
  font-size: 0.85rem;
  color: var(--fg-muted);
}

.record-error {
  color: var(--fg-error);
  background: var(--bg-error);
  border: 1px solid var(--border-error);
  border-radius: var(--radius-sm);
}
</style>
