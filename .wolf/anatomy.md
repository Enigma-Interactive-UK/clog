# anatomy.md

> Auto-maintained by OpenWolf. Last scanned: 2026-05-26T18:11:14.784Z
> Files: 98 tracked | Anatomy hits: 0 | Misses: 0

## ./

- `.gitignore` — Git ignore rules (~98 tok)
- `Cargo.toml` — Rust package manifest (~111 tok)
- `CLAUDE.md` — CLAUDE.md (~1313 tok)
- `clog.code-workspace` (~16 tok)
- `README.md` — Project documentation (~783 tok)

## .cargo/

- `config.toml` (~37 tok)

## .claude/

- `settings.json` (~441 tok)

## .claude/rules/

- `openwolf.md` (~429 tok)

## .claude/skills/release/

- `SKILL.md` — Release (~993 tok)

## crates/clog-app/

- `build.rs` (~12 tok)
- `Cargo.toml` — Rust package manifest (~228 tok)
- `tauri.conf.json` (~456 tok)

## crates/clog-app/capabilities/

- `default.json` (~142 tok)

## crates/clog-app/gen/schemas/

- `acl-manifests.json` — Declares command (~20508 tok)
- `capabilities.json` (~114 tok)
- `desktop-schema.json` (~38047 tok)
- `windows-schema.json` (~38047 tok)

## crates/clog-app/src/

- `channels.rs` — 60 Hz coalescing layer for streaming IPC channels. (~1041 tok)
- `main.rs` — Tauri commands take `State` by value by convention; the lint fires on every (~36348 tok)
- `paths.rs` — Filesystem layout for clog's persistent data. (~994 tok)
- `persistence.rs` — On-disk JSON state: `settings.json`, `session.json`, `patterns.json`. (~4899 tok)

## crates/clog-core/

- `Cargo.toml` — Rust package manifest (~105 tok)

## crates/clog-core/examples/

- `fake_tailer.rs` — Append synthetic log4j2-shaped records to a file at a fixed rate. Used (~1096 tok)

## crates/clog-core/src/

- `idx_cache.rs` — Persistent on-disk cache of the `(LineIndex, Vec<RecordHeader>)` produced (~2765 tok)
- `index.rs` — In-memory line offset index. (~782 tok)
- `lib.rs` — Clog engine. No Tauri deps. (~2861 tok)
- `pattern.rs` — log4j2 `PatternLayout` compiler. (~9536 tok)
- `record.rs` — Record header type and scanner. (~2236 tok)
- `regex_scanner.rs` — Regex escape hatch. (~1269 tok)
- `search.rs` — Smart + regex search engine. P6. (~6059 tok)
- `slow_requests.rs` — Slow-request detection, aggregation, and speed-grid builder. (~9320 tok)
- `source.rs` — Line-source abstraction. The v1 impl streams a local file via `BufReader`; (~946 tok)
- `tail.rs` — Polling tail loop + rotation detection. (~4417 tok)
- `thread_groups.rs` — Thread-name classification into a fixed five-group taxonomy + Other. (~2379 tok)

## crates/clog-core/tests/

- `pattern_proptest.rs` — Property test for the `PatternLayout` compiler: generate well-formed (~943 tok)

## design/

- `icon.psd` (~39819 tok)

## docs/

- `build-phases.md` — Clog v1 — Build phases (~4689 tok)
- `design.md` — Clog v1 — Design (~5493 tok)
- `future-ideas.md` — Clog - Future ideas (~999 tok)

## docs/superpowers/plans/

- `2026-05-23-minimap-heatmap.md` — Minimap heatmap implementation plan (~8500 tok)
- `2026-05-23-slow-request-insights.md` — Slow request insights implementation plan (~30761 tok)
- `2026-05-24-thread-insights.md` — Thread insights + consolidated filter flyout implementation plan (~11303 tok)

## docs/superpowers/specs/

- `2026-05-23-minimap-heatmap-design.md` — Minimap heatmap upgrade - design (~1642 tok)
- `2026-05-23-slow-request-insights-design.md` — Slow request insights - design (~9472 tok)
- `2026-05-24-thread-insights-design.md` — Thread insights + consolidated filter flyout - design (~3328 tok)

## research/

- `log4j.prod.properties` (~331 tok)
- `log4j2.wsl-oink.xml` (~729 tok)
- `solopress-wsl-oink-short.out` (~200 tok)
- `solopress-wsl-oink.out` — Declares set (~11695 tok)
- `test - Copy.log` (~202 tok)

## scripts/

