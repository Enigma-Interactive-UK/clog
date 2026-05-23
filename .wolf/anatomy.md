# anatomy.md

> Auto-maintained by OpenWolf. Last scanned: 2026-05-23
> P1 scaffold landed. P2 vertical slice landed (records + virtualised list).

## ./

- `Cargo.toml` - Rust workspace (members: clog-core, clog-app)
- `CLAUDE.md` - OpenWolf project instructions
- `README.md` - dev setup + how to run P1 demo
- `clog.code-workspace`
- `.gitignore`
- `.cargo/config.toml` - `cargo dev` alias for `cargo tauri dev --config crates/clog-app/tauri.conf.json`

## .claude/rules/

- `openwolf.md`

## crates/clog-core/

- `Cargo.toml` - pure engine crate, no Tauri deps (serde, thiserror)
- `src/lib.rs` - `FileSummary`, `CoreError`, `summarise_file()`, `index_file()`, re-exports + smoke + watertight tests
- `src/index.rs` - `LineIndex` (in-memory `Vec<u64>` of line-start offsets) + edge-case unit tests
- `src/source.rs` - `LineSource` trait + `StreamedFile` impl (BufReader + seek)
- `src/record.rs` - `Level` enum, `RecordHeader`, `RecordScanner` trait, hardcoded `WslOinkScanner`, `scan_records()` + unit tests

## crates/clog-app/

- `Cargo.toml` - Tauri v2 binary `clog`, depends on clog-core + plugin-dialog
- `build.rs` - calls `tauri_build::build()`
- `tauri.conf.json` - app config, `frontendDist: ../../ui/dist`
- `src/main.rs` - `AppState` (Mutex file registry), IPC commands: `open_file` (returns `OpenedFilePayload` with file_id + record_count), `get_records(file_id, start, end)` returning `RecordsPayload`, `close_file`
- `capabilities/default.json` - grants dialog open to main window
- `icons/` - 32/128/128@2x/icon.png + icon.ico

## ui/

- Vue 3 + TypeScript + Vite scaffold
- `package.json` - deps: @tauri-apps/api, @tauri-apps/plugin-dialog, @tanstack/vue-virtual; dev: @tauri-apps/cli
- `vite.config.ts` - port 1420, ignores crates/ and target/ from HMR watch
- `src/App.vue` - virtualised record list backed by `useVirtualizer`, PAGE_SIZE=256 paged record fetch via `get_records`, fixed 18px row height with line-number gutter
- `src/main.ts`, `src/style.css`

## docs/

- `design.md` - v1 design snapshot
- `build-phases.md` - P1..P10 phase plan

## research/ (gitignored)

- `log4j.prod.properties`
- `log4j2.wsl-oink.xml`
- `solopress-prod.log` (~8.7 MB, 74,921 lines - P1 smoke fixture)
- `solopress-wsl-oink.out` (~42 KB, 386 lines)
