<script setup lang="ts">
/**
 * Search + filter + level-mask control bar for a single tab. All state
 * lives on the tab object; this component is mostly markup + the small
 * methods that translate v-model writes into the right tab mutations.
 *
 * `next-hit` and `prev-hit` are emitted to the parent so it can call into
 * the LogViewport's exposed `scrollToCurrentHit()` -- this component does
 * not touch the DOM.
 */
import { useTemplateRef } from 'vue'
import { LEVEL_KEYS, type LevelKey, type SearchMode } from '../types'
import type { Tab } from '../tab'

const props = defineProps<{
  tab: Tab
}>()

const emit = defineEmits<{
  (e: 'next-hit'): void
  (e: 'prev-hit'): void
}>()

const searchInputEl = useTemplateRef<HTMLInputElement>('searchInputEl')

function setSearchMode(mode: SearchMode) {
  props.tab.setSearchMode(mode)
}

function toggleFilterMode() {
  props.tab.filterMode.value = !props.tab.filterMode.value
}

function toggleLevel(level: LevelKey) {
  props.tab.toggleLevel(level)
}

function clearSearch() {
  if (props.tab.searchQuery.value.length === 0) return
  props.tab.searchQuery.value = ''
  props.tab.searchError.value = null
  props.tab.clearSearchState()
  searchInputEl.value?.focus()
}

function onNextHit() {
  if (props.tab.nextHitIdx() !== null) emit('next-hit')
}
function onPrevHit() {
  if (props.tab.prevHitIdx() !== null) emit('prev-hit')
}

defineExpose({
  focus: () => searchInputEl.value?.focus(),
})
</script>

<template>
  <section class="search-bar">
    <fieldset class="mode-toggle">
      <legend class="sr-only">Search mode</legend>
      <span class="mode-label">Search:</span>
      <button
        type="button"
        class="mode-btn"
        :class="{ 'is-on': tab.searchMode.value === 'smart' }"
        :aria-pressed="tab.searchMode.value === 'smart'"
        title="Smart proximity-ranked substring search"
        @click="setSearchMode('smart')"
      >Smart</button>
      <button
        type="button"
        class="mode-btn"
        :class="{ 'is-on': tab.searchMode.value === 'regex' }"
        :aria-pressed="tab.searchMode.value === 'regex'"
        title="Regular expression search (regex::bytes)"
        @click="setSearchMode('regex')"
      >Regex</button>
    </fieldset>
    <span class="search-input-wrap">
      <input
        ref="searchInputEl"
        v-model="tab.searchQuery.value"
        class="search-input"
        :class="{ 'has-error': !!tab.searchError.value }"
        :placeholder="tab.searchMode.value === 'smart' ? `e.g., 'installed core'...` : `regular expression, e.g., 'installed.*core'...`"
        spellcheck="false"
        @input="tab.scheduleSearch()"
        @keydown.enter.prevent="onNextHit"
        @keydown.shift.enter.prevent="onPrevHit"
        @keydown.esc.prevent="clearSearch"
      />
      <button
        v-if="tab.searchQuery.value.length > 0"
        type="button"
        class="btn-dismiss clear-search"
        title="Clear search (Esc)"
        aria-label="Clear search"
        @click="clearSearch"
      >
        <svg class="dismiss-glyph" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
          <path d="M4 4 L12 12 M12 4 L4 12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" fill="none" />
        </svg>
      </button>
    </span>
    <label class="case" title="Case-sensitive search">
      <input type="checkbox" v-model="tab.searchCaseSensitive.value" @change="tab.scheduleSearch()" />
      Aa
    </label>
    <span v-if="tab.hitOrder.value.length > 0" class="hit-count">
      <strong>{{ tab.currentHit.value + 1 }}</strong> / {{ tab.hitOrder.value.length }}
    </span>
    <span v-else-if="tab.searchQuery.value.trim() && !tab.searchInflight.value && !tab.searchError.value" class="hit-count muted">
      0 hits
    </span>
    <span v-else-if="tab.searchInflight.value" class="hit-count muted">searching...</span>
    <button type="button" :disabled="tab.hitOrder.value.length === 0" @click="onPrevHit">&uarr;</button>
    <button type="button" :disabled="tab.hitOrder.value.length === 0" @click="onNextHit">&darr;</button>
    <button
      type="button"
      class="filter-toggle"
      :class="{ 'is-on': tab.filterMode.value }"
      :title="tab.filterMode.value ? 'Showing only matching records -- click to show all' : 'Filter to matching records'"
      @click="toggleFilterMode"
    >
      {{ tab.filterMode.value ? 'Filter on' : 'Filter' }}
    </button>
    <span class="level-mask">
      <button
        v-for="lvl in LEVEL_KEYS"
        :key="lvl"
        type="button"
        class="lvl-btn"
        :class="['lvl-' + lvl, { 'is-off': !tab.levelAllow.value[lvl] }]"
        :title="`Toggle ${lvl.toUpperCase()} records`"
        @click="toggleLevel(lvl)"
      >{{ lvl.toUpperCase() }}</button>
    </span>
    <span v-if="tab.searchError.value" class="search-error">{{ tab.searchError.value }}</span>
  </section>
