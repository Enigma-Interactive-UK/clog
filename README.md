# Clog

**Core Log** - a Windows desktop viewer for log4j2-formatted log files
produced by Play 1.x Java applications.

The design lives in [`docs/design.md`](docs/design.md). The build is sliced
into ten vertical phases - see [`docs/build-phases.md`](docs/build-phases.md).
Current phase: **P1 - It opens**.

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
For the P1 demo, click **Open file...**, pick `research/solopress-prod.log`
(or any `.log`/`.out`), and the file path + line count + byte size appears.

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
  docs/                     design + phased build plan
  research/                 sample logs + log4j2 configs (gitignored)
  .wolf/                    OpenWolf project memory
```
