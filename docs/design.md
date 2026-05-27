# Clog v1 — Design

> Snapshot dated 2026-05-23. This document describes the intended v1 design before
> implementation begins. As code is written some details will drift; substantive
> changes should be reflected here and, if contested, lifted into an ADR under
> `docs/adr/`. The authoritative decision log with full rationale lives in
> `.wolf/cerebrum.md`.

## 1. What Clog is

Clog (Core Log) is a Windows desktop application for viewing, tailing, filtering
and searching log4j2-formatted log files produced by Play 1.x Java applications.
Files are typically large (>50 MB) and accessed from a WSL Ubuntu image via an
SMB bridge, though local files must work identically for the developer loop. A
real sample log lives at `research/cheesecake.out` (~8.7 MB) and is the primary
fixture used throughout development and testing.

The originating log4j2 configs in `research/` show that production patterns vary
between deployments. Two confirmed layouts are in use:

```
[%-5level] %d{yyyy-MM-dd HH:mm:ss.SSS} [%t] %c{1} - %msg%n     (wsl-oink)
%d{yyyy-MM-dd HH:mm:ss.SSS} %level [%t] - %msg%n               (prod)
```

The parser must therefore be pattern-driven, not hardcoded.

## 2. Stack

- Rust (stable, msvc target) for the engine.
- Tauri v2 as the application shell.
- Vue 3 + Vite for the UI, using Single-File Components, the Composition API,
  bare `ref`/`reactive` state (no Pinia until state genuinely outgrows it),
  selective VueUse helpers, and `@tanstack/vue-virtual` for list virtualisation.

Rust + Tauri was chosen over .NET/WPF (weaker portability story, no shared
language for the future WSL companion) and Electron (binary size, JS heap
pressure on big files). Vue 3 was chosen over Solid because the team already
knows it; framework reactivity overhead is the least performance-critical layer
once the virtualiser caps the active DOM tree to ~50-100 rows.

## 3. Workspace layout

```
clog/
  Cargo.toml                       # workspace
  crates/
    clog-core/                     # pure engine, no Tauri deps
      src/
        source/                    # LineSource trait + StreamedFile impl
        index/                     # line-offset + RecordHeader cache
        pattern/                   # PatternLayout compiler -> RecordScanner
        parser/                    # RecordHeader builder + scanner runtime
        search/                    # smart + regex engines
        tail/                      # polling loop, rotation detection
        types.rs
      examples/
        fake_tailer.rs             # dev-loop helper
    clog-app/                      # Tauri binary
      src/
        main.rs
        commands.rs                # #[tauri::command] handlers
        channels.rs                # tail/search streaming, 60Hz coalescing
        state.rs
        prefs.rs
      tauri.conf.json
      build.rs
  ui/                              # Vue 3 + Vite, separate package.json
    src/
      components/
      stores/
      ipc/                         # typed invoke/channel/event wrappers
  research/                        # existing sample logs and configs
  docs/
    design.md                      # this file
```

The 2-crate split exists so `clog-core` is a reusable library. A future WSL
companion daemon becomes a third crate depending on the same core. A
`clog-protocol` crate is deliberately deferred until that daemon is real.

## 4. File access

Files are opened through a `LineSource` trait. The v1 implementation,
`StreamedFile`, uses a `BufReader` with explicit seeks. Memory-mapped access is
out of v1 because it is unsafe over SMB — truncation and rotation produce
access-violation faults on the mapped region. A future `MmapFile` impl can drop
in behind the same trait for local files when measured to matter; a future
`WslSocket` impl can do the same for the daemon transport.

On first open the file is walked once to produce two structures:

- A `Vec<u64>` of line-start byte offsets, one entry per physical line.
- A packed `Vec<RecordHeader>` (~32 bytes/record) of parsed structural fields
  including byte offset, byte length, timestamp (epoch ms), level enum, and
  byte offsets into the record for thread, logger and msg.

Both are cached to disk at `%LOCALAPPDATA%\clog\index\<blake3-hash>.idx` keyed
by absolute path, file size and mtime, so reopening the same file is instant.
At 32 B/record, even a one-million-record file is ~32 MB of in-memory headers,
which is acceptable.

The raw file bytes are not held in RAM. The webview requests byte ranges on
demand for visible records.