- `make-portable-zip.ps1` — make-portable-zip.ps1 (~805 tok)
- `release.ps1` — release.ps1 (~288 tok)

## ui/

- `.gitignore` — Git ignore rules (~68 tok)
- `index.html` — ui (~94 tok)
- `package-lock.json` — npm lock file (~31329 tok)
- `package.json` — Node.js package manifest (~195 tok)
- `README.md` — Project documentation (~111 tok)
- `tsconfig.app.json` — /*.ts", "src/**/*.tsx", "src/**/*.vue"], (~122 tok)
- `tsconfig.json` — TypeScript configuration (~34 tok)
- `tsconfig.node.json` (~169 tok)
- `vite.config.ts` — Vite build configuration (~124 tok)
- `vitest.config.ts` — Vitest test configuration (~79 tok)

## ui/src/

- `App.vue` — App orchestrator. Composes the tab list, session save/restore, (~4537 tok)
- `main.ts` (~32 tok)
- `style.css` — Styles: 214 vars (~5725 tok)
- `tab.ts` — Per-tab state container. A Tab owns every reactive ref that was (~6104 tok)
- `types.ts` — Shared TypeScript interfaces used across the UI. Mirrors the wire shapes (~2402 tok)

## ui/src/components/

- `AboutModal.vue` — About modal. Lazily resolves the Tauri app name/version/tauri-version on (~1130 tok)
- `AppHeader.vue` — Title bar: app logo (opens About), Open button, Settings cog, and the (~1247 tok)
- `BaseModal.vue` — Shared modal scaffold: backdrop, frame, header bar with title + close. (~693 tok)
- `ColourPickerPopover.vue` — Compact popover that surfaces both foreground and background palette (~1606 tok)
- `ContextMenu.vue` — Custom right-click context menu surface. Renders the items in (~3241 tok)
- `DropOverlay.vue` — Drop-target overlay shown while the user drags files over the window. (~286 tok)
- `FiltersPopover.vue` — Popover hosting the level mask and thread-group mask toggles. Anchored (~1469 tok)
- `HelloWorld.vue` — Vue: setup, TS (~755 tok)
- `HighlightRulesEditor.vue` — Editable table of user highlight rules with a live preview pane. (~4219 tok)
- `InsightsDrawer.vue` — Right-side collapsible drawer hosting the slow-request insights for (~10874 tok)
- `LogViewport.vue` — Per-tab viewport. Owns the virtualised line list, the minimap canvas, (~21188 tok)
- `PatternModal.vue` — Pattern editor modal. Operates directly on the current tab's pattern (~1171 tok)
- `RecordModal.vue` — Full-record viewer modal. Shows the raw text of a single log record so (~1158 tok)
- `SearchBar.vue` — Search + filter + level-mask control bar for a single tab. All state (~2888 tok)
- `SettingsModal.vue` — Settings modal split into four tabs: General (appearance / behaviour / (~7388 tok)
- `StatusBar.vue` — Footer status bar: cache hint, record/line/byte stats for the current (~1382 tok)
- `TabStrip.vue` — Tab strip across the top of the app. Lists open tabs with a tail status (~3851 tok)

## ui/src/composables/

- `useAppShortcuts.ts` — Global keyboard shortcuts wired to the document in capture phase. (~769 tok)
- `useContextMenu.ts` — Global custom context-menu state. One menu at a time; module-scoped (~470 tok)
- `useHighlightRules.ts` — Global + per-file highlight rule loading and engine wiring. (~1118 tok)
- `useSession.ts` — Multi-tab session save/restore + the autosave watcher. (~1203 tok)
- `useSettings.ts` — Global settings, theme handling, and font-size scaling. Owns the (~2237 tok)
- `useStartupPaths.ts` — CLI argv + single-instance forward handler. (~450 tok)
- `useTabs.ts` — Tab list ownership: the reactive `tabs` array, the active tab pointer, (~1307 tok)
- `useWindowChrome.ts` — Window chrome: maximize/restore tracking + the three title-bar buttons. (~443 tok)

## ui/src/highlight/

- `default-rules.json` (~358 tok)
- `engine.test.ts` — HighlightRulesFile: findCls (~1401 tok)
- `engine.ts` — Reactive version counter. Bumped on every `setRules()` call so any Vue (~3424 tok)
- `record-render.ts` — Render a log line's text into LeafSpans, applying axis-1 header field (~804 tok)
- `user-rule.test.ts` — UserHighlightRule: makeUserRule (~1304 tok)
- `user-rule.ts` — Compose the effective engine rule set from the three layers: (~460 tok)
