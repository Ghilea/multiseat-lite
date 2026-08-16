[CmdletBinding()]
param(
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Debug'
)

$ErrorActionPreference = 'Stop'
$project = Resolve-Path (Join-Path $PSScriptRoot '..\driver\MultiSeatKeyboardFilter.vcxproj')
$msbuild = 'C:\Program Files\Microsoft Visual Studio\2022\Community\MSBuild\Current\Bin\MSBuild.exe'
$wdkTargets = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\build' `
    -Recurse -Filter WindowsDriver.Common.targets -ErrorAction SilentlyContinue |
    Where-Object FullName -Match '\\10\.0\.26100\.0\\' |
    Select-Object -First 1
$wdfHeader = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\Include' `
    -Recurse -Filter wdf.h -ErrorAction SilentlyContinue |
    Where-Object FullName -Match '\\wdf\\kmdf\\1\.31\\' |
    Select-Object -First 1
$driverToolset = 'C:\Program Files\Microsoft Visual Studio\2022\Community\MSBuild\Microsoft\VC\v170\Platforms\x64\PlatformToolsets\WindowsKernelModeDriver10.0'
$infVerif = 'C:\Program Files (x86)\Windows Kits\10\Tools\10.0.26100.0\x64\infverif.exe'

if (-not (Test-Path -LiteralPath $msbuild)) {
    throw 'Visual Studio 2022 Community MSBuild is not installed at the expected path.'
}
if (-not $wdkTargets -or -not $wdfHeader -or -not (Test-Path -LiteralPath $driverToolset)) {
    throw 'WDK 26100 KMDF build targets, KMDF 1.31 headers, or Visual Studio driver build integration are missing. This script will not install them.'
}
if (-not (Test-Path -LiteralPath $infVerif)) {
    throw 'WDK 26100 x64 InfVerif is missing. This script will not install it.'
}

Write-Host "MSBuild: $msbuild"
Write-Host "WDK targets: $($wdkTargets.FullName)"
Write-Host "KMDF headers: $($wdfHeader.FullName)"
Write-Host "Driver toolset: $driverToolset"
Write-Host "InfVerif: $infVerif"

& (Join-Path $PSScriptRoot 'check-pass-through-source.ps1')
if (-not $?) {
    throw 'Pass-through source invariant check failed.'
}

& $msbuild $project `
    /m `
    /t:Rebuild `
    /p:Configuration=$Configuration `
    /p:Platform=x64 `
    /p:RunCodeAnalysis=true
if ($LASTEXITCODE -ne 0) {
    throw "Driver build/code analysis failed with exit code $LASTEXITCODE"
}

Write-Host 'Stage A complete: build and compiler analysis only.'
Write-Host 'No INF was staged, no driver was installed, and no device was restarted.'
