param(
  [Parameter(Mandatory = $true)]
  [string]$OutputDirectory
)

$ErrorActionPreference = 'Stop'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'Stopping the Kernel-Process ETW session requires an elevated PowerShell.'
}

$OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path
$sessionPath = Join-Path $OutputDirectory 'etw-session.txt'
$etlPath = Join-Path $OutputDirectory 'processes.etl'
$xmlPath = Join-Path $OutputDirectory 'processes.xml'
$metadataPath = Join-Path $OutputDirectory 'etw-control.jsonl'
if (-not (Test-Path -LiteralPath $sessionPath)) { throw "ETW session file not found: $sessionPath" }
$sessionName = (Get-Content -LiteralPath $sessionPath -Raw).Trim()
if (-not $sessionName) { throw "ETW session file is empty: $sessionPath" }

function Write-ControlEvent {
  param([string]$Kind, [hashtable]$Fields = @{})
  $record = @{ timestamp = [DateTimeOffset]::Now.ToString('o'); kind = $Kind }
  foreach ($key in $Fields.Keys) { $record[$key] = $Fields[$key] }
  $json = $record | ConvertTo-Json -Compress -Depth 6
  [System.IO.File]::AppendAllText($metadataPath, $json + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
}

$queryOutput = & logman.exe query $sessionName -ets 2>&1
if ($LASTEXITCODE -ne 0 -or ($queryOutput -join "`n") -notmatch [regex]::Escape($sessionName)) {
  throw "The recorded ETW session is not active; refusing to treat an unflushed ETL as complete: $($queryOutput -join ' ')"
}

$stopOutput = & logman.exe stop $sessionName -ets 2>&1
$stopCode = $LASTEXITCODE
Write-ControlEvent -Kind 'etw-stop' -Fields @{ exitCode = $stopCode; output = ($stopOutput -join ' ') }
if ($stopCode -ne 0) { throw "logman could not stop ETW session $sessionName (exit $stopCode): $($stopOutput -join ' ')" }

if (-not (Test-Path -LiteralPath $etlPath)) { throw "ETL file was not created: $etlPath" }
$etlBytes = (Get-Item -LiteralPath $etlPath).Length
if ($etlBytes -le 0) { throw "ETL file is empty: $etlPath" }
Write-ControlEvent -Kind 'etl-verified' -Fields @{ path = $etlPath; bytes = $etlBytes }

$reportOutput = & tracerpt.exe $etlPath -o $xmlPath -of XML -y 2>&1
$reportCode = $LASTEXITCODE
Write-ControlEvent -Kind 'etl-converted' -Fields @{ exitCode = $reportCode; xmlPath = $xmlPath; output = ($reportOutput -join ' ') }
if ($reportCode -ne 0 -or -not (Test-Path -LiteralPath $xmlPath) -or (Get-Item -LiteralPath $xmlPath).Length -le 0) {
  throw "tracerpt failed to produce a non-empty process event XML (exit $reportCode): $($reportOutput -join ' ')"
}

Write-Host "ETW session stopped and verified."
Write-Host "ETL ($etlBytes bytes): $etlPath"
Write-Host "Decoded events: $xmlPath"
