# Shared helper, dot-sourced by smoke-windows.ps1 and check-windows-install.ps1.
# Why: quill.exe is built with windows_subsystem = "windows" (src/main.rs), so it is a
# GUI-subsystem program. Capturing its stdout with a plain PowerShell call and a variable
# assignment fails: the pipe is closed and Rust panics with
# "failed printing to stdout: The pipe is being closed (os error 232)".
# Every native call therefore goes through Start-Process with file redirection.
# Arguments that contain spaces must be pre-quoted by the caller (Start-Process joins
# the ArgumentList array with spaces and does not quote for us).

function Invoke-Native {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$ArgumentList = @(),
        [string]$StdinText = $null,
        [int]$TimeoutSec = 60
    )

    $dir = [IO.Path]::GetTempPath() + 'quill-native-' + [guid]::NewGuid()
    try {
        $null = New-Item -ItemType Directory -Force -Path $dir
        $utf8 = New-Object System.Text.UTF8Encoding($false)
        $outPath = Join-Path $dir 'stdout.txt'
        $errPath = Join-Path $dir 'stderr.txt'
        [IO.File]::WriteAllText($outPath, '', $utf8)
        [IO.File]::WriteAllText($errPath, '', $utf8)

        $params = @{
            FilePath               = $FilePath
            RedirectStandardOutput = $outPath
            RedirectStandardError  = $errPath
            NoNewWindow            = $true
            PassThru               = $true
            ErrorAction            = 'Stop'
        }
        if ($ArgumentList.Count -gt 0) {
            $params['ArgumentList'] = $ArgumentList
        }
        # A typed [string] parameter turns $null into '', so test whether the caller passed it.
        if ($PSBoundParameters.ContainsKey('StdinText')) {
            $inPath = Join-Path $dir 'stdin.txt'
            [IO.File]::WriteAllText($inPath, $StdinText, $utf8)
            $params['RedirectStandardInput'] = $inPath
        }

        $proc = Start-Process @params
        # Reading the Handle caches it, so ExitCode is populated after exit.
        $null = $proc.Handle

        if (-not $proc.WaitForExit($TimeoutSec * 1000)) {
            try { $proc.Kill() } catch { }
            $null = $proc.WaitForExit(5000)
            throw "timed out after $TimeoutSec s: $FilePath"
        }
        # Parameterless WaitForExit flushes the redirected output.
        $proc.WaitForExit()
        if ($null -eq $proc.ExitCode) {
            throw "no exit code for: $FilePath"
        }

        $out = ''
        if (Test-Path -LiteralPath $outPath) {
            $out = [IO.File]::ReadAllText($outPath)
        }
        $err = ''
        if (Test-Path -LiteralPath $errPath) {
            $err = [IO.File]::ReadAllText($errPath)
        }
        return [pscustomobject]@{
            Code = [int]$proc.ExitCode
            Out  = $out
            Err  = $err
        }
    }
    finally {
        Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
