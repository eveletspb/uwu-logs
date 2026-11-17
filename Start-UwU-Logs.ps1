param([switch]$NoBrowser)
$ErrorActionPreference = 'Stop'
$localRoot = $PSScriptRoot
$pythonPath = Join-Path $localRoot '.venv\Scripts\pythonw.exe'
$serverPath = Join-Path $localRoot 'local_server.py'
$healthy = $false
try {
    $health = Invoke-RestMethod 'http://127.0.0.1:5000/local-health' -TimeoutSec 2
    $healthy = $health.app -eq 'UwU Logs local'
} catch { }
if (-not $healthy) {
    $serverProcess = Start-Process -FilePath $pythonPath -ArgumentList ('-X utf8 "' + $serverPath + '"') -WorkingDirectory $localRoot -WindowStyle Hidden -RedirectStandardOutput (Join-Path $localRoot 'local-server.out.log') -RedirectStandardError (Join-Path $localRoot 'local-server.err.log') -PassThru
    $serverProcess.Id | Set-Content (Join-Path $localRoot 'local-server.pid')
    for ($attempt = 0; $attempt -lt 30; $attempt++) {
        Start-Sleep -Milliseconds 500
        try {
            $health = Invoke-RestMethod 'http://127.0.0.1:5000/local-health' -TimeoutSec 1
            if ($health.app -eq 'UwU Logs local') { $healthy = $true; break }
        } catch { }
        if ($serverProcess.HasExited) { break }
    }
}
if (-not $healthy) { throw 'UwU Logs could not start. See local-server.err.log.' }
if (-not $NoBrowser) { Start-Process 'http://127.0.0.1:5000/logs_list' }
