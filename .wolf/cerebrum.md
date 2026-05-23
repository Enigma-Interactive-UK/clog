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
