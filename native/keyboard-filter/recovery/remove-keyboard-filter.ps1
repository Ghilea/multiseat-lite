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

Write-Host 'MultiSeat Lite keyboard-filter recovery'
Write-Host "Published package: $PublishedInf"
Write-Host "Target instance: $TargetInstanceId"
Write-Host 'Current service state:'
& sc.exe query $serviceName
Write-Host 'Current target stack:'
& pnputil.exe /enum-devices /instanceid $TargetInstanceId /drivers

if (-not $Execute) {
    Write-Host ''
    Write-Host 'DRY RUN ONLY. No device or driver changes were made.'
    Write-Host 'After checking the exact oemNN.inf and instance ID, rerun from an elevated'
    Write-Host 'PowerShell with -Execute. Keep the primary keyboard and remote recovery active.'
    exit 0
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Recovery execution requires an elevated PowerShell.'
}

Write-Host 'Removing only the explicitly named published driver package/binding...'
& pnputil.exe /delete-driver $PublishedInf /uninstall
if ($LASTEXITCODE -ne 0) {
    throw "pnputil removal failed with exit code $LASTEXITCODE"
}

Write-Host 'Rescanning Plug and Play devices...'
& pnputil.exe /scan-devices
if ($LASTEXITCODE -ne 0) {
    throw "PnP rescan failed with exit code $LASTEXITCODE"
}

Write-Host 'Verifying the target stack after removal:'
& pnputil.exe /enum-devices /instanceid $TargetInstanceId /drivers
Write-Host 'Verifying filter service/package state:'
& sc.exe query $serviceName
& pnputil.exe /enum-drivers /class Keyboard
Write-Host 'Manually type with BOTH keyboards now. Reboot if Windows requested it.'