## 5. Pattern parser

The pattern parser is a small log4j2 PatternLayout compiler. At file open
(either from auto-detection, a saved per-file override or a user paste) a
pattern string is compiled into a `RecordScanner` — an ordered sequence of token
matchers. The supported subset covers the realistic Play 1.x case:

| Specifier | Notes |
|---|---|
| `%d{...}` | timestamp, date-format string parsed to chrono format |
| `%level`, `%-5level`, `%p`, `%-5p` | level, pad-width respected |
| `%t` | thread, greedy until next literal |
| `%c`, `%c{N}` | logger, depth handled at parse |
| `%msg`, `%m` | body, consumes to end of record (incl. continuations) |
| `%n` | record terminator |
| Literal text | brackets, separators, etc. |

Unknown specifiers (`%X{}`, `%mdc`, `%throwable{}` etc.) emit one info-level
warning per file and the parser falls back to treating the rest of the line as
raw `%msg` content.

Three operating modes are exposed in the UI:

- **Auto-detect** against a built-in library (the two known production patterns
  plus the log4j2 default `%d{ISO8601} [%t] %-5level %c{36} - %msg%n`). Pick
  whichever matches the highest fraction of the first ~20 records.
- **User-supplied pattern string** pasted from the application's
  `log4j2.properties` or `log4j2.xml`. Persisted per-file-path in
  `%LOCALAPPDATA%\clog\patterns.json`.
- **Raw regex escape hatch** with named captures (`level`, `timestamp`,
  `thread`, `logger`, `msg`) for anything exotic.

Header detection ("is this physical line the start of a new record?") is a
hand-written byte scanner generated from the compiled token sequence. No regex
on the hot path.

Multi-line records are handled at parse time: lines that do not match the
header prefix belong to the preceding record. `RecordHeader.byte_len` spans the
whole record including its continuations.

## 6. Tailing and rotation

Tailing uses a single mechanism for both local NTFS and SMB paths: poll the
file every 250 ms while tail is active, do nothing while it is paused.
`notify`-based change events are not used because their SMB behaviour is too
inconsistent to depend on and because the 250 ms latency difference is
imperceptible to a human watching logs scroll.

Rotation is detected when *either* the current size is less than the last
known size *or* the first 256 bytes hash to a different value than at the last
check. This covers both `OnStartupTriggeringPolicy` (wsl-oink, file truncated
and reopened) and `TimeBasedTriggeringPolicy` (prod, file renamed and a new
empty file created) without depending on inode/file-id semantics that SMB does
not reliably expose.

On rotation the file is re-opened, re-indexed, and a `Rotated` info-level event
is emitted to the UI so the footer can briefly note the rotation.

## 7. Search and filter

The search engine is shared between two UI surfaces: a live filter that narrows
the visible set as the user types (the `tail -f | grep` experience) and a
search bar that flags hits in place and lets the user navigate hit-by-hit.

Two modes:

- **Smart search**: in-order proximity-ranked substring matching. The query
  `connection refused` matches text where `connection` appears before `refused`
  within the same record (continuation lines included), ranked by the total
  number of characters the gap consumed:

  | Match text | Gap chars | Rank |
  |---|---|---|
  | `connectionrefused` | 0 | best |
  | `connection refused` | 1 | next |
  | `connection was refused` | 5 | worse |

  Multi-token generalises: `foo bar baz` becomes `foo<x>bar<y>baz`, ranked by
  `len(x)+len(y)`. Case-insensitive by default. Implemented as a small custom
  byte scanner because the regex engine does not expose gap length cheaply.

- **Regex**: the user's pattern is passed straight to `regex::bytes::Regex`,
  anchored per record. Bytes mode avoids UTF-8 validation cost.

Both modes run as a parallel iterator (`rayon`) over `Vec<RecordHeader>`. Hits
are returned as `HitRef { record_idx, byte_ranges, score }` — small payloads
that the webview combines with already-fetched record bytes to draw highlight
spans. The live filter throttles input at ~60 Hz and cancels in-flight searches
when the input changes.

Search scope is the whole record by default (level, thread, logger and body).
Field-scoped operators (`level:ERROR thread:main connection refused`) are out
of v1.

## 8. Styling

