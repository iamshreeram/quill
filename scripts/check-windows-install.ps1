# Windows install check for the release pipeline (run after scripts/package.sh produced the zip).
# Why: quill.exe is a GUI-subsystem program, so every native call here goes through
# lib-native.ps1 (Start-Process with file redirection) instead of assigning a native call's output to a variable, which
# fails with os error 232. Three parts: (i) self-tests of the wrapper, (ii) archive and
# checksum checks plus a smoke run of the extracted exe, (iii) end-to-end installs of
# install.ps1 against a local fixture server, under pwsh and Windows PowerShell 5.1.

param(
    [Parameter(Mandatory = $true)][string]$Dist,
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$Bin
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'lib-native.ps1')

function Assert-That {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) {
        throw "ASSERT FAILED: $Message"
    }
}

function Write-Banner {
    param([string]$Text)
    Write-Host ''
    Write-Host "== $Text =="
}

$Dist = (Resolve-Path -LiteralPath $Dist).Path
$Bin = (Resolve-Path -LiteralPath $Bin).Path
$repo = Split-Path -Parent $PSScriptRoot
$smoke = Join-Path $PSScriptRoot 'smoke-windows.ps1'

# ---- Part (i): wrapper self-tests ----
Write-Banner 'part (i): wrapper self-tests'

$exitRun = Invoke-Native -FilePath 'cmd.exe' -ArgumentList @('/c', '"echo boom 1>&2 & exit 3"')
Assert-That -Condition ($exitRun.Code -eq 3) -Message "expected exit code 3, got $($exitRun.Code)"
Assert-That -Condition ($exitRun.Err -match 'boom') -Message "stderr was not captured: '$($exitRun.Err)'"
Write-Host 'ok: nonzero exit code and stderr captured'

