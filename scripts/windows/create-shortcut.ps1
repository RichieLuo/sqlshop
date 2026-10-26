param(
    [Parameter(Mandatory = $true)][string]$ShortcutPath,
    [Parameter(Mandatory = $true)][string]$TargetPath,
    [Parameter(Mandatory = $true)][string]$IconPath,
    [Parameter(Mandatory = $true)][string]$WorkingDirectory,
    [Parameter(Mandatory = $true)][string]$Description
)
$ErrorActionPreference = 'Stop'
$shopShell = New-Object -ComObject WScript.Shell
$shopShortcut = $shopShell.CreateShortcut($ShortcutPath)
$shopShortcut.TargetPath = $TargetPath
$shopShortcut.Arguments = ''
$shopShortcut.WorkingDirectory = $WorkingDirectory
$shopShortcut.IconLocation = "$IconPath,0"
$shopShortcut.Description = $Description
$shopShortcut.WindowStyle = 1
$shopShortcut.Save()
