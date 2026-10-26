param(
    [Parameter(Mandatory = $true)][string]$PythonPath,
    [Parameter(Mandatory = $true)][string]$HelperPath,
    [Parameter(Mandatory = $true)][string]$ExchangeDirectory
)
$ErrorActionPreference = 'Stop'
# Resolve the automation object hosted by the real desktop Explorer process.
# A freshly created Shell.Application object alone can retain the caller's MSIX view.
$shopWindows = (New-Object -ComObject Shell.Application).Windows()
$shopLocation = [object]0
$shopRootLocation = [object]0
$shopDesktopHwnd = 0
$shopDesktop = $shopWindows.FindWindowSW([ref]$shopLocation, [ref]$shopRootLocation, 8, [ref]$shopDesktopHwnd, 1)
if (-not $shopDesktop -or -not $shopDesktop.Document.Application) {
    throw 'The current user desktop Explorer is unavailable; no installation was performed.'
}
# Windows filenames cannot contain quotes. These arguments are paths to files/directories.
$shopArguments = '"{0}" --worker "{1}"' -f $HelperPath, $ExchangeDirectory
$shopDesktop.Document.Application.ShellExecute($PythonPath, $shopArguments, (Split-Path $HelperPath), 'open', 0)
