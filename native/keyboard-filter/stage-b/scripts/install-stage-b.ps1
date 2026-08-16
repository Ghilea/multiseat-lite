[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string]$ContainerId,
    [Parameter(Mandatory = $true)] [string]$StablePhysicalId,
    [Parameter(Mandatory = $true)] [string]$TlcInstanceId,
    [Parameter(Mandatory = $true)] [string]$HardwareId,
    [Parameter(Mandatory = $true)] [string]$InfPath,
    [Parameter(Mandatory = $true)] [string]$ConfigPath,
    [switch]$RemoteRecoveryConfirmed,
    [switch]$Execute
)

$ErrorActionPreference = 'Stop'
$tool = Join-Path $PSScriptRoot '..\bin\multiseat-keyboard-filter-tools.exe'
$template = Join-Path $PSScriptRoot '..\templates\MultiSeatKeyboardFilter.inf.template'
$sys = Join-Path (Split-Path -Parent $InfPath) 'MultiSeatKeyboardFilter.sys'
$cat = Join-Path (Split-Path -Parent $InfPath) 'MultiSeatKeyboardFilter.cat'

foreach ($required in @($tool, $template)) {
    if (-not (Test-Path -LiteralPath $required)) { throw "Required package file is missing: $required" }
}

$readinessArguments = @('readiness', '--config', $ConfigPath)
if ($RemoteRecoveryConfirmed) { $readinessArguments += '--remote-recovery-confirmed' }
& $tool @readinessArguments
if ($LASTEXITCODE -ne 0) { throw 'Stage B readiness is NotReady. Installation is prohibited.' }

$generatedNow = $false
if (-not (Test-Path -LiteralPath $InfPath)) {
    & $tool generate-inf --config $ConfigPath --target $StablePhysicalId --container $ContainerId --tlc $TlcInstanceId --hardware-id $HardwareId --output $InfPath --template $template
    if ($LASTEXITCODE -ne 0) { throw 'Exact target INF generation failed.' }
    $generatedNow = $true
} else {
    $comparison = Join-Path (Split-Path -Parent $InfPath) ('.identity-check-' + [Guid]::NewGuid().ToString('N') + '.inf')
    try {
        & $tool generate-inf --config $ConfigPath --target $StablePhysicalId --container $ContainerId --tlc $TlcInstanceId --hardware-id $HardwareId --output $comparison --template $template
        if ($LASTEXITCODE -ne 0) { throw 'Exact target identity validation failed.' }
        if ((Get-FileHash -LiteralPath $comparison -Algorithm SHA256).Hash -ne (Get-FileHash -LiteralPath $InfPath -Algorithm SHA256).Hash) {
            throw 'Existing INF does not match the currently resolved exact physical target.'
        }
    } finally {
        if (Test-Path -LiteralPath $comparison) { Remove-Item -LiteralPath $comparison -Force }
    }
}

$infText = Get-Content -LiteralPath $InfPath -Raw
if ($infText -notmatch [regex]::Escape($HardwareId) -or $infText -match 'REPLACE_WITH_EXACT') {
    throw 'Generated INF does not contain exactly the requested concrete hardware ID.'
}

Write-Host 'STAGE B EXACT MODIFICATION PREVIEW'
Write-Host "  Physical ID: $StablePhysicalId"
Write-Host "  Container ID: $ContainerId"
Write-Host "  TLC instance: $TlcInstanceId"
Write-Host "  Hardware ID: $HardwareId"
Write-Host "  INF: $InfPath"
Write-Host '  Future operation: add the exact INF package and update only a matching device.'
Write-Host '  Recovery acknowledgement:' $RemoteRecoveryConfirmed

if (-not $Execute) {
    Write-Host 'DRY RUN ONLY. No driver was installed, loaded, registered, or bound.'
    exit 0
}
if ($generatedNow) { throw 'INF was just generated. Sign its catalog, inspect it, then run a separate -Execute invocation.' }
if (-not $RemoteRecoveryConfirmed) { throw 'Execution requires -RemoteRecoveryConfirmed.' }
if (-not (Test-Path -LiteralPath $sys) -or -not (Test-Path -LiteralPath $cat)) {
    throw 'Signed SYS/CAT package files must be present beside the exact INF.'
}
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Execution requires an elevated PowerShell on the dedicated test machine.'
}

& pnputil.exe /add-driver $InfPath /install
if ($LASTEXITCODE -ne 0) { throw "PnPUtil installation failed with exit code $LASTEXITCODE" }
Write-Host 'Installation command completed. Verify BOTH keyboards immediately and use standalone recovery if either fails.'
