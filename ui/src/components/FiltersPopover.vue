<script setup lang="ts">
/**
 * Popover hosting the level mask and thread-group mask toggles. Anchored
 * by the parent, which positions us absolutely. We do not own the
 * trigger -- only the menu surface and its outside-click/Esc dismissal.
 */
import { computed, inject, onBeforeUnmount, onMounted, ref, type Ref } from 'vue'
import {
  LEVEL_KEYS,
  THREAD_GROUP_KEYS,
  THREAD_GROUP_LABEL,
  type CollapseMode,
  type LevelKey,
  type Settings,
  type ThreadGroupKey,
} from '../types'
import { defaultLevelAllow, defaultThreadGroupAllow } from '../tab'
import type { Tab } from '../tab'
import { effectiveMode } from '../collapse'

const props = defineProps<{ tab: Tab }>()
const emit = defineEmits<{ (e: 'close'): void }>()

const COLLAPSE_OPTIONS: CollapseMode[] = ['inherit', 'none', 'errors', 'all']
const COLLAPSE_LABEL: Record<CollapseMode, string> = {
  inherit: 'Inherit',
  none: 'None',
  errors: 'Errors',
  all: 'All',
}

const settings = inject<Ref<Settings> | null>('settings', null)
const inheritedMode = computed(() => {
  const def = settings?.value.collapse_records_default
  return def === 'errors' || def === 'all' ? def : 'none'
})
const effectiveLabel = computed(() =>
  COLLAPSE_LABEL[effectiveMode('inherit', inheritedMode.value)],
)

function setCollapseMode(mode: CollapseMode) {
  props.tab.setCollapseMode(mode)
}

const rootEl = ref<HTMLElement | null>(null)

function toggleLevel(level: LevelKey) {
  props.tab.toggleLevel(level)
}

function toggleThreadGroup(group: ThreadGroupKey) {
  props.tab.toggleThreadGroup(group)
}

function resetAll() {
  // Reset levels.
  const allLevels = defaultLevelAllow()
  for (const k of LEVEL_KEYS) {
    if (props.tab.levelAllow.value[k] !== allLevels[k]) toggleLevel(k)
  }
  // Reset thread groups.
  const allGroups = defaultThreadGroupAllow()
  for (const k of THREAD_GROUP_KEYS) {
    if (props.tab.threadGroupAllow.value[k] !== allGroups[k]) toggleThreadGroup(k)
  }
}

function onDocClick(ev: MouseEvent) {
  const root = rootEl.value
  if (!root) return
  if (root.contains(ev.target as Node)) return
  // If the click landed on the trigger (or anywhere inside the anchor
  // that wraps trigger + popover), let the trigger's own click handler
  // toggle us shut. Emitting close here would race the trigger's click,
  // and because mousedown fires before click, our close would land first
  // and the trigger would immediately re-open the popover.
  const target = ev.target as Element | null
  if (target && target.closest('.filters-anchor')) return
  emit('close')
}

function onKey(ev: KeyboardEvent) {
  if (ev.key === 'Escape') {
    ev.preventDefault()
    emit('close')
  }
}

onMounted(() => {
  // setTimeout so the originating click that opened us doesn't immediately
  // re-fire this handler and close the popover.
  setTimeout(() => document.addEventListener('mousedown', onDocClick), 0)
  document.addEventListener('keydown', onKey)
})

onBeforeUnmount(() => {
  document.removeEventListener('mousedown', onDocClick)
  document.removeEventListener('keydown', onKey)
})
</script>

