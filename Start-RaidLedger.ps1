param([switch]$NoBrowser)
$ErrorActionPreference = 'Stop'
$localRoot = $PSScriptRoot
$reportsBase = 'http://127.0.0.1:5000'
$settingsPath = Join-Path $localRoot 'auto-config.json'
if (Test-Path -LiteralPath $settingsPath) {
    $settings = Get-Content -LiteralPath $settingsPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($settings.reports_url) { $reportsBase = $settings.reports_url.TrimEnd('/') }
    elseif ($settings.remote_export_url) { $reportsBase = $settings.remote_export_url.TrimEnd('/') }
}

function Get-LocalRaidLedger {
    try { return Invoke-RestMethod 'http://127.0.0.1:5000/local-health' -TimeoutSec 2 }
    catch { return $null }
}

$health = Get-LocalRaidLedger
if ($health -and $health.app -ne 'RaidLedger') {
    throw 'Port 5000 is in use by another service. Stop it before launching RaidLedger.'
}
if (-not $health) {
    $serverPath = if ($env:RAIDLEDGER_EXE) { $env:RAIDLEDGER_EXE } else { Join-Path $localRoot 'raidledger.exe' }
    if (-not (Test-Path -LiteralPath $serverPath)) {
        $serverPath = Join-Path $localRoot 'target\release\raidledger.exe'
    }
    if (-not (Test-Path -LiteralPath $serverPath)) {
        throw 'RaidLedger executable not found. Copy raidledger.exe here or build with cargo build --locked --release --jobs 8.'
    }
    $localSevenZip = Join-Path $localRoot '7z.exe'
    if ((Test-Path -LiteralPath $localSevenZip) -and -not $env:RAIDLEDGER_7Z -and -not $env:UWU_7Z) {
        $env:RAIDLEDGER_7Z = $localSevenZip
    }
    $process = Start-Process -FilePath $serverPath -WorkingDirectory $localRoot -WindowStyle Hidden -RedirectStandardOutput (Join-Path $localRoot 'local-server.out.log') -RedirectStandardError (Join-Path $localRoot 'local-server.err.log') -PassThru
    $process.Id | Set-Content (Join-Path $localRoot 'local-server.pid')
    for ($attempt = 0; $attempt -lt 30; $attempt++) {
        Start-Sleep -Milliseconds 500
        $health = Get-LocalRaidLedger
        if ($health.app -eq 'RaidLedger' -or $process.HasExited) { break }
    }
}
if ($health.app -ne 'RaidLedger') { throw 'RaidLedger could not start. See local-server.err.log.' }
if (-not $NoBrowser) { Start-Process ($reportsBase + '/logs_list') }
