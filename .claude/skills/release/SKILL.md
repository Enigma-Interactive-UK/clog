---
name: release
description: Ship a new Clog release. Bumps the workspace version across Cargo.toml, ui/package.json and crates/clog-app/tauri.conf.json, runs the build, signs the NSIS installer, writes an updater latest.json, commits, pushes, tags and creates the GitHub release with the installer, portable zip and latest.json attached. Use when the user asks to release, ship, cut a release, publish a release, or bump the version.
---

# Release

End-to-end Clog release pipeline. Never skip a step; never guess a version.

## Version sources (all three must stay in sync)

- `Cargo.toml` (workspace) - `version = "X.Y.Z"`
- `ui/package.json` - `"version": "X.Y.Z"`
- `crates/clog-app/tauri.conf.json` - `"version": "X.Y.Z"`

## Updater signing key (one-time setup, do not commit)

- Private key: `$HOME\.tauri\clog-updater.key`, password `clog-updater`.
- Public key: committed in `crates/clog-app/tauri.conf.json` under `plugins.updater.pubkey`.
- The password is intentionally trivial and persisted in `scripts/make-latest-json.ps1` as the `-Password` default. PowerShell cannot pass an empty-string password to `cargo tauri signer sign` (both `--password ""` and an empty env var are dropped, leaving the signer to prompt interactively and hang). The threat model for this project is "filesystem permissions on the key file are enough" - if an attacker has the key file, the password offers no real defence on top.
- If the private key is missing or invalid, `scripts/make-latest-json.ps1` will refuse to write `latest.json` and the release MUST NOT proceed - an unsigned (or wrongly-signed) `latest.json` is worse than no release, since every installed copy will then reject updates until the key is rotated and they manually reinstall.
- To rotate the key (key compromised, lost, or password change needed):
  ```powershell
  cargo tauri signer generate -w "$HOME\.tauri\clog-updater.key" --ci -p "clog-updater" -f
  ```
  then copy the new `clog-updater.key.pub` contents into the `pubkey` field of `tauri.conf.json`. If you change the password, also bump the default in `scripts/make-latest-json.ps1`. Every previously-installed copy will refuse to auto-update after rotation; they need a fresh manual install.

## Steps

1. **Read current version** from `Cargo.toml` and **list commits since the last `v*` tag** (`git describe --tags --abbrev=0` then `git log <tag>..HEAD --oneline`). Skim the messages to judge whether the bump is **patch** (fixes, internal changes), **minor** (new user-visible feature, additive) or **major** (breaking change or paradigm shift). For a 0.x project go one notch more conservative than that would suggest; for >=1.0 follow SemVer strictly.
2. **Propose the new version to the user.** Show: current version, the commit list, your reasoning, and the recommended next version. Ask them to confirm or supply a different number. Do not proceed until they answer.
3. **Bump all three files** to the agreed version (use Edit, not Write). Then run `cargo check --workspace` once to regenerate `Cargo.lock` cleanly.
4. **Build artefacts** via `.\scripts\release.ps1`. Confirm both files exist:
   - `target/release/bundle/nsis/Clog_<version>_x64-setup.exe`
   - `target/release/bundle/portable/clog_<version>_x64-portable.zip`
4a. **Sign the installer and emit `latest.json`** via:
    ```powershell
    pwsh scripts/make-latest-json.ps1 -Notes "<one-line summary that lands in the updater banner>"
    ```
    The `-Notes` string is what an installed user sees in the in-app update banner ("Clog X.Y.Z is available - <Notes>"). Keep it under ~80 characters and stick to plain British English. Use the same one-liner you'll put in the `git tag` message at step 7. Confirm the script printed a path to `latest.json` and a non-empty signature preview.
4b. **Verify** the three release files now exist on disk:
    - `target/release/bundle/nsis/Clog_<version>_x64-setup.exe`
    - `target/release/bundle/nsis/latest.json`
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
     "target/release/bundle/portable/clog_<version>_x64-portable.zip" `
     "target/release/bundle/nsis/latest.json"
   ```
   `latest.json` MUST be attached so the auto-updater endpoint (`releases/latest/download/latest.json`) resolves to the new release.
9. **Report the release URL** back to the user.

## Vendored NSIS installer template

`crates/clog-app/installer.nsi` is a vendored copy of `tauri-bundler`'s stock NSIS template, patched to skip the "uninstall before installing" maintenance prompt on upgrade (the prompt's uninstall step was failing in practice and is unnecessary - SetOverwrite is ON and `CheckIfAppIsRunning` already handles a running instance). The patch is a ~10-line insert in `PageReinstall`, right after the "no existing install" Abort, gated on `$WixMode <> 1`.

If `tauri-cli` is bumped and the bundled template diverges, the vendored copy will go stale. After any Tauri version bump:

1. Locate the new stock template at `~/.cargo/registry/src/index.crates.io-*/tauri-bundler-<new-version>/src/bundle/windows/nsis/installer.nsi`.
2. Diff it against `crates/clog-app/installer.nsi` to spot upstream changes.
3. Re-vendor: copy the new stock template over the project copy, then re-apply the `$WixMode <> 1` Abort patch in `PageReinstall`.
4. Smoke-test by running the produced installer over an existing install - it should proceed without prompting for uninstall.

## Hard rules

- Never bump the version without explicit user confirmation of the number.
- Never push or tag before the build has succeeded and all three artefacts (installer, portable zip, `latest.json`) exist on disk.
- Never publish a release without a `latest.json` attached - existing installed copies rely on it to discover the new version, and the GitHub `releases/latest/download/latest.json` URL silently 404s if the asset is missing.
- Never use Conventional Commits prefixes (`feat:`, `fix:`, etc.) or `Co-Authored-By` trailers.
- Never amend or force-push a tag that has already been pushed to `origin`. If a mistake is caught after step 7, ask the user before any destructive recovery.
- Never mark a release as `--prerelease` unless you explicitly intend to keep installed copies on the prior version - GitHub's `releases/latest` alias skips prereleases, so a prerelease-only newest tag will go unseen by the updater.
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

- `Clog_<version>_x64-setup.exe` - NSIS installer (per-user install, no admin required). Existing installed copies will auto-update to this release on next launch.
- `clog_<version>_x64-portable.zip` - portable build; extract anywhere, run `clog.exe`. State lives in `clog-data\` next to the exe, so it travels with the folder (including USB sticks). Portable copies see the update banner but do not auto-install - re-download this zip when prompted.
- `latest.json` - auto-updater manifest. You do not need to download this; the app reads it directly.

## Requirements

- Windows 10 or 11, x64.
- WebView2 Runtime (preinstalled on Windows 11; the NSIS installer will pull it in if missing).
```

Drop empty sections.
