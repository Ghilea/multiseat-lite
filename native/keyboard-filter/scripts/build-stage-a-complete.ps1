[CmdletBinding()]
param(
    [ValidateSet('Release')]
    [string]$Configuration = 'Release',

    [string]$CodeQlPath = 'C:\codeql-home\codeql\codeql.exe',

    [switch]$ReuseValidatedCodeQlResults
)

$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
$project = Join-Path $root 'driver\MultiSeatKeyboardFilter.vcxproj'
$validationInf = Join-Path $root 'driver\MultiSeatKeyboardFilter.validation.inf'
$templateInf = Join-Path $root 'driver\MultiSeatKeyboardFilter.inf.template'
$infVerif = 'C:\Program Files (x86)\Windows Kits\10\Tools\10.0.26100.0\x64\infverif.exe'
$dvlTool = 'C:\Program Files (x86)\Windows Kits\10\Tools\dvl\dvl.exe'
$artifactRoot = Join-Path $root 'artifacts\stage-a'
$codeqlRoot = Join-Path $artifactRoot 'codeql'
# Keep the traced database outside the source tree. A WDK Rebuild cleans project
# outputs recursively and must never contend with CodeQL's open tracer log.
$database = Join-Path ([IO.Path]::GetTempPath()) 'multiseat-keyboard-filter-codeql-database'
$dvlRoot = Join-Path $codeqlRoot 'dvl'
$packageRoot = Join-Path $root 'stage-b\package'
$driverOutput = Join-Path $root "build\$Configuration\x64"
$toolsManifest = Join-Path $root 'tools\Cargo.toml'
$policyManifest = Join-Path $root 'service-policy\Cargo.toml'

foreach ($required in @($CodeQlPath, $infVerif, $dvlTool, $project, $validationInf, $templateInf)) {
    if (-not (Test-Path -LiteralPath $required)) { throw "Required Stage A prerequisite is missing: $required" }
}
New-Item -ItemType Directory -Path $artifactRoot, $codeqlRoot, $dvlRoot, $packageRoot -Force | Out-Null

& (Join-Path $PSScriptRoot 'build-stage-a.ps1') -Configuration $Configuration
if ($LASTEXITCODE -ne 0) { throw 'Release WDK/PREfast build failed.' }
& $infVerif /v /w $validationInf
if ($LASTEXITCODE -ne 0) { throw 'InfVerif failed.' }

cargo fmt --manifest-path $policyManifest -- --check
if ($LASTEXITCODE -ne 0) { throw 'service-policy formatting failed.' }
cargo test --manifest-path $policyManifest
if ($LASTEXITCODE -ne 0) { throw 'service-policy tests failed.' }
cargo fmt --manifest-path $toolsManifest -- --check
if ($LASTEXITCODE -ne 0) { throw 'tools formatting failed.' }
cargo test --manifest-path $toolsManifest
if ($LASTEXITCODE -ne 0) { throw 'tools tests failed.' }
cargo build --release --manifest-path $toolsManifest
if ($LASTEXITCODE -ne 0) { throw 'tools release build failed.' }

if (-not $ReuseValidatedCodeQlResults) {
    & $CodeQlPath database create $database --overwrite --language=cpp --source-root=$root --command='scripts\codeql-build-driver.cmd' --threads=0
    if ($LASTEXITCODE -ne 0) { throw 'CodeQL database creation failed.' }
}

$suiteResults = [ordered]@{}
foreach ($suite in @('mustfix', 'mustrun', 'recommended')) {
    $sarif = Join-Path $codeqlRoot "$suite.sarif"
    if (-not $ReuseValidatedCodeQlResults) {
        & $CodeQlPath database analyze $database --format=sarifv2.1.0 "--output=$sarif" --threads=0 --rerun "microsoft/windows-drivers@1.10.0:windows-driver-suites/$suite.qls"
        if ($LASTEXITCODE -ne 0) { throw "CodeQL $suite analysis failed." }
    } elseif (-not (Test-Path -LiteralPath $sarif)) {
        throw "Cannot reuse missing CodeQL result: $sarif"
    }
    $document = Get-Content -LiteralPath $sarif -Raw | ConvertFrom-Json
    $count = @($document.runs | ForEach-Object { $_.results }).Count
    $suiteResults[$suite] = [ordered]@{ result = $(if ($count -eq 0) { 'Pass' } else { 'Findings' }); findings = $count; sarif = "$suite.sarif" }
}
if ($suiteResults.mustfix.findings -ne 0) { throw 'Must-Fix findings block Stage B packaging.' }
if ($suiteResults.mustrun.findings -ne 0 -or $suiteResults.recommended.findings -ne 0) {
    throw 'Untriaged Must-Run/Recommended findings block this development package.'
}

