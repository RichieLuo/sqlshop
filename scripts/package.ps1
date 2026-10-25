param(
    [string]$Target,
    [string]$OutputDirectory,
    [switch]$SkipBuild,
    [switch]$CratesIo
)
. (Join-Path $PSScriptRoot 'common.ps1')
Initialize-ShopToolchain
$shopPackageArguments = @((Join-Path $PSScriptRoot 'package.py'))
if ($Target) { $shopPackageArguments += @('--target', $Target) }
if ($OutputDirectory) { $shopPackageArguments += @('--output-directory', $OutputDirectory) }
if ($SkipBuild) { $shopPackageArguments += '--skip-build' }
& python @shopPackageArguments
if ($LASTEXITCODE -ne 0) { throw "Packaging failed with exit code $LASTEXITCODE." }
