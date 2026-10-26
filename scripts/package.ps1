param(
    [string]$Target,
    [string]$OutputDirectory,
    [string]$ExpectedTag,
    [switch]$SkipBuild,
    [switch]$CratesIo,
    [switch]$Install,
    [switch]$NoInstall
)
. (Join-Path $PSScriptRoot 'common.ps1')
Initialize-ShopToolchain
if ($Install -and $NoInstall) { throw 'Use either -Install or -NoInstall.' }
$shopPackageArguments = @((Join-Path $PSScriptRoot 'package.py'))
if ($Target) { $shopPackageArguments += @('--target', $Target) }
if ($OutputDirectory) { $shopPackageArguments += @('--output-directory', $OutputDirectory) }
if ($ExpectedTag) { $shopPackageArguments += @('--expected-tag', $ExpectedTag) }
if ($SkipBuild) { $shopPackageArguments += '--skip-build' }
if ($Install) { $shopPackageArguments += '--install' }
if ($NoInstall) { $shopPackageArguments += '--no-install' }
& python @shopPackageArguments
if ($LASTEXITCODE -ne 0) { throw "Packaging failed with exit code $LASTEXITCODE." }
