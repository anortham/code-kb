#
# restore-julie-extract.ps1 — restore the pinned julie-extract binary into .tools\
#
[CmdletBinding()]
param(
    [string]$FromSource = ""
)

$ErrorActionPreference = "Stop"
# Invoke-WebRequest's progress bar writes to the console buffer, which throws "Access is denied"
# under a non-interactive host (SSH, CI service) and leaves a 0-byte archive behind.
$ProgressPreference = "SilentlyContinue"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Split-Path -Parent $ScriptDir
$PinsPath = Join-Path $ScriptDir "julie-pins.json"
$ToolsDir = Join-Path $RepoRoot ".tools"

if (-not (Test-Path $PinsPath)) {
    Write-Error "pins file not found at $PinsPath"
}

$Pins = Get-Content $PinsPath -Raw | ConvertFrom-Json
$Version = $Pins.version
$Triple = "x86_64-pc-windows-msvc"

if ($FromSource -or $env:JULIE_EXTRACTORS_SOURCE) {
    $SourceRoot = if ($FromSource) { $FromSource } else { $env:JULIE_EXTRACTORS_SOURCE }
    $Manifest = Join-Path $SourceRoot "Cargo.toml"
    if (-not (Test-Path $Manifest)) {
        Write-Error "Cargo.toml not found at $SourceRoot"
    }
    if (-not (Test-Path $ToolsDir)) {
        New-Item -ItemType Directory -Path $ToolsDir -Force | Out-Null
    }
    Write-Host "Building julie-extract v$Version from source: $SourceRoot"
    cargo build --manifest-path $Manifest --release -p julie-extract-cli --bin julie-extract
    $Built = Join-Path $SourceRoot "target\release\julie-extract.exe"
    $Target = Join-Path $ToolsDir "julie-extract.exe"
    Copy-Item -Path $Built -Destination $Target -Force
    Write-Host "Installed: $Target"
    & $Target --version
    exit 0
}

$AssetInfo = $Pins.assets.$Triple
if (-not $AssetInfo) {
    Write-Error "No asset configuration for $Triple in $PinsPath"
}

$AssetName = $AssetInfo.name.Replace("{VER}", $Version)
$ExpectedSha256 = $AssetInfo.sha256
$Url = $Pins.urlTemplate.Replace("{VER}", $Version).Replace("{asset}", $AssetName)

if (-not (Test-Path $ToolsDir)) {
    New-Item -ItemType Directory -Path $ToolsDir -Force | Out-Null
}

$ArchivePath = Join-Path $ToolsDir $AssetName
$BinaryPath = Join-Path $ToolsDir "julie-extract.exe"

Write-Host "Restoring julie-extract v$Version for $Triple..."
Write-Host "  URL:    $Url"
Write-Host "  SHA256: $ExpectedSha256"

Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $ArchivePath

$ActualSha256 = (Get-FileHash -Path $ArchivePath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($ActualSha256 -ne $ExpectedSha256.ToLowerInvariant()) {
    Remove-Item -Path $ArchivePath -Force -ErrorAction SilentlyContinue
    Write-Error "SHA256 mismatch for $ArchivePath. Expected: $ExpectedSha256, Actual: $ActualSha256"
}
Write-Host "  Checksum verified."

$Staging = Join-Path $ToolsDir "staging_$(Get-Random)"
Expand-Archive -Path $ArchivePath -DestinationPath $Staging -Force

$ExtractedBinary = Get-ChildItem -Path $Staging -Recurse -Filter "julie-extract.exe" | Select-Object -First 1
if (-not $ExtractedBinary) {
    Write-Error "julie-extract.exe not found in archive."
}

Move-Item -Path $ExtractedBinary.FullName -Destination $BinaryPath -Force
Remove-Item -Path $ArchivePath -Force -ErrorAction SilentlyContinue
Remove-Item -Path $Staging -Recurse -Force -ErrorAction SilentlyContinue

Write-Host "Successfully installed: $BinaryPath"
& $BinaryPath --version
