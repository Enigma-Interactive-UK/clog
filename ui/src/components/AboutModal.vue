<script setup lang="ts">
/**
 * About modal. Lazily resolves the Tauri app name/version/tauri-version on
 * first open and caches the result for the lifetime of the window.
 */

import { ref } from 'vue'
import { getName, getTauriVersion, getVersion } from '@tauri-apps/api/app'
import { openUrl } from '@tauri-apps/plugin-opener'
import BaseModal from './BaseModal.vue'

defineProps<{ open: boolean }>()

const emit = defineEmits<{ (e: 'close'): void }>()

const aboutInfo = ref<{ name: string; version: string; tauri: string } | null>(null)

export interface AboutModalExposed {
  ensureLoaded: () => Promise<void>
}

async function ensureLoaded() {
  if (aboutInfo.value) return
  try {
    const [name, version, tauri] = await Promise.all([
      getName(), getVersion(), getTauriVersion(),
    ])
    aboutInfo.value = { name, version, tauri }
  } catch {
    aboutInfo.value = { name: 'Clog', version: 'unknown', tauri: 'unknown' }
  }
}

defineExpose({ ensureLoaded })
</script>

<template>
  <BaseModal
    v-if="open"
    :title="`About Clog ${aboutInfo?.version ?? '...'}`"
    aria-label="About Clog"
    modal-class="about-modal"
    @close="emit('close')"
  >
    <div class="about-hero">
      <img src="/clog-icon.png" alt="" class="about-icon" />
      <div>
        <h3 class="about-name">{{ aboutInfo?.name ?? 'Clog' }} 👞</h3>
        <p class="about-tag muted">The Core log viewer: tail, search and filter your logs.</p>
        <p class="about-version">Version <code>{{ aboutInfo?.version ?? '...' }}</code></p>
      </div>
    </div>
    <br>
    <h3>Credits</h3>
    <p>
      Vibed by <a href="https://github.com/lewster32/" class="link-btn" target="_blank" rel="noopener noreferrer">Lewis Lane</a> for <a href="https://www.enigma-interactive.co.uk" class="link-btn" target="_blank" rel="noopener noreferrer">Enigma Interactive</a>.
      A collaboration of meat and metal, built with blood, sweat and oil using <a class="link-btn" href="https://claude.ai" target="_blank" rel="noopener noreferrer">Claude</a> Opus 4.7 🦀 
    </p>

    <h3>Built with</h3>
    <ul class="dep-list">
      <li><strong>Rust</strong>: the engine, parser, search and tail loop.</li>
      <li><strong>Tauri</strong> v{{ aboutInfo?.tauri ?? '2.x' }}: <a href="https://tauri.app/" class="link-btn" target="_blank" rel="noopener noreferrer">tauri.app</a></li>
      <li><strong>Vue 3</strong> + <strong>Vite</strong> + <strong>TypeScript</strong>: the UI shell.</li>
      <li><strong>@tanstack/vue-virtual</strong>: virtualised line viewer.</li>
      <li><strong>rayon</strong>: parallel record search.</li>
    </ul>

    <p class="footer-note muted">
      Licensed under the MIT license. See the repository for full third-party notices.
    </p>
  </BaseModal>
</template>

<style scoped>
:deep(.about-modal) { max-width: 500px; }

h3 {
  margin: 1.2rem 0 0.4rem;
  font-size: 0.95rem;
  border-bottom: 1px solid var(--border-default);
  padding-bottom: 0.25rem;
}
h3:first-of-type { margin-top: 0; }

p.muted { color: var(--fg-muted); font-size: 0.85rem; margin: 0.4rem 0; }
code { background: var(--bg-button); padding: 0.05rem 0.3rem; border-radius: 3px; font-family: var(--font-mono); }

.about-hero {
  display: flex;
  gap: 1rem;
  align-items: center;
  margin-bottom: 0.4rem;
}
.about-icon { width: 64px; height: 64px; object-fit: contain; flex: 0 0 auto; }
.about-name { margin: 0; font-size: 1.2rem; border: 0; padding: 0; }
.about-tag { margin: 0.2rem 0 0.3rem; font-size: 0.9rem; }
.about-version {
  margin: 0;
  font-size: 0.85rem;
  color: var(--fg-muted);

  code { color: var(--fg-default); font-family: var(--font-mono); }
}
.dep-list { padding-left: 1.2rem; font-size: 0.85rem; line-height: 1.5; }
.link-btn {
  background: transparent;
  border: 0;
  color: var(--accent);
  cursor: pointer;
  padding: 0;
  font: inherit;
  text-decoration: underline;
  text-underline-offset: 2px;

  &:hover { color: var(--fg-default); }
}

.footer-note { margin-top: 1.2rem; padding-top: 0.6rem; border-top: 1px solid var(--border-default); font-size: 0.8rem; }
</style>
