# anatomy.md

> Auto-maintained by OpenWolf. Last scanned: 2026-05-30T10:58:37.821Z
> Files: 127 tracked | Anatomy hits: 0 | Misses: 0

## ./

- `.gitattributes` — /*.md   text eol=lf (~131 tok)
- `.gitignore` — Git ignore rules (~142 tok)
- `Cargo.toml` — Rust package manifest (~111 tok)
- `CLAUDE.md` — CLAUDE.md (~1581 tok)
- `clog.code-workspace` (~16 tok)
- `LICENSE` — Project license (~295 tok)
- `README.md` — Project documentation (~1224 tok)

## .cargo/

- `config.toml` (~37 tok)

## .claude/

- `settings.json` (~441 tok)

## .claude/rules/

- `openwolf.md` (~429 tok)

## .claude/skills/release/

- `SKILL.md` — Release (~2087 tok)

## .superpowers/brainstorm/1198-1779914439/content/

- `collapsed-row.html` (~1908 tok)

## .superpowers/brainstorm/1198-1779914439/state/

- `server-stopped` (~14 tok)
- `server.pid` (~2 tok)

## crates/clog-app/

- `build.rs` (~12 tok)
- `Cargo.toml` — Rust package manifest (~252 tok)
- `installer.nsi` — Declares because (~8587 tok)
- `tauri.conf.json` (~589 tok)

## crates/clog-app/capabilities/

- `default.json` (~182 tok)

## crates/clog-app/gen/schemas/

- `acl-manifests.json` — Declares command (~21040 tok)
- `capabilities.json` (~128 tok)
- `desktop-schema.json` (~38940 tok)
- `windows-schema.json` (~38940 tok)

## crates/clog-app/src/

- `channels.rs` — 60 Hz coalescing layer for streaming IPC channels. (~1041 tok)
- `main.rs` — Tauri commands take `State` by value by convention; the lint fires on every (~39469 tok)
- `paths.rs` — Filesystem layout for clog's persistent data. (~1092 tok)
- `persistence.rs` — On-disk JSON state: `settings.json`, `session.json`, `patterns.json`. (~6103 tok)
- `update.rs` — Auto-update wiring: persisted cadence/snooze state and the small Rust (~1995 tok)

## crates/clog-core/

- `Cargo.toml` — Rust package manifest (~105 tok)

## crates/clog-core/examples/

- `fake_tailer.rs` — Append synthetic log4j2-shaped records to a file at a fixed rate. Used (~1096 tok)

## crates/clog-core/src/

- `idx_cache.rs` — Persistent on-disk cache of the `(LineIndex, Vec<RecordHeader>)` produced (~2765 tok)
- `index.rs` — In-memory line offset index. (~782 tok)
- `lib.rs` — Clog engine. No Tauri deps. (~2863 tok)
- `pattern.rs` — log4j2 `PatternLayout` compiler. (~9536 tok)
- `record.rs` — Record header type and scanner. (~2236 tok)
- `regex_scanner.rs` — Regex escape hatch. (~1269 tok)
- `search.rs` — Smart + regex search engine. P6. (~6060 tok)
- `slow_requests.rs` — Slow-request detection, aggregation, and speed-grid builder. (~9320 tok)
- `source.rs` — Line-source abstraction. The v1 impl streams a local file via `BufReader`; (~946 tok)
- `tail.rs` — Polling tail loop + rotation detection. (~4417 tok)
- `thread_groups.rs` — Thread-name classification into a fixed five-group taxonomy + Other. (~2379 tok)

## crates/clog-core/tests/

- `pattern_proptest.rs` — Property test for the `PatternLayout` compiler: generate well-formed (~943 tok)

## design/

- `icon.psd` (~39819 tok)

## docs/

- `build-phases.md` — Clog v1 — Build phases (~4816 tok)
- `design.md` — Clog v1 — Design (~5613 tok)
- `future-ideas.md` — Clog - Future ideas (~1084 tok)

## docs/superpowers/plans/

- `2026-05-23-minimap-heatmap.md` — Minimap heatmap implementation plan (~8735 tok)
- `2026-05-23-slow-request-insights.md` — Slow request insights implementation plan (~31657 tok)
- `2026-05-24-thread-insights.md` — Thread insights + consolidated filter flyout implementation plan (~11654 tok)
- `2026-05-27-zen-mode.md` — Zen mode implementation plan (~4103 tok)
- `2026-05-28-collapse-records.md` — Collapse Records Implementation Plan (~17269 tok)
- `2026-05-30-truncate-logs.md` — Truncate logs (collapse above/below) Implementation Plan (~10571 tok)

## docs/superpowers/specs/

- `2026-05-23-minimap-heatmap-design.md` — Minimap heatmap upgrade - design (~1687 tok)
- `2026-05-23-slow-request-insights-design.md` — Slow request insights - design (~9701 tok)
- `2026-05-24-thread-insights-design.md` — Thread insights + consolidated filter flyout - design (~3404 tok)
- `2026-05-26-auto-update-design.md` — Auto-update - design (~4102 tok)
- `2026-05-27-collapse-records-design.md` — Collapse records - design spec (~5272 tok)
- `2026-05-27-zen-mode-design.md` — Zen mode - design spec (~1752 tok)
- `2026-05-30-truncate-logs-design.md` — Truncate logs (collapse above/below) - design (~2228 tok)

## research/

- `cheesecake-wsl-oink-short.out` (~200 tok)
- `cheesecake-wsl-oink.out` — Declares set (~11695 tok)
- `log4j.prod.properties` (~331 tok)
- `log4j2.wsl-oink.xml` (~729 tok)
- `test - Copy.log` (~202 tok)

## scripts/

- `make-latest-json.ps1` — make-latest-json.ps1 (~1274 tok)
- `make-portable-zip.ps1` — make-portable-zip.ps1 (~808 tok)
- `release.ps1` — release.ps1 (~288 tok)

## tmp/

- `release-notes-1.3.0.md` — Downloads (~454 tok)

## tmp/update-stub/

- `Clog_1.0.1_x64-setup.exe.sig` (~110 tok)
- `latest.json` (~193 tok)

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

- `App.vue` — App orchestrator. Composes the tab list, session save/restore, (~5219 tok)
- `collapse.test.ts` — CollapseSets: sets, rec (~1777 tok)
- `collapse.ts` — Pure collapse-records logic. No Vue or DOM dependencies so every rule is (~1641 tok)
- `main.ts` (~32 tok)
- `perf.ts` — [clog-perf] Temporary performance instrumentation. (~1021 tok)
- `style.css` — Styles: 219 vars (~6197 tok)
- `tab.ts` — Per-tab state container. A Tab owns every reactive ref that was (~7739 tok)
- `types.ts` — Shared TypeScript interfaces used across the UI. Mirrors the wire shapes (~2906 tok)

## ui/src/components/

- `AboutModal.vue` — About modal. Lazily resolves the Tauri app name/version/tauri-version on (~1880 tok)
- `AppHeader.vue` — Title bar: app logo (opens About), Open button, Settings cog, and the (~2080 tok)
- `BaseModal.vue` — Shared modal scaffold: backdrop, frame, header bar with title + close. (~693 tok)
- `ClawdCameo.vue` — Konami-code easter egg: pixel-art Clawd scuttles across the tab strip (~659 tok)
- `ClawdSprite.vue` — Pixel-art Clawd sprite, shared between the About modal (static inline (~590 tok)
- `ColourPickerPopover.vue` — Compact popover that surfaces both foreground and background palette (~1606 tok)
- `ContextMenu.vue` — Custom right-click context menu surface. Renders the items in (~3365 tok)
- `DropOverlay.vue` — Drop-target overlay shown while the user drags files over the window. (~286 tok)
- `FiltersPopover.vue` — Popover hosting the level mask and thread-group mask toggles. Anchored (~1868 tok)
- `HelloWorld.vue` — Vue: setup, TS (~755 tok)
- `HighlightRulesEditor.vue` — Editable table of user highlight rules with a live preview pane. (~4219 tok)
- `InsightsDrawer.vue` — Right-side collapsible drawer hosting the slow-request insights for (~11736 tok)
- `LogViewport.vue` — Per-tab viewport. Owns the virtualised line list, the minimap canvas, (~24696 tok)
- `PatternModal.vue` — Pattern editor modal. Operates directly on the current tab's pattern (~1171 tok)
- `RecordModal.vue` — Full-record viewer modal. Shows the raw text of a single log record so (~1158 tok)
- `SearchBar.vue` — Search + filter + level-mask control bar for a single tab. All state (~2914 tok)
- `SettingsModal.vue` — Settings modal split into four tabs: General (appearance / behaviour / (~7864 tok)
- `StatusBar.vue` — Footer status bar: cache hint, record/line/byte stats for the current (~1382 tok)
- `TabStrip.vue` — Tab strip across the top of the app. Lists open tabs with a tail status (~4336 tok)
- `UpdateBanner.vue` — Non-modal banner that surfaces an available update. Sits at the bottom (~1396 tok)
- `ZenExitPill.vue` — Floating "Exit zen mode" pill. Rendered by App.vue only when zen is (~395 tok)

## ui/src/composables/

- `useAppShortcuts.ts` — Global keyboard shortcuts wired to the document in capture phase. (~928 tok)
- `useContextMenu.ts` — Global custom context-menu state. One menu at a time; module-scoped (~1386 tok)
- `useHighlightRules.ts` — Global + per-file highlight rule loading and engine wiring. (~1118 tok)
- `useKonamiCode.ts` — Konami-code detector: up up down down left right left right b a. (~394 tok)
- `useSession.ts` — Multi-tab session save/restore + the autosave watcher. (~1232 tok)
- `useSettings.ts` — Global settings, theme handling, and font-size scaling. Owns the (~2426 tok)
- `useStartupPaths.ts` — CLI argv + single-instance forward handler. (~450 tok)
- `useTabs.ts` — Tab list ownership: the reactive `tabs` array, the active tab pointer, (~1307 tok)
- `useUpdateBanner.ts` — Update-banner state machine. Talks to the Rust shim (`check_for_update`, (~1462 tok)
- `useWindowChrome.ts` — Window chrome: maximize/restore tracking + the three title-bar buttons. (~487 tok)
- `useZenMode.test.ts` (~416 tok)
- `useZenMode.ts` — Zen mode - hides the app chrome so the log records own the viewport. (~632 tok)

## ui/src/highlight/

- `default-rules.json` (~558 tok)
- `engine.test.ts` — HighlightRulesFile: findCls (~1401 tok)
- `engine.ts` — Reactive version counter. Bumped on every `setRules()` call so any Vue (~3473 tok)
- `record-render.ts` — Render a log line's text into LeafSpans, applying axis-1 header field (~1552 tok)
- `user-rule.test.ts` — UserHighlightRule: makeUserRule (~1304 tok)
- `user-rule.ts` — Compose the effective engine rule set from the three layers: (~460 tok)
