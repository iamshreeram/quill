# install.ps1 -- EXPERIMENTAL Windows installer for quill (the Windows target is still experimental; this script is exercised in CI on windows-latest under PowerShell 7 and Windows PowerShell 5.1 against a local fixture server).
# Usage (PowerShell): $env:QUILL_VERSION='1.2.3'; irm https://github.com/iamshreeram/quill/releases/latest/download/install.ps1 | iex
$ErrorActionPreference = 'Stop'
try { [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12 } catch { }
Add-Type -AssemblyName System.Net.Http

# Return the tag that GET $Base/latest redirects to, without following the redirect.
# GitHub rejects requests without a User-Agent, so one is always set.
function Get-LatestTag([string]$Base) {
    $handler = New-Object System.Net.Http.HttpClientHandler
    $handler.AllowAutoRedirect = $false
    $client = New-Object System.Net.Http.HttpClient($handler)
    try {
        $client.Timeout = [TimeSpan]::FromSeconds(30)
        $client.DefaultRequestHeaders.UserAgent.ParseAdd('quill-install.ps1')
        $resp = $client.GetAsync("$Base/latest").GetAwaiter().GetResult()
        try {
            $code = [int]$resp.StatusCode
            if (($code -lt 300) -or ($code -ge 400) -or ($null -eq $resp.Headers.Location)) {
                throw 'could not resolve the latest release tag'
            }
            # The Location may be relative (/tag/v1.2.3) or absolute (.../releases/tag/v1.2.3).
            $loc = $resp.Headers.Location.ToString()
            return $loc.TrimEnd('/').Split('/')[-1]
        } finally {
            $resp.Dispose()
        }
    } finally {
        $client.Dispose()
        $handler.Dispose()
    }
}

# Run quill.exe --version and return its trimmed stdout, or "exit code N".
# quill.exe is a GUI-subsystem binary, so PowerShell cannot capture it via `& exe`;
# Start-Process with stdout redirected to a temp file is used instead.
function Get-QuillVersion([string]$Exe) {
    $out = Join-Path ([IO.Path]::GetTempPath()) ('quill-version-' + [guid]::NewGuid() + '.txt')
    try {
        $p = Start-Process -FilePath $Exe -ArgumentList '--version' -RedirectStandardOutput $out -NoNewWindow -PassThru
        $null = $p.Handle
        if (-not $p.WaitForExit(30000)) {
            try { $p.Kill() } catch { }
            throw 'quill.exe --version did not finish within 30 s'
        }
        $p.WaitForExit()
        if ($p.ExitCode -ne 0) { return "exit code $($p.ExitCode)" }
        $text = [string](Get-Content -Path $out -Raw)
        return $text.Trim()
    } finally {
        Remove-Item -Path $out -Force -ErrorAction SilentlyContinue
    }
}

$Base = if ($env:QUILL_BASE_URL) { $env:QUILL_BASE_URL.TrimEnd('/') } else { 'https://github.com/iamshreeram/quill/releases' }
$InstallDir = if ($env:QUILL_INSTALL_DIR) { $env:QUILL_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'quill\bin' }

if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') { throw "unsupported architecture: $env:PROCESSOR_ARCHITECTURE (only x86_64 is published)" }

# Resolve one exact tag: explicit QUILL_VERSION, else follow the latest redirect.
if ($env:QUILL_VERSION) {
    $tag = $env:QUILL_VERSION
    if ($tag -notlike 'v*') { $tag = "v$tag" }
} else {
    $tag = Get-LatestTag $Base
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
    $got = Get-QuillVersion $exe.FullName
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
