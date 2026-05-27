# Clog

**Core Log** - a Windows desktop viewer for log4j2-formatted log files
produced by Play 1.x Java applications.

The design lives in [`docs/design.md`](docs/design.md). The v1 build was
sliced into ten vertical phases in [`docs/build-phases.md`](docs/build-phases.md);
all ten have landed (workspace version `1.0.0`, P10 slice A: NSIS installer +
portable zip). Post-v1 enhancement candidates live in
[`docs/future-ideas.md`](docs/future-ideas.md).

## How to develop locally

Prerequisites: Rust stable (1.94+), Node 20+, and the Tauri v2 CLI.

```powershell
# one-off
cargo install tauri-cli --version "^2.0" --locked
npm --prefix ui install
```

Then, from the repository root:

```powershell
cargo tauri dev --config crates/clog-app/tauri.conf.json
```

This launches the Vite dev server for the UI and a Tauri-hosted window.
Click **Open file...**, pick `research/cheesecake-prod.log` (or any
`.log`/`.out`) to start tailing, searching, filtering and highlighting.
A `cargo dev` alias for the full command lives in `.cargo/config.toml`.

### Producing release artefacts

```powershell
# NSIS installer + portable zip (P10 slice A)
cargo tauri build --config crates/clog-app/tauri.conf.json
.\scripts\make-portable-zip.ps1
```

### Useful one-off commands

```powershell
# Workspace build
cargo build --workspace

# UI production build (produces ui/dist/)
npm --prefix ui run build

# Lints (CI-equivalent)
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings

# Tests
cargo test --workspace
```

## Repository layout

```
clog/
  Cargo.toml                workspace manifest
  crates/
    clog-core/              pure engine (no Tauri deps)
    clog-app/               Tauri v2 binary
      tauri.conf.json       Tauri config
      capabilities/         capability files
      icons/                bundle icons
  ui/                       Vue 3 + Vite frontend
  docs/                     design, phased build plan, future ideas
  scripts/                  release packaging (portable zip)
  research/                 sample logs + log4j2 configs (gitignored)
  .wolf/                    OpenWolf project memory
```

## Features (v1)

- Open any log4j2 PatternLayout-formatted log file; pattern is auto-detected
  from a built-in library, or pasted/edited per file. Regex escape hatch
  with named captures.
- Virtualised viewport - the full 8.7 MB / 75k-line sample log scrolls smoothly.
- Live tail with rotation detection (size-shrink or first-256-byte hash change),
  auto-scroll toggle, jump-to-bottom.
- Stack-trace and structural highlighting (axis-1 fields + axis-2 user/default
  rules). Clickable URLs.
- Smart and regex search; live filter; per-level mask toggles.
- Persistent settings + session restore + persistent index cache.
- Highlight rule editor with live preview (global and per-file).
- Multi-tab UI, single-instance with CLI argv forwarding, drag-drop to open.
- Light/dark/system themes; runtime font-size control.
- NSIS installer + portable zip; `.log` and `.out` file associations.