</template>

<style scoped>
.search-bar {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.5rem 1rem;
  border-bottom: 1px solid var(--border-default);
  background: var(--bg-elevated);
  flex-wrap: wrap;
  font-size: 0.85rem;
  color: var(--fg-muted);

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }

  .mode-toggle {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    border: none;
    padding: 0;
    margin: 0;

    .mode-label { color: var(--fg-muted); }

    .mode-btn {
      background: var(--bg-button);
      color: var(--fg-muted);
      border: 1px solid var(--border-button);
      padding: 0.25rem 0.7rem;
      font-size: 0.8rem;
      font-family: var(--font-mono);
      cursor: pointer;

      &:first-of-type {
        border-radius: var(--radius-sm) 0 0 var(--radius-sm);
        border-right-width: 0;
      }
      &:last-of-type {
        border-radius: 0 var(--radius-sm) var(--radius-sm) 0;
      }

      &:hover:not(.is-on) { background: var(--bg-button-hover); }

      &.is-on {
        background: var(--accent);
        color: var(--fg-on-accent);
        border-color: var(--accent);
        font-weight: 600;
      }
    }
  }

  .search-input-wrap {
    flex: 1 1 16rem;
    min-width: 12rem;
    position: relative;
    display: inline-flex;
    align-items: stretch;
  }

  .search-input {
    flex: 1 1 auto;
    width: 100%;
    background: var(--bg-viewport);
    color: var(--fg-default);
    border: 1px solid var(--border-button);
    border-radius: var(--radius-sm);
    padding: 0.3rem 1.6rem 0.3rem 0.5rem;
    font-family: var(--font-mono);
    font-size: 0.85rem;

    &.has-error {
      border-color: var(--level-error);
      color: var(--fg-error);
      text-decoration: underline;
      text-decoration-color: var(--level-error);
      text-decoration-style: wavy;
    }

    &::placeholder {
      color: var(--fg-dim);
      font-style: italic;
    }
  }

  .clear-search {
    /* Anchored inside the search input. The scoped `button` rule below
       paints a solid background, padding and a border on every <button> in
       the bar; without overriding here the dismiss-glyph SVG gets squeezed
       out of the 1.2rem hit area, which is why the X went missing. */
    position: absolute;
    top: 50%;
    right: 0.3rem;
    transform: translateY(-50%);
    width: 1.2rem;
    height: 1.2rem;
    padding: 0;
    border: 0;
    background: transparent;
    font-size: 1.05rem;
    border-radius: 50%;
  }

  .case {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    font-family: var(--font-mono);
    cursor: pointer;
    user-select: none;
  }

  .hit-count {
    font-family: var(--font-mono);
    color: var(--fg-default);

    strong { color: var(--accent); }
    &.muted { color: var(--fg-dim); }
  }

  button {
    background: var(--bg-button);
    color: var(--fg-default);
    border: 1px solid var(--border-button);
    border-radius: var(--radius-sm);
    padding: 0.25rem 0.55rem;
    font-size: 0.8rem;
    font-family: var(--font-mono);
    cursor: pointer;

    &:hover:not(:disabled) { background: var(--bg-button-hover); }
    &:disabled { opacity: 0.4; cursor: default; }
  }

  .filter-toggle.is-on {
    border-color: var(--accent);
    color: var(--accent);
  }

  .level-mask {
    display: inline-flex;
    gap: 0.15rem;

    .lvl-btn {
      padding: 0.2rem 0.4rem;
      font-size: 0.72rem;
      letter-spacing: 0.04em;
      border-color: var(--border-button);

      &.is-off {
        opacity: 0.35;
        text-decoration: line-through;
      }
    }
    .lvl-trace { color: var(--level-trace); }
    .lvl-debug { color: var(--level-debug); }
    .lvl-info { color: var(--level-info); }
    .lvl-warn { color: var(--level-warn); }
    .lvl-error { color: var(--level-error); }
    .lvl-fatal { color: var(--level-fatal); }
  }

  .search-error {
    color: var(--fg-error);
    font-family: var(--font-mono);
    flex-basis: 100%;
  }
}
</style>
