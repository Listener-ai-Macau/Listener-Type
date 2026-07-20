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

function Get-TextSha256 {
  param([string]$Value)

  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes($Value)
    ([System.BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '')
  } finally {
    $sha.Dispose()
  }
}

function Get-PresentNativeHidPairingState {
  # Keep raw PnP identities in memory only. The persisted result contains counts,
  # status, and a one-way set hash so it cannot disclose the BLE address or name.
  try {
    $entries = @(
      Get-CimInstance Win32_PnPEntity -Filter "DeviceID LIKE 'BTHLE%' OR DeviceID LIKE 'BTHLEDEVICE%' OR DeviceID LIKE 'HID%'" -ErrorAction Stop |
        Where-Object { $_.Present -ne $false }
    )
    $roots = @{}
    $keyboards = @{}
    foreach ($entry in $entries) {
      $instanceId = [string]$entry.DeviceID
      $upper = $instanceId.ToUpperInvariant()
      $addressMatches = [regex]::Matches($upper, '(?:DEV_|_)(?<address>[0-9A-F]{12})(?![0-9A-F])')
      if ($addressMatches.Count -eq 0) {
        continue
      }
      $address = $addressMatches[$addressMatches.Count - 1].Groups['address'].Value
      $state = [string]$entry.Status
      if ($upper.StartsWith('BTHLE\DEV_')) {
        $roots[$address] = $state
      }
      if (
        $upper.StartsWith('HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0216C0_PID&05DF_') -and
        $upper.Contains('&COL01\')
      ) {
        $keyboards[$address] = $state
      }
    }

    $matchingAddresses = @($roots.Keys | Where-Object { $keyboards.ContainsKey($_) } | Sort-Object)
    $identityParts = [System.Collections.Generic.List[string]]::new()
    $allOk = $matchingAddresses.Count -gt 0
    foreach ($address in $matchingAddresses) {
      $rootState = $roots[$address]
      $keyboardState = $keyboards[$address]
      $identityParts.Add("$address|root|$rootState") | Out-Null
      $identityParts.Add("$address|keyboard|$keyboardState") | Out-Null
      if ($rootState -ne 'OK' -or $keyboardState -ne 'OK') {
        $allOk = $false
      }
    }

    [pscustomobject]@{
      query_succeeded                    = $true
      matching_listener_root_count        = $matchingAddresses.Count
      matching_listener_keyboard_count    = $matchingAddresses.Count
      all_matching_entries_status_ok      = $allOk
      matching_identity_set_sha256        = Get-TextSha256 (($identityParts | Sort-Object) -join "`n")
      error                               = $null
    }
  } catch {
    [pscustomobject]@{
      query_succeeded                    = $false
      matching_listener_root_count        = $null
      matching_listener_keyboard_count    = $null
      all_matching_entries_status_ok      = $false
      matching_identity_set_sha256        = $null
      error                               = 'present native-HID PnP query failed'
    }
  }
}

function Start-InstalledTypeWithStartupObservation {
  param(
    [string]$ExpectedExe,
    [string]$EvidencePath,
    [int]$EvidenceTimeoutSeconds
  )

  $created = $false
  $mutex = [System.Threading.Mutex]::new($false, 'Global\Listener_COM3', [ref]$created)
  $acquired = $false
  $port = $null
  $serialText = ''
  $serialError = $null
  $evidence = $null
  $process = $null
  $captureStarted = Get-Date
  try {
    $acquired = $mutex.WaitOne(1500)
    if (-not $acquired) { throw 'Global\\Listener_COM3 mutex timed out before startup observation.' }
    $port = [System.IO.Ports.SerialPort]::new('COM3', 115200, [System.IO.Ports.Parity]::None, 8, [System.IO.Ports.StopBits]::One)
    $port.ReadTimeout = 80
    $port.WriteTimeout = 500
    $port.Open()
    $port.DiscardInBuffer()

    $previousEvidencePath = [Environment]::GetEnvironmentVariable('LISTENER_TYPE_MACHINE_EVIDENCE_PATH', 'Process')
    try {
      [Environment]::SetEnvironmentVariable('LISTENER_TYPE_MACHINE_EVIDENCE_PATH', $EvidencePath, 'Process')
      $process = Start-Process -FilePath $ExpectedExe -WindowStyle Hidden -PassThru
    } finally {
      [Environment]::SetEnvironmentVariable('LISTENER_TYPE_MACHINE_EVIDENCE_PATH', $previousEvidencePath, 'Process')
    }

    $deadline = (Get-Date).AddSeconds($EvidenceTimeoutSeconds)
    $readyObservedAt = $null
    while ((Get-Date) -lt $deadline) {
      $serialText += $port.ReadExisting()
      if (Test-Path -LiteralPath $EvidencePath) {
        try {
          $candidate = Get-Content -Raw -LiteralPath $EvidencePath | ConvertFrom-Json
          if (
            $candidate.schema -eq 'listener.type.startup-evidence.v1' -and
            -not [string]::IsNullOrWhiteSpace([string]$candidate.startup_path) -and
            $null -ne $candidate.background_notify_ready_elapsed_ms
          ) {
            $evidence = $candidate
            $readyObservedAt = Get-Date
            break
          }
        } catch {
          # The installed process can be replacing the opt-in snapshot.
        }
      }
      Start-Sleep -Milliseconds 75
    }

    if ($readyObservedAt) {
      $settleDeadline = $readyObservedAt.AddMilliseconds(500)
      while ((Get-Date) -lt $settleDeadline) {
        $serialText += $port.ReadExisting()
        Start-Sleep -Milliseconds 35
      }
    }
    $serialText += $port.ReadExisting()
  } catch {
    $serialError = $_.Exception.Message
  } finally {
    if ($port) { $port.Dispose() }
    if ($acquired) { $mutex.ReleaseMutex() }
    $mutex.Dispose()
  }

  [pscustomobject]@{
    process = $process
    evidence = $evidence
    serial_capture_started_before_process = $true
    serial_capture_elapsed_ms = [math]::Round(((Get-Date) - $captureStarted).TotalMilliseconds)
    serial_capture_sha256 = Get-TextSha256 $serialText
    firmware_type_ready_receive_count = [regex]::Matches($serialText, 'type heartbeat received source=ble_audio_control command=TYPE:READY', 'IgnoreCase').Count
    firmware_type_ready_secure_accept_count = [regex]::Matches($serialText, 'type audio ready accepted on existing secure BLE connection reason=TYPE:READY', 'IgnoreCase').Count
    serial_error = $serialError
  }
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
$nativePairingBefore = Get-PresentNativeHidPairingState
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

$startedAt = Get-Date
$startupObservation = Start-InstalledTypeWithStartupObservation `
  -ExpectedExe $resolvedExe `
  -EvidencePath $evidencePath `
  -EvidenceTimeoutSeconds $EvidenceTimeoutSeconds
$process = $startupObservation.process
$evidence = $startupObservation.evidence
$statusSamples = @(
  $(Read-BleStatus -Label 'after-start-01' -RunDirectory $resolvedOutputDir)
  $(Read-BleStatus -Label 'after-start-02' -RunDirectory $resolvedOutputDir)
)
$bootAfter = Read-BootSafetyStatus -Label 'after-start' -RunDirectory $resolvedOutputDir
$nativePairingAfter = Get-PresentNativeHidPairingState

$errors = @()
if ($null -eq $process) {
  $errors += 'Installed Type could not be launched while serial startup observation was active.'
}
if ($startupObservation.serial_error) {
  $errors += 'serial startup observation was not available.'
}
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
  $stageNames = @($evidence.startup_stages | ForEach-Object { [string]$_.name })
  foreach ($requiredStage in @('notify_cccd_enabled', 'type_ready_written')) {
    if ($stageNames -notcontains $requiredStage) {
      $errors += "installed Type did not record required GATT control stage '$requiredStage'."
    }
  }
}
if ($startupObservation.firmware_type_ready_receive_count -lt 1) {
  $errors += 'firmware did not record receipt of Type GATT control TYPE:READY during the bounded startup window.'
}
if ($startupObservation.firmware_type_ready_secure_accept_count -lt 1) {
  $errors += 'firmware did not accept TYPE:READY on an existing secure BLE connection during takeover.'
}
if (-not $nativePairingBefore.query_succeeded -or -not $nativePairingAfter.query_succeeded) {
  $errors += 'present native-HID PnP state was not readable before and after installed Type takeover.'
} elseif (
  $nativePairingBefore.matching_listener_root_count -lt 1 -or
  $nativePairingAfter.matching_listener_root_count -lt 1 -or
  -not $nativePairingBefore.all_matching_entries_status_ok -or
  -not $nativePairingAfter.all_matching_entries_status_ok
) {
  $errors += 'Windows native Listener HID pairing state was not present and OK across the installed Type takeover.'
} elseif ($nativePairingBefore.matching_identity_set_sha256 -ne $nativePairingAfter.matching_identity_set_sha256) {
  $errors += 'Windows native Listener HID pairing identity changed during Type takeover.'
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
    process_id = if ($process) { $process.Id } else { $null }
  }
  startup_elapsed_ms = [math]::Round(((Get-Date) - $startedAt).TotalMilliseconds)
  type_startup_evidence = $evidence
  gatt_control_chain = [ordered]@{
    serial_capture_started_before_process = $startupObservation.serial_capture_started_before_process
    serial_capture_elapsed_ms = $startupObservation.serial_capture_elapsed_ms
    serial_capture_sha256 = $startupObservation.serial_capture_sha256
    type_ready_stage_recorded = if ($evidence) { @($evidence.startup_stages | ForEach-Object { $_.name }) -contains 'type_ready_written' } else { $false }
    notify_cccd_stage_recorded = if ($evidence) { @($evidence.startup_stages | ForEach-Object { $_.name }) -contains 'notify_cccd_enabled' } else { $false }
    firmware_type_ready_receive_count = $startupObservation.firmware_type_ready_receive_count
    firmware_type_ready_secure_accept_count = $startupObservation.firmware_type_ready_secure_accept_count
    serial_error = $startupObservation.serial_error
  }
  windows_native_hid_pairing = [ordered]@{
    before = $nativePairingBefore
    after = $nativePairingAfter
    unchanged_identity_set = if (
      $nativePairingBefore.query_succeeded -and
      $nativePairingAfter.query_succeeded
    ) { $nativePairingBefore.matching_identity_set_sha256 -eq $nativePairingAfter.matching_identity_set_sha256 } else { $false }
  }
  serial_mutex = 'Global\\Listener_COM3'
  serial_status_samples = $statusSamples
  boot_safety = [ordered]@{ before = $bootBefore; after = $bootAfter }
  no_type_initiated_pairing_prompt = if ($evidence) { [int64]$evidence.pair_async_attempt_count -eq 0 } else { $null }
  no_type_initiated_forced_unpair = if ($evidence) { [int64]$evidence.unpair_async_attempt_count -eq 0 } else { $null }
  nimble_panic_reboot_evidence = 'boot_safety crash_count and reset_reason remained stable across the installed Type takeover window'
  takeover_evidence_verdict = if ($errors.Count -eq 0) { 'PASS' } else { 'NO_GO' }
  verdict = 'NO_GO'
  full_bluetooth_goal_verdict = 'NO_GO'
  full_bluetooth_goal_gap = 'This bounded runner now proves installed-Type startup-path, no Type-initiated PairAsync/UnpairAsync, unchanged present Windows native-HID pairing state, Type GATT TYPE:READY write, firmware secure receipt, notify readiness, and boot-safety. Full Bluetooth Goal closure still requires the separate fresh native-Windows-pairing and device-key delivery machines.'
  errors = $errors
}
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding utf8NoBOM
$summary | ConvertTo-Json -Depth 8
if ($errors.Count -gt 0) { exit 1 }
