param(
    [ValidateSet('run', 'check', 'test', 'build', 'release', 'clippy', 'fmt', 'verify')]
    [string]$Task = 'run',
    [switch]$CratesIo,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$AppArguments
)
. (Join-Path $PSScriptRoot 'common.ps1')
Initialize-ShopToolchain
Push-Location -LiteralPath $shopProjectRoot
try {
    # -CratesIo remains accepted for older LinkShop commands. All projects now
    # use Cargo's configured registry without injecting a machine-specific mirror.
    switch ($Task) {
        'run' { Invoke-ShopCargo -CargoArguments (@('run', '--locked', '--') + $AppArguments) }
        'check' { Invoke-ShopCargo -CargoArguments @('check', '--locked', '--all-targets') }
        'test' { Invoke-ShopCargo -CargoArguments @('test', '--locked') }
        'build' { Invoke-ShopCargo -CargoArguments @('build', '--locked') }
        'release' { Invoke-ShopCargo -CargoArguments @('build', '--locked', '--release') }
        'clippy' { Invoke-ShopCargo -CargoArguments @('clippy', '--locked', '--all-targets', '--', '-D', 'warnings') }
        'fmt' { Invoke-ShopCargo -CargoArguments @('fmt', '--all', '--check') }
        'verify' {
            Invoke-ShopCargo -CargoArguments @('fmt', '--all', '--check')
            Invoke-ShopCargo -CargoArguments @('clippy', '--locked', '--all-targets', '--', '-D', 'warnings')
            Invoke-ShopCargo -CargoArguments @('test', '--locked')
        }
    }
} finally {
    Pop-Location
}
