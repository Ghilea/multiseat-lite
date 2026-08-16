[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$TargetInstanceId,

    [Parameter(Mandatory = $true)]
    [string]$OutputDirectory,

    [Parameter(Mandatory = $true)]
    [string]$ConfigPath,

    [ValidateSet('Unknown', 'Pass', 'Fail')]
    [string]$DennisKeyboardCheck = 'Unknown',

    [ValidateSet('Unknown', 'Pass', 'Fail')]
    [string]$BarnenKeyboardCheck = 'Unknown'
)

$ErrorActionPreference = 'Stop'
$tool = Join-Path $PSScriptRoot '..\bin\multiseat-keyboard-filter-tools.exe'
if (-not (Test-Path -LiteralPath $tool)) {
    throw "Identity tool is missing: $tool"
}
if (Test-Path -LiteralPath $OutputDirectory) {
    throw 'Refusing to overwrite an existing snapshot directory.'
}

New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$hardwarePath = Join-Path $OutputDirectory 'hardware.json'
& $tool snapshot --config $ConfigPath --output $hardwarePath
if ($LASTEXITCODE -ne 0) {
    throw "Hardware snapshot failed with exit code $LASTEXITCODE"
}

function Invoke-ReadOnlyCommand([string]$File, [string[]]$Arguments) {
    $result = & $File @Arguments 2>&1 | Out-String
    [ordered]@{ command = "$File $($Arguments -join ' ')"; output = $result; exitCode = $LASTEXITCODE }
}

$keyboardClass = '{4D36E96B-E325-11CE-BFC1-08002BE10318}'
$snapshot = [ordered]@{
    schemaVersion = 1
    capturedAtUtc = [DateTime]::UtcNow.ToString('o')
    targetInstanceId = $TargetInstanceId
    keyboardFunctionality = [ordered]@{
        dennis = $DennisKeyboardCheck
        barnen = $BarnenKeyboardCheck
        statement = 'Manual pre-install check; no key contents were collected.'
    }
    hardware = Get-Content -LiteralPath $hardwarePath -Raw | ConvertFrom-Json
    targetDriverStack = Invoke-ReadOnlyCommand 'pnputil.exe' @('/enum-devices', '/instanceid', $TargetInstanceId, '/drivers')
    keyboardDriverPackages = Invoke-ReadOnlyCommand 'pnputil.exe' @('/enum-drivers', '/class', 'Keyboard', '/files')
    keyboardClassUpperFilters = Invoke-ReadOnlyCommand 'reg.exe' @('query', "HKLM\SYSTEM\CurrentControlSet\Control\Class\$keyboardClass", '/v', 'UpperFilters')
    targetUpperFilters = Invoke-ReadOnlyCommand 'reg.exe' @('query', "HKLM\SYSTEM\CurrentControlSet\Enum\$TargetInstanceId", '/v', 'UpperFilters')
    multiSeatService = Invoke-ReadOnlyCommand 'sc.exe' @('query', 'MultiSeatKeyboardFilter')
}
$snapshot | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'identity-snapshot.json') -Encoding utf8
Write-Host "Snapshot saved to: $OutputDirectory"
Write-Host 'Read-only commands only. No key contents, driver changes, or PnP changes occurred.'

