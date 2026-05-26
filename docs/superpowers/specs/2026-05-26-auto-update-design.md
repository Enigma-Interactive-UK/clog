# Auto-update - design

> Status: P11.A landed (banner + cadence + snooze, dev-key smoke-tested);
> P11.B landed (production keypair, signed installer, latest.json emit,
> release skill extended).
> Source idea: [docs/design.md §16](../../design.md) - the v1 design called for
> Tauri's auto-updater plugin but it was deferred out of P10 when v1 shipped.

## Goal

Stop hand-shipping new versions. When a user is running an installed Clog and
a newer release lands on GitHub, the app notices, fetches the new NSIS
installer, verifies its signature, and offers to install it on next launch
(or on demand).

The release-author flow (the `release` skill) stays local and manual - what
changes is that it now also produces a signed `latest.json` and uploads it
alongside the existing artefacts, and the installed app does the rest.

## Non-goals

- CI-driven builds. The release skill remains a local PowerShell workflow.
- Auto-update for the portable zip. The portable artefact is, by design, a
  drop-in folder with no installer; Tauri's updater can't replace a running
  loose `clog.exe` cleanly on Windows. Portable users instead see a
  non-blocking banner with a "Download v1.2.3" link to the GitHub release
  page. (See "Portable behaviour" below.)
- Delta updates. NSIS-only, full-installer download every time. Installer
  is ~10 MB; not worth the complexity.
- Channel selection (stable/beta). Single channel.
- Background download during use. Updates are fetched only when the user
  consents in the prompt; no silent prefetch.
- Rollback. If a new version is broken, users uninstall and grab the prior
  release from GitHub manually.

## User-visible behaviour

### Installed build (NSIS)

1. **On launch**, once per 24 hours, the app silently checks `latest.json`.
   First check is delayed ~10 s after window-ready so it never competes with
   file opening.
2. If a newer version is available, a **non-modal banner** appears at the
   bottom of the window:
   `Clog 1.2.3 is available. [What's new] [Update now] [Later] [x]`
   - `What's new` opens the GitHub release page in the default browser.
   - `Update now` downloads + verifies + relaunches into the installer.
     Progress shown in the banner (bytes / total). On success the app
     restarts into the new version.
   - `Later` dismisses for this session; reappears next launch if still
     newer.
   - `x` dismisses and snoozes this specific version for 7 days
     (persisted in app state).
3. **Help -> Check for updates** triggers the same check immediately,
   bypassing the 24h cadence and the 7-day snooze. If already up to date,
   shows a brief toast: `You're on the latest version (1.2.3).`
4. On network failure / corrupt signature / any other updater error: log
   to the existing tracing sink, show nothing to the user on the silent
   check, show a toast `Update check failed - see logs.` on the manual
   check.

### Portable build

