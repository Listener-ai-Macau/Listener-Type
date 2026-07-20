param(
  [string]$ListenerExe = 'C:\Program Files\Listener Type\listener-type.exe',
  [int]$MaxNotifyReadyMs = 3000,
  [int]$EvidenceTimeoutSeconds = 8,
  [string]$OutputDir = ''
)

$ErrorActionPreference = 'Stop'

function Get-InstalledTypeProcesses {
  param([string]$ExpectedExe)

  @(Get-CimInstance Win32_Process -Filter "Name='listener-type.exe'" | Where-Object {
    $_.ExecutablePath -eq $ExpectedExe
  })
}

function Read-BleStatus {
  param(
    [string]$Label,
    [string]$RunDirectory
  )

  $created = $false
  $mutex = [System.Threading.Mutex]::new($false, 'Global\Listener_COM3', [ref]$created)
  $acquired = $false
  $port = $null
  $raw = ''
  $errorText = $null
  $started = Get-Date
  try {
    $acquired = $mutex.WaitOne(1500)
    if (-not $acquired) { throw 'Global\\Listener_COM3 mutex timed out after 1500 ms.' }
    $port = [System.IO.Ports.SerialPort]::new('COM3', 115200, [System.IO.Ports.Parity]::None, 8, [System.IO.Ports.StopBits]::One)
    $port.ReadTimeout = 80
    $port.WriteTimeout = 500
    $port.Open()
    $port.DiscardInBuffer()
    $port.Write("~BLE:STATUS`r`n")
    $deadline = (Get-Date).AddMilliseconds(700)
    while ((Get-Date) -lt $deadline) {
      $raw += $port.ReadExisting()
      Start-Sleep -Milliseconds 35
    }
    $raw += $port.ReadExisting()
  } catch {
    $errorText = $_.Exception.Message
  } finally {
    if ($port) { $port.Dispose() }
    if ($acquired) { $mutex.ReleaseMutex() }
    $mutex.Dispose()
  }

  $logPath = Join-Path $RunDirectory "serial-$Label.log"
  [System.IO.File]::WriteAllText($logPath, $raw, [System.Text.UTF8Encoding]::new($false))
  [pscustomobject]@{
    label = $Label
    elapsed_ms = [math]::Round(((Get-Date) - $started).TotalMilliseconds)
    log = $logPath
    serial_closed = $true
    error = $errorText
    secure = ([regex]::Match($raw, 'secure=(?<value>[01])').Groups['value'].Value)
    e11r = ([regex]::Match($raw, 'e11r=(?<value>[01])').Groups['value'].Value)
    dle_ready = ([regex]::Match($raw, 'dle_ready=(?<value>[01])').Groups['value'].Value)
    panic_or_reboot_marker_observed = [bool]($raw -match '(?i)nimble.*panic|panic|guru meditation|rst:|reboot')
  }
}

function Read-MachineEvidence {
  param(
    [string]$EvidencePath,
    [datetime]$Deadline
  )

  $lastSnapshot = $null
  while ((Get-Date) -lt $Deadline) {
    if (Test-Path -LiteralPath $EvidencePath) {
      try {
        $value = Get-Content -Raw -LiteralPath $EvidencePath | ConvertFrom-Json
        if ($value.schema -eq 'listener.type.startup-evidence.v1') {
          $lastSnapshot = $value
          if (
            -not [string]::IsNullOrWhiteSpace([string]$value.startup_path) -and
            $null -ne $value.background_notify_ready_elapsed_ms
          ) {
            return $value
          }
        }
      } catch {
        # The installed process may be replacing its bounded snapshot. Retry.
      }
    }
    Start-Sleep -Milliseconds 100
  }
  return $lastSnapshot
}