Copy-Item -LiteralPath (Join-Path $codeqlRoot 'recommended.sarif') -Destination (Join-Path $dvlRoot 'MultiSeatKeyboardFilter.sarif') -Force
Copy-Item -LiteralPath $project -Destination $dvlRoot -Force
Push-Location $dvlRoot
try {
    & $dvlTool /manualCreate MultiSeatKeyboardFilter X64
    if ($LASTEXITCODE -ne 0) { throw 'Development DVL generation failed.' }
} finally {
    Pop-Location
}
$dvlLabel = Join-Path $dvlRoot 'DEVELOPMENT-NOT-CERTIFICATION.txt'
@'
This DVL is development validation only. It was produced using the general-use
Microsoft Windows-driver CodeQL workflow, not a selected WHCP/HLK certification
matrix, and must not be represented as Windows certification evidence.
'@ | Set-Content -LiteralPath $dvlLabel -Encoding utf8

$packageDirectories = @('driver', 'bin', 'scripts', 'templates', 'analysis', 'validation')
foreach ($directory in $packageDirectories) { New-Item -ItemType Directory -Path (Join-Path $packageRoot $directory) -Force | Out-Null }
Copy-Item -LiteralPath (Join-Path $driverOutput 'MultiSeatKeyboardFilter.sys') -Destination (Join-Path $packageRoot 'driver') -Force
Copy-Item -LiteralPath (Join-Path $driverOutput 'MultiSeatKeyboardFilter.pdb') -Destination (Join-Path $packageRoot 'driver') -Force
Copy-Item -LiteralPath (Join-Path $root 'tools\target\release\multiseat-keyboard-filter-tools.exe') -Destination (Join-Path $packageRoot 'bin') -Force
Copy-Item -Path (Join-Path $root 'stage-b\scripts\*.ps1') -Destination (Join-Path $packageRoot 'scripts') -Force
Copy-Item -LiteralPath (Join-Path $root 'stage-b\README.md') -Destination $packageRoot -Force
Copy-Item -LiteralPath $templateInf -Destination (Join-Path $packageRoot 'templates') -Force
Copy-Item -LiteralPath $validationInf -Destination (Join-Path $packageRoot 'validation') -Force
Copy-Item -LiteralPath (Join-Path $codeqlRoot 'mustfix.sarif') -Destination (Join-Path $packageRoot 'analysis') -Force
Copy-Item -LiteralPath (Join-Path $codeqlRoot 'mustrun.sarif') -Destination (Join-Path $packageRoot 'analysis') -Force
Copy-Item -LiteralPath (Join-Path $codeqlRoot 'recommended.sarif') -Destination (Join-Path $packageRoot 'analysis') -Force
Copy-Item -LiteralPath (Join-Path $dvlRoot 'MultiSeatKeyboardFilter.DVL.XML') -Destination (Join-Path $packageRoot 'analysis') -Force
Copy-Item -LiteralPath $dvlLabel -Destination (Join-Path $packageRoot 'analysis') -Force

$codeqlVersion = (& $CodeQlPath version --format=json | ConvertFrom-Json).version
$windowsPack = Get-Content -LiteralPath "$env:USERPROFILE\.codeql\packages\microsoft\windows-drivers\1.10.0\qlpack.yml" -Raw
$cppPack = Get-Content -LiteralPath "$env:USERPROFILE\.codeql\packages\microsoft\cpp-queries\0.0.5\qlpack.yml" -Raw
if ($windowsPack -notmatch 'version:\s+1\.10\.0' -or $cppPack -notmatch 'version:\s+0\.0\.5') { throw 'Unexpected CodeQL pack versions.' }
$msvcVersion = Get-ChildItem 'C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC' -Directory | Sort-Object Name -Descending | Select-Object -First 1 -ExpandProperty Name
$wdkVersion = (Get-Item -LiteralPath $infVerif).VersionInfo.ProductVersion
$wdkPointerParserDiagnosticCount = 0
if (Test-Path -LiteralPath (Join-Path $database 'log\extractor')) {
    $wdkPointerParserDiagnosticCount = @(
        Get-ChildItem (Join-Path $database 'log\extractor') -Recurse -File |
            Select-String -SimpleMatch '__ptr32 and __ptr64 must follow a'
    ).Count
}
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $root '..\..'))
$gitCommit = 'unavailable'
if ((Test-Path -LiteralPath (Join-Path $repositoryRoot '.git')) -and (Get-Command git -ErrorAction SilentlyContinue)) {
    $previousErrorPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $candidateCommit = & git -C $repositoryRoot rev-parse --verify HEAD 2>$null
    $gitExitCode = $LASTEXITCODE
    $ErrorActionPreference = $previousErrorPreference
    if ($gitExitCode -eq 0 -and $candidateCommit) { $gitCommit = $candidateCommit.Trim() }
}