Detected via the existing portable-mode check (`clog-data\` next to
`clog.exe`). In portable mode:

- The 24h silent check still runs. If a newer version exists, the banner
  reads `Clog 1.2.3 is available. [Download] [Later] [x]`. `Download`
  opens the GitHub release page; there is no in-app install.
- Help -> Check for updates behaves the same, opening the release page on
  hit.

This keeps portable honest: a portable build never mutates its own folder
or spawns an installer.

## Architecture

### Crates / dependencies

- Add `tauri-plugin-updater = "2"` to `crates/clog-app/Cargo.toml`.
- Register the plugin in `clog-app/src/main.rs` alongside the existing
  `dialog`, `opener`, `single-instance` registrations.
- New capability file or addition to `default.json`:
  `updater:default` + `updater:allow-check` + `updater:allow-download-and-install`.
  Restrict the endpoint allowlist to the single GitHub URL we host
  `latest.json` at.

The plugin handles the HTTP fetch, signature verification, download,
unpack, relaunch, and per-OS install command. We provide:

- The signing keypair (public in config, private at release time).
- The `latest.json` endpoint URL.
- A small Rust shim that exposes two commands to the UI:
  - `check_for_update(force: bool) -> Result<UpdateStatus, IpcError>`
    where `UpdateStatus { available: bool, version: Option<String>, notes: Option<String>, mode: "installer" | "portable" }`.
  - `install_update_now() -> Result<(), IpcError>`
    no-op + error in portable mode.
- A Vue composable `useUpdateBanner()` that owns the banner state, talks
  to the two commands, and persists `snoozed_version + snoozed_until`.

### Signing

- One-time generation:
  ```powershell
  cargo tauri signer generate --ci -p "clog-updater" -w "$HOME\.tauri\clog-updater.key"
  ```
  Stores `clog-updater.key` (private) + `clog-updater.key.pub` (public).
  **The trivial `clog-updater` password is a deliberate workaround**:
  the intent was no password ("filesystem perms are enough"), but
  PowerShell silently drops empty-string args/env vars when calling
  `cargo tauri signer sign`, which then prompts interactively and hangs.
  Persisting the password in `scripts/make-latest-json.ps1` preserves
  the original threat model (anyone with the key file can sign anyway)
  while keeping the release pipeline non-interactive.
- The public key in `clog-updater.key.pub` is copied verbatim into
  `crates/clog-app/tauri.conf.json` under `plugins.updater.pubkey`.
- Private key lives **only** on the release author's machine. Never
  committed. Never in a CI secret (no CI exists). Should be backed up to
  the author's password manager or another offline location.
- A separate dev keypair (`clog-updater-dev.key`, password `clog-dev`)
  was generated during P11.A for local stub-server smoke testing. It is
  **not** the production key; the dev pubkey was overwritten in
  `tauri.conf.json` when P11.B landed the production pubkey.
- Loss of the private key requires generating a new keypair, swapping
  the pubkey in `tauri.conf.json`, and accepting that every currently
  installed copy will refuse to auto-update (signature mismatch) until
  the user reinstalls from the NSIS installer manually. The release
  skill's "Updater signing key" section documents the recovery path.

### `latest.json` hosting

Use GitHub Releases' "latest" alias. The updater endpoint config:

```
https://github.com/Enigma-Interactive-UK/clog/releases/latest/download/latest.json
```

GitHub redirects `releases/latest/download/<asset>` to the newest non-prerelease
release's asset of that name. So the release skill just needs to attach a
fresh `latest.json` to every release; the URL never changes.

`latest.json` shape (Tauri v2 native):

```json
{
  "version": "1.2.3",
  "notes": "Headline change in one sentence.",
  "pub_date": "2026-05-26T12:00:00Z",
  "platforms": {
    "windows-x86_64": {
      "signature": "<base64 signature of the installer .exe>",
      "url": "https://github.com/Enigma-Interactive-UK/clog/releases/download/v1.2.3/Clog_1.2.3_x64-setup.exe"
    }
  }
}
```

Note: this points the updater at the **NSIS installer**, not the portable
zip. Tauri's Windows updater drives the installer in silent mode (`/S`)
to update the installed copy in place.

### Per-user install path requirement

The existing NSIS install is already per-user (`%LOCALAPPDATA%\Programs\clog\`,
no UAC). This is mandatory for the updater - a per-machine install would
prompt for elevation on every update and ruin the UX. Verify before P11.A
lands that the NSIS template is still per-user.

## Release-flow changes (landed in P11.B)

The `release` skill gained two new steps between step 4 (build) and step
5 (commit), and one extra asset in step 8.

- **4a. Sign the installer and emit `latest.json`.**
  ```powershell
  pwsh scripts/make-latest-json.ps1 -Notes "<one-line summary>"
  ```
  `scripts/make-latest-json.ps1`:
  - Reads the workspace version from `Cargo.toml` (single source of truth).
  - Defaults the private key to `$HOME\.tauri\clog-updater.key` (no
    password). Override with `-PrivateKey` and `-Password` if rotating.
  - Calls `cargo tauri signer sign --private-key-path ... --password ...`
    to produce `<installer>.sig` next to the NSIS installer.
  - Reads the canonical signature back from the `.sig` file (the stdout
    form is easier to misparse).
  - Writes `latest.json` next to the installer with:
    - `version`: workspace version.
    - `notes`: the `-Notes` parameter verbatim (becomes the banner
      headline in installed copies).
    - `pub_date`: UTC now in RFC 3339.
    - `platforms.windows-x86_64.signature`: the signature.
    - `platforms.windows-x86_64.url`: the canonical GitHub release URL
      for the installer asset.
  - Reads `latest.json` back and asserts (a) version matches, (b)
    signature non-empty, (c) URL matches. Fails the script early on
    drift so a malformed manifest never reaches the tag.

- **4b. Verify** the three release artefacts exist on disk:
  - `target/release/bundle/nsis/Clog_<version>_x64-setup.exe`
  - `target/release/bundle/nsis/latest.json`
  - `target/release/bundle/portable/clog_<version>_x64-portable.zip`

Step 8 (`gh release create`) attaches all three:

```powershell
gh release create v<version> --title "Clog <version>" --notes $body `
  "target/release/bundle/nsis/Clog_<version>_x64-setup.exe" `
  "target/release/bundle/portable/clog_<version>_x64-portable.zip" `
  "target/release/bundle/nsis/latest.json"
```

New hard rules added to the skill:
- **Never publish a release without `latest.json`.** Existing installed
  copies will silently 404 on the auto-updater endpoint otherwise.
- **Never mark the newest release `--prerelease`.** GitHub's
  `releases/latest` alias skips prereleases, so the updater would never
  see it.

## State / persistence

Two new fields in the persisted app state (whatever P7 currently uses):

```rust
struct UpdateState {
    last_check_utc: Option<DateTime<Utc>>,
    snoozed_version: Option<String>,
    snoozed_until_utc: Option<DateTime<Utc>>,
}
```

Stored as a single JSON blob `update.json` in the data dir
(`%LOCALAPPDATA%\clog\` or `clog-data\` for portable). Not added to the
existing session state file so it survives "clear session" operations.

## Failure modes and how we handle them

| Failure                             | User-visible behaviour                                  |
|-------------------------------------|---------------------------------------------------------|
| No network on silent check          | Silent. Retry on next launch.                           |
| No network on manual check          | Toast `Update check failed - see logs.`                 |
| `latest.json` 404                   | Same as no network.                                     |
| Signature verification fails        | Banner / toast `Update download failed verification.` Log full error. **Do not install.** |
| Installer download interrupted      | Banner shows `Retry` button. No partial install.        |
| User declines installer UAC prompt  | Updater reports failure to the app; we show a toast and leave the running version intact. |
| Updater plugin panic                | Caught by the existing panic hook; app keeps running, update banner removed for the session. |
| Private key lost                    | Author-side. Document: cut a new keypair, ship a transitional release manually, accept that current installed copies need a manual reinstall. |

## Cross-references

- design.md §16 (packaging and distribution, original updater intent).
- design.md §14 (portable-mode detection - feeds the install-vs-portable
  branch in the updater).
- `.claude/skills/release/SKILL.md` (extended in P11.B).
- `crates/clog-app/tauri.conf.json` (gains the `plugins.updater` block).

## Build phases

Sliced into two phases, both small. Each is independently demoable.

### P11.A - Updater plumbed, no signing yet

**Scope**

- Generate the keypair (one-time, author-local). Commit public key into
  `tauri.conf.json`'s `plugins.updater.pubkey`. Document the private-key
  storage location in the release skill (not the value itself).
- Add `tauri-plugin-updater` to `clog-app/Cargo.toml`. Register it.
- Add the capability entries to `capabilities/default.json`.
- Implement the Rust shim commands `check_for_update` and
  `install_update_now` (the latter is a thin wrapper over the plugin's
  `download_and_install`).
- Implement `update.json` persistence and the 24h cadence guard.
- Implement the Vue banner component + composable. Wire portable
  detection so portable hits the "go to release page" path.
- Add Help -> Check for updates menu item.
- Hand-author a `latest.json` and host it temporarily on a gist (or
  point the endpoint at a localhost file) to verify the **detection +
  banner + portable branch** end-to-end without a real signed payload.

**Done-criteria**

- `cargo build --workspace`, `cargo clippy -D warnings`, `cargo fmt --check`,
  `cargo test --workspace`, `npm -C ui run build`, `npm -C ui run test`
  all pass.
- With a stubbed `latest.json` pointing at a fake newer version, the
  banner appears on launch. `Later` dismisses. `x` snoozes for 7 days
  (verified by editing `update.json` and relaunching).
- In portable mode, the banner shows `Download` + opens the release page
  in the browser; in installed mode it shows `Update now`.
- Help -> Check for updates triggers an immediate check and surfaces the
  up-to-date toast when applicable.

**Tests landed**

- Unit test for the 24h cadence guard (`should_check(last_check, now)`).
- Unit test for the 7-day snooze logic.
- Vitest test for the banner component's three button states.

**Demo**

Run the dev build with the endpoint pointed at a local `latest.json`
declaring `version: 99.0.0`. Launch the app. Banner appears within
10 s. Click `Later`, banner goes away. Relaunch. Banner returns.
Click `x`. Edit `update.json` to backdate `snoozed_until`. Relaunch.
Banner returns.

### P11.B - Signed releases, real end-to-end update

**Scope**

- Extend the `release` skill with steps 4a + 4b (sign installer, emit
  `latest.json`) and the third asset in step 8.
- Add a template `scripts/latest.json.template` consumed by the skill.
- Add `install_update_now` to the production path - actually drive the
  Tauri updater install flow. Verify it relaunches into the new
  version.
- Update the release skill's hard rules to include "never publish
  without `latest.json`".

**Done-criteria**

- Cutting a release via the skill produces three GitHub assets:
  installer, portable zip, `latest.json`. `latest.json` has a non-empty
  `signature` matching the installer.
- A previously-installed copy (one version behind) launches, shows the
  banner within 10 s, accepts `Update now`, downloads, verifies,
  relaunches into the new version. Settings + open tabs survive the
  restart.
- A portable copy one version behind shows the `Download` banner and
  opens the release page on click.
- Tampering with the installer (flip a byte) causes the updater to
  refuse the install and surface the verification-failed toast.

**Tests landed**

- Manual end-to-end: documented in the release skill as a post-publish
  smoke check on the author's machine using a held-back prior install.
- No automated test for the signing path (would require committing or
  CI-storing the private key, which we've explicitly chosen not to do).

**Demo**

From an installed `vN-1`, run the app. Cut release `vN` via the skill.
Within a minute, the running `vN-1` shows the banner. Click `Update now`.
App relaunches as `vN` with the same tabs open.

## Open questions

- **Endpoint stability.** Confirm that
  `releases/latest/download/latest.json` correctly resolves the newest
  non-draft, non-prerelease release. If we ever publish a prerelease,
  it must be marked as such in `gh release create` (the skill already
  doesn't pass `--prerelease`, so default behaviour is fine).
- **Notes field source.** Should `latest.json.notes` be the first
  paragraph of the GitHub release body, or a separate one-line summary?
  Recommend: one-line summary, derived from the same one-line tag
  message the skill already writes (`git tag -a v<version> -m "Clog
  <version> - <one-line summary>"`).
- **Banner placement.** Bottom-of-window banner needs to coexist with the
  existing status bar. Confirm there's vertical room or add a slim
  dedicated row above the status bar.
