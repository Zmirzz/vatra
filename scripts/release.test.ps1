# Isolated release orchestration tests. No builds, real credentials, or GitHub requests.
$ErrorActionPreference = 'Stop'
$fixture = Join-Path ([IO.Path]::GetTempPath()) ("vatra-release-test-" + [guid]::NewGuid().ToString('N'))
$names = @('TAURI_SIGNING_PRIVATE_KEY', 'VATRA_GOOGLE_CLIENT_ID', 'VATRA_GOOGLE_CLIENT_SECRET', 'VATRA_MICROSOFT_CLIENT_ID', 'RUSTFLAGS', 'CFLAGS')
$saved = @{}
foreach ($name in $names) {
  $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
  [Environment]::SetEnvironmentVariable($name, 'fixture-value', 'Process')
}
$global:vatraReleaseTestbuildExit = 0
$global:vatraReleaseTestpublishExit = 0
$global:vatraReleaseTestbuildCalls = 0
$global:vatraReleaseTestpublishCalls = 0
$global:vatraReleaseTestsignature = 'fixture-signature'
$global:vatraReleaseTestcargoVersion = '1.2.3'
$global:vatraReleaseTestomitSignature = $false
function cargo {
  $global:LASTEXITCODE = 0
  return (@{ packages = @(@{ name = 'vatra-desktop'; version = $global:vatraReleaseTestcargoVersion }) } | ConvertTo-Json -Depth 4 -Compress)
}
function npm {
  $global:vatraReleaseTestbuildCalls++
  if ($env:TAURI_SIGNING_PRIVATE_KEY -ne 'fixture-value') { throw 'CI signing key was replaced' }
  $dir = Join-Path $fixture 'target\release\bundle\nsis'
  New-Item -ItemType Directory -Path $dir -Force | Out-Null
  Set-Content (Join-Path $dir 'Vatra_1.2.3_x64-setup.exe') 'fixture-installer'
  if (-not $global:vatraReleaseTestomitSignature) { Set-Content (Join-Path $dir 'Vatra_1.2.3_x64-setup.exe.sig') $global:vatraReleaseTestsignature }
  $global:LASTEXITCODE = $global:vatraReleaseTestbuildExit
}
function gh {
  $global:vatraReleaseTestpublishCalls++
  $global:vatraReleaseTestpublishArgs = $args
  $global:LASTEXITCODE = $global:vatraReleaseTestpublishExit
}
function Assert($condition, $message) { if (-not $condition) { throw $message } }
function Expect-Failure($action, $message) {
  try { & $action } catch {
    if ($_.Exception.Message -like "*$message*") { return }
    throw
  }
  throw "Expected failure: $message"
}
try {
  New-Item -ItemType Directory -Path (Join-Path $fixture 'scripts'), (Join-Path $fixture 'apps\desktop') -Force | Out-Null
  Copy-Item (Join-Path $PSScriptRoot 'release.ps1') (Join-Path $fixture 'scripts\release.ps1')
  Set-Content (Join-Path $fixture 'apps\desktop\package.json') '{"version":"1.2.3"}'
  $release = Join-Path $fixture 'scripts\release.ps1'
  $cwd = (Get-Location).Path
  & $release -Draft -ExpectedTag v1.2.3 -Target fixture-commit -Repository example/vatra
  Assert ($global:vatraReleaseTestpublishCalls -eq 1) 'Draft was not created'
  Assert ($global:vatraReleaseTestpublishArgs -contains '--draft') 'Draft flag missing'
  Assert ($global:vatraReleaseTestpublishArgs -contains 'fixture-commit') 'Release commit missing'
  Assert ($global:vatraReleaseTestpublishArgs -notcontains '--clobber') 'Release must not overwrite assets'
  $manifest = Get-Content (Join-Path $fixture 'target\release\bundle\nsis\latest.json') -Raw | ConvertFrom-Json
  Assert ($manifest.version -eq '1.2.3') 'Wrong updater version'
  Assert ($manifest.platforms.'windows-x86_64'.signature -eq 'fixture-signature') 'Wrong updater signature'
  Assert ($manifest.platforms.'windows-x86_64'.url -eq 'https://github.com/example/vatra/releases/download/v1.2.3/Vatra_1.2.3_x64-setup.exe') 'Wrong updater URL'
  Assert ((Get-Location).Path -eq $cwd) 'Working directory was not restored'
  foreach ($name in $names) { Assert ([Environment]::GetEnvironmentVariable($name, 'Process') -eq 'fixture-value') "Environment not restored: $name" }

  Expect-Failure { & $release -ExpectedTag v9.9.9 -Target fixture-commit } 'does not match'
  Assert ($global:vatraReleaseTestbuildCalls -eq 1) 'Tag mismatch should fail before building'
  $global:vatraReleaseTestcargoVersion = '1.2.4'
  Expect-Failure { & $release -Target fixture-commit } 'versions must match'
  $global:vatraReleaseTestcargoVersion = '1.2.3'
  $global:vatraReleaseTestbuildExit = 1
  Expect-Failure { & $release -Target fixture-commit } 'Build failed'
  Assert ($global:vatraReleaseTestpublishCalls -eq 1) 'Failed build was published'
  Assert ($env:RUSTFLAGS -eq 'fixture-value') 'Environment not restored after failed build'
  $global:vatraReleaseTestbuildExit = 0
  $global:vatraReleaseTestsignature = ''
  Expect-Failure { & $release -Target fixture-commit } 'signature is empty'
  Remove-Item -LiteralPath (Join-Path $fixture 'target\release\bundle\nsis\Vatra_1.2.3_x64-setup.exe.sig')
  $global:vatraReleaseTestomitSignature = $true
  Expect-Failure { & $release -Target fixture-commit } 'signature missing'
  Assert ($global:vatraReleaseTestpublishCalls -eq 1) 'Unsigned installer was published'
  $global:vatraReleaseTestomitSignature = $false
  $global:vatraReleaseTestsignature = 'fixture-signature'
  & $release -BuildOnly
  Assert ($global:vatraReleaseTestpublishCalls -eq 1) 'BuildOnly published a release'
  $global:vatraReleaseTestpublishExit = 1
  Expect-Failure { & $release -Draft -Target fixture-commit } 'release creation failed'
  Write-Host 'PASS: draft target/assets/manifest, version guards, failure handling, BuildOnly, and environment restoration'
} finally {
  foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
  $resolved = [IO.Path]::GetFullPath($fixture)
  $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
  if ($resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and (Split-Path $resolved -Leaf) -like 'vatra-release-test-*') {
    Remove-Item -LiteralPath $resolved -Recurse -Force
  }
}
