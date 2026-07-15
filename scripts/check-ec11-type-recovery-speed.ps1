[CmdletBinding(PositionalBinding = $false)]
param(
    [string]$Port = "COM3",
    [ValidateRange(10, 60)]
    [int]$CaptureSeconds = 25,
    [ValidateRange(1, 60000)]
    [int]$MaxTriggerToTypeReadyMs = 10000,
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
        if ($lines[$index] -match "TYPE:READY") {
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
$elapsedMs = if ($null -ne $triggerUptimeMs -and $null -ne $typeReadyUptimeMs) {
    $typeReadyUptimeMs - $triggerUptimeMs
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
$typeObservedNotice = $newTypeLog.Contains("received EC11 hardware recovery notice before pairing reset")

$failures = @()
if ($triggerIndex -lt 0) { $failures += "missing firmware EC11 recovery double-click trigger" }
if ($noticeIndex -lt 0) { $failures += "missing firmware pre-reset EC11 recovery notice" }
if ($typeReadyIndex -lt 0) { $failures += "missing firmware TYPE:READY after recovery trigger" }
if ($null -eq $elapsedMs) { $failures += "missing parseable recovery trigger or TYPE:READY uptime" }
elseif ($elapsedMs -gt $MaxTriggerToTypeReadyMs) { $failures += "recovery trigger to TYPE:READY was $elapsedMs ms, above $MaxTriggerToTypeReadyMs ms" }
if (-not $typeObservedNotice) { $failures += "Type did not observe the pre-reset EC11 recovery notice in its new log bytes" }

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
    max_trigger_to_type_ready_ms = $MaxTriggerToTypeReadyMs
    trigger_uptime_ms = $triggerUptimeMs
    type_ready_uptime_ms = $typeReadyUptimeMs
    trigger_to_type_ready_ms = $elapsedMs
    firmware_pre_reset_notice_sent = ($noticeIndex -ge 0)
    type_pre_reset_notice_observed = $typeObservedNotice
    failure_reasons = @($failures)
}
Write-Measurement $measurement
if ($failures.Count -ne 0) {
    throw "EC11 Type recovery speed gate failed: $($failures -join '; ')"
}
