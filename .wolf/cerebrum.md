# Cerebrum

> OpenWolf's learning memory. Updated automatically as the AI learns from interactions.
> Do not edit manually unless correcting an error.
> Last updated: 2026-05-23

## User Preferences

- Vue 3 is the team's known frontend framework; prefer it over less-familiar alternatives unless a hard perf reason rules it out.
- Light/dark theming must follow OS settings with an in-app manual toggle; no custom-theme/palette editor needed.
- Register both `.log` and `.out` extensions in the installer's "Open with" menu (real sample file is `solopress.out`).
- **CSS style:** always use CSS custom properties (two-layer: palette in `ui/src/style.css` `:root`, semantic mapping on top) - never hardcode colours, fonts, sizes, or radii in component styles. Reference tokens via `var(--name)`.
- **CSS syntax:** always use native nested CSS (it's 2026, browsers support it). Nest descendant selectors and pseudo-classes (`&:hover`) inside their parent rule rather than flattening with descendant combinators.

## Key Learnings

- **Project:** clog (Core Log) — Windows desktop viewer for log4j2-formatted Play 1.x logs.
- Production log patterns vary across deployments. Two confirmed shapes:
  - `[%-5level] %d{yyyy-MM-dd HH:mm:ss.SSS} [%t] %c{1} - %msg%n` (wsl-oink)
  - `%d{yyyy-MM-dd HH:mm:ss.SSS} %level [%t] - %msg%n` (prod, no logger field, level unbracketed)
  Parser must be PatternLayout-driven, not hardcoded.
- Files are typically reached via SMB from a WSL Ubuntu share. mmap is unsafe over SMB (rotation triggers access violations); streamed reads are the v1 path. SMB also makes `notify`-style change events unreliable — polling is the unified tail mechanism.
- `OnStartupTriggeringPolicy` (wsl-oink) and `TimeBasedTriggeringPolicy` (prod) both rotate files; rotation detection via `(size shrank) OR (first 256 bytes hash changed)` covers both.

## Do-Not-Repeat

<!-- Mistakes made and corrected. Each entry prevents the same mistake recurring. -->
<!-- Format: [YYYY-MM-DD] Description of what went wrong and what to do instead. -->

- [2026-05-23] Initially conflated structural styling (PatternLayout field slicing) with content styling (highlight rules) when describing continuation-line rendering. They are orthogonal: continuation lines do not repeat the record-header prefix but DO receive full highlight-rule decoration. Always keep these two axes distinct in design discussions.
- [2026-05-23] Initially recommended SolidJS for the UI without first asking about team framework familiarity. For a non-perf-critical layer (framework reactivity overhead is dwarfed by virtualiser + IPC + parser), familiarity should win. Ask before recommending unfamiliar stacks.
- [2026-05-23] The sample-log fixture referenced in `docs/build-phases.md` as `research/solopress.out` actually lives at `research/solopress-prod.log` (8.7 MB, 74,921 lines, prod pattern) and `research/solopress-wsl-oink.out` (42 KB, 386 lines, wsl-oink pattern). Docs are stale - until they are updated, code should reference the actual filenames. Smoke-test constant for the prod fixture is `74_921` lines.
- [2026-05-23] Used `Bash` with Windows-style backslash paths (`mkdir e:\Work\clog\docs`). Bash interprets `\` as an escape character so this can collapse to a single mangled directory name like `eWorkclogdocs`. On Windows, for directory operations either (a) use `PowerShell` with `New-Item -ItemType Directory -Force <path>`, or (b) if using Bash, use forward slashes (`e:/Work/clog/docs`) or quote-and-escape carefully. Prefer PowerShell for filesystem ops on this machine.

## Decision Log

### [2026-05-23] P5 vertical slice landed
Axis-2 content styling pipeline. `ui/src/highlight/engine.ts` exposes `setRules`/`computeHighlights`/`highlightsFor`/`overlay`/`rulesVersion`; rules compile once with `gd` flags, matches paint a per-char (cls, priority, url) tri-array (higher priority overwrites, zero-width matches skipped, 256-iter per-rule guard), runs collapse into ordered non-overlapping spans, and a size-capped 4000-entry text cache (keyed by `version + text`, oldest-quarter eviction) keeps repeated lookups O(1). `default-rules.json` ships 6 rules (caused-by, stack-frame with fqn/file/line sub-groups, java-exception, url self-href, windows path, unix path) and is loaded once at module-eval time. App.vue's renderer replaced the old `sliceHeader` with `renderLine(row)` which slices axis-1 fields and calls `overlay(text, base, axis2)` to produce flat leaf spans with space-joined classes ("s-message h-exception") and optional url; continuation rows wrap their full text in a single `message` base span so the gutter + indent styling still applies. URLs click through to `openUrl` from `@tauri-apps/plugin-opener` (capability set extended with `opener:default` + `opener:allow-open-url`; clog-app picked up `tauri-plugin-opener` 2 and `.plugin(tauri_plugin_opener::init())`). `style.css` palette gained `--hl-*-fg` tokens; the `.h-*` classes only set fg/weight/decoration so row hover stays uniform. Vitest added with `npm test` script + `vitest.config.ts`; `tsconfig.app.json` excludes `src/**/*.test.ts` so vue-tsc doesn't drag vitest's node types into the App.vue compilation (would otherwise retype `setTimeout` to return `NodeJS.Timeout` and break the existing tail/toast timer fields). 14 vitest cases + 39 cargo tests green; cargo fmt/clippy/build + npm build all clean.

### [2026-05-23] P5 design choice: per-char paint then collapse instead of interval tree
Each `computeHighlights` allocates three N-length arrays (cls/pri/url) and walks every rule's matches, painting per-char with a "higher priority wins" rule. After all rules run, identical-(cls,url) runs collapse into spans. Cost is O(N * matches) per line; for typical visible-window line lengths (< 1KB) and ~6 rules this is single-digit microseconds. An interval-tree merge was considered and rejected as needless complexity for the scale -- the cache amortises repeat lookups anyway and the line-level cap (256 iters per rule) bounds the worst case.

### [2026-05-23] P5 design choice: bake rules into the bundle, runtime-swappable
P5 keeps the rule set static and bundled (`default-rules.json` imported via Vite's JSON loader, parsed once at module-eval time, fed to `setRules`). The engine exposes `setRules`/`rulesVersion` so P8's editor can swap rules at runtime and the cache automatically invalidates on the next call (the version is part of every cache key). No per-rule "enabled" flag yet -- that's P8 too.

### [2026-05-23] P5 design choice: overlay produces flat leaf spans, not nested DOM
The renderer needs to emit a flat list of `<span>`s for each row (nesting axis-2 inside axis-1 would mean two tag types and break the row's grid layout). `overlay(text, base, axis2)` computes the set of boundary offsets (every start/end on both axes plus 0 and length), walks adjacent boundary pairs, and emits one leaf per pair with space-joined class names from whichever spans contain that range. Adjacent leaves with identical cls+url are then merged. Template binding stays one-level deep: `v-for span in renderLine(row)` -> `<span :class>` -> text.

### [2026-05-23] P4 vertical slice landed
clog-core gained `tail` (TailState polling state machine; emits NoChange/Appended/Rotated; FNV-1a hash anchored to a fixed prefix length so file growth doesn't trip the hash; partial-line trimming holds a trailing fragment until the writer flushes a newline; reset_to re-anchors after the caller re-indexes; HEAD_HASH_BYTES=256, DEFAULT_POLL_INTERVAL_MS=250; 6 tempfile unit tests covering append-only, partial-write buffering, size-shrink rotation, head-hash rotation, idle, and single-poll latency) and a `fake_tailer` example (configurable rate, --rotate flag, occasional stack-trace continuations). clog-app gained `channels::TailEmitter` (pass-through; seam for the 60Hz coalescer that activates with search in P6 -- tail's 250ms cadence is already under budget), `ScannerKind` (source-string sum) + `CompiledScanner` (sized sum impl RecordScanner) so the tail task can recompile the active scanner each tick without trait-object orphan-impl issues, `start_tail`/`stop_tail` IPC commands emitting `TailDelta { new_record_count, line_count, record_count, last_offset, rotated }` on a tauri::ipc::Channel, `extend_with_appended` (incremental line/record extension with byte_len fix-up across the touched range) and `apply_rotation` (re-index off the lock, swap under the lock). Tokio dep added to clog-app (sync/time/macros) for the spawn loop. UI gained a tail-controls cluster (idle/tailing/pulsing indicator, follow-tail toggle, jump-to-bottom), Channel<TailDelta> wired on open_file, scroll handler that disengages follow-tail when the user scrolls away, and a transient rotation toast. 32 tests green. Playwright tail-survives-rotation smoke deferred until Playwright is set up.

### [2026-05-23] P4 design choice: anchored-prefix head hash
The first naive head-hash impl hashed `min(HEAD_HASH_BYTES, current_size)` on every poll, which made the hash flip on every append for files shorter than 256 bytes (we hashed a longer prefix the next time). Fix: pin `head_prefix_len` at construction (or after a rotation re-anchor) and ALWAYS rehash exactly that prefix length on subsequent polls. Growth keeps the anchored bytes unchanged so the hash is stable; only a rewrite or rename-recreate flips it. Below-prefix-length shrinks are caught by the size-shrink branch first, so the rule remains complete. The 256-byte prefix is large enough that real log files exceed it on the first record.

### [2026-05-23] P4 design choice: incremental record extension instead of full re-scan
On Appended, the running tail task does NOT re-run `scan_records` over the whole file. Instead `extend_with_appended` walks only the freshly-appended bytes, pushes new RecordHeaders (or extends the last header's line_count for continuation lines), and recomputes byte_len starting from the last-pre-existing record forward. Cost is O(appended_lines) per tick instead of O(total_records). Rotation is the opposite -- full re-index via the existing `index_file` path -- because the file shape has changed wholesale.

### [2026-05-23] P4 design choice: ScannerKind/CompiledScanner sum over trait objects
Initially tried storing the active scanner as `Box<dyn RecordScanner + Send + Sync>` so the tail task could hold one across polls. Two problems: (a) Rust's orphan rule blocks `impl RecordScanner for Box<dyn RecordScanner...>`, and (b) `index_file<S: RecordScanner>` requires Sized so `&dyn RecordScanner` is rejected. Switched to a sized sum (`CompiledScanner { Pattern(CompiledPattern) | Regex(RegexScanner) }`) plus a source-string sum (`ScannerKind { Pattern(String) | Regex(String) }`) stored on OpenedFile; the tail task recompiles each tick (pattern strings are short, cost is negligible vs. disk I/O).

### [2026-05-23] P4 design choice: 60Hz coalescing layer ships as a pass-through
P4 scope calls for a 60Hz coalescing layer in `clog-app::channels`. Tail polls at 250ms, well under the 60Hz (16ms) budget, so there is nothing to merge today. The `TailEmitter` is shipped as a pass-through with a `flush()` no-op seam reserved for P6 where search streaming can fan in faster than the UI can usefully redraw. Documenting this here so the next session doesn't reinvent it; the merge logic lands with P6.

### [2026-05-23] P3 vertical slice landed
clog-core gained `pattern` (PatternLayout compiler: Token enum Literal/Level/Date/Thread/Logger/Message/Newline, DateFormat compiles SimpleDateFormat strings into per-byte digit/literal atoms, CompiledPattern produces ParsedHeader { Level, HeaderFields } where HeaderFields carries `Option<(u32,u32)>` byte spans relative to the line start, BUILTIN_PATTERNS + auto_detect score-based picker over wsl-oink/prod/log4j2-default), `regex_scanner` (RegexScanner escape hatch with named captures level/timestamp/thread/logger/msg). RecordHeader gained `fields: HeaderFields`. RecordScanner trait switched to `try_parse_header(line) -> Option<ParsedHeader>`. WslOinkScanner deleted; CompiledPattern impls RecordScanner directly. clog-app: open_file auto-detects from a 64KB sample (returns pattern_name/pattern_source/pattern_score); new `test_pattern` (returns match score over sample) and `set_pattern` (rebuilds records in place against new pattern or regex) commands; new `get_lines(file_id, start, end)` IPC for per-physical-line virtualisation, mapping virtual row index -> record via O(log n) `partition_point` over cached `record_first_line` (initial) then linear walk. UI switched to per-line virtualisation: pattern paste bar with mode toggle + Test/Apply + match-score readout, axis-1 spans (level bold + level-colour, timestamp muted, thread default, logger italic+muted, separators dim), 4px level-coloured left gutter spanning all lines of a record, sticky record-header overlay when scrolled mid-record (sticky div is a sibling of `.total`, not a child of absolutely-positioned rows), indented continuation lines. CSS palette gained per-level colours and axis-1 fg tokens. 26 tests green (incl. proptest round trip for wsl-oink + prod, and auto-detect fixture tests for both sample files).

### [2026-05-23] P3 design choice: per-physical-line virtualisation in the UI
Switched away from P2's per-record virtualisation. Each virtual row is now ONE physical line so the sticky record-header overlay and indented continuation lines can be implemented cleanly without variable row heights (which would cost scrollbar accuracy). The line -> record mapping happens server-side per page: clog-app caches `record_first_line: Vec<u64>` at open and `get_lines(file_id, start, end)` walks it with `partition_point` to find the first record then advances forward. This keeps the IPC payload compact (per-line text + record_idx + line_within_record + level + optional fields when line_within_record == 0) while preserving design.md s9 ("perfect scrollbar accuracy from the first frame").

### [2026-05-23] P3 design choice: PatternLayout-driven scanner replaces hardcoded scanner
WslOinkScanner from P2 is gone. CompiledPattern (built from a pattern string) now implements RecordScanner directly. Header detection is a left-to-right walk over Token, with variable-length tokens (Thread/Logger) bounded by the next Literal. Validate_variable_terminators rejects two consecutive variable tokens at compile time so the walker never has to guess. Unknown specifiers (%X{}, %mdc, %throwable{} etc.) compile to a no-op + PatternWarning rather than erroring, so the rest of the line still parses (design.md s5).

### [2026-05-23] P2 vertical slice landed
clog-core gained `source` (LineSource trait + StreamedFile), `index` (in-memory LineIndex), `record` (Level, RecordHeader, hardcoded WslOinkScanner, scan_records). `index_file(path, scanner)` composes them. clog-app holds an `AppState { files: Mutex<HashMap<file_id, OpenedFile>>, next_id }`; `open_file` now allocates a file_id and returns `OpenedFilePayload`, `get_records(file_id, start, end)` reads the byte range on demand and returns `RecordsPayload { start, base_offset, headers, text }`. UI uses `@tanstack/vue-virtual` with a 256-record page cache and 18px fixed row height. 11 tests green (LineIndex edges, scanner classification, single/continuation/orphan record cases, wsl-oink fixture watertight coverage, prod fixture line count smoke). Hardcoded wsl-oink scanner is deliberate per P2 scope; pattern generalisation arrives in P3.

### [2026-05-23] P2 design choice: pages of records, fetched on demand
UI fetches 256 records per page via `get_records`. Pages cached in a `Map<pageIdx, string[]>`. Avoids round-tripping the whole file (74k+ records) at open time while still keeping IPC traffic bounded by visible rows + overscan. Rust side keeps RecordHeader array in RAM (~32B/record) but does NOT keep the raw bytes; bytes are re-read from disk per `get_records` call (OS page cache makes this cheap for warm files). Aligns with design §4: "raw file bytes are not held in RAM".

### [2026-05-23] P1 scaffold landed
Workspace + 2 crates + Vite UI + Tauri v2 wired. `open_file` IPC returns `FileSummary { path, size_bytes, line_count }`. UI uses `tauri-plugin-dialog` for the native picker. Build verifies clean: `cargo build --workspace`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm --prefix ui run build`. Icons generated programmatically via `System.Drawing` (placeholder "C" glyph; design pass deferred to P10). Capability set is minimal: `core:default` + `dialog:default` + `dialog:allow-open`.

### [2026-05-23] Stack: Rust + Tauri v2 + Vue 3
Chosen over .NET/WPF (weaker portability story, no shared language with future WSL companion) and Electron (binary size, JS heap pressure on big files). Rust gives first-class ripgrep/memmap2/notify primitives. Vue 3 picked over Solid because the team knows it; perf gap is theoretical given TanStack Virtual caps the active DOM tree.

### [2026-05-23] File access: streamed reader behind `LineSource` trait
mmap unsafe over SMB. Streamed `BufReader` + persistent line-offset index (`%LOCALAPPDATA%\clog\index\<blake3-hash>.idx`) is the v1 path. Trait-abstracted so future `MmapFile` (local) and `WslSocket` (companion daemon) impls drop in without touching parser/search/render.

### [2026-05-23] Tail mechanism: polling at 250ms
Unified across local NTFS and SMB. `notify` is unreliable over SMB. Rotation detected via size-shrink OR first-256-bytes hash change.

### [2026-05-23] Parser: PatternLayout-driven, not hardcoded
Compile pattern string at file-open into a `RecordScanner`. Supports `%d{...}`, `%level/%-5level/%p`, `%t`, `%c/%c{N}`, `%msg/%m`, `%n`, literals. Auto-detect against built-in library; user-paste override; regex escape hatch. Packed `RecordHeader` array (~32B/record) for fast level/time filtering. Hand-written byte scanner for headers; no regex on hot path.

### [2026-05-23] Search: smart proximity-ranked + regex toggle
Smart search = in-order substring tokens within a record, ranked by total wildcard gap chars (fewer = better). Continuation lines included; record boundaries not crossed. Case-insensitive default. `regex::bytes::Regex` for regex mode. Same engine powers filter and search; `rayon` parallelism over `Vec<RecordHeader>`. Hits = `HitRef { record_idx, byte_ranges, score }`.

### [2026-05-23] Styling: two orthogonal axes
Axis 1 (structural) = PatternLayout field slicing on first line of each record. Axis 2 (content) = user-configurable regex+style highlight rules applied to every visible line (header AND continuation). Built-in defaults (Java exception, `Caused by:`, stack frame, file path, URL) + user rules (global) + per-file overrides. Regex evaluation runs JS-side for live-preview iteration.

### [2026-05-23] Virtualisation: fixed-height visual lines with record back-pointers
One physical line = one virtual row of fixed pixel height; each row carries `(record_idx, line_within_record)`. Gives perfect scrollbar accuracy from frame one while preserving record-level effects (sticky header, level-coloured gutter, row hover). No soft-wrap in v1 (horizontal scroll instead).

### [2026-05-23] IPC: 3-layer contract with 60Hz coalescing
Commands (`invoke`) for request/response, per-file channels (`tauri::ipc::Channel`) for streaming tail/search deltas, global events for low-rate status. Tail emits counts+offsets not contents; webview fetches byte ranges and does its own span slicing. All streaming coalesced at 60Hz (16ms flush) to cap IPC traffic regardless of log volume.

### [2026-05-23] Multi-file UX: tabs v1.0, panes v1.1
Single window, single-instance app (`tauri-plugin-single-instance`). Session restored on launch (open tabs, scroll positions, tail state). Recent files list capped at 20.

### [2026-05-23] Workspace layout: 2 crates + sibling ui/
`crates/clog-core` (engine, no Tauri deps) + `crates/clog-app` (Tauri binary, IPC + state) + `ui/` (Vue 3 + Vite). `clog-protocol` crate deferred until WSL daemon is real.

### [2026-05-23] Testing: layered + one proptest
Unit tests in `clog-core` against `research/solopress.out` fixture + synthetic edge files. Tempfile-based integration tests for tail rotation. IPC contract test in `clog-app`. 3 Playwright smoke tests (open, search, rotation). One `proptest` on the PatternLayout compiler. `criterion` benches not CI-gated. `cargo run --example fake_tailer` for the dev loop.

### [2026-05-23] Error handling: typed `CoreError` -> `IpcError` translation
`thiserror`-based enum in core, `anyhow` only at app boundary, structured payloads to Vue with `kind`/`message`/`suggestion`/`recoverable` fields. UI surfaces by recoverability (inline / banner / modal / footer-toast). Panic hook keeps app alive and disables only the offending file handle. 2GB v1 file-size cap. `tracing` -> daily-rotated logs in `%LOCALAPPDATA%\clog\logs\`.

### [2026-05-23] Theming: dark + light, OS-following
Two-layer CSS custom property tokens (palette + semantic), `prefers-color-scheme` default with manual toggle. WCAG AA level-colour contrast verified at design time. No Tailwind/SCSS; no custom-palette UI v1.

### [2026-05-23] Persistence: per-concern JSON files in `%LOCALAPPDATA%\clog\`
`settings.json`, `session.json`, `highlight-rules.json`, `patterns.json`, `per-file-rules/<hash>.json`, `index/<hash>.idx`, `logs/`. All JSON schema-versioned. `tauri-plugin-store` for prefs, `std::fs` for index/logs. **Portable-mode v1.0**: if `clog-data\` sits next to `clog.exe`, that path takes precedence.

### [2026-05-23] Packaging: NSIS + portable zip
Per-user install (no UAC). NSIS installer offers `.log` AND `.out` "Open with" registration as opt-in checkbox. Tauri auto-updater wired from v1 with self-hosted `latest.json`; update-signing keypair generated now. Code-signing cert deferred until SmartScreen friction is real; CI build cert-aware via env vars (`CLOG_SIGNING_PFX_BASE64`).
