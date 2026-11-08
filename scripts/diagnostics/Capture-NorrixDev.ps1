param(
  [string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path,
  [Parameter(Mandatory = $true)]
  [string]$OutputDirectory,
  [string]$BunPath,
  [switch]$ValidateOnly
)

$ErrorActionPreference = 'Stop'
$RepositoryRoot = (Resolve-Path -LiteralPath $RepositoryRoot).Path
$tauriConfig = Join-Path $RepositoryRoot 'crates\norrix\tauri.conf.json'
if (-not (Test-Path -LiteralPath $tauriConfig)) {
  throw "RepositoryRoot does not appear to be Norrix: $RepositoryRoot"
}

if (-not $BunPath) {
  $bunCommand = Get-Command bun.cmd -ErrorAction SilentlyContinue
  if ($bunCommand) {
    $BunPath = Join-Path (Split-Path $bunCommand.Source -Parent) 'node_modules\bun\bin\bun.exe'
  }
}
if (-not $BunPath -or -not (Test-Path -LiteralPath $BunPath)) {
  throw 'Could not find bun.exe. Pass -BunPath with the full path to bun.exe.'
}
$BunPath = (Resolve-Path -LiteralPath $BunPath).Path

$runDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path
$nodeReports = Join-Path $runDirectory 'node-reports'
New-Item -ItemType Directory -Path $nodeReports -Force | Out-Null

$etlPath = Join-Path $runDirectory 'processes.etl'
$xmlPath = Join-Path $runDirectory 'processes.xml'
$sessionFile = Join-Path $runDirectory 'etw-session.txt'
if (-not (Test-Path -LiteralPath $sessionFile)) {
  throw "ETW session metadata is missing. Run Start-NorrixEtw.ps1 first: $sessionFile"
}
$sessionName = (Get-Content -LiteralPath $sessionFile -Raw).Trim()
if (-not $sessionName) { throw "ETW session metadata is empty: $sessionFile" }
$observedProcessesPath = Join-Path $runDirectory 'observed-processes.jsonl'
$windowEventsPath = Join-Path $runDirectory 'window-events.jsonl'
$portEventsPath = Join-Path $runDirectory 'port-3000.jsonl'
$metadataPath = Join-Path $runDirectory 'capture.jsonl'
$stdoutPath = Join-Path $runDirectory 'bun-run-dev.stdout.log'
$stderrPath = Join-Path $runDirectory 'bun-run-dev.stderr.log'
$stdoutTimelinePath = Join-Path $runDirectory 'stdout-timestamps.log'
$stderrTimelinePath = Join-Path $runDirectory 'stderr-timestamps.log'
$streamTimelinePath = Join-Path $runDirectory 'streams-timeline.log'
$nodeEventsPath = Join-Path $runDirectory 'node-failures.jsonl'
$nodePreload = Join-Path $PSScriptRoot 'NodeDiagnostics.cjs'
$etwValidated = $false

$script:jsonLock = New-Object object
function Write-JsonLine {
  param([string]$Path, [object]$Value)
  $line = $Value | ConvertTo-Json -Compress -Depth 8
  [System.Threading.Monitor]::Enter($script:jsonLock)
  try {
    [System.IO.File]::AppendAllText($Path, $line + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
  }
  finally {
    [System.Threading.Monitor]::Exit($script:jsonLock)
  }
}

function Write-CaptureEvent {
  param([string]$Kind, [hashtable]$Fields = @{})
  $record = @{ timestamp = [DateTimeOffset]::Now.ToString('o'); kind = $Kind }
  foreach ($key in $Fields.Keys) { $record[$key] = $Fields[$key] }
  Write-JsonLine -Path $metadataPath -Value $record
}

function Get-Port3000State {
  $connections = @(Get-NetTCPConnection -LocalPort 3000 -State Listen -ErrorAction SilentlyContinue)
  $listeners = foreach ($connection in $connections) {
    $owner = Get-CimInstance Win32_Process -Filter "ProcessId=$($connection.OwningProcess)" -ErrorAction SilentlyContinue
    [ordered]@{
      address = $connection.LocalAddress
      port = $connection.LocalPort
      pid = $connection.OwningProcess
      executable = $owner.ExecutablePath
      commandLine = $owner.CommandLine
    }
  }
  @($listeners | ConvertTo-Json -Compress -Depth 5)
}

function Update-TrackedProcessTree {
  $snapshot = @(Get-CimInstance Win32_Process)
  $byId = @{}
  foreach ($entry in $snapshot) { $byId[[int]$entry.ProcessId] = $entry }

  foreach ($entry in $snapshot) {
    $processId = [int]$entry.ProcessId
    $parentProcessId = [int]$entry.ParentProcessId
    if ($script:trackedPids.Contains($parentProcessId) -and $script:trackedPids.Add($processId)) {
      $creationTime = $null
      try { $creationTime = [Management.ManagementDateTimeConverter]::ToDateTime($entry.CreationDate).ToString('o') } catch {}
      Write-JsonLine -Path $observedProcessesPath -Value @{
        observedAt = [DateTimeOffset]::Now.ToString('o')
        processCreationTime = $creationTime
        pid = $processId
        ppid = $parentProcessId
        executable = $entry.ExecutablePath
        commandLine = $entry.CommandLine
      }
    }
  }

  foreach ($processId in @($script:trackedPids)) {
    if (-not $byId.ContainsKey($processId)) { $script:trackedPids.Remove($processId) | Out-Null }
  }
}

$winEventSource = @'
using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public sealed class NorrixWinEventWatcher : IDisposable
{
    private const uint EventSystemForeground = 0x0003;
    private const uint EventSystemMinimizeStart = 0x0016;
    private const uint EventSystemMinimizeEnd = 0x0017;
    private const uint EventObjectCreate = 0x8000;
    private const uint EventObjectDestroy = 0x8001;
    private const uint EventObjectShow = 0x8002;
    private const uint EventObjectHide = 0x8003;
    private const uint WineventOutOfContext = 0x0000;
    private const uint WmQuit = 0x0012;

    [StructLayout(LayoutKind.Sequential)]
    private struct Point { public int X; public int Y; }
    [StructLayout(LayoutKind.Sequential)]
    private struct Message
    {
        public IntPtr HWnd;
        public uint Value;
        public UIntPtr WParam;
        public IntPtr LParam;
        public uint Time;
        public Point Point;
    }

    private delegate void WinEventDelegate(IntPtr hook, uint eventType, IntPtr hwnd,
        int objectId, int childId, uint eventThread, uint eventTime);
    [DllImport("user32.dll", SetLastError = true)]
    private static extern IntPtr SetWinEventHook(uint eventMin, uint eventMax, IntPtr module,
        WinEventDelegate callback, uint processId, uint threadId, uint flags);
    [DllImport("user32.dll")]
    private static extern bool UnhookWinEvent(IntPtr hook);
    [DllImport("user32.dll", SetLastError = true)]
    private static extern int GetMessage(out Message message, IntPtr hwnd, uint min, uint max);
    [DllImport("user32.dll")]
    private static extern bool TranslateMessage(ref Message message);
    [DllImport("user32.dll")]
    private static extern IntPtr DispatchMessage(ref Message message);
    [DllImport("user32.dll")]
    private static extern bool PostThreadMessage(uint threadId, uint message, UIntPtr wParam, IntPtr lParam);
    [DllImport("kernel32.dll")]
    private static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int maxCount);

    private readonly string path;
    private readonly object fileLock = new object();
    private readonly ManualResetEvent ready = new ManualResetEvent(false);
    private readonly WinEventDelegate callback;
    private readonly Thread thread;
    private readonly IntPtr[] hooks = new IntPtr[7];
    private uint threadId;
    private Exception startupError;

    public NorrixWinEventWatcher(string outputPath)
    {
        path = outputPath;
        callback = OnEvent;
        thread = new Thread(Run) { IsBackground = true, Name = "NorrixWinEventWatcher" };
        thread.Start();
        ready.WaitOne();
        if (startupError != null) throw new InvalidOperationException("Could not start WinEvent hooks", startupError);
    }

    private void Run()
    {
        threadId = GetCurrentThreadId();
        uint[] events = { EventSystemForeground, EventSystemMinimizeStart, EventSystemMinimizeEnd,
                          EventObjectCreate, EventObjectDestroy, EventObjectShow, EventObjectHide };
        uint[] ends = { EventSystemForeground, EventSystemMinimizeStart, EventSystemMinimizeEnd,
                        EventObjectCreate, EventObjectDestroy, EventObjectShow, EventObjectHide };
        try
        {
            for (int i = 0; i < events.Length; i++)
            {
                hooks[i] = SetWinEventHook(events[i], ends[i], IntPtr.Zero, callback, 0, 0, WineventOutOfContext);
                if (hooks[i] == IntPtr.Zero) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
            }
        }
        catch (Exception error) { startupError = error; ready.Set(); return; }
        ready.Set();
        Message message;
        while (GetMessage(out message, IntPtr.Zero, 0, 0) > 0)
        {
            TranslateMessage(ref message);
            DispatchMessage(ref message);
        }
        foreach (IntPtr hook in hooks) if (hook != IntPtr.Zero) UnhookWinEvent(hook);
    }

    private void OnEvent(IntPtr hook, uint eventType, IntPtr hwnd, int objectId, int childId,
        uint eventThread, uint eventTime)
    {
        if (hwnd == IntPtr.Zero) return;
        uint pid;
        GetWindowThreadProcessId(hwnd, out pid);
        try
        {
            if (!String.Equals(Process.GetProcessById((int)pid).ProcessName, "norrix",
                StringComparison.OrdinalIgnoreCase)) return;
        }
        catch { return; }

        var title = new StringBuilder(512);
        GetWindowText(hwnd, title, title.Capacity);
        var record = String.Format(
            "{{\"timestamp\":\"{0}\",\"event\":\"{1}\",\"pid\":{2},\"hwnd\":\"0x{3:X}\",\"objectId\":{4},\"childId\":{5},\"title\":\"{6}\"}}",
            DateTimeOffset.Now.ToString("o"), eventType.ToString("X4"), pid, hwnd.ToInt64(),
            objectId, childId, Escape(title.ToString()));
        lock (fileLock) File.AppendAllText(path, record + Environment.NewLine, new UTF8Encoding(false));
    }

    private static string Escape(string value)
    {
        return value.Replace("\\", "\\\\").Replace("\"", "\\\"")
            .Replace("\r", "\\r").Replace("\n", "\\n");
    }

    public void Dispose()
    {
        if (threadId != 0) PostThreadMessage(threadId, WmQuit, UIntPtr.Zero, IntPtr.Zero);
        if (thread.IsAlive) thread.Join(3000);
        ready.Dispose();
    }
}
'@

$windowWatcher = $null
$script:trackedPids = [System.Collections.Generic.HashSet[int]]::new()
$predevMarkers = [System.Collections.Concurrent.ConcurrentQueue[string]]::new()
$process = $null
$oldNodeOptions = $env:NODE_OPTIONS
$oldNodeLog = $env:NORRIX_DIAG_NODE_LOG

try {
  foreach ($path in @($observedProcessesPath, $windowEventsPath, $portEventsPath, $metadataPath, $stdoutPath, $stderrPath, $stdoutTimelinePath, $stderrTimelinePath, $streamTimelinePath, $nodeEventsPath)) {
    [System.IO.File]::WriteAllText($path, '', [System.Text.UTF8Encoding]::new($false))
  }

  $portStateBeforePredev = Get-Port3000State
  Write-CaptureEvent -Kind 'capture-start' -Fields @{
    repositoryRoot = $RepositoryRoot
    command = 'bun run dev'
    bunExecutable = $BunPath
    outputDirectory = $runDirectory
    powershellPid = $PID
    nodeOptions = '--require NodeDiagnostics.cjs --trace-uncaught --report-uncaught-exception --report-on-fatalerror'
    port3000BeforePredev = $portStateBeforePredev
  }

  $queryOutput = & logman.exe query $sessionName -ets 2>&1
  if ($LASTEXITCODE -ne 0 -or ($queryOutput -join "`n") -notmatch [regex]::Escape($sessionName)) {
    throw "ETW session is not active. Stop and start it again with Start-NorrixEtw.ps1: $($queryOutput -join ' ')"
  }
  if (-not (Test-Path -LiteralPath $etlPath) -or (Get-Item -LiteralPath $etlPath).Length -le 0) {
    throw "ETW session is active but its ETL is missing or empty: $etlPath"
  }
  Write-CaptureEvent -Kind 'etw-attached' -Fields @{
    session = $sessionName
    provider = 'Microsoft-Windows-Kernel-Process'
    keywordMask = '0x10'
    level = 4
    query = ($queryOutput -join ' ')
    etlPath = $etlPath
    etlBytesAtStart = (Get-Item -LiteralPath $etlPath).Length
  }
  $etwValidated = $true

  Add-Type -TypeDefinition $winEventSource
  $windowWatcher = [NorrixWinEventWatcher]::new($windowEventsPath)
  Write-CaptureEvent -Kind 'window-hooks-active' -Fields @{ events = @('foreground', 'minimize-start', 'minimize-end', 'window-create', 'window-destroy', 'window-show', 'window-hide') }

  if ($ValidateOnly) {
    Write-CaptureEvent -Kind 'validate-only' -Fields @{ message = 'ETW session and window hooks validated; no application was started.' }
    return
  }

  $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
  $startInfo.FileName = $BunPath
  $startInfo.Arguments = 'run dev'
  $startInfo.WorkingDirectory = $RepositoryRoot
  $startInfo.UseShellExecute = $false
  $startInfo.CreateNoWindow = $false
  $startInfo.RedirectStandardOutput = $true
  $startInfo.RedirectStandardError = $true
  $startInfo.StandardOutputEncoding = [System.Text.UTF8Encoding]::new($false)
  $startInfo.StandardErrorEncoding = [System.Text.UTF8Encoding]::new($false)
  $env:NORRIX_DIAG_NODE_LOG = $nodeEventsPath
  $escapedPreload = $nodePreload.Replace('\', '\\')
  $escapedReportDirectory = $nodeReports.Replace('\', '\\')
  $nodeFlags = "--require=`"$escapedPreload`" --trace-uncaught --report-uncaught-exception --report-on-fatalerror --report-directory=`"$escapedReportDirectory`""
  $env:NODE_OPTIONS = (($oldNodeOptions + ' ' + $nodeFlags).Trim())

  $process = [System.Diagnostics.Process]::new()
  $process.StartInfo = $startInfo
  $process.EnableRaisingEvents = $true
  $stdoutWriter = [System.IO.StreamWriter]::new($stdoutPath, $true, [System.Text.UTF8Encoding]::new($false))
  $stderrWriter = [System.IO.StreamWriter]::new($stderrPath, $true, [System.Text.UTF8Encoding]::new($false))
  $stdoutTimelineWriter = [System.IO.StreamWriter]::new($stdoutTimelinePath, $true, [System.Text.UTF8Encoding]::new($false))
  $stderrTimelineWriter = [System.IO.StreamWriter]::new($stderrTimelinePath, $true, [System.Text.UTF8Encoding]::new($false))
  $streamTimelineWriter = [System.IO.StreamWriter]::new($streamTimelinePath, $true, [System.Text.UTF8Encoding]::new($false))
  foreach ($writer in @($stdoutWriter, $stderrWriter, $stdoutTimelineWriter, $stderrTimelineWriter, $streamTimelineWriter)) {
    $writer.AutoFlush = $true
  }
  $streamLock = New-Object object
  $process.add_OutputDataReceived({
    param($sender, $eventArgs)
    if ($null -ne $eventArgs.Data) {
      $now = [DateTimeOffset]::Now.ToString('o')
      [System.Threading.Monitor]::Enter($streamLock)
      try {
        $stdoutWriter.WriteLine($eventArgs.Data)
        $stdoutTimelineWriter.WriteLine("[$now] $($eventArgs.Data)")
        $streamTimelineWriter.WriteLine("[$now] [stdout] $($eventArgs.Data)")
        [Console]::Out.WriteLine($eventArgs.Data)
        if ($eventArgs.Data -match 'Process on port 3000 killed') { $predevMarkers.Enqueue($now) }
      } finally { [System.Threading.Monitor]::Exit($streamLock) }
    }
  }.GetNewClosure())
  $process.add_ErrorDataReceived({
    param($sender, $eventArgs)
    if ($null -ne $eventArgs.Data) {
      $now = [DateTimeOffset]::Now.ToString('o')
      [System.Threading.Monitor]::Enter($streamLock)
      try {
        $stderrWriter.WriteLine($eventArgs.Data)
        $stderrTimelineWriter.WriteLine("[$now] $($eventArgs.Data)")
        $streamTimelineWriter.WriteLine("[$now] [stderr] $($eventArgs.Data)")
        [Console]::Error.WriteLine($eventArgs.Data)
      } finally { [System.Threading.Monitor]::Exit($streamLock) }
    }
  }.GetNewClosure())

  if (-not $process.Start()) { throw 'Failed to start bun run dev.' }
  $script:trackedPids.Clear()
  $script:trackedPids.Add($process.Id) | Out-Null
  $rootCim = Get-CimInstance Win32_Process -Filter "ProcessId=$($process.Id)" -ErrorAction SilentlyContinue
  Write-CaptureEvent -Kind 'root-process-start' -Fields @{
    pid = $process.Id
    ppid = $PID
    executable = $rootCim.ExecutablePath
    commandLine = $rootCim.CommandLine
    processCreationTime = $process.StartTime.ToUniversalTime().ToString('o')
    port3000AtRootStart = (Get-Port3000State)
  }
  $process.BeginOutputReadLine()
  $process.BeginErrorReadLine()
  Write-Host "Capturing Norrix dev run. Output: $runDirectory"
  Write-Host 'Keep this PowerShell window open. Close Norrix deliberately to stop normally; do not press Ctrl+C.'

  $previousPortState = $portStateBeforePredev -join ''
  $lastPortSample = [DateTime]::MinValue
  $lastProcessSample = [DateTime]::MinValue
  while (-not $process.HasExited) {
    $predevMarker = $null
    while ($predevMarkers.TryDequeue([ref]$predevMarker)) {
      Write-CaptureEvent -Kind 'predev-port-cleanup-reported' -Fields @{
        markerTimestamp = $predevMarker
        port3000 = (Get-Port3000State)
      }
      $predevMarker = $null
    }
    $now = Get-Date
    if (($now - $lastPortSample).TotalMilliseconds -ge 250) {
      $portState = (Get-Port3000State) -join ''
      if ($portState -ne $previousPortState) {
        Write-JsonLine -Path $portEventsPath -Value @{
          timestamp = [DateTimeOffset]::Now.ToString('o')
          pid = $process.Id
          listeners = $portState
        }
        $previousPortState = $portState
      }
      $lastPortSample = $now
    }
    if (($now - $lastProcessSample).TotalMilliseconds -ge 1000) {
      Update-TrackedProcessTree
      $lastProcessSample = $now
    }
    Start-Sleep -Milliseconds 100
    $process.Refresh()
  }
  $process.WaitForExit()
  Start-Sleep -Seconds 2
  $process.Refresh()
  Write-CaptureEvent -Kind 'root-process-stop' -Fields @{
    pid = $process.Id
    exitCode = $process.ExitCode
    exitedAt = [DateTimeOffset]::Now.ToString('o')
    classification = if ($process.ExitCode -eq 0) { 'normal-zero-exit; inspect window/process timeline to determine deliberate close' } else { 'non-zero-exit; compare process events and Tauri output to identify initiating process' }
  }

  Write-Host 'The bun run dev process ended. Waiting for tracked descendants to exit before stopping ETW.'
  while ($true) {
    Update-TrackedProcessTree
    if ($script:trackedPids.Count -eq 0) { break }
    Start-Sleep -Milliseconds 250
  }
  Write-CaptureEvent -Kind 'all-tracked-processes-stopped'
}
finally {
  if ($windowWatcher) { $windowWatcher.Dispose() }

  Write-CaptureEvent -Kind 'capture-script-stopped' -Fields @{ etwSessionLeftRunning = $etwValidated }

  if ($process) {
    foreach ($writerName in @('stdoutWriter', 'stderrWriter', 'stdoutTimelineWriter', 'stderrTimelineWriter', 'streamTimelineWriter')) {
      $writer = Get-Variable -Name $writerName -ValueOnly -ErrorAction SilentlyContinue
      if ($writer) { $writer.Dispose() }
    }
    $process.Dispose()
  }
  $env:NODE_OPTIONS = $oldNodeOptions
  $env:NORRIX_DIAG_NODE_LOG = $oldNodeLog
  Write-Host "Diagnostic files: $runDirectory"
  if ($etwValidated) {
    Write-Host "ETW is still recording. After this capture ends, run Stop-NorrixEtw.ps1 as Administrator with -OutputDirectory `"$runDirectory`"."
  }
}
