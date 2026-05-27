# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

**Clog** (Core Log) - a Windows desktop application for viewing, tailing, searching and filtering log4j2-formatted log files produced by Play 1.x Java applications.

The design lives in [docs/design.md](docs/design.md). The v1 build was sliced into ten vertical phases in [docs/build-phases.md](docs/build-phases.md); all ten have landed (workspace version `1.0.0`, P10 slice A: NSIS installer + portable zip). Post-v1 enhancement candidates live in [docs/future-ideas.md](docs/future-ideas.md) - that is the source of truth for "what's next". At the start of any session that's about to touch code, check the relevant phase doc (or future-ideas entry) for context before editing.

## Stack

- **Rust** (stable, msvc target) for the engine. MSRV 1.94.
- **Tauri v2** as the app shell. Tauri CLI installed via `cargo install tauri-cli --version "^2.0" --locked`.
- **Vue 3 + Vite + TypeScript** for the UI. Bare `ref`/`reactive` (no Pinia yet), Composition API + `<script setup>`.

## Workspace layout

```
clog/
  Cargo.toml                workspace manifest
  crates/
    clog-core/              pure engine (no Tauri deps)
    clog-app/               Tauri v2 binary (bin name: clog)
      tauri.conf.json
      capabilities/         capability files (default.json grants dialog open)
      icons/                bundle icons (real, used by NSIS installer)
  ui/                       Vue 3 + Vite + TS frontend
  docs/                     design.md, build-phases.md, future-ideas.md
  scripts/                  release packaging (make-portable-zip.ps1)
  research/                 sample logs + log4j2 configs (gitignored)
  .wolf/                    OpenWolf project memory
```

**Crate split rule:** `clog-core` stays Tauri-free (engine, parser, search, tail, persistence). `clog-app` is the only crate allowed to depend on `tauri`, `tauri-build`, or `tauri-plugin-*`. A `clog-protocol` crate is reserved for when the WSL companion daemon is real.

## Run / build / verify commands

All run from the workspace root.

```powershell
# Dev (launches Vite + Tauri window) -- alias from .cargo/config.toml
cargo dev
# (equivalent to: cargo tauri dev --config crates/clog-app/tauri.conf.json)

# Workspace build
cargo build --workspace

# UI production build
npm --prefix ui run build

# Lints (CI-equivalent — must stay green)
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings

# Tests
cargo test --workspace

# UI unit tests (vitest)
npm --prefix ui run test

# Release artefacts (NSIS installer + portable zip) -- one-shot
.\scripts\release.ps1

# Or run the steps individually:
cargo dist                           # alias for: cargo tauri build --config crates/clog-app/tauri.conf.json
.\scripts\make-portable-zip.ps1      # accepts -SkipBuild to reuse an existing build
```

**Tauri config gotcha:** `beforeDevCommand` runs with cwd set to the frontend root (`ui/`), not the workspace root and not the tauri.conf.json dir. Keep it as `npm run dev` / `npm run build`. Prefixing with `--prefix ui` double-paths to `ui/ui/`.

## Log format Clog must parse

Sample logs and the originating log4j2 configs live in [research/](research/). Two production patterns are confirmed:

```
[%-5level] %d{yyyy-MM-dd HH:mm:ss.SSS} [%t] %c{1} - %msg%n    (wsl-oink)
%d{yyyy-MM-dd HH:mm:ss.SSS} %level [%t] - %msg%n              (prod, no logger field, level unbracketed)
```

The parser is **PatternLayout-driven**, not hardcoded - see the decision log in `.wolf/cerebrum.md`.

Notes that matter for the parser:
- Level may be bracketed and left-padded to 5 chars (`[INFO ]`) or bare (`INFO`).
- Thread name is bracketed and may contain spaces or punctuation.
- Logger name (`%c{1}`) is the short class name, typically `play` for framework lines.
- `%msg` may contain newlines (stack traces) - continuation lines belong to the previous record.
- Files rotate via `OnStartupTriggeringPolicy` (wsl-oink) and `TimeBasedTriggeringPolicy` (prod). Rotation is detected via `(size shrank) OR (first 256 bytes hash changed)`.

### Sample fixtures

- [research/cheesecake-prod.log](research/cheesecake-prod.log) - 8.7 MB, **74,921 lines**, prod pattern. Primary smoke fixture.
- [research/cheesecake-wsl-oink.out](research/cheesecake-wsl-oink.out) - 42 KB, 386 lines, wsl-oink pattern.

Note: `docs/build-phases.md` and `docs/design.md` still reference `research/cheesecake.out` (the historical name). Code should use the real filenames above.

## OpenWolf

This repo is managed by OpenWolf. The protocol in [.wolf/OPENWOLF.md](.wolf/OPENWOLF.md) is binding for every session:

- Consult [.wolf/anatomy.md](.wolf/anatomy.md) before reading files; update it when files are added/renamed/removed.
- Consult [.wolf/cerebrum.md](.wolf/cerebrum.md) before generating code; record preferences, learnings, do-not-repeats and decisions there as they emerge.
- Log bugs to `.wolf/buglog.json` per the protocol (low threshold - when in doubt, log).
- Append a one-line memory entry to `.wolf/memory.md` after significant actions.
