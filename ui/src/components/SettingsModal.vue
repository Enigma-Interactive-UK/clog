<script setup lang="ts">
/**
 * Settings modal split into four tabs: General (appearance / behaviour /
 * recent files), Slow requests (global fast/slow thresholds), Highlighting
 * (global rule editor + reset), Advanced (data folder + per-scope reset
 * buttons). The active tab is local state; close resets nothing else.
 */

import { computed, ref, watchEffect } from 'vue'
import { ask } from '@tauri-apps/plugin-dialog'

import BaseModal from './BaseModal.vue'
import HighlightRulesEditor from './HighlightRulesEditor.vue'
import type { DataDirPayload, Settings, UserHighlightRule } from '../types'

const props = defineProps<{
  settings: Settings
  dataDir: DataDirPayload | null
  globalRules: UserHighlightRule[]
}>()

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'update', patch: Partial<Settings>): void
  (e: 'bump-font', delta: number): void
  (e: 'reset-font'): void
  (e: 'open-recent', path: string): void
  (e: 'forget-recent', path: string): void
  (e: 'open-data-folder'): void
  (e: 'reset-data', scope: 'settings' | 'session' | 'patterns' | 'index' | 'highlight' | 'all'): void
  (e: 'save-global-rules', rules: UserHighlightRule[]): void
}>()

type TabId = 'general' | 'slow-requests' | 'highlighting' | 'advanced'
const activeTab = ref<TabId>('general')

// --- Slow request thresholds (global) ------------------------------------

// Auto-tier defaults, mirrored from the Rust side. A blank input on save
// falls back to these so the user can tweak one side without having to
// type out both.
const DEFAULT_FAST_MS = 2000
const DEFAULT_SLOW_MS = 10000

const fastInput = ref('')
const slowInput = ref('')

// Sync the inputs whenever the upstream settings.slow_request_thresholds
// changes (e.g. modal opened, or a reset hit). When no global override is
// set, pre-populate with the auto defaults so the user can immediately
// tweak rather than having to look up the starting values.
watchEffect(() => {
  const t = props.settings.slow_request_thresholds
  fastInput.value = t ? String(t.fast_ms) : String(DEFAULT_FAST_MS)
  slowInput.value = t ? String(t.slow_ms) : String(DEFAULT_SLOW_MS)
})

function resolvedThreshold(input: string, fallback: number): number | null {
  if (input === '') return fallback
  const n = Number(input)
  if (Number.isNaN(n)) return null
  return n
}

const thresholdError = computed<string | null>(() => {
  const f = resolvedThreshold(fastInput.value, DEFAULT_FAST_MS)
  const s = resolvedThreshold(slowInput.value, DEFAULT_SLOW_MS)
  if (f === null || s === null) return 'Numbers only'
  if (f >= s) return 'fast must be less than slow'
  if (s > 600_000) return 'slow capped at 600,000 (10 min)'
  return null
})

function saveGlobalThresholds() {
  if (thresholdError.value) return
  const fast = resolvedThreshold(fastInput.value, DEFAULT_FAST_MS) ?? DEFAULT_FAST_MS
  const slow = resolvedThreshold(slowInput.value, DEFAULT_SLOW_MS) ?? DEFAULT_SLOW_MS
  emit('update', {
    slow_request_thresholds: { fast_ms: fast, slow_ms: slow },
  })
}

function resetGlobalThresholds() {
  // Sending `null` clears the global override server-side via the custom
  // Option<Option<...>> deserialiser; auto-detection takes over again.
  // The watchEffect re-populates the inputs with the auto defaults.
  emit('update', { slow_request_thresholds: null })
}

function basename(p: string): string {
  const m = p.match(/[^\\/]+$/)
  return m ? m[0] : p
}

interface ResetConfig {
  scope: 'settings' | 'session' | 'patterns' | 'index' | 'highlight' | 'all'
  title: string
  message: string
}

async function confirmReset(cfg: ResetConfig) {
  const ok = await ask(cfg.message, {
    title: cfg.title,
    kind: 'warning',
    okLabel: 'Reset',
    cancelLabel: 'Cancel',
  })
  if (ok) emit('reset-data', cfg.scope)
}

function onResetHighlight() {
  void confirmReset({
    scope: 'highlight',
    title: 'Reset highlight rules',
    message: 'Delete all global and per-file highlight rules? Built-in rules will continue to apply.',
  })
}

function onResetSession() {
  void confirmReset({
    scope: 'session',
    title: 'Reset session state',
    message: 'Forget the currently-open tabs, scroll positions and search state? No log files will be touched.',
  })
}

