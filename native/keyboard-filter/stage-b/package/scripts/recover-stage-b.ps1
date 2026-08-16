[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^oem\d+\.inf$')]
    [string]$PublishedInf,

    [Parameter(Mandatory = $true)]
    [string]$TargetInstanceId,

    [switch]$Execute
)

$ErrorActionPreference = 'Stop'
$serviceName = 'MultiSeatKeyboardFilter'
$packages = & pnputil.exe /enum-drivers /class Keyboard /files 2>&1 | Out-String
if ($packages -notmatch [regex]::Escape($PublishedInf) -or $packages -notmatch 'MultiSeat Lite') {
    throw 'The selected published INF was not unambiguously identified as a MultiSeat Lite keyboard package.'
}

Write-Host 'STANDALONE RECOVERY PREVIEW'
Write-Host "  Exact published package: $PublishedInf"
Write-Host "  Exact target instance: $TargetInstanceId"
Write-Host 'Current target stack:'
& pnputil.exe /enum-devices /instanceid $TargetInstanceId /drivers
Write-Host 'Current target UpperFilters:'
& reg.exe query "HKLM\SYSTEM\CurrentControlSet\Enum\$TargetInstanceId" /v UpperFilters
Write-Host 'Current service state:'
& sc.exe query $serviceName

if (-not $Execute) {
    Write-Host 'DRY RUN ONLY. No driver or PnP state was changed.'
    exit 0
}
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Recovery execution requires an elevated PowerShell.'
}

& pnputil.exe /delete-driver $PublishedInf /uninstall
if ($LASTEXITCODE -ne 0) { throw "Exact package removal failed with exit code $LASTEXITCODE" }
& pnputil.exe /scan-devices
if ($LASTEXITCODE -ne 0) { throw "PnP rescan failed with exit code $LASTEXITCODE" }

$remaining = & pnputil.exe /enum-drivers /class Keyboard /files 2>&1 | Out-String
if ($remaining -match [regex]::Escape($PublishedInf)) {
    throw 'Verification failed: the selected published package is still present.'
}
Write-Host 'Resulting target stack:'
& pnputil.exe /enum-devices /instanceid $TargetInstanceId /drivers
Write-Host 'Resulting target UpperFilters:'
& reg.exe query "HKLM\SYSTEM\CurrentControlSet\Enum\$TargetInstanceId" /v UpperFilters
Write-Host 'MultiSeat filter package is absent. Manually verify BOTH keyboards now.'

