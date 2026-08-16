[CmdletBinding()]
param([Parameter(Mandatory = $true)][string]$TargetInstanceId)

$ErrorActionPreference = 'Continue'
Write-Host 'MultiSeat keyboard filter status (read-only)'
& sc.exe query MultiSeatKeyboardFilter
& pnputil.exe /enum-devices /instanceid $TargetInstanceId /drivers
& pnputil.exe /enum-drivers /class Keyboard /files
& reg.exe query "HKLM\SYSTEM\CurrentControlSet\Enum\$TargetInstanceId" /v UpperFilters
Write-Host 'No key contents were collected and no machine state was changed.'

