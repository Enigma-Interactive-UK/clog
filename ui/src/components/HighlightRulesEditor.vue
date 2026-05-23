<script setup lang="ts">
/**
 * Editable table of user highlight rules with a live preview pane.
 *
 * Used in two scopes:
 *   - global: rules apply across every open file.
 *   - per-file: rules apply only to the active file.
 *
 * The component owns a draft `rules` ref initialised from the `modelValue`
 * prop and emits `save` with the draft when the user clicks Save. The
 * parent (App.vue) routes that through useHighlightRules.saveGlobal /
 * savePerFile, which both persist to disk and refresh the engine.
 *
 * Live preview compiles the draft rules against a small sample of
 * representative log lines (or, when the parent passes one, the active
 * file's visible records). The preview uses the engine's `computeWith`
 * helper so the in-flight draft never disturbs the active engine state.
 */

import { computed, reactive, ref, watch } from 'vue'

import ColourPickerPopover from './ColourPickerPopover.vue'
import { computeWith, tryCompileRule } from '../highlight/engine'
import { userToEngineRule } from '../highlight/user-rule'
import { newUserRule, type UserHighlightRule } from '../types'

const props = defineProps<{
  modelValue: UserHighlightRule[]
  scope: 'global' | 'per-file'
  /** Optional override of the preview source text. One string per line. */
  previewLines?: string[]
}>()

const emit = defineEmits<{
  (e: 'save', rules: UserHighlightRule[]): void
  (e: 'forget'): void
  (e: 'dirty', dirty: boolean): void
}>()

const DEFAULT_PREVIEW_LINES = [
  '[TRACE] 2026-05-22 16:28:58.012 [main] play - Entering Bootstrap.start(args=[--config=/etc/play.conf])',
  '[DEBUG] 2026-05-22 16:28:58.103 [main] play - Resolved JAVA_HOME=/usr/lib/jvm/temurin-21 (build 21.0.5+11-LTS)',
  '[INFO ] 2026-05-22 16:28:59.001 [main] play - Listening on http://0.0.0.0:9000 (pid 18472, host wsl-oink)',
  '[INFO ] 2026-05-22 16:29:01.001 [worker-3] play - Loaded module from C:\\opt\\play\\modules\\crud (12 controllers, 4 jobs)',
  '[WARN ] 2026-05-22 16:29:02.412 [worker-3] play - Slow query in /var/log/queries.sql took 1.42s (threshold 500ms)',
  '[WARN ] 2026-05-22 16:29:02.502 [worker-3] play - Retry 2/3 for upstream 10.0.4.17:5432 after timeout',
  '[ERROR] 2026-05-22 16:29:03.246 [main] play - Failed to start: java.lang.IllegalStateException: db unreachable',
  '    at com.cheesecake.boot.Bootstrap.start(Bootstrap.java:42)',
  '    at com.cheesecake.boot.Main.main(Main.java:18)',
  '    at jdk.internal.reflect.NativeMethodAccessorImpl.invoke0(Native Method)',
  'Caused by: java.net.ConnectException: Connection refused to https://db.internal:5432/cheesecake?ssl=true',
  '    ... 14 more',
  '[FATAL] 2026-05-22 16:29:03.901 [main] play - Aborting boot - see /var/log/play/clog.log for the full trace',
  'GET /api/orders/47821 HTTP/1.1 -> 503 in 1812ms (client 192.168.1.42, ua "curl/8.7.0")',
  '[INFO ] 2026-05-22 16:29:05.117 [scheduler] play - Job ImportFeed-d3f9a1c2 finished ok (842 rows, 0 errors)',
]

const rules = reactive<UserHighlightRule[]>(deepClone(props.modelValue))
const initialSnapshot = ref(JSON.stringify(props.modelValue))

const isDirty = computed(() => JSON.stringify(rules) !== initialSnapshot.value)
watch(isDirty, (d) => emit('dirty', d), { immediate: true })

watch(() => props.modelValue, (next) => {
  rules.splice(0, rules.length, ...deepClone(next))
  initialSnapshot.value = JSON.stringify(next)
}, { deep: false })

function deepClone(rs: UserHighlightRule[]): UserHighlightRule[] {
  return rs.map((r) => ({ ...r }))
}