Styling has two orthogonal axes. They are independent: a continuation line is
not given the structural prefix again but is fully subject to content rules.

**Axis 1 - structural styling** comes from the PatternLayout. On the first
line of each record the parser provides byte offsets for level, timestamp,
thread, logger and msg-start, and the renderer wraps each in a styled span.
Default appearances:

| Element | Style |
|---|---|
| Level `[INFO ]` / `[WARN ]` / etc. | bold, coloured per level |
| Timestamp | muted, no weight change |
| Thread `[main]` | normal weight |
| Logger | italic, muted |
| `-` separator | dim |
| Body | default |
| Continuation lines | default, indented two spaces |
| Left gutter | 4 px solid, level-coloured, spans all lines of the record |
| Sticky record header | translucent panel overlay during scroll |
| Selected record | subtle row background |

**Axis 2 - content styling** comes from highlight rules: regex+style pairs
applied to every visible line (header *and* continuation). Built-in defaults
cover Java exception class names, `Caused by:`, stack frame patterns, file
paths and URLs. Users can add their own rules (e.g. "any token starting with
Foundation" -> bold blue; "any token starting with Core" -> bold green). Rules
are stored globally in `%LOCALAPPDATA%\clog\highlight-rules.json` with
per-file overrides at `%LOCALAPPDATA%\clog\per-file-rules\<hash>.json`.

Highlight regex evaluation happens in JavaScript inside the webview, not in
Rust. This is so a user editing a rule can see the effect re-rendered live
without a round-trip. The cost is bounded — visible rows are capped by the
virtualiser to ~50-100, even with 30 rules the per-frame work is a few hundred
thousand regex bytes.

## 9. Virtualisation and rendering

A `@tanstack/vue-virtual` instance treats the file as a flat list of
fixed-pixel-height visual rows. Each row corresponds to one physical line, with
a back-pointer `(record_idx, line_within_record)` computed once and held
alongside the offset index. Approximate cost: 16 bytes per physical line.

This gives perfect scrollbar accuracy from the first frame (total height is
known immediately from the physical line count) while still allowing
record-level effects:

- The level-coloured left gutter is drawn on every line whose record matches
  that level.
- The sticky record header overlays the topmost visible row when its
  `line_within_record > 0`, so a user scrolled mid-stack-trace always sees the
  originating record's header.
- Search-hit navigation jumps to `record_first_line[hit.record_idx]`, an O(1)
  lookup in a precomputed `Vec<u64>`.

No soft-wrap in v1. Long lines scroll horizontally. Soft-wrap would force the
virtualiser back to variable row heights and lose scrollbar accuracy.

Font is bundled (Cascadia Mono or JetBrains Mono) to ensure identical
rendering across machines. Row height derives from font size via `calc`; the
standard browser zoom keybinds (Ctrl-+/Ctrl-minus/Ctrl-0) adjust a
`--font-size-base` CSS variable.

## 10. IPC contract

Three primitives, used for three message shapes:

- **Commands** (`#[tauri::command]`, invoked by the UI, return a value):
  `open_file`, `close_file`, `get_records(range)`, `start_search(query)`,
  `cancel_search`, `set_filter`, `set_pattern`, `start_tail`, `stop_tail`.
- **Per-file channels** (`tauri::ipc::Channel`, opened at `open_file`,
  streamed by Rust):
  - `tail_channel` emits `TailDelta { new_record_count, last_offset, rotated }`
  - `search_channel` emits `SearchDelta { search_id, hits: Vec<HitRef> }`
- **Global events** (low rate, broadcast): `file_error`, `file_status`.

All streaming traffic is coalesced inside Rust at 60 Hz (a
`tokio::time::interval(Duration::from_millis(16))` per file). Tail emits counts
and offsets, not record contents — the webview already knows how to fetch the
new range from its virtualiser. Hits carry record indices and byte ranges, not
strings — the webview combines them with bytes it has already fetched.

This caps IPC traffic at roughly `60 messages/sec * a few KB` regardless of
how fast the underlying log grows.

The webview does its own structural span slicing (axis 1) and highlight rule
evaluation (axis 2) from the raw record bytes plus the `RecordHeader` it
received over IPC. The Rust core does not know about colours.

## 11. Multi-file UX

v1.0 ships **tabs**. Single window, single-instance app (via
`tauri-plugin-single-instance`). Double-clicking a `.log` or `.out` file in
Explorer opens it as a new tab in the running window. Session state (open
files, scroll positions, tail active/paused, last-active tab, window geometry)
is persisted to `%LOCALAPPDATA%\clog\session.json` and restored on launch.
A "Recent files" picker caps at 20 entries in MRU order.

v1.1 adds **splittable tiled panes**: drag a tab to spawn a horizontal or
vertical split, side-by-side tailing of the main log and the per-level
appenders. The Rust side requires no changes for this; it is a recursive
`SplitPane` component on the Vue side, each leaf still containing a tab strip.

Multi-top-level-window mode is an explicit non-goal. Memory budget across many
open files is dominated by the `RecordHeader` arrays (32 B/record); ten files
of 200k records each is ~64 MB.

## 12. Error handling

`clog-core` exposes a typed `CoreError` enum (`thiserror`-derived) covering
file-not-found, permission-denied, empty-file, file-too-large, generic I/O,
pattern-invalid, regex-invalid, index-corrupt, and rotated. The `clog-app`
boundary translates these into a structured `IpcError { kind, message,
suggestion, recoverable }` payload, where `kind` is a kebab-case string the UI
can switch on. SMB-specific suggestions are added at translation time (a
permission denied on a `\\wsl$\...` path suggests checking that the WSL
distribution is running).

The UI surfaces errors by recoverability:

- `recoverable: false` -> modal in the affected pane only.
- `recoverable: true` -> dismissable banner across the affected pane.
- `kind: rotated | index-rebuilt` -> brief footer toast (~2s), no interaction.
- `kind: regex-invalid` -> inline red underline on the search bar, never a
  modal (expected during typing).

A `std::panic::set_hook` in `clog-app` catches panics anywhere in Rust, ships
them through IPC as a fatal-error event, and disables only the offending file
handle. The app keeps running so the user does not lose other open files'
state.

The v1 file-size hard limit is 2 GB, with a clear error including the actual
size if exceeded. Above that we would want chunked indexing with on-disk
record headers; deferred.

Clog's own logs are written via `tracing` + `tracing-appender` to
`%LOCALAPPDATA%\clog\logs\` with daily rotation and 7-day retention. A
`--verbose` CLI flag also echoes to stderr for dev.

## 13. Theming

Dark and light themes only. Default follows the OS via
`matchMedia('(prefers-color-scheme: dark)')`; a manual toggle in settings
overrides. The mascot/personality of the application is otherwise
self-contained; no custom-palette UI is offered in v1.

CSS uses a two-layer custom-property token system. The palette layer is
theme-scoped under `:root[data-theme="dark"]` / `[data-theme="light"]` and
contains raw colours (background levels, foreground levels, accent, and one
colour per log level). The semantic layer is theme-agnostic and is what
components consume (`--bg-app`, `--fg-default`, `--gutter-error`,
`--row-height`, `--font-mono`, etc.).

Level colours are verified at design time to meet WCAG AA contrast against
their theme's primary background. Colour is never the sole signal: the
`[LEVEL]` text and the position of the gutter convey the same information.

No SCSS or utility-class framework. The component surface is small enough
that plain CSS files alongside SFCs are clearer.

## 14. Persistence

```
%LOCALAPPDATA%\clog\
  settings.json              # global app prefs
  session.json               # open tabs, recent files, geometry
  highlight-rules.json       # global axis-2 rules
  patterns.json              # per-file PatternLayout overrides (path-keyed)
  per-file-rules/
    <path-hash>.json         # per-file highlight rule overrides
  index/
    <path-hash>.idx          # persistent line + RecordHeader cache
  logs/
    clog.log                 # current day
    clog.log.YYYY-MM-DD      # rotated
```

Path hashing is `blake3(absolute_path_lowercased)` truncated to 16 hex chars.
Stable across runs, collision-free in practice, and keeps potentially
sensitive paths out of filenames.

All JSON files carry a top-level `"schema": 1` field. Future migrations bump
it; a missing field is treated as v1. JSON (not TOML) is used because the
highlight rule shape is nested enough that JSON expresses it more naturally.

Prefs are read and written through `tauri-plugin-store` for automatic IPC
serialisation and disk coalescing. The index cache and logs use plain
`std::fs` from the core; they do not need to round-trip through a plugin.

Index cache eviction is not automated in v1; users can clear it from the
settings panel. Automated LRU/age-based sweeping is a v1.x feature.

**Portable mode** is supported from v1.0. If `clog.exe` finds a `clog-data\`
folder next to itself, that path is used in place of `%LOCALAPPDATA%\clog\`.
This makes the portable zip distribution genuinely portable.

## 15. Testing

The strategy is layered by where bugs hide:

- **`clog-core` unit tests** against `research/cheesecake.out` as the primary
  fixture, plus tiny synthetic fixtures for edge cases (empty file, single
  line, no trailing newline, malformed records).
- **One `proptest`** on the PatternLayout compiler — the one place where
  property-based testing pays for itself, because a bug here corrupts every
  downstream layer.
- **Tempfile-based integration tests** for tail and rotation. These cannot be
  unit-tested with mocks because filesystem mocks diverge from real behaviour
  in exactly the ways that produce bugs.
- **IPC contract test** in `clog-app` that spawns the app headless, drives
  commands, and asserts on emitted channel/event payloads.
- **Three Playwright smoke tests**: open-file, search, tail-survives-rotation.
- **`criterion` benches** for `index_file`, `compile_pattern`, `search_smart`
  and `search_regex`, run manually before releases, not CI-gated.

No per-component Vue snapshot tests — they generate noise on every CSS tweak
and rarely catch real bugs.

A `fake_tailer` example binary lives at
`crates/clog-core/examples/fake_tailer.rs`. It appends synthetic log lines to
a scratch file at a configurable rate and occasionally truncates to simulate
rotation. Used both for manual dev iteration and as the driver for the
rotation Playwright test.

CI runs `cargo fmt --check`, `cargo clippy -D warnings`,
`cargo test --workspace`, `npm -C ui run build`, and the Playwright suite
headless. Windows-specific tests are `#[cfg(target_os = "windows")]`-gated so
the Linux runner stays useful for the cross-platform parts.

## 16. Packaging and distribution

Two artefacts per tagged release, built in one CI pass on a Windows runner:

- An **NSIS installer** (`clog_x.y.z_x64-setup.exe`), per-user install by
  default (no UAC), installs to `%LOCALAPPDATA%\Programs\clog\`. Offers an
  opt-in checkbox to register clog under the "Open with..." menu for both
  `.log` and `.out` extensions.
- A **portable zip** (`clog_x.y.z_x64-portable.zip`) containing `clog.exe` and
  its sibling files. Combined with the portable-mode detection above, this is
  a true drop-in distribution.

Tauri's auto-updater plugin is wired from v1. A signed `latest.json` is
self-hosted (GitHub Pages or equivalent). The update-signing keypair is
generated once and stored only in CI secrets; the public key sits in
`tauri.conf.json`. The on-launch check runs once per day, async, and never
blocks the UI. A "Check for updates" item in the Help menu triggers it
manually.

Code-signing infrastructure is in place from day one but no cert is purchased
for v1. The CI build is cert-aware via env vars
(`CLOG_SIGNING_PFX_BASE64`, `CLOG_SIGNING_PASSWORD`); if absent, signing is
skipped without failing the build. A standard OV cert can be added later
without changing the build pipeline.

MSI installers, EV signing, and macOS/Linux builds are all explicit non-goals
for v1.

## 17. v1 non-goals (explicit)

The following are deliberately deferred so that v1 ships:

- WSL companion daemon (the `LineSource` trait reserves the slot).
- Memory-mapped backend for local files.
- Soft-wrap of long lines.
- Split-pane tab UX (planned v1.1).
- Field-scoped search operators.
- Generic pattern inference beyond the built-in library.
- Custom user themes / palette editor.
- MSI installer.
- Code-signing certificate.
- Index cache eviction sweep.
- Files larger than 2 GB.
- Localisation / i18n.
- macOS and Linux desktop builds.
- Network telemetry / crash reporting.
- `notify`-based file watching.
- log4j2 specifiers beyond the supported subset (`%X{}`, `%mdc`,
  `%throwable{}` etc.).
