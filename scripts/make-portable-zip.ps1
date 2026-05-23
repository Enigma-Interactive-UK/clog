# make-portable-zip.ps1
#
# Builds Clog in release mode and produces a portable zip artefact at
# target/release/bundle/portable/clog_<version>_x64-portable.zip
#
# Portable mode is detected at runtime by paths::data_dir() -- if a
# `clog-data\` directory sits next to clog.exe, that path takes precedence
# over %LOCALAPPDATA%\clog. The zip therefore ships:
#   clog.exe
#   clog-data\         (empty placeholder)
#   README.txt
#
# Usage (from workspace root):
#   pwsh scripts/make-portable-zip.ps1
#   pwsh scripts/make-portable-zip.ps1 -SkipBuild   # reuse existing build

[CmdletBinding()]
param(
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$workspaceRoot = Split-Path -Parent $PSScriptRoot
Set-Location $workspaceRoot

# Read version from workspace Cargo.toml
$cargoToml = Get-Content "Cargo.toml" -Raw
if ($cargoToml -notmatch '(?m)^version\s*=\s*"([^"]+)"') {
    throw "Could not find version in Cargo.toml"
}
$version = $Matches[1]
Write-Host "Building portable zip for clog v$version" -ForegroundColor Cyan

if (-not $SkipBuild) {
    Write-Host "Running cargo tauri build (NSIS + exe)..." -ForegroundColor Cyan
    & cargo tauri build --config crates/clog-app/tauri.conf.json
    if ($LASTEXITCODE -ne 0) { throw "cargo tauri build failed" }
}

$exePath = Join-Path $workspaceRoot "target\release\clog.exe"
if (-not (Test-Path $exePath)) {
    throw "Built exe not found at $exePath. Run without -SkipBuild first."
}

$outDir = Join-Path $workspaceRoot "target\release\bundle\portable"
$stageDir = Join-Path $outDir "clog_${version}_x64-portable"

if (Test-Path $stageDir) { Remove-Item -Recurse -Force $stageDir }
New-Item -ItemType Directory -Force -Path $stageDir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $stageDir "clog-data") | Out-Null

Copy-Item $exePath (Join-Path $stageDir "clog.exe")

$readme = @"
Clog $version - portable distribution
======================================

This is a portable build of Clog. All settings, session state, the index
cache, and log output are stored in the clog-data\ folder beside clog.exe
rather than in %LOCALAPPDATA%\clog. You can move this folder anywhere
(including a USB stick) and Clog will follow.

Quick start:
  1. Extract this archive somewhere writable.
  2. Run clog.exe.
  3. Open File... and pick a log file. That's it.

To "uninstall", just delete the folder. Nothing is written outside it.

Project home: https://github.com/lewster32/clog
"@

$readmePath = Join-Path $stageDir "README.txt"
Set-Content -Path $readmePath -Value $readme -Encoding utf8

$zipPath = Join-Path $outDir "clog_${version}_x64-portable.zip"
if (Test-Path $zipPath) { Remove-Item -Force $zipPath }

Write-Host "Compressing -> $zipPath" -ForegroundColor Cyan
Compress-Archive -Path (Join-Path $stageDir "*") -DestinationPath $zipPath -CompressionLevel Optimal

$zipInfo = Get-Item $zipPath
$kb = [math]::Round($zipInfo.Length / 1KB, 1)
Write-Host "Done: $($zipInfo.Name) ($kb KB)" -ForegroundColor Green
Write-Host "Output: $zipPath"
