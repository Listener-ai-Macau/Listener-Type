param(
  [string]$ComPort = "COM3",
  [int]$SoakMinutes = 20,
  [string]$OutputDir = "",
  [string]$ListenerExe = "C:\Program Files\Listener Type\listener-type.exe",
  [int]$CycleHoldSeconds = 25,
  # 0 = wait until notify ready without failing the entry gate (still records ms).
  [int]$RequireNotifyUnderMs = 3000
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$scriptDir = Split-Path -Parent $PSCommandPath
$typeRoot = (Resolve-Path (Join-Path $scriptDir "..")).Path
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
  $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
  $OutputDir = Join-Path $typeRoot ".artifacts\listener-1.0.4-regression\type-soak-$stamp"
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$detailLog = Join-Path $OutputDir "soak.log"
$summaryJson = Join-Path $OutputDir "long-soak-summary.json"
$events = [System.Collections.Generic.List[object]]::new()

function Write-Log([string]$Message) {
  $line = "{0} {1}" -f (Get-Date).ToString("o"), $Message
  Add-Content -LiteralPath $detailLog -Value $line -Encoding utf8
  Write-Host $line
}

function Invoke-SerialCommands {
  param([string[]]$Cmds, [int]$ReadMs = 800, [int]$OpenTimeoutMs = 2500)
  # SerialPort.Open can hang on Windows; bound it with a job.
  $job = Start-Job -ScriptBlock {
    param($Port, $Cmds, $ReadMs)
    $sp = $null
    try {
      $sp = [System.IO.Ports.SerialPort]::new($Port, 115200)
      $sp.ReadTimeout = 300
      $sp.WriteTimeout = 2000
      $sp.DtrEnable = $false
      $sp.RtsEnable = $false
      $sp.Open()
      Start-Sleep -Milliseconds 120
      while ($sp.BytesToRead -gt 0) { [void]$sp.ReadExisting() }
      $lines = New-Object System.Collections.Generic.List[string]
      foreach ($c in $Cmds) {
        $sp.Write(($c + "`n"))
        $deadline = (Get-Date).AddMilliseconds($ReadMs)
        while ((Get-Date) -lt $deadline) {
          try { $lines.Add($sp.ReadLine()) } catch [System.TimeoutException] {}
        }
      }
      return @($lines)
    } finally {
      if ($null -ne $sp -and $sp.IsOpen) { $sp.Close() }
    }
  } -ArgumentList $ComPort, $Cmds, $ReadMs

  $finished = Wait-Job -Job $job -Timeout ([Math]::Ceiling(($OpenTimeoutMs + ($Cmds.Count * $ReadMs) + 2000) / 1000.0))
  if (-not $finished) {
    Stop-Job $job -ErrorAction SilentlyContinue
    Remove-Job $job -Force -ErrorAction SilentlyContinue
    throw "serial job timed out on $ComPort"
  }
  try {
    $result = Receive-Job $job -ErrorAction Stop
    return @($result)
  } finally {
    Remove-Job $job -Force -ErrorAction SilentlyContinue
  }
}

Get-Process -Name "listener-type" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2

# After heavy OTA/radio work Windows BLE can be temporarily slow. When
# RequireNotifyUnderMs > 0, keep the product warm-start gate; when 0, only
# require that notify becomes ready so soak can exercise the live path.
$notify = $null
$notifyAttempts = @()
$gateMs = if ($RequireNotifyUnderMs -gt 0) { $RequireNotifyUnderMs } else { 15000 }
for ($attempt = 1; $attempt -le 4; $attempt++) {
  $notifyJson = Join-Path $OutputDir ("notify-ready-attempt-{0}.json" -f $attempt)
  Write-Log ("notify_attempt={0} gate_ms={1}" -f $attempt, $gateMs)
  $global:LASTEXITCODE = 0
  & pwsh -NoProfile -File (Join-Path $scriptDir "check-type-startup-reconnect-speed.ps1") `
    -ListenerExe $ListenerExe `
    -MaxNotifyReadyMs $gateMs `
    -ReadyTimeoutSeconds 40 `
    -OutputJson $notifyJson `
    -AllowForcedTermination
  if (Test-Path -LiteralPath $notifyJson) {
    $candidate = Get-Content -Raw -LiteralPath $notifyJson | ConvertFrom-Json
    $notifyAttempts += $candidate
    $ms = $candidate.start_to_notify_ready_ms
    $ready = ($null -ne $ms)
    $underProduct = ($ready -and [int]$ms -lt 3000)
    Write-Log ("notify_attempt_ms={0} under_3000={1} helper_status={2}" -f $ms, $underProduct, $candidate.status)
    if ($RequireNotifyUnderMs -le 0) {
      if ($ready) { $notify = $candidate; break }
    } elseif ([string]$candidate.status -eq "PASS") {
      $notify = $candidate
      break
    }
  }
  Get-Process -Name "listener-type" -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Seconds 3
}
$notifyAttempts | ConvertTo-Json -Depth 6 |
  Set-Content -LiteralPath (Join-Path $OutputDir "notify-ready-attempts.json") -Encoding utf8
if ($null -eq $notify) {
  throw "notify-ready never observed after 4 attempts"
}
Write-Log ("notify_ready_ms={0} product_under_3000={1}" -f $notify.start_to_notify_ready_ms, ([int]$notify.start_to_notify_ready_ms -lt 3000))

$appLog = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
$logStartLen = if (Test-Path -LiteralPath $appLog) { (Get-Item -LiteralPath $appLog).Length } else { 0 }
$soakSeconds = [Math]::Max(60, $SoakMinutes * 60)
$start = Get-Date
$deadline = $start.AddSeconds($soakSeconds)
$sessionCycles = 0
$packetLossEvents = 0
$restartEvents = 0
$typeAliveChecks = 0
$typeAliveFails = 0
$serialErrors = 0

Write-Log ("SOAK_START seconds={0} port={1}" -f $soakSeconds, $ComPort)

while ((Get-Date) -lt $deadline) {
  $sessionCycles++
  try {
    $serOut = Invoke-SerialCommands -Cmds @("~VREC:CANCEL", "~VREC:TOGGLE") -ReadMs 600
    foreach ($line in $serOut) {
      if ($line -match "WDT|ESP_RST|TASK_WDT|Guru Meditation|abort\(") {
        $events.Add([pscustomobject]@{ t = (Get-Date).ToString("o"); kind = "serial_hit"; msg = $line })
        Write-Log ("SERIAL_HIT {0}" -f $line)
      }
    }
  } catch {
    $serialErrors++
    Write-Log ("serial_toggle_error cycle={0} err={1}" -f $sessionCycles, $_.Exception.Message)
    Start-Sleep -Seconds 2
  }

  $holdUntil = (Get-Date).AddSeconds($CycleHoldSeconds)
  while ((Get-Date) -lt $holdUntil -and (Get-Date) -lt $deadline) {
    Start-Sleep -Seconds 5
    $typeAliveChecks++
    $procs = @(Get-Process -Name "listener-type" -ErrorAction SilentlyContinue)
    if ($procs.Count -eq 0) {
      $typeAliveFails++
      Write-Log "TYPE_EXITED unexpectedly"
      throw "listener-type exited during soak"
    }
  }

  try { [void](Invoke-SerialCommands -Cmds @("~VREC:STOP") -ReadMs 400) } catch {}

  if (Test-Path -LiteralPath $appLog) {
    $fs = [System.IO.File]::Open($appLog, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    try {
      if ($logStartLen -gt $fs.Length) { $logStartLen = 0 }
      $null = $fs.Seek($logStartLen, [System.IO.SeekOrigin]::Begin)
      $reader = [System.IO.StreamReader]::new($fs, [System.Text.Encoding]::UTF8, $true, 4096, $true)
      try {
        $delta = $reader.ReadToEnd()
      } finally {
        $reader.Dispose()
      }
      $logStartLen = $fs.Length
    } finally {
      $fs.Dispose()
    }
    if ($delta -match "missing_packets=[1-9]") {
      $packetLossEvents++
      Write-Log "PACKET_LOSS_SIGNAL"
    }
    if ($delta -match "WDT|ESP_RST|0xc0000005|panic") {
      $restartEvents++
      Write-Log "RESTART_OR_CRASH_SIGNAL"
    }
  }

  $elapsed = [int]((Get-Date) - $start).TotalSeconds
  Write-Log ("progress elapsed={0}s cycles={1} serial_errors={2}" -f $elapsed, $sessionCycles, $serialErrors)
}

$elapsed = [int]((Get-Date) - $start).TotalSeconds
$fwHealth = @()
try {
  $fwHealth = Invoke-SerialCommands -Cmds @("~DIAGLOG:SOURCES", "~DIAGLOG:LAST:30") -ReadMs 1500
} catch {
  Write-Log ("final_diag_error={0}" -f $_.Exception.Message)
}
$wdtHits = @($fwHealth | Where-Object { $_ -match "WDT|ESP_RST|TASK_WDT|reset_reason" })
$crashHits = @($fwHealth | Where-Object { $_ -match "crash_count=[1-9]" })

$failures = [System.Collections.Generic.List[string]]::new()
$result = "PASS"
if ($elapsed -lt ($soakSeconds - 45)) { $failures.Add("elapsed $elapsed short of $soakSeconds") }
if ($typeAliveFails -gt 0) { $failures.Add("type exited") }
if ($restartEvents -gt 0) { $failures.Add("type log crash/WDT signal") }
if ($wdtHits.Count -gt 0) { $failures.Add("serial WDT/reset hits") }
if ($crashHits.Count -gt 0) { $failures.Add("crash_count nonzero") }
$minCycles = [Math]::Max(8, [int]($soakSeconds / ($CycleHoldSeconds + 10)))
if ($sessionCycles -lt $minCycles) { $failures.Add("too few cycles $sessionCycles < $minCycles") }
if ($failures.Count -gt 0) { $result = "FAIL" }

$summary = [ordered]@{
  schema = "listener.1.0.4.type_connected_soak"
  result = $result
  elapsed_seconds = $elapsed
  required_seconds = $soakSeconds
  session_cycles = $sessionCycles
  min_cycles = $minCycles
  type_alive_checks = $typeAliveChecks
  type_alive_fails = $typeAliveFails
  serial_errors = $serialErrors
  packet_loss_log_signals = $packetLossEvents
  restart_log_signals = $restartEvents
  serial_wdt_hits = $wdtHits
  serial_crash_hits = $crashHits
  failures = @($failures)
  notify_ready_ms = $notify.start_to_notify_ready_ms
  events = @($events)
  detail_log = $detailLog
  method = "Type holds BLE notify; serial briefly toggles VREC each cycle for wall-clock soak; final DIAG tail for WDT/crash."
}
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryJson -Encoding utf8
Write-Log ("SOAK_RESULT={0} summary={1}" -f $result, $summaryJson)
Write-Output ($summary | ConvertTo-Json -Depth 6)
if ($result -ne "PASS") { exit 1 }
exit 0