$pingBefore = @(Get-Process ping -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
$timeoutMessage = ''
try {
    $null = Invoke-Native -FilePath 'ping.exe' -ArgumentList @('-n', '30', '127.0.0.1') -TimeoutSec 2
} catch {
    $timeoutMessage = $_.Exception.Message
}
Assert-That -Condition ($timeoutMessage -match 'timed out') -Message "expected a timeout error, got: '$timeoutMessage'"
Start-Sleep -Milliseconds 500
$pingAfter = @(Get-Process ping -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
$newPings = @($pingAfter | Where-Object { $pingBefore -notcontains $_ })
Assert-That -Condition ($newPings.Count -eq 0) -Message "ping still running after timeout: $($newPings -join ',')"
Write-Host 'ok: timeout thrown and child process gone'

$wrongRun = Invoke-Native -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-File', ('"' + $smoke + '"'), '-Bin', ('"' + $Bin + '"'), '-Version', '9.9.9')
Assert-That -Condition ($wrongRun.Code -ne 0) -Message 'smoke with wrong version exited 0'
Assert-That -Condition (($wrongRun.Out + $wrongRun.Err) -match '9\.9\.9') -Message 'wrong-version output does not mention 9.9.9'
Write-Host 'ok: wrong version makes smoke exit non-zero'

$missingThrew = $false
try {
    $null = Invoke-Native -FilePath (Join-Path ([IO.Path]::GetTempPath()) 'no-such-quill.exe')
} catch {
    $missingThrew = $true
}
Assert-That -Condition $missingThrew -Message 'missing executable did not throw'
Write-Host 'ok: missing executable throws'

# ---- Part (ii): archive checks ----
Write-Banner 'part (ii): archive and checksum checks'

$zipName = "quill-$Version-x86_64-pc-windows-msvc.zip"
$zip = Join-Path $Dist $zipName
Assert-That -Condition (Test-Path -LiteralPath $zip) -Message "missing archive: $zip"

$sumsPath = Join-Path $Dist 'SHA256SUMS'
Assert-That -Condition (Test-Path -LiteralPath $sumsPath) -Message "missing file: $sumsPath"

$listedHash = $null
foreach ($line in (Get-Content -LiteralPath $sumsPath)) {
    if ($line -match '^([0-9a-fA-F]{64})\s+\*?(.+)$') {
        if ($Matches[2].Trim() -eq $zipName) {
            $listedHash = $Matches[1]
        }
    }
}
Assert-That -Condition ($null -ne $listedHash) -Message "SHA256SUMS has no line for $zipName"

$actualHash = (Get-FileHash -Algorithm SHA256 -Path $zip).Hash
Assert-That -Condition ($listedHash -eq $actualHash) -Message "SHA256 mismatch for ${zipName}: listed $listedHash, actual $actualHash"
Write-Host 'ok: SHA256SUMS matches the archive'

$extract = Join-Path ([IO.Path]::GetTempPath()) ('quill-extract-' + [guid]::NewGuid())
try {
    $null = New-Item -ItemType Directory -Force -Path $extract
    Expand-Archive -LiteralPath $zip -DestinationPath $extract -Force
    $exeFound = Get-ChildItem -LiteralPath $extract -Recurse -Filter 'quill.exe' | Select-Object -First 1
    Assert-That -Condition ($null -ne $exeFound) -Message "quill.exe not found inside $zipName"
    & $smoke -Bin $exeFound.FullName -Version $Version
} finally {
    Remove-Item -LiteralPath $extract -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Host 'ok: extracted quill.exe passes smoke'

# ---- Part (iii): end-to-end installs against the fixture server ----
Write-Banner 'part (iii): end-to-end installs against fixture server'

$py = $null
foreach ($candidate in @('python', 'python3')) {
    if ($null -eq $py) {
        $found = Get-Command $candidate -ErrorAction SilentlyContinue
        if ($null -ne $found) {
            $py = $found.Source
        }
    }
}
if ($null -eq $py) {
    throw 'python or python3 is required for the fixture server'
}

$root = Join-Path ([IO.Path]::GetTempPath()) ('quill-fixture-' + [guid]::NewGuid())
$downloadDir = Join-Path (Join-Path $root 'download') "v$Version"
$null = New-Item -ItemType Directory -Force -Path $downloadDir
Copy-Item -LiteralPath $zip -Destination $downloadDir
Copy-Item -LiteralPath $sumsPath -Destination $downloadDir

$listener = New-Object -TypeName System.Net.Sockets.TcpListener -ArgumentList @([System.Net.IPAddress]::Loopback, 0)
$listener.Start()
$port = $listener.LocalEndpoint.Port
$listener.Stop()

$serverLog = Join-Path ([IO.Path]::GetTempPath()) ('quill-fixture-err-' + [guid]::NewGuid() + '.txt')
$server = $null
$installDirs = @()
try {
    $serverArgs = @(('"' + (Join-Path $PSScriptRoot 'windows-fixture-server.py') + '"'), '--root', ('"' + $root + '"'), '--port', "$port", '--version', $Version)
    $server = Start-Process -FilePath $py -ArgumentList $serverArgs -PassThru -WindowStyle Hidden -RedirectStandardError $serverLog

    $ready = $false
    $deadline = (Get-Date).AddSeconds(15)
    while ((-not $ready) -and ((Get-Date) -lt $deadline)) {
        try {
            $probe = New-Object -TypeName System.Net.Sockets.TcpClient -ArgumentList @('127.0.0.1', $port)
            $probe.Close()
            $ready = $true
        } catch {
            Start-Sleep -Milliseconds 200
        }
    }
    if (-not $ready) {
        throw "fixture server not ready on port $port"
    }
    Write-Host "fixture server ready on port $port"

    $sq = "'"
    $baseUrl = "http://127.0.0.1:$port"
    $repoInstall = (Join-Path $repo 'install.ps1').Replace($sq, $sq + $sq)

    foreach ($shell in @('pwsh', 'powershell')) {
        foreach ($explicit in @($true, $false)) {
            $installDir = Join-Path ([IO.Path]::GetTempPath()) ('quill-install-' + [guid]::NewGuid())
            $null = New-Item -ItemType Directory -Force -Path $installDir
            $installDirs += $installDir
            $installQ = $installDir.Replace($sq, $sq + $sq)

            $cmd = '$env:QUILL_BASE_URL=' + $sq + $baseUrl + $sq + '; '
            $cmd = $cmd + '$env:QUILL_INSTALL_DIR=' + $sq + $installQ + $sq + '; '
            if ($explicit) {
                $cmd = $cmd + '$env:QUILL_VERSION=' + $sq + $Version + $sq + '; '
            } else {
                $cmd = $cmd + 'Remove-Item Env:QUILL_VERSION -ErrorAction SilentlyContinue; '
            }
            $cmd = $cmd + 'Get-Content -Raw -LiteralPath ' + $sq + $repoInstall + $sq + ' | Invoke-Expression'

            $shellArgs = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', ('"' + $cmd + '"'))
            $run = Invoke-Native -FilePath $shell -ArgumentList $shellArgs -TimeoutSec 180
            if ($run.Code -ne 0) {
                throw "install failed: shell=$shell explicit-version=$explicit exit=$($run.Code); stdout: $($run.Out.Trim()); stderr: $($run.Err.Trim())"
            }

            $installedExe = Join-Path $installDir 'quill.exe'
            Assert-That -Condition (Test-Path -LiteralPath $installedExe) -Message "quill.exe not installed into $installDir"
            & $smoke -Bin $installedExe -Version $Version
            Write-Host "install ok: shell=$shell explicit-version=$explicit"
        }
    }
} finally {
    if ($null -ne $server) {
        Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $serverLog -Force -ErrorAction SilentlyContinue
    foreach ($dir in $installDirs) {
        Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Host 'check-windows-install: all checks passed'
