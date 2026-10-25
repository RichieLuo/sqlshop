$ErrorActionPreference = 'Stop'
$shopProjectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

function Initialize-ShopToolchain {
    if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { return }
    # Prefer a complete VS 2022 installation over incomplete preview toolchains.
    $shopCandidates = @(
        'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools',
        'C:\Program Files\Microsoft Visual Studio\2022\Community',
        'C:\Program Files\Microsoft Visual Studio\2022\Professional',
        'C:\Program Files\Microsoft Visual Studio\2022\Enterprise'
    )
    $shopVsWhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (Test-Path -LiteralPath $shopVsWhere) {
        $shopDetected = & $shopVsWhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if ($shopDetected) { $shopCandidates += $shopDetected }
    }
    foreach ($shopCandidate in $shopCandidates) {
        $shopVcRoot = Join-Path $shopCandidate 'VC\Tools\MSVC'
        if (-not (Test-Path -LiteralPath $shopVcRoot)) { continue }
        $shopVc = Get-ChildItem -LiteralPath $shopVcRoot -Directory | Sort-Object Name -Descending | Select-Object -First 1
        if (-not $shopVc -or -not (Test-Path -LiteralPath (Join-Path $shopVc.FullName 'bin\HostX64\x64\link.exe'))) { continue }
        $shopSdkRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10'
        $shopSdk = Get-ChildItem -LiteralPath (Join-Path $shopSdkRoot 'Lib') -Directory -ErrorAction SilentlyContinue |
            Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'um\x64\kernel32.lib') } |
            Sort-Object Name -Descending | Select-Object -First 1
        if (-not $shopSdk) { continue }
        $shopLauncher = Join-Path $shopCandidate 'Common7\Tools\Launch-VsDevShell.ps1'
        if (Test-Path -LiteralPath $shopLauncher) {
            & $shopLauncher -SkipAutomaticLocation -Arch amd64 -HostArch amd64 | Out-Null
        }
        $env:LIB = @(
            (Join-Path $shopVc.FullName 'lib\x64'),
            (Join-Path $shopSdk.FullName 'um\x64'),
            (Join-Path $shopSdk.FullName 'ucrt\x64')
        ) -join ';'
        $shopIncludes = Join-Path $shopSdkRoot "Include\$($shopSdk.Name)"
        $env:INCLUDE = @(
            (Join-Path $shopVc.FullName 'include'),
            (Join-Path $shopIncludes 'ucrt'), (Join-Path $shopIncludes 'shared'),
            (Join-Path $shopIncludes 'um'), (Join-Path $shopIncludes 'winrt')
        ) -join ';'
        $env:PATH = @(
            (Join-Path $shopVc.FullName 'bin\HostX64\x64'),
            (Join-Path $shopSdkRoot "bin\$($shopSdk.Name)\x64"), $env:PATH
        ) -join ';'
        $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = Join-Path $shopVc.FullName 'bin\HostX64\x64\link.exe'
        return
    }
    # A developer shell or a custom toolchain may already be configured.
}

function Invoke-ShopCargo {
    param([string[]]$CargoArguments)
    & cargo @CargoArguments
    if ($LASTEXITCODE -ne 0) { throw "cargo $($CargoArguments -join ' ') failed with exit code $LASTEXITCODE." }
}
