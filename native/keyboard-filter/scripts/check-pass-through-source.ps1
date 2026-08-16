[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$driverPath = Join-Path $root 'keyboard-filter\driver\driver.c'
$protocolPath = Join-Path $root 'keyboard-filter\shared\multiseat_keyboard_filter_protocol.h'
$policyPath = Join-Path $root 'keyboard-filter\service-policy\src\lib.rs'

$driver = Get-Content -LiteralPath $driverPath -Raw
$protocol = Get-Content -LiteralPath $protocolPath -Raw
$policy = Get-Content -LiteralPath $policyPath -Raw
$failures = [Collections.Generic.List[string]]::new()

if ($driver -notmatch '(?s)case IOCTL_MSKL_ENABLE:.*?STATUS_NOT_SUPPORTED;') {
    $failures.Add('EnableSuppression is not visibly rejected with STATUS_NOT_SUPPORTED.')
}
if ($driver -notmatch 'response->suppression_capability = MsklSuppressionDisabledByBuild;') {
    $failures.Add('QueryStatus does not report DisabledByBuild.')
}
if ($driver -notmatch 'response->suppression_active = FALSE;') {
    $failures.Add('QueryStatus does not hard-code suppression_active=false.')
}
$callbackCalls = [regex]::Matches($driver, 'callback\.Typed\(').Count
if ($callbackCalls -ne 1) {
    $failures.Add("Expected exactly one original class callback invocation; found $callbackCalls.")
}
foreach ($forbidden in @('InputDataConsumed\s*=\s*0', 'InputDataStart\s*\+\+', 'InputDataEnd\s*--')) {
    if ($driver -match $forbidden) {
        $failures.Add("Forbidden packet-drop/edit pattern found: $forbidden")
    }
}
$cVersion = [regex]::Match($protocol, 'MSKL_PROTOCOL_VERSION\s+(\d+)u').Groups[1].Value
$rustVersion = [regex]::Match($policy, 'PROTOCOL_VERSION:\s*u16\s*=\s*(\d+)').Groups[1].Value
if (-not $cVersion -or $cVersion -ne $rustVersion) {
    $failures.Add("Protocol versions differ (C='$cVersion', Rust='$rustVersion').")
}

if ($failures.Count -gt 0) {
    $failures | ForEach-Object { Write-Error $_ }
    exit 1
}

Write-Host 'Pass-through source invariants: PASS'
Write-Host 'This is a repository guard, not SDV, CodeQL, HLK, or Microsoft verification.'