function onResetSettings() {
  void confirmReset({
    scope: 'settings',
    title: 'Reset settings',
    message: 'Restore theme, font size, follow-tail and recent-files settings to defaults?',
  })
}

function onResetPatterns() {
  void confirmReset({
    scope: 'patterns',
    title: 'Reset pattern overrides',
    message: 'Forget every per-file pattern override? Files will use auto-detection on next open.',
  })
}

function onResetIndex() {
  void confirmReset({
    scope: 'index',
    title: 'Clear index cache',
    message: 'Clear the on-disk index cache? The next open of each file will rebuild its index from scratch.',
  })
}

function onResetAll() {
  void confirmReset({
    scope: 'all',
    title: 'Reset all data',
    message: 'This wipes settings, session, pattern overrides, highlight rules and the index cache. Open log files themselves are untouched. Continue?',
  })
}
</script>

<template>
  <BaseModal title="Settings" modal-class="settings-modal" @close="emit('close')">
    <div class="settings-tabs" role="tablist">
      <button
        type="button"
        role="tab"
        class="tab-btn"
        :class="{ 'is-on': activeTab === 'general' }"
        :aria-selected="activeTab === 'general'"
        @click="activeTab = 'general'"
      >General</button>
      <button
        type="button"
        role="tab"
        class="tab-btn"
        :class="{ 'is-on': activeTab === 'slow-requests' }"
        :aria-selected="activeTab === 'slow-requests'"
        @click="activeTab = 'slow-requests'"
      >Slow requests</button>
      <button
        type="button"
        role="tab"
        class="tab-btn"
        :class="{ 'is-on': activeTab === 'highlighting' }"
        :aria-selected="activeTab === 'highlighting'"
        @click="activeTab = 'highlighting'"
      >Highlighting</button>
      <button
        type="button"
        role="tab"
        class="tab-btn"
        :class="{ 'is-on': activeTab === 'advanced' }"
        :aria-selected="activeTab === 'advanced'"
        @click="activeTab = 'advanced'"
      >Advanced</button>
    </div>

    <!-- General -->
    <section v-if="activeTab === 'general'" class="tab-panel" role="tabpanel">
      <h3>Appearance</h3>
      <div class="row-grid">
        <span class="row-label">Theme</span>
        <span class="seg">
          <button
            v-for="opt in (['system', 'light', 'dark'] as const)"
            :key="opt"
            type="button"
            class="seg-btn"
            :class="{ 'is-on': settings.theme === opt }"
            @click="emit('update', { theme: opt })"
          >{{ opt[0].toUpperCase() + opt.slice(1) }}</button>
        </span>
      </div>
      <div class="row-grid">
        <span class="row-label">Font size</span>
        <span class="seg font-seg">
          <button type="button" class="seg-btn" @click="emit('bump-font', -1)" title="Decrease (Ctrl--)">&minus;</button>
          <button type="button" class="seg-btn font-val" @click="emit('reset-font')" title="Reset to default (Ctrl-0)">{{ settings.font_size }}px</button>
          <button type="button" class="seg-btn" @click="emit('bump-font', 1)" title="Increase (Ctrl-+)">+</button>
        </span>
      </div>

      <div class="row-grid">
        <label for="colour-blind" class="row-label">Colour-blind mode</label>
        <span class="control-cell">
          <input
            id="colour-blind"
            type="checkbox"
            :checked="!!settings.colour_blind"
            @change="(e: Event) => emit('update', { colour_blind: (e.target as HTMLInputElement).checked })"
          />
        </span>
      </div>

      <h3>Behaviour</h3>
      <div class="row-grid">
        <label for="follow-tail-default">Follow tail by default</label>
        <span class="control-cell">
          <input
            id="follow-tail-default"
            type="checkbox"
            :checked="settings.follow_tail_default"
            @change="(e: Event) => emit('update', { follow_tail_default: (e.target as HTMLInputElement).checked })"
          />
        </span>
      </div>

      <h3>Recent files</h3>
      <ul v-if="settings.recent_files.length > 0" class="recent-list">
        <li v-for="p in settings.recent_files" :key="p">
          <button type="button" class="open-btn" @click="emit('open-recent', p)">{{ basename(p) }}</button>
          <span class="path">{{ p }}</span>
          <button type="button" class="btn-dismiss is-destructive forget-btn" @click="emit('forget-recent', p)" title="Remove from list" aria-label="Forget recent file">
            <svg class="dismiss-glyph" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
              <path d="M4 4 L12 12 M12 4 L4 12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" fill="none" />
            </svg>
          </button>
        </li>
      </ul>
      <p v-else class="muted">No recent files yet. Open a log to populate this list.</p>
    </section>

    <!-- Slow requests -->
    <section v-if="activeTab === 'slow-requests'" class="tab-panel" role="tabpanel">
      <h3>Global thresholds</h3>
      <p class="muted hint">
        Set the thresholds for slow requests; values less than or equal to 'fast' show <span style="color: var(--speed-fast);">green</span>,
        values greater than or equal to 'slow' show <span style="color: var(--speed-slow);">red</span>, and in between blend through <span style="color: var(--speed-mid);">yellow</span>.
        Both capped at 600,000 ms (10 minutes). Per-file overrides
        from the insights drawer take precedence over these.
      </p>
      <div class="threshold-grid">
        <label>Fast (ms) <input v-model="fastInput" type="number" min="0" max="600000" step="100" placeholder="2000" /></label>
        <label>Slow (ms) <input v-model="slowInput" type="number" min="0" max="600000" step="100" placeholder="10000" /></label>
      </div>
      <div v-if="thresholdError" class="threshold-error">{{ thresholdError }}</div>
      <div class="threshold-actions">
        <button
          type="button"
          class="seg-btn"
          :disabled="!!thresholdError"
          @click="saveGlobalThresholds"
        >Save</button>
        <button type="button" class="seg-btn" @click="resetGlobalThresholds">Reset to default</button>
      </div>
    </section>

    <!-- Highlighting -->
    <section v-if="activeTab === 'highlighting'" class="tab-panel" role="tabpanel">
      <HighlightRulesEditor
        :model-value="globalRules"
        scope="global"
        @save="(rules) => emit('save-global-rules', rules)"
      />
      <div class="row-grid reset-row">
        <span class="row-label">Reset rules</span>
        <span class="control-cell">
          <button
            type="button"
            class="seg-btn danger"
            :disabled="globalRules.length === 0"
            @click="onResetHighlight"
          >Reset all highlight rules</button>
        </span>
      </div>
      <p class="footer-note muted">
        Built-in highlights (Java exceptions, <code>Caused by:</code>, stack frames, file paths, URLs)
        always apply. User rules layer on top by priority; per-file rules layer on top of global ones.
      </p>
    </section>

    <!-- Advanced -->
    <section v-if="activeTab === 'advanced'" class="tab-panel" role="tabpanel">
      <h3>Data folder</h3>
      <div class="row-grid">
        <span class="row-label">Location</span>
        <span class="control-cell data-cell">
          <code class="data-path">{{ dataDir?.path ?? '(loading...)' }}</code>
          <span v-if="dataDir?.portable" class="badge">portable</span>
          <button type="button" class="seg-btn" @click="emit('open-data-folder')">Open folder</button>
        </span>
      </div>

      <h3>Reset</h3>
      <div class="reset-grid">
        <div class="row-grid">
          <span class="row-label">Session state</span>
          <span class="control-cell"><button type="button" class="seg-btn danger" @click="onResetSession">Reset</button></span>
        </div>
        <div class="row-grid">
          <span class="row-label">Settings</span>
          <span class="control-cell"><button type="button" class="seg-btn danger" @click="onResetSettings">Reset</button></span>
        </div>
        <div class="row-grid">
          <span class="row-label">Pattern overrides</span>
          <span class="control-cell"><button type="button" class="seg-btn danger" @click="onResetPatterns">Reset</button></span>
        </div>
        <div class="row-grid">
          <span class="row-label">Index cache</span>
          <span class="control-cell"><button type="button" class="seg-btn danger" @click="onResetIndex">Clear</button></span>
        </div>
        <div class="row-grid">
          <span class="row-label">Everything</span>
          <span class="control-cell"><button type="button" class="seg-btn danger" @click="onResetAll">Reset all data</button></span>
        </div>
      </div>

      <p class="footer-note muted">
        Automatic update checks are planned for a later milestone.
      </p>
    </section>
  </BaseModal>
