param([switch]$NoBrowser)
& (Join-Path $PSScriptRoot 'Start-RaidLedger.ps1') -NoBrowser:$NoBrowser
