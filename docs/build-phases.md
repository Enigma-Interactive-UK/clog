# Clog v1 — Build phases

> Snapshot dated 2026-05-23. Each phase is a vertical slice: it ends with
> something runnable and demoable, builds on the previous phase's code, and is
> sized to fit one focused working session. Phase IDs are stable (P1..P10)
> and may be referenced from commits, branches, and session notes.
>
> The design that these phases implement lives in `docs/design.md`. The
> per-decision rationale lives in `.wolf/cerebrum.md`.

## How to use this document

At the start of a session, identify the phase being worked on. Read its
**Scope**, **Done-criteria** and **Demo**. Cross-references like `design §4`
point at sections of `docs/design.md`.

A phase is *done* when:

1. Its scope is implemented.
2. Its listed tests are landing green in CI.
3. Its **Demo** can be performed on the developer's machine.
4. The state is committed and pushed.

Deferred work belongs to a later phase or to the explicit non-goals list in
`docs/design.md §17`. Resist scope creep into the current phase.

## Phase index

| ID  | Name                              | Size  | Demo in one line |
|-----|-----------------------------------|-------|------------------|
| P1  | It opens                          | S     | Picker shows file + line count |
| P2  | It shows records                  | M     | Smooth scrolling of the whole sample log |
| P3  | It parses any pattern             | M     | Both production patterns render with field colouring |
| P4  | It tails                          | M     | Live appends + simulated rotation behave seamlessly |
| P5  | It highlights stack traces        | S-M   | Exception classes and file paths pop in stack traces |
| P6  | It searches and filters           | M-L   | Smart search ranks hits; regex toggle works |
| P7  | It remembers                      | M     | Close and reopen the app; everything restored |
| P8  | It is customisable                | M     | Rule editor with live preview; per-file overrides |
| P9  | It multitabs                      | M     | Four files open simultaneously, tabs persist |
| P10 | It ships                          | M     | Installer runs cleanly on a fresh Windows VM |

---

## P1 — It opens

**Scope**

- Cargo workspace at the repo root with `crates/clog-core` and `crates/clog-app`.
- `ui/` directory as a sibling, scaffolded by `npm create vite` with the Vue +
  TypeScript template.
- Tauri v2 wired into `clog-app` with `tauri.conf.json` pointing `distDir` at
  `../../ui/dist`.
- One Rust command: `open_file(path: String) -> Result<FileSummary, IpcError>`,
  where `FileSummary { path, size_bytes, line_count }`.
- One UI surface: a button that opens the native file picker via
  `tauri-plugin-dialog`, calls `open_file`, and renders the returned summary
  as text.

**Done-criteria**

- `cargo build --workspace` and `npm -C ui run build` both succeed.
- `cargo tauri dev` launches a window, the file picker opens the sample log,
  and `"cheesecake.out -- 84,231 lines"` (or equivalent) appears.
- `cargo fmt --check` and `cargo clippy -D warnings` pass.
- README has a minimal "How to develop locally" section pointing at
  `cargo tauri dev`.

**Tests landed**

- One smoke test in `clog-core` that `line_count` of `research/cheesecake.out`
  matches a known constant.
- No Playwright yet.

**Demo**

Open `cargo tauri dev`, click the file picker button, pick
`research/cheesecake.out`, see the line count.

**Cross-references:** design §2, §3.

**Deferred:** everything else.

---

## P2 — It shows records

**Scope**

- `LineSource` trait in `clog-core::source` with the `StreamedFile` impl.
- `LineIndex` (in-memory only; no disk caching yet) producing
  `Vec<u64>` of line-start offsets.
- A *hardcoded* `RecordScanner` for the wsl-oink pattern only
  (`[%-5level] %d{...} [%t] %c{1} - %msg%n`).
- `RecordHeader` struct and the parser that produces a `Vec<RecordHeader>`
  from the line index + bytes.
- IPC command: `get_records(file_id, range: Range<u64>) -> RecordsPayload`
  returning headers + raw bytes for that range.
- `@tanstack/vue-virtual` rendering raw record text in a virtualised list.
- One fixed row height; no styling, no colour, no gutter.

**Done-criteria**

- Opening `research/cheesecake.out` produces a fully scrollable view of the
  entire file.
- Scroll position is responsive (no perceptible jank) from top to bottom.
- The scrollbar accurately reflects total position from the first frame.

**Tests landed**

- Integration test asserting `RecordHeader` count, first/last record offsets,
  and that `header[i].byte_offset + header[i].byte_len == header[i+1].byte_offset`
  for the whole sample log.
- Playwright smoke test 1: open-file and confirm at least one record rendered.

**Demo**

