# Norrix dev capture

These scripts are opt-in. They do not change the normal `bun run dev` command or the Tauri startup configuration.

## One reproduction

Use the same Windows account for both PowerShell windows.

1. Open **PowerShell as Administrator** and start the Kernel-Process ETW session:

   ```powershell
   $root = 'C:\Users\ZEF_L\Downloads\koharu-0.83.5 (1)\koharu-0.83.5'
   & "$root\scripts\diagnostics\Start-NorrixEtw.ps1"
   ```

   Continue only if it prints that the session is active and gives a non-empty ETL path. Copy the `OutputDirectory` shown by the script. The application is not started in this elevated window.

2. Open a **normal, non-elevated PowerShell** in the same account and run the capture script once, using that exact directory:

   ```powershell
   $root = 'C:\Users\ZEF_L\Downloads\koharu-0.83.5 (1)\koharu-0.83.5'
   & "$root\scripts\diagnostics\Capture-NorrixDev.ps1" -OutputDirectory 'PASTE_OUTPUT_DIRECTORY_HERE'
   ```

   This launches `bun run dev` itself. Do not start a second Bun/Tauri instance. Leave this PowerShell open. Wait for a spontaneous failure; if the frontend fails but Norrix remains open, close the Norrix window deliberately and let the capture script finish. Do not press Ctrl+C.

3. After the capture script returns, stop and decode ETW from the **elevated PowerShell**:

   ```powershell
   & "$root\scripts\diagnostics\Stop-NorrixEtw.ps1" -OutputDirectory 'PASTE_OUTPUT_DIRECTORY_HERE'
   ```

   Continue only if it reports a stopped ETW session, a non-empty `processes.etl`, and a non-empty `processes.xml`.

If the normal-user capture script refuses to start because the ETW session is inactive, do not reproduce; restart the elevated ETW session first. If capture ends unexpectedly while the app still runs, close Norrix normally before stopping ETW.

## Capture files

- `processes.etl` and `processes.xml`: Windows process-start/stop events, including PIDs, parent PIDs, command lines, and process exit status.
- `observed-processes.jsonl`: timestamped process snapshots with executable, command line, and creation time for descendants observed during the run. ETW remains authoritative for short-lived processes and exact start/stop times.
- `capture.jsonl`, `etw-control.jsonl`: capture setup, active-session validation, root Bun PID/exit code, and ETW stop/verification records.
- `bun-run-dev.stdout.log`, `bun-run-dev.stderr.log`: complete line-oriented stdout/stderr received by the capture process. Bun's parallel-job prefixes are preserved; the timestamped files and `streams-timeline.log` add arrival times and stream labels.
- `port-3000.jsonl`: listener state before `predev`, state when the `kill-port` message is observed, and subsequent listener changes.
- `window-events.jsonl`: native Norrix window create/destroy/show/hide, foreground, and minimize events with PID, HWND, title, and timestamp.
- `node-failures.jsonl` and `node-reports\`: Node uncaught-exception notifications and Node fatal/uncaught reports. Diagnostic options are passed only to this run; the monitor does not register an `unhandledRejection` handler or suppress the normal non-zero exit.

In `window-events.jsonl`, event codes `8000`–`8003` are create, destroy, show, and hide; `0003` is foreground; `0016`/`0017` are minimize start/end. ETW process-stop events contain the reported exit status; correlate them by PID with the observed command lines and package labels.

Kernel-process ETW is system-wide and may include command lines from unrelated processes. The capture stays local; review and redact it before sharing. ETW does not directly identify the caller of `TerminateProcess`. Use event order and exit status together with Bun/Tauri output to distinguish a process's own failure from shutdown of its parent tree. Do not infer the cause solely from a child being signaled.
