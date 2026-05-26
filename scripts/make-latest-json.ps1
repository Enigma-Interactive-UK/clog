# make-latest-json.ps1
#
# Signs the NSIS installer for the current workspace version and writes a
# Tauri-v2-shaped `latest.json` next to it. Used by the `release` skill
# between the build step and the GitHub release publication step.
#
# Requires the private updater signing key. Defaults to
#   $HOME\.tauri\clog-updater.key
# (no password by deliberate choice; see docs/superpowers/specs/
# 2026-05-26-auto-update-design.md). Override with -PrivateKey to point at
# a different file, and pass -Password if the key has one.
#
# Usage (from workspace root):
#   pwsh scripts/make-latest-json.ps1 -Notes "Bug-fix: crash on huge files"
#   pwsh scripts/make-latest-json.ps1 -Notes "..." -PrivateKey C:\path\key
#
# Always run AFTER `scripts/release.ps1` has produced the installer.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Notes,
    [string]$PrivateKey = (Join-Path $HOME ".tauri\clog-updater.key"),
    # Default matches the password used at key generation. Persisted in
    # the script because PowerShell cannot reliably pass an empty string
    # password to the Tauri signer, and the threat model for this
    # project is "filesystem perms on the key file are enough" rather
    # than "defend against a stolen-laptop password attack".
    [string]$Password = "clog-updater"
)

$ErrorActionPreference = "Stop"

$workspaceRoot = Split-Path -Parent $PSScriptRoot
Set-Location $workspaceRoot

# Read version from workspace Cargo.toml (single source of truth).
$cargoToml = Get-Content "Cargo.toml" -Raw
if ($cargoToml -notmatch '(?m)^version\s*=\s*"([^"]+)"') {
    throw "Could not find version in Cargo.toml"
}
$version = $Matches[1]

$nsisDir = Join-Path $workspaceRoot "target\release\bundle\nsis"
$installerName = "Clog_${version}_x64-setup.exe"
$installerPath = Join-Path $nsisDir $installerName
if (-not (Test-Path $installerPath)) {
    throw "Installer not found: $installerPath. Run scripts/release.ps1 first."
}

if (-not (Test-Path $PrivateKey)) {
    throw "Updater private key not found: $PrivateKey. Generate one with `cargo tauri signer generate -w `"$PrivateKey`" --ci -p `"`"`."
}

Write-Host "Signing $installerName..." -ForegroundColor Cyan
# Tauri's signer writes the signature to <installer>.sig next to the file
# AND prints it to stdout. We read it back from the .sig file because the
# stdout form is easier to misparse and the file is canonical.
# Pass the password through the env var the signer documents
# (`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) because PowerShell collapses
# `--password ""` to an empty arg, which the CLI then mis-parses.
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $Password
try {
    $sigOutput = & cargo tauri signer sign --private-key-path "$PrivateKey" "$installerPath" 2>&1
} finally {
    Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
}
if ($LASTEXITCODE -ne 0) {
    throw "Signing failed: $sigOutput"
}
$sigPath = "$installerPath.sig"
if (-not (Test-Path $sigPath)) {
    throw "Signature file missing: $sigPath"
}
$signature = (Get-Content $sigPath -Raw).Trim()
if ([string]::IsNullOrWhiteSpace($signature)) {
    throw "Signature file is empty: $sigPath"
}

$pubDate = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
$downloadUrl = "https://github.com/Enigma-Interactive-UK/clog/releases/download/v${version}/${installerName}"

# Tauri-v2-shaped `latest.json`. Static platform map keyed on
# `<target>-<arch>` (see tauri-plugin-updater RemoteRelease decoder).
$payload = [ordered]@{
    version   = $version
    notes     = $Notes
    pub_date  = $pubDate
    platforms = [ordered]@{
        "windows-x86_64" = [ordered]@{
            signature = $signature
            url       = $downloadUrl
        }
    }
}
$json = ($payload | ConvertTo-Json -Depth 6).Replace("`r`n", "`n")

$outPath = Join-Path $nsisDir "latest.json"
[System.IO.File]::WriteAllText($outPath, $json + "`n", [System.Text.UTF8Encoding]::new($false))

# Read it back and re-parse to confirm the JSON we just wrote is the JSON
# the updater will see; bail early on shape drift before the release goes
# anywhere near a tag.
$verify = Get-Content $outPath -Raw | ConvertFrom-Json
if ($verify.version -ne $version) {
    throw "latest.json verification failed: version mismatch ($($verify.version) vs $version)"
}
if (-not $verify.platforms."windows-x86_64".signature) {
    throw "latest.json verification failed: empty signature field"
}
if ($verify.platforms."windows-x86_64".url -ne $downloadUrl) {
    throw "latest.json verification failed: url mismatch"
}

Write-Host ""
Write-Host "Wrote latest.json:" -ForegroundColor Green
Write-Host "  $outPath"
Write-Host "  version=$version  signature=$($signature.Substring(0, [Math]::Min(40, $signature.Length)))..."
