[CmdletBinding(PositionalBinding = $false)]
param(
    [string]$Port = "COM3",
    [ValidateRange(10, 60)]
    [int]$CaptureSeconds = 25,
    [ValidateRange(1, 60000)]
    [int]$MaxDoubleClickToAdvertisingAcceptedMs = 250,
    [ValidateRange(1, 60000)]
    [int]$MaxConnectionToEncryptionMs = 1200,
    [ValidateRange(1, 60000)]
    [int]$MaxFreshPairingToTypeReadyMs = 6000,
    [string]$OutputJson = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Write-Measurement {
    param([System.Collections.IDictionary]$Measurement)

    $json = $Measurement | ConvertTo-Json -Depth 8
    if (-not [string]::IsNullOrWhiteSpace($OutputJson)) {
        $directory = Split-Path -Parent $OutputJson
        if (-not [string]::IsNullOrWhiteSpace($directory)) {
            New-Item -ItemType Directory -Force -Path $directory | Out-Null
        }
        Set-Content -LiteralPath $OutputJson -Value $json -Encoding utf8
    }
    Write-Output $json
}

function Get-FirmwareUptimeMs {
    param([string]$Line)

    if ($Line -match "\(\s*(?<uptime>\d+)\)") {
        return [long]$Matches.uptime
    }
    return $null
}

$typeExe = "C:\Program Files\Listener Type\listener-type.exe"
$installedType = Get-CimInstance Win32_Process -Filter "Name = 'listener-type.exe'" |
    Where-Object { $_.ExecutablePath -eq $typeExe } |
    Select-Object -First 1
if ($null -eq $installedType) {
    throw "The installed Program Files Listener Type process is not running: $typeExe"
}

$firmwareRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..\\..\\Listener-Firmware")).Path
$captureScript = Join-Path $firmwareRoot "tools\\send_serial_and_capture.ps1"
if (-not (Test-Path -LiteralPath $captureScript)) {
    throw "Missing firmware serial capture script: $captureScript"
}

$capturePath = if ([string]::IsNullOrWhiteSpace($OutputJson)) {
    Join-Path $env:TEMP "listener-ec11-recovery-speed-$([guid]::NewGuid().ToString('N')).log"
} else {
    [System.IO.Path]::ChangeExtension($OutputJson, ".log")
}
$typeLogPath = Join-Path $env:LOCALAPPDATA "Listener Type\\Logs\\listener-type.log"
$typeLogLength = if (Test-Path -LiteralPath $typeLogPath) {
    (Get-Item -LiteralPath $typeLogPath).Length
} else {
    0
}

& pwsh -NoProfile -File $captureScript -Port $Port -Command "~KEY:EC11:DOUBLE" -CaptureSeconds $CaptureSeconds -OutputPath $capturePath
if ($LASTEXITCODE -ne 0) {
    throw "EC11 recovery serial capture failed with exit code $LASTEXITCODE"
}

$lines = @(Get-Content -LiteralPath $capturePath)
$triggerIndex = -1
for ($index = 0; $index -lt $lines.Count; $index += 1) {
    if ($lines[$index] -match "recovery double-click accepted") {
        $triggerIndex = $index
        break
    }
}

$typeReadyIndex = -1
if ($triggerIndex -ge 0) {
    for ($index = $triggerIndex + 1; $index -lt $lines.Count; $index += 1) {
        if ($lines[$index] -match "type audio ready accepted.*reason=TYPE:READY") {
            $typeReadyIndex = $index
            break
        }
    }
}

$noticeIndex = -1
if ($triggerIndex -ge 0) {
    for ($index = $triggerIndex + 1; $index -lt $lines.Count; $index += 1) {
        if ($lines[$index] -match "type recovery notice sent before EC11 pairing reset") {
            $noticeIndex = $index
            break
        }
    }
}

$triggerUptimeMs = if ($triggerIndex -ge 0) { Get-FirmwareUptimeMs $lines[$triggerIndex] } else { $null }
$typeReadyUptimeMs = if ($typeReadyIndex -ge 0) { Get-FirmwareUptimeMs $lines[$typeReadyIndex] } else { $null }
$triggerToTypeReadyMs = if ($null -ne $triggerUptimeMs -and $null -ne $typeReadyUptimeMs) {
    $typeReadyUptimeMs - $triggerUptimeMs
} else {
    $null
}

$typeControlledRecovery = $false
$advertisingCommandAcceptedMs = $null
$connectionUptimeMs = $null
$encryptionUptimeMs = $null
$failedEncryptionStatuses = @()
if ($triggerIndex -ge 0) {
    for ($index = $triggerIndex + 1; $index -lt $lines.Count; $index += 1) {
        $line = $lines[$index]
        if ($line -match "recovery: pairing window opened type_controlled=1") {
            $typeControlledRecovery = $true
        }
        if ($null -eq $advertisingCommandAcceptedMs -and
            $line -match "EC11 recovery timing:.*advertising_command_accepted_ms=(?<elapsed>-?\d+)") {
            $candidate = [long]$Matches.elapsed
            if ($candidate -ge 0) {
                $advertisingCommandAcceptedMs = $candidate
            }
        }
        if ($line -match "global GAP listener connection established") {
            $connectionUptimeMs = Get-FirmwareUptimeMs $line
            continue
        }
        if ($line -match "encryption change event; status=(?<status>\d+)") {
            $status = [int]$Matches.status
            if ($status -eq 0 -and $null -ne $connectionUptimeMs) {
                $encryptionUptimeMs = Get-FirmwareUptimeMs $line
                break
            }
            $failedEncryptionStatuses += $status
        }
    }
}

$connectionToEncryptionMs = if ($null -ne $connectionUptimeMs -and $null -ne $encryptionUptimeMs) {
    $encryptionUptimeMs - $connectionUptimeMs
} else {
    $null
}
$freshPairingToTypeReadyMs = if ($null -ne $encryptionUptimeMs -and $null -ne $typeReadyUptimeMs) {
    $typeReadyUptimeMs - $encryptionUptimeMs
} else {
    $null
}

$newTypeLog = ""
if (Test-Path -LiteralPath $typeLogPath) {
    # Type keeps its log handle open. Get-Content uses a shareable read instead of
    # treating the active log as unavailable during the measurement window.
    $typeLogBytes = Get-Content -LiteralPath $typeLogPath -AsByteStream -Raw
    if ($typeLogBytes.Length -ge $typeLogLength) {
        $newTypeLog = [System.Text.Encoding]::UTF8.GetString($typeLogBytes, [int]$typeLogLength, [int]($typeLogBytes.Length - $typeLogLength))
    }
}
$typeObservedNotice = $newTypeLog.Contains("received EC11 hardware recovery notice; retaining the GATT session until the firmware disconnect completes")

$failures = @()
if ($triggerIndex -lt 0) { $failures += "missing firmware EC11 recovery double-click trigger" }
if ($null -eq $advertisingCommandAcceptedMs) { $failures += "missing firmware advertising-command acceptance timing" }
elseif ($advertisingCommandAcceptedMs -gt $MaxDoubleClickToAdvertisingAcceptedMs) { $failures += "EC11 double-click to controller advertising acceptance was $advertisingCommandAcceptedMs ms, above $MaxDoubleClickToAdvertisingAcceptedMs ms" }
if ($null -eq $connectionToEncryptionMs) { $failures += "missing successful Windows connection to encryption timing" }
elseif ($connectionToEncryptionMs -gt $MaxConnectionToEncryptionMs) { $failures += "Windows connection to encryption was $connectionToEncryptionMs ms, above $MaxConnectionToEncryptionMs ms" }
if ($null -eq $freshPairingToTypeReadyMs) { $failures += "missing fresh pairing evidence to TYPE:READY timing" }
elseif ($freshPairingToTypeReadyMs -gt $MaxFreshPairingToTypeReadyMs) { $failures += "fresh pairing evidence to TYPE:READY was $freshPairingToTypeReadyMs ms, above $MaxFreshPairingToTypeReadyMs ms" }
if ($typeReadyIndex -lt 0) { $failures += "missing firmware TYPE:READY after recovery trigger" }
if ($typeControlledRecovery -and $noticeIndex -lt 0) { $failures += "missing firmware pre-reset EC11 recovery notice for Type-controlled recovery" }
if ($typeControlledRecovery -and -not $typeObservedNotice) { $failures += "Type did not observe the pre-reset EC11 recovery notice in its new log bytes" }

$measurement = [ordered]@{
    status = if ($failures.Count -eq 0) { "PASS" } else { "FAIL" }
    measurement_scope = "firmware-generated EC11 double-click recovery state machine from accepted double-click event to firmware TYPE:READY"
    physical_gpio_measurement = $false
    trigger_kind = "firmware_generated_ec11_double_after_debounce"
    installed_type_exe = $typeExe
    installed_type_process_id = $installedType.ProcessId
    port = $Port
    capture_path = $capturePath
    type_log_path = $typeLogPath
    max_double_click_to_advertising_accepted_ms = $MaxDoubleClickToAdvertisingAcceptedMs
    max_connection_to_encryption_ms = $MaxConnectionToEncryptionMs
    max_fresh_pairing_to_type_ready_ms = $MaxFreshPairingToTypeReadyMs
    trigger_uptime_ms = $triggerUptimeMs
    advertising_command_accepted_ms = $advertisingCommandAcceptedMs
    connection_uptime_ms = $connectionUptimeMs
    encryption_uptime_ms = $encryptionUptimeMs
    type_ready_uptime_ms = $typeReadyUptimeMs
    connection_to_encryption_ms = $connectionToEncryptionMs
    fresh_pairing_to_type_ready_ms = $freshPairingToTypeReadyMs
    trigger_to_type_ready_ms_informational = $triggerToTypeReadyMs
    encryption_failure_statuses_before_success = @($failedEncryptionStatuses)
    type_controlled_recovery = $typeControlledRecovery
    firmware_pre_reset_notice_sent = ($noticeIndex -ge 0)
    type_pre_reset_notice_observed = $typeObservedNotice
    failure_reasons = @($failures)
}
Write-Measurement $measurement
if ($failures.Count -ne 0) {
    throw "EC11 Type recovery speed gate failed: $($failures -join '; ')"
}
