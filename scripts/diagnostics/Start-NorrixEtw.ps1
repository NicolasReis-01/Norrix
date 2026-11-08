param(
  [string]$OutputDirectory = (Join-Path $env:LOCALAPPDATA ("Norrix\diagnostics\norrix-dev-" + (Get-Date -Format 'yyyyMMdd-HHmmss')))
)

$ErrorActionPreference = 'Stop'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'Kernel process ETW requires an elevated PowerShell. No application was started.'
}

New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path
if ((Get-ChildItem -LiteralPath $OutputDirectory -Force | Measure-Object).Count -gt 0) {
  throw "OutputDirectory must be empty to avoid overwriting a previous capture: $OutputDirectory"
}

$sessionName = 'NorrixDiag_' + (Get-Date -Format 'yyyyMMdd_HHmmss') + "_$PID"
$etlPath = Join-Path $OutputDirectory 'processes.etl'
$sessionPath = Join-Path $OutputDirectory 'etw-session.txt'
$metadataPath = Join-Path $OutputDirectory 'etw-control.jsonl'

function Write-ControlEvent {
  param([string]$Kind, [hashtable]$Fields = @{})
  $record = @{ timestamp = [DateTimeOffset]::Now.ToString('o'); kind = $Kind }
  foreach ($key in $Fields.Keys) { $record[$key] = $Fields[$key] }
  $json = $record | ConvertTo-Json -Compress -Depth 6
  [System.IO.File]::AppendAllText($metadataPath, $json + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
}

$startAttempted = $false
try {
  [System.IO.File]::WriteAllText($sessionPath, $sessionName, [System.Text.UTF8Encoding]::new($false))
  Write-ControlEvent -Kind 'etw-start-request' -Fields @{
    session = $sessionName
    provider = 'Microsoft-Windows-Kernel-Process'
    keywordMask = '0x10'
    level = 4
    etlPath = $etlPath
  }

  $startAttempted = $true
  $startOutput = & logman.exe start $sessionName -p 'Microsoft-Windows-Kernel-Process' 0x10 4 -o $etlPath -ets 2>&1
  if ($LASTEXITCODE -ne 0) {
    throw "logman could not start the process trace (exit $LASTEXITCODE): $($startOutput -join ' ')"
  }
  $queryOutput = & logman.exe query $sessionName -ets 2>&1
  if ($LASTEXITCODE -ne 0 -or ($queryOutput -join "`n") -notmatch [regex]::Escape($sessionName)) {
    throw "ETW session did not validate as active: $($queryOutput -join ' ')"
  }
  Start-Sleep -Milliseconds 500
  if (-not (Test-Path -LiteralPath $etlPath) -or (Get-Item -LiteralPath $etlPath).Length -le 0) {
    throw "ETW session is active but no non-empty ETL exists: $etlPath"
  }

  Write-ControlEvent -Kind 'etw-active' -Fields @{
    session = $sessionName
    query = ($queryOutput -join ' ')
    etlBytes = (Get-Item -LiteralPath $etlPath).Length
  }
  Write-Host "ETW session validated as active: $sessionName"
  Write-Host "Output directory: $OutputDirectory"
  Write-Host "ETL: $etlPath"
  Write-Host "Run Capture-NorrixDev.ps1 as the normal user with -OutputDirectory `"$OutputDirectory`"."
  Write-Host "After that script completes, run Stop-NorrixEtw.ps1 as Administrator with the same -OutputDirectory."
}
catch {
  Write-ControlEvent -Kind 'etw-start-failed' -Fields @{ error = $_.Exception.Message }
  if ($startAttempted) {
    $cleanupQuery = & logman.exe query $sessionName -ets 2>&1
    if ($LASTEXITCODE -eq 0 -and ($cleanupQuery -join "`n") -match [regex]::Escape($sessionName)) {
      & logman.exe stop $sessionName -ets 2>&1 | Out-Null
    }
  }
  throw
}