function addRule() {
  rules.push(newUserRule(`rule-${rules.length + 1}`))
}

function removeRule(idx: number) {
  rules.splice(idx, 1)
}

function moveRule(idx: number, delta: number) {
  const j = idx + delta
  if (j < 0 || j >= rules.length) return
  const [r] = rules.splice(idx, 1)
  rules.splice(j, 0, r)
}

function save() {
  emit('save', deepClone(rules))
  initialSnapshot.value = JSON.stringify(rules)
}

function reset() {
  rules.splice(0, rules.length, ...deepClone(props.modelValue))
  initialSnapshot.value = JSON.stringify(props.modelValue)
}

// --- Per-rule compile validation -----------------------------------------

interface RuleStatus {
  error: string | null
  matchCount: number
  visibleChars: number
}

const previewText = computed(() => (props.previewLines ?? DEFAULT_PREVIEW_LINES).join('\n'))

const ruleStatuses = computed<RuleStatus[]>(() => {
  const text = previewText.value
  return rules.map((u) => {
    if (!u.pattern) return { error: null, matchCount: 0, visibleChars: 0 }
    const engineRule = userToEngineRule(u)
    const compileError = tryCompileRule(engineRule)
    if (compileError) return { error: compileError, matchCount: 0, visibleChars: 0 }
    // Count matches by running the regex alone.
    let count = 0
    try {
      const flags = ensureFlags(u.flags || '')
      const re = new RegExp(u.pattern, flags)
      let m: RegExpExecArray | null
      let guard = 0
      while ((m = re.exec(text)) !== null) {
        if (++guard > 1024) break
        if (m[0].length === 0) { re.lastIndex++; continue }
        count++
      }
    } catch {
      // covered by tryCompileRule above
    }
    return { error: null, matchCount: count, visibleChars: 0 }
  })
})

function ensureFlags(flags: string): string {
  let out = flags
  if (!out.includes('g')) out += 'g'
  if (!out.includes('d')) out += 'd'
  return out
}

// --- Preview spans (effective rules; runs the engine's overlap merge) ----

interface PreviewLine {
  text: string
  segments: { text: string; cls: string; url?: string }[]
}

const previewLines = computed<PreviewLine[]>(() => {
  const lines = props.previewLines ?? DEFAULT_PREVIEW_LINES
  const engineRules = rules
    .filter((r) => r.enabled && r.pattern && !tryCompileRule(userToEngineRule(r)))
    .map(userToEngineRule)
  return lines.map((line) => {
    const result = computeWith(engineRules, line)
    if (!result.ok || result.spans.length === 0) {
      return { text: line, segments: [{ text: line, cls: '' }] }
    }
    const segs: { text: string; cls: string; url?: string }[] = []
    let cursor = 0
    for (const sp of result.spans) {
      if (sp.start > cursor) segs.push({ text: line.slice(cursor, sp.start), cls: '' })
      const seg: { text: string; cls: string; url?: string } = {
        text: line.slice(sp.start, sp.end),
        cls: sp.cls,
      }
      if (sp.url) seg.url = sp.url
      segs.push(seg)
      cursor = sp.end
    }
    if (cursor < line.length) segs.push({ text: line.slice(cursor), cls: '' })
    return { text: line, segments: segs }
  })
})

// --- Overlap detection ---------------------------------------------------
// Flag rules whose every match was painted over by something
// higher-priority. The cheap heuristic: render the full set, then render
// the set with this rule removed; if the painted-char count is the same,
// the rule is fully overridden in the preview window.

const overlapStatus = computed<boolean[]>(() => {
  const compiledRules = rules.map((r) => ({
    rule: r,
    engine: r.pattern ? userToEngineRule(r) : null,
  }))
  const enabledRules = compiledRules
    .filter((c) => c.rule.enabled && c.engine && !tryCompileRule(c.engine))
    .map((c) => c.engine!)
  const text = previewText.value
  if (text.length === 0) return rules.map(() => false)

  const fullResult = computeWith(enabledRules, text)
  if (!fullResult.ok) return rules.map(() => false)
  const fullPaint = paintMap(fullResult.spans)

  return rules.map((r, idx) => {
    if (!r.enabled || !r.pattern) return false
    const status = ruleStatuses.value[idx]
    if (!status || status.error || status.matchCount === 0) return false
    // Re-render without this rule.
    const subset = enabledRules.filter((er) => er.name !== r.name)
    const withoutResult = computeWith(subset, text)
    if (!withoutResult.ok) return false
    const withoutPaint = paintMap(withoutResult.spans)
    return fullPaint === withoutPaint
  })
})