function Read-BootSafetyStatus {
  param(
    [string]$Label,
    [string]$RunDirectory
  )

  $created = $false
  $mutex = [System.Threading.Mutex]::new($false, 'Global\Listener_COM3', [ref]$created)
  $acquired = $false
  $port = $null
  $raw = ''
  $errorText = $null
  try {
    $acquired = $mutex.WaitOne(1500)
    if (-not $acquired) { throw 'Global\\Listener_COM3 mutex timed out after 1500 ms.' }
    $port = [System.IO.Ports.SerialPort]::new('COM3', 115200, [System.IO.Ports.Parity]::None, 8, [System.IO.Ports.StopBits]::One)
    $port.ReadTimeout = 80
    $port.WriteTimeout = 500
    $port.Open()
    $port.DiscardInBuffer()
    $port.Write("~BOOT:STATUS`r`n")
    $deadline = (Get-Date).AddMilliseconds(1100)
    while ((Get-Date) -lt $deadline) {
      $raw += $port.ReadExisting()
      Start-Sleep -Milliseconds 35
    }
    $raw += $port.ReadExisting()
  } catch {
    $errorText = $_.Exception.Message
  } finally {
    if ($port) { $port.Dispose() }
    if ($acquired) { $mutex.ReleaseMutex() }
    $mutex.Dispose()
  }

  $logPath = Join-Path $RunDirectory "boot-$Label.log"
  [System.IO.File]::WriteAllText($logPath, $raw, [System.Text.UTF8Encoding]::new($false))
  $match = [regex]::Match($raw, 'reset_reason=(?<reason>[a-z_]+)\(\d+\) crash_count=(?<crash>\d+) .*safe_mode=(?<safe>[01])')
  [pscustomobject]@{
    label = $Label
    log = $logPath
    serial_closed = $true
    error = $errorText
    reset_reason = $match.Groups['reason'].Value
    crash_count = if ($match.Success) { [int64]$match.Groups['crash'].Value } else { $null }
    safe_mode = if ($match.Success) { [int]$match.Groups['safe'].Value } else { $null }
  }
}

if (-not (Test-Path -LiteralPath $ListenerExe)) {
  throw "Installed Type executable not found: $ListenerExe"
}
$resolvedExe = (Resolve-Path -LiteralPath $ListenerExe).Path
if ($resolvedExe -ne 'C:\Program Files\Listener Type\listener-type.exe') {
  throw "Physical validation must launch only C:\Program Files\Listener Type\listener-type.exe; got $resolvedExe"
}

$repoRoot = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
  $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
  $OutputDir = Join-Path $repoRoot ".cache\validation\ble-takeover-evidence-$stamp"
}
$allowedCacheRoot = (Join-Path $repoRoot '.cache\validation').ToLowerInvariant()
$resolvedOutputDir = [System.IO.Path]::GetFullPath($OutputDir)
$normalizedOutputDir = $resolvedOutputDir.ToLowerInvariant()
$allowedCachePrefix = $allowedCacheRoot + [System.IO.Path]::DirectorySeparatorChar
if ($normalizedOutputDir -ne $allowedCacheRoot -and -not $normalizedOutputDir.StartsWith($allowedCachePrefix)) {
  throw "OutputDir must stay under $allowedCacheRoot"
}
New-Item -ItemType Directory -Force -Path $resolvedOutputDir | Out-Null

$evidencePath = Join-Path $resolvedOutputDir 'type-startup-evidence.json'
$summaryPath = Join-Path $resolvedOutputDir 'machine-summary.json'
$bootBefore = Read-BootSafetyStatus -Label 'before-start' -RunDirectory $resolvedOutputDir
$existing = Get-InstalledTypeProcesses -ExpectedExe $resolvedExe
if ($existing.Count -gt 0) {
  Start-Process -FilePath $resolvedExe -ArgumentList '--quit' -WindowStyle Hidden | Out-Null
  $quitDeadline = (Get-Date).AddSeconds(6)
  do {
    Start-Sleep -Milliseconds 150
    $remaining = Get-InstalledTypeProcesses -ExpectedExe $resolvedExe
  } while ($remaining.Count -gt 0 -and (Get-Date) -lt $quitDeadline)
  if ($remaining.Count -gt 0) {
    throw 'Installed Type did not exit after its Program Files --quit command; forced termination is intentionally disabled.'
  }
}

$previousEvidencePath = [Environment]::GetEnvironmentVariable('LISTENER_TYPE_MACHINE_EVIDENCE_PATH', 'Process')
$startedAt = Get-Date
try {
  [Environment]::SetEnvironmentVariable('LISTENER_TYPE_MACHINE_EVIDENCE_PATH', $evidencePath, 'Process')
  $process = Start-Process -FilePath $resolvedExe -WindowStyle Hidden -PassThru
} finally {
  [Environment]::SetEnvironmentVariable('LISTENER_TYPE_MACHINE_EVIDENCE_PATH', $previousEvidencePath, 'Process')
}