</template>

<style scoped>
:deep(.settings-modal) { width: min(720px, 94vw); max-height: 86vh; }

.settings-tabs {
  display: flex;
  gap: 0;
  margin: -1rem -1.2rem 1rem;
  padding: 0 1.2rem;
  border-bottom: 1px solid var(--border-default);

  .tab-btn {
    background: transparent;
    color: var(--fg-muted);
    border: 0;
    border-bottom: 2px solid transparent;
    padding: 0.55rem 1rem;
    margin-bottom: -1px;
    font-size: 0.9rem;
    font-weight: 500;
    cursor: pointer;

    &:hover { color: var(--fg-default); }

    &.is-on {
      color: var(--fg-default);
      border-bottom-color: var(--accent);
    }
  }
}

.tab-panel {
  /* Pin a stable working height so switching tabs doesn't reflow the
     modal up and down. Internal scroll picks up whatever overflows. */
  height: 62vh;
  overflow-y: auto;
  padding-right: 0.4rem;
  animation: tab-fade 120ms ease-out;
}
@keyframes tab-fade {
  from { opacity: 0; transform: translateY(2px); }
  to { opacity: 1; transform: translateY(0); }
}

h3 {
  margin: 1.2rem 0 0.4rem;
  font-size: 0.95rem;
  border-bottom: 1px solid var(--border-default);
  padding-bottom: 0.25rem;
}
h3:first-of-type { margin-top: 0; }