Open the sample log, scroll from top to bottom, prove the list is responsive
and the scrollbar accurate.

**Cross-references:** design §3, §4, §9.

**Deferred:** persistent index cache (P7), pattern generalisation (P3), all
styling (P3+), highlight rules (P5).

---

## P3 — It parses any pattern

**Scope**

- PatternLayout compiler in `clog-core::pattern` covering the full supported
  subset (`%d{...}`, `%level`/`%-5level`/`%p`, `%t`, `%c`/`%c{N}`, `%msg`/`%m`,
  `%n`, literals).
- Auto-detect against the built-in pattern library (wsl-oink, prod, log4j2
  default) at file open.
- "User-paste pattern" UI: a field in the file's header bar where a custom
  pattern string can be entered, with a "test against first N lines: X% match"
  readout.
- Regex escape hatch accepting named captures.
- Axis-1 structural styling in the renderer: level coloured per
  `--gutter-{level}`, timestamp muted, thread normal, logger italic muted,
  separator dim, body default.
- 4 px level-coloured left gutter spanning all lines of each record.
- Sticky record header overlay when scrolling mid-record.
- CSS custom property token system (palette + semantic layers), **dark theme
  only** for now.
- Indented continuation lines.

**Done-criteria**

- Opening the wsl-oink sample (`research/cheesecake.out`) renders with full
  axis-1 colouring.
- Pasting the prod pattern (`%d{...} %level [%t] - %msg%n`) at the top bar
  and applying it makes the prod-shaped lines render correctly (use a
  hand-built test fixture for this).
- The "match score" readout is accurate.

**Tests landed**

- Unit tests for the PatternLayout compiler covering every supported
  specifier and at least three full patterns.
- The single `proptest` on the compiler (generators + parse-render-reparse
  property).
- Integration test asserting that auto-detect chooses the wsl-oink pattern
  for `cheesecake.out`.

**Demo**

Open the wsl-oink sample; observe full structural colouring. Paste a prod
pattern, open a prod-shaped synthetic fixture, observe correct rendering.

**Cross-references:** design §5, §8 (axis 1), §13.

**Deferred:** light theme + manual toggle (P7), highlight rules (P5),
persistent per-file pattern overrides (P7).

---

## P4 — It tails

**Scope**

- Polling tail loop in `clog-core::tail` (250 ms while active, idle otherwise).
- Rotation detection: size-shrink OR first-256-byte hash change.
- `tail_channel` IPC channel emitting `TailDelta { new_record_count,
  last_offset, rotated }`.
- 60 Hz coalescing layer in `clog-app::channels`.
- UI: auto-scroll toggle, "jump to bottom" button, visual indicator of
  follow-tail state.
- `fake_tailer` example binary in `crates/clog-core/examples/` that appends
  synthetic lines at configurable rate and supports a `--rotate` flag to
  truncate and rewrite the file.

**Done-criteria**

- Pointing clog at a file being written by `fake_tailer` shows new records
  appearing live, no visible flicker.
- Triggering `fake_tailer --rotate` produces a brief footer toast and
  uninterrupted tailing of the new file content.
- Auto-scroll can be toggled off (user pages up) and back on (jump to bottom).

**Tests landed**

- Tempfile-based integration test in `clog-core::tail` for:
  append-only growth, size-shrink rotation, hash-change rotation,
  detection latency within 1 polling interval.
- Playwright smoke test 2: tail-survives-rotation, driven by `fake_tailer`.

**Demo**

Run `cargo run --example fake_tailer -- ./scratch.log --rate 50` in one
terminal, open `./scratch.log` in clog, watch the lines stream. Hit Ctrl-C
on `fake_tailer`, re-run with `--rotate` and watch clog handle it without
losing position relative to the new file.

**Cross-references:** design §6, §10.

**Deferred:** tail status indicators in tab strip (P9), `notify` integration
(never).

---

## P5 — It highlights stack traces

**Scope**

- Axis-2 content styling pipeline in the Vue renderer.
- JS-side regex evaluation per visible line, with rule output merged on top
  of axis-1 spans by priority.
- A shipped `default-highlight-rules.json` baked into the app bundle:
  - Java exception class names (`\b\w+(?:Exception|Error)\b`)
  - `Caused by:` line
  - Stack frame `at <fqn>(<file>:<line>)` with sub-spans for class, method,
    file, line
  - File paths (Linux and Windows)
  - URLs (clickable via `shell.open`)
- Per-line cache of computed spans keyed by `(line_byte_offset, rules_version)`.

**Done-criteria**

- A log with stack traces (use `research/cheesecake.out` if present; otherwise
  hand-author a fixture) renders with class names bold, file paths
  underlined, URLs clickable, `Caused by:` standing out.
