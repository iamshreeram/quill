# install.ps1 -- EXPERIMENTAL Windows installer for quill.
# UNTESTED ON WINDOWS: written without a Windows host. Review before running.
# Usage (PowerShell): $env:QUILL_VERSION='1.2.3'; irm https://github.com/iamshreeram/quill/releases/latest/download/install.ps1 | iex
$ErrorActionPreference = 'Stop'

$Base = if ($env:QUILL_BASE_URL) { $env:QUILL_BASE_URL.TrimEnd('/') } else { 'https://github.com/iamshreeram/quill/releases' }
$InstallDir = if ($env:QUILL_INSTALL_DIR) { $env:QUILL_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'quill\bin' }

if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') { throw "unsupported architecture: $env:PROCESSOR_ARCHITECTURE (only x86_64 is published)" }

# Resolve one exact tag: explicit QUILL_VERSION, else follow the latest redirect.
if ($env:QUILL_VERSION) {
    $tag = $env:QUILL_VERSION
    if ($tag -notlike 'v*') { $tag = "v$tag" }
} else {
    $loc = $null
    try {
        Invoke-WebRequest -Uri "$Base/latest" -MaximumRedirection 0 -UseBasicParsing -ErrorAction Stop | Out-Null
    } catch {
        $loc = [string]$_.Exception.Response.Headers['Location']
    }
    if (-not $loc) { throw 'could not resolve the latest release tag' }
    $tag = $loc.TrimEnd('/').Split('/')[-1]
}
if ($tag -notmatch '^v\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$') { throw "could not resolve a release tag (got '$tag'); set QUILL_VERSION" }
$ver = $tag.Substring(1)
$zip = "quill-$ver-x86_64-pc-windows-msvc.zip"

$tmp = Join-Path ([IO.Path]::GetTempPath()) ('quill-install-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Invoke-WebRequest -Uri "$Base/download/$tag/$zip" -OutFile "$tmp\$zip" -UseBasicParsing
    Invoke-WebRequest -Uri "$Base/download/$tag/SHA256SUMS" -OutFile "$tmp\SHA256SUMS" -UseBasicParsing

    $line = Get-Content "$tmp\SHA256SUMS" | Where-Object { $_ -match "\s\*?$([regex]::Escape($zip))$" } | Select-Object -First 1
    if (-not $line) { throw "no checksum entry for $zip; nothing installed" }
    $expected = ($line -split '\s+')[0].ToLower()
    $actual = (Get-FileHash -Algorithm SHA256 -Path "$tmp\$zip").Hash.ToLower()
    if ($actual -ne $expected) { throw "checksum mismatch for $zip; nothing installed" }

    Expand-Archive -Path "$tmp\$zip" -DestinationPath "$tmp\x" -Force
    $exe = Get-ChildItem -Path "$tmp\x" -Recurse -Filter quill.exe | Select-Object -First 1
    if (-not $exe) { throw "archive does not contain quill.exe" }
    $got = (& $exe.FullName --version)
    if ($got -ne "quill $ver") { throw "binary reports '$got', expected 'quill $ver'; nothing installed" }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -Path $exe.FullName -Destination (Join-Path $InstallDir 'quill.exe') -Force
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

Write-Host "quill $ver installed to $InstallDir\quill.exe"
if (-not (($env:Path -split ';') -contains $InstallDir)) {
    Write-Host 'Add it to your user PATH yourself (this script does not edit PATH):'
    Write-Host "  [Environment]::SetEnvironmentVariable('Path', [Environment]::GetEnvironmentVariable('Path', 'User') + ';$InstallDir', 'User')"
}
Write-Host 'Next: run "quill init", then "quill setup".'
