param(
    [string]$PackageDirectory,
    [switch]$OpenDefaultApps
)
$ErrorActionPreference = 'Stop'
if (-not $PackageDirectory) {
    if (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'package.json')) {
        $PackageDirectory = $PSScriptRoot
    } else {
        throw 'Specify -PackageDirectory with the completed, unpacked Windows package directory.'
    }
}
$shopInstallArguments = @((Join-Path $PSScriptRoot 'windows_install.py'), '--package-directory', $PackageDirectory)
if ($OpenDefaultApps) { $shopInstallArguments += '--open-default-apps' }
& python @shopInstallArguments
if ($LASTEXITCODE -ne 0) { throw "Installation failed with exit code $LASTEXITCODE." }