function paintMap(spans: { start: number; end: number; cls: string }[]): string {
  return spans.map((s) => `${s.start}:${s.end}:${s.cls}`).join('|')
}
</script>

<template>
  <div class="rules-editor">
    <header class="editor-head">
      <h4>{{ scope === 'global' ? 'Global rules' : 'Rules for this file' }}</h4>
      <div class="head-actions">
        <button type="button" class="seg-btn" @click="addRule">+ Add rule</button>
        <button type="button" class="seg-btn" :disabled="!isDirty" @click="reset">Revert</button>
        <button type="button" class="seg-btn primary" :disabled="!isDirty" @click="save">Save</button>
        <button
          v-if="scope === 'per-file'"
          type="button"
          class="seg-btn danger"
          :disabled="rules.length === 0"
          @click="emit('forget')"
        >Forget all</button>
      </div>
    </header>

    <p v-if="rules.length === 0" class="muted">
      No {{ scope === 'global' ? 'global' : 'file-specific' }} highlight rules yet.
      Click <em>+ Add rule</em> to create one. Built-in rules (Java exceptions,
      <code>Caused by:</code>, stack frames, paths, URLs) always apply.
    </p>

    <table v-else class="rules-table">
      <thead>
        <tr>
          <th class="col-on"></th>
          <th class="col-name">Name</th>
          <th class="col-pattern">Pattern (regex)</th>
          <th class="col-colour">Colour</th>
          <th class="col-style">Style</th>
          <th class="col-pri">Priority</th>
          <th class="col-actions"></th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="(r, idx) in rules" :key="idx" :class="{ disabled: !r.enabled, overlapped: overlapStatus[idx] }">
          <td class="col-on">
            <input type="checkbox" v-model="r.enabled" :title="r.enabled ? 'Enabled' : 'Disabled'" />
          </td>
          <td class="col-name">
            <input type="text" v-model="r.name" placeholder="rule name" spellcheck="false" />
          </td>
          <td class="col-pattern">
            <input
              type="text"
              v-model="r.pattern"
              class="pat"
              :class="{ 'pat-error': ruleStatuses[idx]?.error }"
              :title="ruleStatuses[idx]?.error ?? `${ruleStatuses[idx]?.matchCount ?? 0} matches in preview`"
              spellcheck="false"
              placeholder="e.g. \\bFoundation\\w*"
            />
            <span v-if="ruleStatuses[idx]?.error" class="hint err">{{ ruleStatuses[idx]?.error }}</span>
            <span v-else-if="overlapStatus[idx]" class="hint warn">All matches in preview are overridden by a higher-priority rule.</span>
            <span v-else class="hint muted">{{ ruleStatuses[idx]?.matchCount ?? 0 }} preview match{{ (ruleStatuses[idx]?.matchCount ?? 0) === 1 ? '' : 'es' }}</span>
          </td>
          <td class="col-colour">
            <ColourPickerPopover
              :colour="r.colour"
              :background="r.background"
              @update:colour="(v) => r.colour = v"
              @update:background="(v) => r.background = v"
            />
          </td>
          <td class="col-style">
            <label title="Bold"><input type="checkbox" v-model="r.bold" /> <b>B</b></label>
            <label title="Italic"><input type="checkbox" v-model="r.italic" /> <i>I</i></label>
            <label title="Underline"><input type="checkbox" v-model="r.underline" /> <u>U</u></label>
          </td>
          <td class="col-pri">
            <input type="number" v-model.number="r.priority" min="0" max="999" />
          </td>
          <td class="col-actions">
            <button type="button" class="icon-btn" @click="moveRule(idx, -1)" :disabled="idx === 0" title="Move up">&uarr;</button>
            <button type="button" class="icon-btn" @click="moveRule(idx, 1)" :disabled="idx === rules.length - 1" title="Move down">&darr;</button>
            <button type="button" class="icon-btn danger" @click="removeRule(idx)" title="Delete">&times;</button>
          </td>
        </tr>
      </tbody>
    </table>

    <h4 class="preview-head">Preview</h4>
    <pre class="preview-pane"><span
        v-for="(line, lineIdx) in previewLines"
        :key="lineIdx"
        class="preview-row"
      ><span
        v-for="(seg, segIdx) in line.segments"
        :key="segIdx"
        :class="seg.cls"
      >{{ seg.text }}</span>{{ '\n' }}</span></pre>
  </div>
