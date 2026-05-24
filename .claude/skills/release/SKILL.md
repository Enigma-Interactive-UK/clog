---
name: release
description: Ship a new Clog release. Bumps the workspace version across Cargo.toml, ui/package.json and crates/clog-app/tauri.conf.json, runs the build, commits, pushes, tags and creates the GitHub release with the NSIS installer and portable zip attached. Use when the user asks to release, ship, cut a release, publish a release, or bump the version.
---

# Release

End-to-end Clog release pipeline. Never skip a step; never guess a version.

## Version sources (all three must stay in sync)

- `Cargo.toml` (workspace) - `version = "X.Y.Z"`
- `ui/package.json` - `"version": "X.Y.Z"`
- `crates/clog-app/tauri.conf.json` - `"version": "X.Y.Z"`

## Steps

1. **Read current version** from `Cargo.toml` and **list commits since the last `v*` tag** (`git describe --tags --abbrev=0` then `git log <tag>..HEAD --oneline`). Skim the messages to judge whether the bump is **patch** (fixes, internal changes), **minor** (new user-visible feature, additive) or **major** (breaking change or paradigm shift). For a 0.x project go one notch more conservative than that would suggest; for >=1.0 follow SemVer strictly.
2. **Propose the new version to the user.** Show: current version, the commit list, your reasoning, and the recommended next version. Ask them to confirm or supply a different number. Do not proceed until they answer.
3. **Bump all three files** to the agreed version (use Edit, not Write). Then run `cargo check --workspace` once to regenerate `Cargo.lock` cleanly.
4. **Build artefacts** via `.\scripts\release.ps1`. Confirm both files exist:
   - `target/release/bundle/nsis/Clog_<version>_x64-setup.exe`
   - `target/release/bundle/portable/clog_<version>_x64-portable.zip`
5. **Commit** the version bump. Stage explicitly: `Cargo.toml`, `Cargo.lock`, `ui/package.json`, `crates/clog-app/tauri.conf.json`, plus any auto-maintained `.wolf/` files that changed. Commit message (one line, plain British English, no trailers, no Conventional Commits prefix):
   ```
   Bumped version to <version>.
   ```
6. **Push** `master` to `origin`.
7. **Tag and push the tag:**
   ```powershell
   git tag -a v<version> -m "Clog <version> - <one-line summary>"
   git push origin v<version>
   ```
8. **Create the GitHub release.** If `gh` isn't on PATH, fall back to `& "C:\Program Files\GitHub CLI\gh.exe"`. Draft release notes from the commit list since the previous tag - group into "Added", "Changed", "Fixed" where useful. Always include the Downloads + Requirements block from the v1.0.0 release as a template (adjust the version numbers). Then:
   ```powershell
   gh release create v<version> --title "Clog <version>" --notes $body `
     "target/release/bundle/nsis/Clog_<version>_x64-setup.exe" `
     "target/release/bundle/portable/clog_<version>_x64-portable.zip"
   ```
9. **Report the release URL** back to the user.

## Hard rules

- Never bump the version without explicit user confirmation of the number.
- Never push or tag before the build has succeeded and both artefacts exist on disk.
- Never use Conventional Commits prefixes (`feat:`, `fix:`, etc.) or `Co-Authored-By` trailers.
- Never amend or force-push a tag that has already been pushed to `origin`. If a mistake is caught after step 7, ask the user before any destructive recovery.
- If any step fails, stop and report to the user. Do not retry blindly.

## Release notes template (step 8)

```markdown
<one or two sentences summarising the headline change>

### Added
- ...

### Changed
- ...

### Fixed
- ...

## Downloads

- `Clog_<version>_x64-setup.exe` - NSIS installer (per-user install, no admin required).
- `clog_<version>_x64-portable.zip` - portable build; extract anywhere, run `clog.exe`. State lives in `clog-data\` next to the exe, so it travels with the folder (including USB sticks).

## Requirements

- Windows 10 or 11, x64.
- WebView2 Runtime (preinstalled on Windows 11; the NSIS installer will pull it in if missing).
```

Drop empty sections.