<template>
  <div ref="rootEl" class="filters-popover" role="menu">
    <section class="filters-section">
      <h4 class="filters-heading">Levels</h4>
      <div class="filters-row">
        <button
          v-for="lvl in LEVEL_KEYS"
          :key="lvl"
          type="button"
          class="filter-pill"
          :class="['lvl-' + lvl, { 'is-off': !tab.levelAllow.value[lvl] }]"
          :title="`Toggle ${lvl.toUpperCase()} records`"
          @click="toggleLevel(lvl)"
        >{{ lvl.toUpperCase() }}</button>
      </div>
    </section>
    <section class="filters-section">
      <h4 class="filters-heading">Threads</h4>
      <div class="filters-row">
        <button
          v-for="g in THREAD_GROUP_KEYS"
          :key="g"
          type="button"
          class="filter-pill thread-pill"
          :class="{ 'is-off': !tab.threadGroupAllow.value[g] }"
          :title="`Toggle ${THREAD_GROUP_LABEL[g]} thread records`"
          @click="toggleThreadGroup(g)"
        >{{ THREAD_GROUP_LABEL[g] }}</button>
      </div>
    </section>
    <section class="filters-section">
      <h4 class="filters-heading">Collapse records</h4>
      <div class="filters-row collapse-seg">
        <button
          v-for="opt in COLLAPSE_OPTIONS"
          :key="opt"
          type="button"
          class="filter-pill"
          :class="{ 'is-on': tab.collapseMode.value === opt }"
          @click="setCollapseMode(opt)"
        >{{ COLLAPSE_LABEL[opt] }}</button>
      </div>
      <p v-if="tab.collapseMode.value === 'inherit'" class="collapse-hint">
        Inheriting global default (currently "{{ effectiveLabel }}")
      </p>
    </section>
    <footer class="filters-footer">
      <button type="button" class="reset-link" @click="resetAll">Reset all filters</button>
    </footer>
  </div>
</template>

<style scoped>
.filters-popover {
  position: absolute;
  bottom: calc(100% + 4px);
  right: 0;
  z-index: 50;
  min-width: 18rem;
  background: var(--bg-elevated);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  padding: 0.5rem 0.6rem 0.4rem;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.35);
  font-size: 0.85rem;
  color: var(--fg-default);
}

.filters-section + .filters-section { margin-top: 0.5rem; }

.filters-heading {
  margin: 0 0 0.25rem;
  font-size: 0.7rem;
  font-weight: 600;
  letter-spacing: 0.05em;
  text-transform: uppercase;
  color: var(--fg-muted);
}

.filters-row {
  display: flex;
  flex-wrap: wrap;
  gap: 0.2rem;
}

.filter-pill {
  background: var(--bg-button);
  color: var(--fg-default);
  border: 1px solid var(--border-button);
  border-radius: var(--radius-sm);
  padding: 0.2rem 0.55rem;
  font-size: 0.75rem;
  font-family: var(--font-mono);
  letter-spacing: 0.04em;
  cursor: pointer;

  &:hover:not(.is-off) { background: var(--bg-button-hover); }

  &.is-off {
    opacity: 0.35;
    text-decoration: line-through;
  }
}

.filter-pill.is-on {
  background: var(--accent);
  color: var(--fg-on-accent);
  border-color: var(--accent);
}

.collapse-hint {
  margin: 0.3rem 0 0;
  font-size: 0.72rem;
  color: var(--fg-muted);
  font-style: italic;
}

.lvl-trace { color: var(--level-trace); }
.lvl-debug { color: var(--level-debug); }
.lvl-info  { color: var(--level-info); }
.lvl-warn  { color: var(--level-warn); }
.lvl-error { color: var(--level-error); }
.lvl-fatal { color: var(--level-fatal); }

.thread-pill { color: var(--fg-default); }

.filters-footer {
  display: flex;
  justify-content: flex-end;
  margin-top: 0.5rem;
  padding-top: 0.4rem;
  border-top: 1px solid var(--border-default);
}

.reset-link {
  background: transparent;
  border: 0;
  color: var(--accent);
  font-size: 0.75rem;
  cursor: pointer;
  padding: 0.1rem 0.2rem;

  &:hover { text-decoration: underline; }
}
</style>