- Scrolling a heavily-traced section stays at 60 fps.

**Tests landed**

- Unit tests in the UI's `ipc/` or `highlight/` module asserting that the
  default rules produce expected spans on representative inputs.
- No new Playwright tests (existing smokes still pass).

**Demo**

Open a stack-trace-heavy log, point at the highlighted class names and
file paths in a stack frame, click a URL to confirm `shell.open` works.

**Cross-references:** design §8 (axis 2).

**Deferred:** rule editor UI (P8), user-added global rules (P8), per-file
rule overrides (P8).

---

## P6 — It searches and filters

**Scope**

- Smart search engine in `clog-core::search`: in-order proximity-ranked
  substring matching within a record, ranked by total wildcard gap chars.
- Regex search via `regex::bytes::Regex`, anchored per record.
- `rayon`-parallel iteration over `Vec<RecordHeader>` for both modes.
- `search_channel` IPC channel emitting `SearchDelta { search_id, hits }`
  in 60 Hz batches.
- Filter mode: narrows the visible set live as the user types
  (debounced ~60 Hz, cancels in-flight searches on input change).
- Search mode: flags hits in place, supports prev/next navigation.
- UI: a search bar with mode toggle (smart/regex), case-sensitive toggle,
  hit count, prev/next buttons, level-mask toggles to hide INFO/DEBUG/etc.
- Match spans rendered in axis-2's pipeline with the search-match style on top.

**Done-criteria**

- Typing `connection refused` in smart mode against the sample log produces
  ranked hits.
- Toggling to regex mode and typing a valid pattern works; an invalid
  pattern shows the inline error indicator from `docs/design.md §12`.
- Level-mask toggles hide records of that level live.
- Tail still works correctly while a filter is active (new lines that match
  appear; new lines that don't are silently skipped).

**Tests landed**

- Unit tests for smart-search ranking (asserting the example table from
  `docs/design.md §7`).
- Unit tests for regex search anchoring (a hit must not cross record
  boundaries).
- Integration test on `cheesecake.out` for a known-count smart-search query.
- Playwright smoke test 3: open-search-find-navigate.

**Demo**

Open the sample log. Type a smart query, navigate hits. Toggle to regex,
type an intentionally broken pattern (see the inline error). Fix it. Hide
INFO records via the level mask.

**Cross-references:** design §7, §10.

**Deferred:** field-scoped operators (out of v1), search history (out of v1).

---

## P7 — It remembers

**Scope**

- Persistence layer using `tauri-plugin-store` for the JSON files and
  `std::fs` for the index cache and logs.
- Files: `settings.json`, `session.json`, `patterns.json`, persistent
  index cache at `%LOCALAPPDATA%\clog\index\<blake3-hash>.idx`,
  `logs/clog.log` via `tracing-appender` daily rotation.
- Schema versioning on every JSON file (`"schema": 1`).
- Session restore: on launch, reopen previously-open file(s), restore
  scroll position, tail state, level mask, filter text.
- Recent files list (cap 20, MRU).
- Settings panel UI with sections: Appearance, File Handling, Highlighting
  (read-only for now -- the editor arrives in P8), Updates, Advanced.
- Light theme palette + OS-following default + manual toggle.
- `--font-size-base` adjustable via Ctrl-+/Ctrl-minus/Ctrl-0.
- "Open settings folder" and "Reset..." sub-menu actions.

**Done-criteria**

- Open the sample, scroll partway, close clog. Reopen: same file is open
  at the same scroll position.
- Reopening a previously-indexed large file is visibly faster than the
  first open (cold vs warm).
- Light theme renders correctly with WCAG AA contrast.
- Settings panel works and persists.

**Tests landed**

- Unit tests for index-cache key derivation and roundtrip read/write.
- Integration test: index a file, close, reload, assert the same
  `RecordHeader` array is produced from the cache (parity test).
- No new Playwright tests (existing smokes still pass).

**Demo**

Open the sample, set the filter to `ERROR`, toggle to light theme, close
clog. Reopen: same file, same filter, light theme.

**Cross-references:** design §13, §14.

**Deferred:** index cache eviction sweep (out of v1), highlight rule editor
(P8).

---

## P8 — It is customisable

**Scope**

- Highlight rule editor UI in the settings panel: an editable table of rules
  (name, pattern, scope, colour, bold/italic/underline, enabled).
- Live preview pane showing the currently-visible records re-rendered as
  the rule is edited.
- Global user rules persisted to `%LOCALAPPDATA%\clog\highlight-rules.json`.
- Per-file pattern overrides persisted to `%LOCALAPPDATA%\clog\patterns.json`
  with UI to edit/forget the override from the file's header bar.
- Per-file highlight rule overrides persisted to
  `%LOCALAPPDATA%\clog\per-file-rules\<hash>.json` with UI to manage them.
- Rule priority handling and visible feedback when two rules overlap.

**Done-criteria**

- Adding a global rule "any token starting with `Foundation` -> bold blue"
  takes effect across all open files immediately.
- Adding the same rule scoped to one file affects only that file.
- A file with a saved pattern override opens directly with that pattern
  next time without re-detection.

**Tests landed**

- Unit tests for rule persistence and per-file override resolution.
- Visual sanity check via existing Playwright smokes.

**Demo**

Open two different files. Add a global highlight rule, watch both update.
Add a per-file rule on one of them, watch only that file change. Close
clog. Reopen: rules and per-file pattern overrides persist.

**Cross-references:** design §8 (axis 2), §14.

**Deferred:** sharing/exporting rule packs (out of v1).

---

## P9 — It multitabs

**Scope**

- Multi-file UI: tab strip across the top, drag-to-reorder, close button
  per tab.
- Per-tab `file_id` with its own `tail_channel`, `search_channel`, scroll
  position, filter state.
- Single-instance app via `tauri-plugin-single-instance`. A second launch
  with a file path forwards to the running instance as a new tab.
- File drag-drop into the window adds a new tab.
- `.log` and `.out` "Open with..." registration via the NSIS installer
  (deferred to P10 for the installer itself, but the underlying
  command-line handling lands here so it works for dev builds).
- Tail status indicator in each tab's tab header (e.g. dot pulsing when
  new lines arrive on a non-active tab).
