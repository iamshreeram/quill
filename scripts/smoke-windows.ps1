# Smoke test for a Windows quill.exe: checks `--version` and the offline stdin path.
# Why: quill.exe is a GUI-subsystem program (windows_subsystem = "windows"), so its stdout
# cannot be captured with a plain PowerShell variable assignment (the pipe is closed and
# Rust panics with os error 232). All native calls go through lib-native.ps1, which uses
# Start-Process with file redirection. Any failure throws, so `pwsh -File` exits non-zero.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Bin,
    [Parameter(Mandatory = $true)][string]$Version
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'lib-native.ps1')

if (-not (Test-Path -LiteralPath $Bin)) {
    throw "binary not found: $Bin"
}
$exe = (Resolve-Path -LiteralPath $Bin).Path

$versionRun = Invoke-Native -FilePath $exe -ArgumentList @('--version')
if ($versionRun.Code -ne 0 -or $versionRun.Out.Trim() -ne "quill $Version") {
    throw "version check failed: expected 'quill $Version', got exit code $($versionRun.Code); stdout: $($versionRun.Out.Trim()); stderr: $($versionRun.Err.Trim())"
}

$textRun = Invoke-Native -FilePath $exe -ArgumentList @('--stdin', '--mode', 'offline') -StdinText "this is an test`n"
if ($textRun.Code -ne 0 -or $textRun.Out -notmatch 'a test') {
    throw "stdin check failed: exit code $($textRun.Code); stdout: $($textRun.Out.Trim()); stderr: $($textRun.Err.Trim())"
}

Write-Host "windows smoke ok: $exe quill $Version"
