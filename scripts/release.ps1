# Builds, signs and publishes a Vatra release to GitHub (updates are served from GitHub Releases).
# Usage: powershell -ExecutionPolicy Bypass -File scripts/release.ps1 [-BuildOnly] [-Draft]
param(
  [switch]$BuildOnly,
  [switch]$Draft,
  [string]$ExpectedTag,
  [string]$Target,
  [string]$Repository = 'Zmirzz/vatra'
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$version = (Get-Content (Join-Path $root 'apps\desktop\package.json') -Raw | ConvertFrom-Json).version
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'Release version must be a stable major.minor.patch version' }
$tag = "v$version"
if ($ExpectedTag -and $ExpectedTag -ne $tag) { throw "Tag $ExpectedTag does not match application version $tag" }
$metadata = cargo metadata --no-deps --format-version 1 --manifest-path (Join-Path $root 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'Could not read Cargo package versions' }
$desktop = ($metadata | ConvertFrom-Json).packages | Where-Object { $_.name -eq 'vatra-desktop' }
if (-not $desktop -or $desktop.version -ne $version) { throw 'Cargo and desktop package versions must match' }
if (-not $BuildOnly -and -not $Target) {
  $Target = git -C $root rev-parse HEAD
  if ($LASTEXITCODE -ne 0) { throw 'Could not determine release commit' }
}
# Environment credentials take precedence in CI; local builds can still use the private files.
$names = @('RUSTFLAGS', 'CFLAGS', 'TAURI_SIGNING_PRIVATE_KEY', 'VATRA_GOOGLE_CLIENT_ID', 'VATRA_GOOGLE_CLIENT_SECRET', 'VATRA_MICROSOFT_CLIENT_ID')
$saved = @{}
foreach ($name in $names) { $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
Push-Location (Join-Path $root 'apps\desktop')
try {
  if (-not $env:TAURI_SIGNING_PRIVATE_KEY) {
    $keyFile = Join-Path $env:USERPROFILE '.vatra-release\updater.key'
    if (-not (Test-Path $keyFile)) { throw 'Set TAURI_SIGNING_PRIVATE_KEY or provide the local updater.key file' }
    $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content -LiteralPath $keyFile -Raw
  }
  # Keep the folder of this computer (and the user name in it) out of the compiled programs.
  $env:RUSTFLAGS = "--remap-path-prefix=$env:USERPROFILE=~ --remap-path-prefix=$root=src"
  $env:CFLAGS = "/d1trimfile:$env:USERPROFILE\"
  # Local fallback for credentials that are compiled into the application.
  $oauthFile = Join-Path $env:USERPROFILE '.vatra-release\oauth.env'
  if ((-not $env:VATRA_GOOGLE_CLIENT_ID -or -not $env:VATRA_GOOGLE_CLIENT_SECRET -or -not $env:VATRA_MICROSOFT_CLIENT_ID) -and (Test-Path $oauthFile)) {
    foreach ($line in Get-Content -LiteralPath $oauthFile) {
      if ($line -match '^\s*(VATRA_(?:GOOGLE|MICROSOFT)_CLIENT_(?:ID|SECRET))\s*=\s*(.+?)\s*$') {
        if (-not [Environment]::GetEnvironmentVariable($Matches[1], 'Process')) { Set-Item -Path "Env:$($Matches[1])" -Value $Matches[2] }
      }
    }
  }
  npm run build:desktop
  if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
} finally {
  Pop-Location
  foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
}
$dir = Join-Path $root 'target\release\bundle\nsis'
$exe = Join-Path $dir "Vatra_${version}_x64-setup.exe"
$sig = "$exe.sig"
if (-not (Test-Path $exe) -or -not (Test-Path $sig)) { throw 'Installer or signature missing' }
if ([string]::IsNullOrWhiteSpace((Get-Content $sig -Raw))) { throw 'Installer signature is empty' }
if ($BuildOnly) { Write-Host "Built and signed: $exe"; return }
$manifest = [ordered]@{
  version = $version
  notes = "Vatra $version"
  pub_date = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
  platforms = @{ 'windows-x86_64' = @{ signature = (Get-Content $sig -Raw).Trim(); url = "https://github.com/$Repository/releases/download/$tag/Vatra_${version}_x64-setup.exe" } }
}
$latest = Join-Path $dir 'latest.json'
$manifest | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8 $latest
# Never replace assets of an existing release. A rebuild needs a new version or deletion of its draft.
$releaseArgs = @('release', 'create', $tag, $exe, $sig, $latest, '--repo', $Repository, '--target', $Target, '--title', "Vatra $version", '--generate-notes')
if ($Draft) { $releaseArgs += '--draft' }
gh @releaseArgs
if ($LASTEXITCODE -ne 0) { throw 'GitHub release creation failed; an existing release will not be overwritten' }