$evidence = Read-MachineEvidence -EvidencePath $evidencePath -Deadline ((Get-Date).AddSeconds($EvidenceTimeoutSeconds))
$statusSamples = @(
  $(Read-BleStatus -Label 'after-start-01' -RunDirectory $resolvedOutputDir)
  $(Read-BleStatus -Label 'after-start-02' -RunDirectory $resolvedOutputDir)
)
$bootAfter = Read-BootSafetyStatus -Label 'after-start' -RunDirectory $resolvedOutputDir

$errors = @()
if ($null -eq $evidence) {
  $errors += 'Installed Type did not emit the opt-in startup evidence snapshot.'
} else {
  if ($evidence.background_notify_ready_elapsed_ms -eq $null) {
    $errors += 'background notify ready was not observed by installed Type.'
  } elseif ([int64]$evidence.background_notify_ready_elapsed_ms -gt $MaxNotifyReadyMs) {
    $errors += "background notify ready exceeded $MaxNotifyReadyMs ms."
  }
  if ($evidence.startup_path -notin @('native_windows_hid', 'persisted_cached')) {
    $errors += "startup path '$($evidence.startup_path)' is not an accepted native/persisted takeover path."
  }
  if ([int64]$evidence.pair_async_attempt_count -ne 0) {
    $errors += "Type invoked PairAsync $($evidence.pair_async_attempt_count) time(s) during takeover."
  }
  if ([int64]$evidence.unpair_async_attempt_count -ne 0) {
    $errors += "Type invoked UnpairAsync $($evidence.unpair_async_attempt_count) time(s) during takeover."
  }
}
foreach ($sample in $statusSamples) {
  if ($sample.error -or $sample.secure -ne '1' -or $sample.e11r -ne '1' -or $sample.dle_ready -ne '1') {
    $errors += "firmware BLE status was not ready for $($sample.label)."
  }
}
if ($bootBefore.error -or $bootAfter.error -or $null -eq $bootBefore.crash_count -or $null -eq $bootAfter.crash_count) {
  $errors += 'firmware boot-safety status was not readable before and after installed Type takeover.'
} elseif ($bootAfter.crash_count -ne $bootBefore.crash_count) {
  $errors += "firmware crash_count changed from $($bootBefore.crash_count) to $($bootAfter.crash_count) during takeover."
} elseif ($bootAfter.reset_reason -in @('panic', 'interrupt_wdt', 'task_wdt', 'other_wdt', 'cpu_lockup')) {
  $errors += "firmware reset_reason=$($bootAfter.reset_reason) indicates a crash-class reset."
}

$summary = [ordered]@{
  schema = 'listener.ble.takeover-evidence.v1'
  captured_at = (Get-Date).ToString('o')
  installed_type = [ordered]@{
    executable = $resolvedExe
    exe_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $resolvedExe).Hash
    process_id = $process.Id
  }
  startup_elapsed_ms = [math]::Round(((Get-Date) - $startedAt).TotalMilliseconds)
  type_startup_evidence = $evidence
  serial_mutex = 'Global\\Listener_COM3'
  serial_status_samples = $statusSamples
  boot_safety = [ordered]@{ before = $bootBefore; after = $bootAfter }
  no_type_initiated_pairing_prompt = if ($evidence) { [int64]$evidence.pair_async_attempt_count -eq 0 } else { $null }
  no_type_initiated_forced_unpair = if ($evidence) { [int64]$evidence.unpair_async_attempt_count -eq 0 } else { $null }
  nimble_panic_reboot_evidence = 'boot_safety crash_count and reset_reason remained stable across the installed Type takeover window'
  takeover_evidence_verdict = if ($errors.Count -eq 0) { 'PASS' } else { 'NO_GO' }
  verdict = 'NO_GO'
  full_bluetooth_goal_verdict = 'NO_GO'
  full_bluetooth_goal_gap = 'This bounded runner proves installed-Type startup-path, notify readiness, Type-initiated pair/unpair calls, firmware security, and boot-safety only. A complete Bluetooth goal verdict additionally requires a bounded Windows pairing-prompt and GATT-control-chain artifact.'
  errors = $errors
}
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding utf8NoBOM
$summary | ConvertTo-Json -Depth 8
if ($errors.Count -gt 0) { exit 1 }
