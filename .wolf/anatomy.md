# anatomy.md

> Auto-maintained by OpenWolf. Last scanned: 2026-05-23
> P5 vertical slice landed (axis-2 highlight rules + clickable URLs).

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

- `Cargo.toml` - engine crate, no Tauri deps (serde, thiserror, regex; dev: proptest)
- `src/lib.rs` - `FileSummary`, `CoreError`, `summarise_file()`, `index_file()`, `sample_lines()`, re-exports, smoke + watertight + auto-detect tests
- `src/index.rs` - `LineIndex` (in-memory `Vec<u64>` of line-start offsets) + edge-case unit tests
- `src/source.rs` - `LineSource` trait + `StreamedFile` impl (BufReader + seek)
- `src/record.rs` - `Level` enum, `RecordHeader { ... fields: HeaderFields }`, `RecordScanner` trait (`try_parse_header -> ParsedHeader`), `scan_records()` + unit tests. CompiledPattern impls RecordScanner.
- `src/pattern.rs` - log4j2 PatternLayout compiler. `Token` enum (Literal/Level/Date/Thread/Logger/Message/Newline/SourceFile/SourceLine), `DateFormat`/`DateAtom`, `PatternError`, `PatternWarning`, `CompiledPattern { source, tokens, warnings }`, `HeaderFields`, `ParsedHeader`. Supported specifiers: `%d{...}`, `%level`/`%-Nlevel`/`%p`/`%-Np`, `%t`, `%c`/`%c{N}`, `%C`/`%C{N}` (aliased to logger), `%F` (source filename), `%L` (digit run), `%msg`/`%m`, `%n`, `%%`. Source-filename span lands in `fields.logger` when no `%C`/`%c` already claimed it. `BUILTIN_PATTERNS` (9 entries, ordered most-specific first: wsl-oink, play-class-site, play-absolute-site, prod, log4j2-default, play-short-dash, play-absolute, play-short, prod-no-thread), `auto_detect()`, `builtin_pattern(name)` lookup helper. 14 unit tests.
- `src/regex_scanner.rs` - `RegexScanner` escape hatch reading named captures (level/timestamp/thread/logger/msg). 2 unit tests.
- `src/tail.rs` - polling tail state machine. `TailState { path, consumed, head_hash, head_prefix_len }`, `TailEvent { NoChange | Appended { from_offset, bytes } | Rotated }`. `poll()` stats + FNV-1a-hashes first 256 bytes (anchored to a fixed prefix length so file growth doesn't trip the hash), trims to last `\n` so partial writes stay buffered. `reset_to(size)` re-anchors after caller re-indexes. `HEAD_HASH_BYTES = 256`, `DEFAULT_POLL_INTERVAL_MS = 250`. 6 tempfile unit tests.
- `tests/pattern_proptest.rs` - proptest: render + reparse round trip for wsl-oink + prod patterns.
- `examples/fake_tailer.rs` - dev binary. Appends synthetic wsl-oink-shaped records to `<path>` at `--rate N` lines/sec (default 10). `--rotate` truncates target on entry to exercise rotation. Every 17th record gets 2 stack-trace continuations. Optional `--count N` for a bounded run.

## crates/clog-app/

- `Cargo.toml` - Tauri v2 binary `clog`, depends on clog-core + plugin-dialog + plugin-opener + tokio (sync/time/macros)
- `build.rs` - calls `tauri_build::build()`
- `tauri.conf.json` - app config, `frontendDist: ../../ui/dist`
- `src/main.rs` - `AppState` (Mutex file registry), `OpenedFile { path, records, record_first_line, line_count, bytes, line_offsets, pattern_source, pattern_name, scanner_kind, tail_shutdown, tail_join }`. `ScannerKind { Pattern(String) | Regex(String) }` + `CompiledScanner { Pattern(CompiledPattern) | Regex(RegexScanner) }` sum so runtime-selected scanners stay sized for `index_file`/`scan_records`. IPC commands:
  - `open_file(path)` -> `OpenedFilePayload { file_id, path, size_bytes, line_count, record_count, pattern_name, pattern_source, pattern_score }`. Auto-detects pattern from 64KB sample.
  - `get_records(file_id, start, end)` -> `RecordsPayload` (legacy P2 surface, still served)
  - `get_lines(file_id, start, end)` -> `LinesPayload { start_line, lines: [LinePayload { record_idx, line_within_record, level, fields?, text }] }`. O(log n) record lookup via cached record_first_line.
  - `test_pattern(file_id, pattern?, regex?)` -> `PatternTestPayload { score, sample_size }`
  - `set_pattern(file_id, pattern?, regex?)` -> `ApplyPatternPayload { record_count, pattern_source }`
  - `start_tail(file_id, on_delta: Channel<TailDelta>)` - spawns tokio task that polls TailState every 250ms; on Appended runs `extend_with_appended` (in-place line/record extension); on Rotated runs `apply_rotation` (re-index off the lock, swap under the lock, reset_to). Tear-down via oneshot shutdown stored on OpenedFile.
  - `stop_tail(file_id)` - sends shutdown to running tail task if any.
  - `get_level_minimap(file_id, bucket_count)` -> `LevelMinimapPayload { buckets: Vec<Level>, line_count }`. Walks records and keeps the worst severity per bucket via `level_rank` (fatal > error > warn > all > info > debug > trace > unknown/off), so a single warn/error in an otherwise quiet bucket owns the stripe.
  - `close_file(file_id)` - also tears down any tail task.
  - `TailDelta { new_record_count, line_count, record_count, last_offset, rotated }` is the channel payload.
- `src/channels.rs` - `TailEmitter<T>` pass-through wrapper over `tauri::ipc::Channel`. Reserved seam for the 60Hz coalescer that activates with search streaming in P6; tail's 250ms cadence is already below the budget.
- `capabilities/default.json` - grants dialog open + opener (allow-open-url) to main window
- `icons/` - 32/128/128@2x/icon.png + icon.ico

## ui/

- Vue 3 + TypeScript + Vite scaffold
- `package.json` - deps: @tauri-apps/api, @tauri-apps/plugin-dialog, @tauri-apps/plugin-opener, @tanstack/vue-virtual; dev: @tauri-apps/cli, vitest
- `vite.config.ts` - port 1420, ignores crates/ and target/ from HMR watch
- `vitest.config.ts` - node env, `src/**/*.test.ts` included
- `tsconfig.app.json` - excludes `src/**/*.test.ts` so vue-tsc doesn't pull vitest's node types into the App.vue compilation (would otherwise break setTimeout return-type assumptions)
- `src/App.vue` - per-physical-line virtualised viewer. Includes a 20px minimap scrollbar (canvas painted at devicePixelRatio, faded rgba overlays of the level palette with info/unknown deliberately absent so they read as background; bucket count = min(viewport_height_px, line_count * ROW_HEIGHT) so short files align to the top), a draggable viewport indicator with brighter border treatment for visibility, click/drag scrubbing via pointer capture (disengages follow-tail on grab), a position: fixed hover tooltip (z-index 100, transform translate(calc(-100% - 4px), -50%) anchored to minimap left edge, right-side callout arrow built from stacked ::before/::after triangles, walks back to the record header to resolve timestamps for continuation lines, pages watcher fills the timestamp in once the relevant page lands), and a bottom status bar (.status-bar with .slot.left + .slot.right placeholders, margin-left: auto on .right). .viewport-shell wraps viewport + minimap in flex row with overflow: hidden so the tooltip doesn't pull a page-level scrollbar; .viewport hides its native scrollbar via scrollbar-width: none + ::-webkit-scrollbar { display: none }. Minimap refetches on file open, pattern apply, tail delta, rotation, and viewport resize (ResizeObserver), debounced via requestAnimationFrame. PAGE_SIZE=256 paged `get_lines` fetch with `LineRow { record_idx, line_within_record, level, fields, text }`. `renderLine(row)` produces flat leaf spans by slicing axis-1 fields (`headerBaseSpans`) and overlaying axis-2 highlight matches (`highlightsFor` + `overlay`); continuation rows use a full-line `message` base span. 4px level-coloured left gutter, sticky record-header overlay when scrolled mid-record, indented continuation lines. Spans with `url` (axis-2 URL rule) click through to `openUrl` from @tauri-apps/plugin-opener. Pattern paste bar with PatternLayout/Regex toggle, Test (live match score), Apply (set_pattern). Tail controls cluster in the header bar: tailing indicator (idle/active dot + pulse-on-delivery), follow-tail toggle, jump-to-bottom button (visible when detached). Channel<TailDelta> opened on open_file via `invoke('start_tail', { onDelta: channel })`. Scroll handler disengages follow-tail when user scrolls > 4 rows from bottom. Rotation toast (~2.5s) when delta.rotated.
- `src/highlight/engine.ts` - axis-2 highlight engine. `setRules(rules)` compiles once with gd flags; `computeHighlights(text)` paints a per-char (cls, priority, url) array (higher priority overwrites, zero-width matches skipped, 256-iter per-rule guard) and collapses runs into ordered non-overlapping `HighlightSpan { start, end, cls, url? }`. Sub-groups expand from named captures at default priority `parent+1`. `highlightsFor(text)` is a size-capped cache (4000 entries, oldest-quarter eviction) keyed by `version + text`. `overlay(text, base, axis2)` blends axis-1 base spans with axis-2 spans into ordered `LeafSpan { text, cls (space-joined), url? }`. `rulesVersion()` returns the monotonic version stamp for cache invalidation.
- `src/highlight/default-rules.json` - 6 baked-in rules: caused-by (prio 30), stack-frame with fqn/file/line sub-groups (prio 40), java-exception (prio 20), url with self-href (prio 50), windows path (prio 10), unix path (prio 10).
- `src/highlight/engine.test.ts` - 14 vitest cases covering default rules, overlay merging, span non-overlap, cache stability, version bumping, url propagation, empty-input safety, no-rules behaviour.
- `src/main.ts`, `src/style.css` - two-layer CSS tokens. P3 added level palette (--level-{trace..unknown}), axis-1 fg tokens (--fg-{timestamp,thread,logger,message,separator-dash}), sticky bg + border, continuation-indent + gutter-width primitives. P5 added --hl-{exception,caused-by,stack-fqn,stack-file,stack-line,path,url}-fg axis-2 highlight tokens; `.h-*` classes attached to viewport .row apply colour/weight/decoration only (no backgrounds). Dark theme only (light deferred to P7).

## docs/

- `design.md` - v1 design snapshot
- `build-phases.md` - P1..P10 phase plan

## research/ (gitignored)

- `log4j.prod.properties`
- `log4j2.wsl-oink.xml`
- `solopress-prod.log` (~8.7 MB, 74,921 lines)
- `solopress-wsl-oink.out` (~42 KB, 386 lines)
