[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string]$PackageDirectory,
    [Parameter(Mandatory = $true)] [string]$CertificateThumbprint,
    [switch]$Execute
)

$ErrorActionPreference = 'Stop'
$inf2cat = 'C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x86\Inf2Cat.exe'
$signtool = 'C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\signtool.exe'
$inf = Join-Path $PackageDirectory 'MultiSeatKeyboardFilter.inf'
$sys = Join-Path $PackageDirectory 'MultiSeatKeyboardFilter.sys'
$cat = Join-Path $PackageDirectory 'MultiSeatKeyboardFilter.cat'

Write-Host 'PACKAGE SIGNING PREVIEW (no certificate or trust-store installation)'
Write-Host "  INF: $inf"
Write-Host "  SYS: $sys"
Write-Host "  CAT: $cat"
Write-Host "  Signing certificate thumbprint: $CertificateThumbprint"
Write-Host '  Target: Windows 10/11 x64 test package'
if (-not $Execute) { Write-Host 'DRY RUN ONLY. No catalog or signature was created.'; exit 0 }
foreach ($required in @($inf2cat, $signtool, $inf, $sys)) {
    if (-not (Test-Path -LiteralPath $required)) { throw "Required file is missing: $required" }
}
& $inf2cat /driver:$PackageDirectory /os:10_X64,Server10_X64
if ($LASTEXITCODE -ne 0) { throw "Inf2Cat failed with exit code $LASTEXITCODE" }
& $signtool sign /fd SHA256 /sha1 $CertificateThumbprint $cat
if ($LASTEXITCODE -ne 0) { throw "Catalog signing failed with exit code $LASTEXITCODE" }
& $signtool verify /v /pa $cat
if ($LASTEXITCODE -ne 0) { throw "Catalog signature verification failed with exit code $LASTEXITCODE" }
Write-Host 'Package created only. Nothing was installed or trusted by this script.'
