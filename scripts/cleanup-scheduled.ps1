param([Parameter(Mandatory=$true)][string]$Node, [Parameter(Mandatory=$true)][string]$Cargo)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$log = Join-Path $root '.cleanup.log'
# One bounded log; each daily run replaces the previous entry.
"Cleanup started: $((Get-Date).ToString('o'))" | Set-Content -LiteralPath $log -Encoding utf8
# Windows PowerShell 5 treats redirected native stderr as ErrorRecords, even on
# success (Cargo writes its normal status there). Preserve the native exit code.
$ErrorActionPreference = 'Continue'
& $Node (Join-Path $PSScriptRoot 'cleanup.mjs') --auto --cargo $Cargo 2>&1 | Out-File -LiteralPath $log -Append -Encoding utf8
exit $LASTEXITCODE