- Recent files list updated to handle re-opening into existing or new tabs.

**Done-criteria**

- Open four log files simultaneously. Switch between tabs without losing
  state. Close one tab, reopen via Recent Files.
- Launching `clog.exe path\to\another.log` from a second terminal opens
  it as a new tab in the existing window, not a second process.

**Tests landed**

- Unit/integration tests for the per-tab state model (open, close, focus).
- No new Playwright tests required; existing smokes parameterised to run
  with two tabs open.

**Demo**

Open four files. Run `clog.exe path\to\fifth.log`. Watch it appear as a
fifth tab in the running window.

**Cross-references:** design §11.

**Deferred:** split panes (v1.1), tab groups (v1.1).

---

## P10 — It ships

**Scope**

- GitHub Actions workflow: on tag `v*` push, build NSIS installer and
  portable zip, attach to a GitHub Release.
- NSIS installer: per-user install, no UAC, opt-in `.log`/`.out`
  "Open with..." checkbox.
- Portable zip artefact (`clog_x.y.z_x64-portable.zip`) usable from a
  drop-in folder.
- Portable-mode detection (`clog-data\` next to `clog.exe` overrides
  `%LOCALAPPDATA%\clog\`).
- Tauri auto-updater: self-hosted `latest.json` (commit to a `gh-pages`
  branch or equivalent), public update-signing key in `tauri.conf.json`,
  private key in CI secrets.
- Code-signing infrastructure: CI step that signs only if
  `CLOG_SIGNING_PFX_BASE64` is set, no-ops otherwise.
- Error-handling polish pass: panic hook, full `IpcError` translation
  table, 2 GB file-size cap with clear message, UI surfaces by
  recoverability.
- `tracing` logging configured for release builds (file output only,
  no stderr unless `--verbose`).
- README updated with download links and a one-page user guide.

**Done-criteria**

- Tagging `v0.1.0` produces a GitHub Release containing both artefacts.
- Installer runs cleanly on a fresh Windows 11 VM. App launches, opens a
  file, tails, searches, persists state.
- Portable zip extracted to `C:\Tools\clog\` with `clog-data\` alongside
  uses that folder for all state.
- Auto-updater detects a synthetic newer `latest.json` and offers update
  install.

**Tests landed**

- A release-mode smoke test that builds the installer in CI and verifies
  artefact hashes/sizes against an expected range.
- A panic-hook unit test asserting that a synthetic panic produces the
  expected fatal-error event.

**Demo**

Push tag `v0.1.0`. Download the installer onto a fresh VM. Run it. Use
the app for ten minutes.

**Cross-references:** design §12, §14 (portable mode), §16.

**Deferred:** EV signing, MSI, macOS/Linux builds, telemetry.

---

## After v1

v1.1 candidates explicitly named in the design:
- Split-pane tab layouts (`design.md §11`).
- Index cache eviction sweep (`design.md §14`).
- WSL companion daemon (`design.md §17` and the `LineSource` slot).
- mmap backend for local files.

Decide v1.1 scope after v1 has been used in anger for a few weeks.