$hashFiles = @(
    (Join-Path $packageRoot 'driver\MultiSeatKeyboardFilter.sys'),
    (Join-Path $packageRoot 'driver\MultiSeatKeyboardFilter.pdb'),
    (Join-Path $packageRoot 'templates\MultiSeatKeyboardFilter.inf.template'),
    (Join-Path $packageRoot 'validation\MultiSeatKeyboardFilter.validation.inf')
)
$hashes = [ordered]@{}
foreach ($file in $hashFiles) {
    $relative = $file.Substring($packageRoot.Length + 1).Replace('\', '/')
    $hashes[$relative] = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
}
$manifest = [ordered]@{
    schemaVersion = 1
    generatedAtUtc = [DateTime]::UtcNow.ToString('o')
    classification = 'DEVELOPMENT_NOT_CERTIFICATION'
    driver = [ordered]@{ name = 'MultiSeatKeyboardFilter'; version = '0.1.0.0'; configuration = $Configuration; architecture = 'x64'; kmdfVersion = '1.31'; suppressionCapability = 'DisabledByBuild' }
    source = [ordered]@{ gitCommit = $gitCommit }
    toolchain = [ordered]@{ msvcVersion = $msvcVersion; wdkVersion = $wdkVersion; codeqlCliVersion = $codeqlVersion; windowsDriversPack = '1.10.0'; microsoftCppQueriesPack = '0.0.5' }
    validation = [ordered]@{
        prefast = 'Pass'
        infVerif = 'Pass'
        codeql = $suiteResults
        codeqlExtraction = [ordered]@{
            result = $(if ($wdkPointerParserDiagnosticCount -eq 0) { 'Pass' } else { 'CompletedWithNonFatalWdkHeaderDiagnostic' })
            wdkPointerParserDiagnosticCount = $wdkPointerParserDiagnosticCount
            note = 'CodeQL completed successfully; count records the known parser diagnostic at WDK wdm.h EXTENDED_CREATE_INFORMATION_32, not a query finding.'
        }
        passThroughInvariant = 'Pass'
    }
    sha256 = $hashes
}
$manifestPath = Join-Path $packageRoot 'artifact-manifest.json'
$manifest | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $manifestPath -Encoding utf8

$summary = @(
    'MultiSeatKeyboardFilter Stage A development validation',
    "CodeQL CLI: $codeqlVersion",
    'microsoft/windows-drivers: 1.10.0',
    'microsoft/cpp-queries resolved by pack: 0.0.5',
    "Must-Fix findings: $($suiteResults.mustfix.findings)",
    "Must-Run findings: $($suiteResults.mustrun.findings)",
    "Recommended findings: $($suiteResults.recommended.findings)",
    "WDK header extractor parser diagnostics: $wdkPointerParserDiagnosticCount (non-query diagnostic; see manifest)",
    'PREfast/DriverMinimumRules: Pass',
    'InfVerif /v /w: Pass',
    'SuppressionCapability: DisabledByBuild',
    'Classification: DEVELOPMENT / NOT CERTIFICATION',
    'No driver was installed, registered, loaded, or bound.'
)
$summary | Set-Content -LiteralPath (Join-Path $packageRoot 'analysis\codeql-summary.txt') -Encoding utf8
Write-Host "Deterministic Stage B package: $packageRoot"
Write-Host "Artifact manifest: $manifestPath"
Write-Host 'BUILD/STATIC ANALYSIS ONLY. No driver or device state was changed.'