p.muted { color: var(--fg-muted); font-size: 0.85rem; margin: 0.4rem 0; }
code { background: var(--bg-button); padding: 0.05rem 0.3rem; border-radius: 3px; font-family: var(--font-mono); }

.row-grid {
  display: grid;
  grid-template-columns: 10rem 1fr;
  align-items: center;
  gap: 0.8rem;
  margin: 0.35rem 0;
}
.reset-row { margin-top: 1rem; }
.row-label { color: var(--fg-muted); font-size: 0.85rem; }
.control-cell { display: inline-flex; align-items: center; gap: 0.5rem; min-width: 0; }

.seg { display: inline-flex; gap: 0.3rem; }
.seg-btn {
  background: var(--bg-button);
  color: var(--fg-default);
  border: 1px solid var(--border-button);
  border-radius: var(--radius-sm);
  padding: 0.3rem 0.7rem;
  font-size: 0.85rem;
  cursor: pointer;

  &.is-on { background: var(--fg-default); color: var(--bg-app); border-color: var(--fg-default); }
  &.danger { color: var(--level-error); border-color: var(--level-error); }

  /* Disabled wins over .danger / .is-on - the button should read as
     "inert" regardless of whatever modifier was applied. Listed last so
     same-specificity source order favours it. */
  &:disabled,
  &.danger:disabled,
  &.is-on:disabled {
    color: var(--fg-dim);
    border-color: var(--border-default);
    background: var(--bg-button);
    cursor: not-allowed;
    opacity: 0.55;
  }
}
.font-seg .font-val { font-family: var(--font-mono); min-width: 3.5rem; text-align: center; }

.recent-list {
  list-style: none;
  padding: 0;
  margin: 0.3rem 0 0;
  max-height: 14rem;
  overflow-y: auto;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);

  li {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.3rem 0.5rem;
    border-bottom: 1px dashed var(--border-default);
    font-size: 0.82rem;

    &:last-child { border-bottom: 0; }

    .open-btn {
      background: transparent;
      border: 0;
      color: var(--accent);
      cursor: pointer;
      padding: 0;
      font-weight: 600;
      flex: 0 0 auto;

      &:hover { text-decoration: underline; }
    }

    .path {
      color: var(--fg-dim);
      font-family: var(--font-mono);
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      flex: 1;
      min-width: 0;
    }

    .forget-btn {
      width: 1.4rem;
      height: 1.4rem;
      font-size: 1.05rem;
    }
  }
}

.data-cell {
  flex-wrap: wrap;

  .data-path {
    background: var(--bg-button);
    padding: 0.2rem 0.45rem;
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: 0.8rem;
    color: var(--fg-default);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
    flex: 1;
  }

  .badge {
    padding: 0.05rem 0.4rem;
    border-radius: var(--radius-sm);
    background: var(--accent);
    color: var(--fg-on-accent);
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
}

.reset-grid { margin-top: 0.4rem; display: flex; flex-direction: column; gap: 0; }

.hint { margin-bottom: 0.5rem; }
.hint-inline { font-size: 0.78rem; line-height: 1.35; max-width: 26rem; }
.threshold-grid {
  display: flex;
  gap: 0.8rem;
  margin: 0.4rem 0;

  & label { color: var(--fg-muted); font-size: 0.85rem; display: inline-flex; align-items: center; gap: 0.4rem; }
  & input {
    width: 6rem;
    background: var(--bg-viewport);
    border: 1px solid var(--border-button);
    color: var(--fg-default);
    padding: 0.15rem 0.4rem;
    border-radius: var(--radius-sm);
  }
}
.threshold-error { color: var(--level-error); font-size: 0.8rem; margin: 0.2rem 0; }
.threshold-actions { display: flex; gap: 0.4rem; margin-top: 0.4rem; }
.footer-note { margin-top: 1.2rem; padding-top: 0.6rem; border-top: 1px solid var(--border-default); font-size: 0.8rem; }
</style>
