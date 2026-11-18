param([switch]$NoBrowser)
$ErrorActionPreference = 'Stop'
$localRoot = $PSScriptRoot
$pythonPath = Join-Path $localRoot '.venv\Scripts\pythonw.exe'
$serverPath = Join-Path $localRoot 'local_server.py'
$reportsBase = 'http://127.0.0.1:5000'
$settingsPath = Join-Path $localRoot 'auto-config.json'
if (Test-Path -LiteralPath $settingsPath) {
    $settings = Get-Content -LiteralPath $settingsPath -Raw | ConvertFrom-Json
    if ($settings.reports_url) { $reportsBase = $settings.reports_url.TrimEnd('/') }
    elseif ($settings.remote_export_url) { $reportsBase = $settings.remote_export_url.TrimEnd('/') }
}
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
if (-not $NoBrowser) { Start-Process ($reportsBase + '/logs_list') }
