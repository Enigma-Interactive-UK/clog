# release.ps1
#
# One-shot release build: produces the NSIS installer and the portable zip.
#
# Usage (from workspace root):
#   pwsh scripts/release.ps1

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$workspaceRoot = Split-Path -Parent $PSScriptRoot
Set-Location $workspaceRoot

Write-Host "Running cargo tauri build (NSIS installer + exe)..." -ForegroundColor Cyan
& cargo tauri build --config crates/clog-app/tauri.conf.json
if ($LASTEXITCODE -ne 0) { throw "cargo tauri build failed" }

Write-Host "Packaging portable zip..." -ForegroundColor Cyan
& (Join-Path $PSScriptRoot "make-portable-zip.ps1") -SkipBuild

$nsisDir = Join-Path $workspaceRoot "target\release\bundle\nsis"
$portableDir = Join-Path $workspaceRoot "target\release\bundle\portable"

Write-Host ""
Write-Host "Release artefacts:" -ForegroundColor Green
if (Test-Path $nsisDir) { Get-ChildItem $nsisDir -Filter *.exe | ForEach-Object { Write-Host "  $($_.FullName)" } }
if (Test-Path $portableDir) { Get-ChildItem $portableDir -Filter *.zip | ForEach-Object { Write-Host "  $($_.FullName)" } }
