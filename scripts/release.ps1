# Builds, signs and publishes a Vatra release to GitHub (updates are served from GitHub Releases).
# Usage: powershell -ExecutionPolicy Bypass -File scripts/release.ps1 [-BuildOnly]   (-BuildOnly builds and signs but does not publish)
param([switch]$BuildOnly)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$keyFile = Join-Path $env:USERPROFILE '.vatra-release\updater.key'
if (-not (Test-Path $keyFile)) { throw "Signing key not found at $keyFile" }
$version = (Get-Content (Join-Path $root 'apps\desktop\package.json') -Raw | ConvertFrom-Json).version
$tag = "v$version"
# Keep the folder of this computer (and the user name in it) out of the compiled programs.
$env:RUSTFLAGS = "--remap-path-prefix=$env:USERPROFILE=~ --remap-path-prefix=$root=src"
$env:CFLAGS = "/d1trimfile:$env:USERPROFILE\"
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content -LiteralPath $keyFile -Raw
# Sign-in IDs for Google and Microsoft are compiled in, so people never see a developer console. They live in a private
# file outside the repository, one NAME=value per line: VATRA_GOOGLE_CLIENT_ID, VATRA_GOOGLE_CLIENT_SECRET, VATRA_MICROSOFT_CLIENT_ID.
$oauthFile = Join-Path $env:USERPROFILE '.vatra-release\oauth.env'
$oauthNames = @()
if (Test-Path $oauthFile) {
  foreach ($line in Get-Content -LiteralPath $oauthFile) {
    if ($line -match '^\s*(VATRA_(?:GOOGLE|MICROSOFT)_CLIENT_(?:ID|SECRET))\s*=\s*(.+?)\s*$') { Set-Item -Path "Env:$($Matches[1])" -Value $Matches[2]; $oauthNames += $Matches[1] }
  }
} else { Write-Host "No ${oauthFile}: this build asks people for their own sign-in ID." }
try {
  Push-Location (Join-Path $root 'apps\desktop')
  npm run build:desktop
  if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
} finally {
  Pop-Location
  Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue
  Remove-Item Env:RUSTFLAGS,Env:CFLAGS -ErrorAction SilentlyContinue
  foreach ($name in $oauthNames) { Remove-Item "Env:$name" -ErrorAction SilentlyContinue }
}
$dir = Join-Path $root 'target\release\bundle\nsis'
$exe = Join-Path $dir "Vatra_${version}_x64-setup.exe"
$sig = "$exe.sig"
if (-not (Test-Path $exe) -or -not (Test-Path $sig)) { throw 'Installer or signature missing' }
if ($BuildOnly) { Write-Host "Built and signed: $exe"; return }
$manifest = [ordered]@{
  version = $version
  notes = "Vatra $version"
  pub_date = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
  platforms = @{ 'windows-x86_64' = @{ signature = (Get-Content $sig -Raw).Trim(); url = "https://github.com/Zmirzz/vatra/releases/download/$tag/Vatra_${version}_x64-setup.exe" } }
}
$latest = Join-Path $dir 'latest.json'
$manifest | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8 $latest
$ErrorActionPreference = 'Continue'
gh release view $tag --repo Zmirzz/vatra *> $null
$exists = ($LASTEXITCODE -eq 0)
$ErrorActionPreference = 'Stop'
if ($exists) {
  # The release exists (a rebuild of the same version): replace its files.
  gh release upload $tag $exe $sig $latest --repo Zmirzz/vatra --clobber
} else {
  gh release create $tag $exe $sig $latest --repo Zmirzz/vatra --title "Vatra $version" --generate-notes
}