</template>

<style scoped>
.rules-editor {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.editor-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.6rem;

  h4 { margin: 0; font-size: 0.95rem; }
  .head-actions { display: inline-flex; gap: 0.3rem; }
}

.muted { color: var(--fg-muted); font-size: 0.85rem; }

.rules-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.82rem;

  th, td {
    text-align: left;
    padding: 0.25rem 0.4rem;
    border-bottom: 1px solid var(--border-default);
    vertical-align: top;
  }
  th {
    font-size: 0.75rem;
    color: var(--fg-muted);
    font-weight: 600;
    background: var(--bg-elevated);
  }
  tr.disabled td { opacity: 0.5; }
  tr.overlapped .col-pattern .pat { border-color: var(--level-warn); }

  input[type="text"], input[type="number"] {
    width: 100%;
    background: var(--bg-viewport);
    color: var(--fg-default);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    padding: 0.2rem 0.4rem;
    font-family: var(--font-mono);
    font-size: 0.8rem;

    &.pat-error { border-color: var(--level-error); }
  }
}

.col-on { width: 1.4rem; }
.col-name { width: 8rem; }
.col-colour { width: 3rem; }
.col-style { width: 7rem; white-space: nowrap; }
.col-pri { width: 5rem; }
.col-actions { width: 6rem; white-space: nowrap; }

.hint {
  display: block;
  font-size: 0.7rem;
  margin-top: 0.15rem;

  &.err { color: var(--fg-error); font-family: var(--font-mono); }
  &.warn { color: var(--level-warn); }
  &.muted { color: var(--fg-dim); }
}

.col-style label {
  display: inline-flex;
  align-items: center;
  gap: 0.15rem;
  margin-right: 0.3rem;
  font-size: 0.75rem;
  color: var(--fg-muted);
}

.palette {
  display: inline-flex;
  gap: 0.2rem;
  flex-wrap: wrap;

  .swatch {
    width: 1.3rem;
    height: 1.3rem;
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    background: var(--bg-button);
    cursor: pointer;
    font-weight: 700;
    line-height: 1;
    padding: 0;

    &.is-on { outline: 2px solid var(--accent); outline-offset: 1px; }
  }
}

.icon-btn {
  background: transparent;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  color: var(--fg-default);
  cursor: pointer;
  padding: 0.1rem 0.4rem;
  font-size: 0.85rem;

  &:disabled { opacity: 0.4; cursor: not-allowed; }
  &.danger { color: var(--level-error); border-color: var(--level-error); }
  &:hover:not(:disabled) { background: var(--bg-button-hover); }
}

.seg-btn {
  background: var(--bg-button);
  color: var(--fg-default);
  border: 1px solid var(--border-button);
  border-radius: var(--radius-sm);
  padding: 0.3rem 0.7rem;
  font-size: 0.85rem;
  cursor: pointer;

  &:disabled { opacity: 0.4; cursor: not-allowed; }
  &.primary { background: var(--accent); color: var(--fg-on-accent); border-color: var(--accent); }
  &.danger { color: var(--level-error); border-color: var(--level-error); }
}

.preview-head { margin: 0.8rem 0 0.2rem; font-size: 0.9rem; color: var(--fg-muted); }

.preview-pane {
  background: var(--bg-viewport);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  padding: 0.4rem 0.6rem;
  margin: 0;
  font-family: var(--font-mono);
  font-size: 0.78rem;
  color: var(--fg-default);
  overflow-x: auto;
  max-height: 15rem;
  overflow-y: auto;
  white-space: pre;
}
.preview-row { display: block; }
</style>
