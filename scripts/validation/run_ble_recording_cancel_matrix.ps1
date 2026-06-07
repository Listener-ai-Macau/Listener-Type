[CmdletBinding()]
param(
    [string]$Port = "COMx",
    [string]$DeviceName = "listener",
    [string]$BluetoothAddress = "",
    [string]$OutDir = "artifacts\voice-keyboard-production-readiness-1.4-ble-cancel",
    [string]$ListenerExe = "",
    [int]$TimeoutMs = 90000,
    [int]$SilentAudioMs = 3500,
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

function Resolve-RepoPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [string]$RepoRoot
    )

    if ([System.IO.Path]::IsPathRooted($Path)) {
        return $Path
    }
    return Join-Path $RepoRoot $Path
}

function Get-ValidationUtcNow {
    return (Get-Date).ToUniversalTime().ToString("o")
}

function Convert-ValidationJson {
    param($Value)
    return ($Value | ConvertTo-Json -Depth 16 -Compress)
}

function Test-PowerShellFileParses {
    param([Parameter(Mandatory = $true)][string]$Path)

    $tokens = $null
    $errors = $null
    [System.Management.Automation.Language.Parser]::ParseFile(
        $Path,
        [ref]$tokens,
        [ref]$errors
    ) | Out-Null

    if ($errors -and $errors.Count -gt 0) {
        $messages = $errors | ForEach-Object { "$($_.Extent.StartLineNumber):$($_.Extent.StartColumnNumber) $($_.Message)" }
        throw "PowerShell parser errors in ${Path}: $([string]::Join('; ', $messages))"
    }
}

function Resolve-UniqueSerialPort {
    param([string]$RequestedPort)

    if (-not [string]::IsNullOrWhiteSpace($RequestedPort) -and $RequestedPort -ne "COMx") {
        return $RequestedPort
    }

    $items = @()
    try {
        $items = @(
            Get-CimInstance Win32_SerialPort -ErrorAction Stop |
                Where-Object { $_.DeviceID -match '^COM\d+$' } |
                ForEach-Object {
                    [pscustomobject]@{
                        Port = [string]$_.DeviceID
                        Label = "$($_.Name) $($_.Description) $($_.PNPDeviceID)"
                    }
                }
        )
    } catch {
        $items = @()
    }

    if (-not $items -or $items.Count -eq 0) {
        $items = @(
            [System.IO.Ports.SerialPort]::GetPortNames() |
                Where-Object { $_ -match '^COM\d+$' } |
                Sort-Object |
                ForEach-Object {
                    [pscustomobject]@{
                        Port = [string]$_
                        Label = [string]$_
                    }
                }
        )
    }

    $preferred = @(
        $items |
            Where-Object {
                $_.Label -match 'ESP|USB|UART|CP210|CH340|CH910|Silicon|USB Serial|WCH|serial'
            }
    )

    if ($preferred.Count -eq 1) {
        return [string]$preferred[0].Port
    }
    if ($items.Count -eq 1) {
        return [string]$items[0].Port
    }
    if ($preferred.Count -gt 1) {
        $labels = $preferred | ForEach-Object { "$($_.Port):$($_.Label)" }
        throw "Multiple likely ESP32 serial ports found; pass -Port explicitly. Candidates: $([string]::Join(', ', $labels))"
    }
    if ($items.Count -gt 1) {
        $labels = $items | ForEach-Object { "$($_.Port):$($_.Label)" }
        throw "Multiple serial ports found; pass -Port explicitly. Candidates: $([string]::Join(', ', $labels))"
    }

    throw "No serial ports found for COMx."
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$smokeScript = Join-Path $repoRoot "tools\embedded_audio_replay\run_ble_stream_smoke.ps1"
$resolvedOutDir = Resolve-RepoPath -Path $OutDir -RepoRoot $repoRoot
New-Item -ItemType Directory -Force -Path $resolvedOutDir | Out-Null

if ([string]::IsNullOrWhiteSpace($ListenerExe)) {
    $ListenerExe = Join-Path $repoRoot "src-tauri\target\debug\listener-type.exe"
} else {
    $ListenerExe = Resolve-RepoPath -Path $ListenerExe -RepoRoot $repoRoot
}

Test-PowerShellFileParses -Path $PSCommandPath
Test-PowerShellFileParses -Path $smokeScript

$startedAt = Get-Date
$runStamp = $startedAt.ToString("yyyyMMdd-HHmmss")
$summaryPath = Join-Path $resolvedOutDir "ble-recording-cancel-matrix.$runStamp.json"

if ($DryRun) {
    $summary = [ordered]@{
        report_schema = "ble_recording_cancel_matrix"
        status = "PASS"
        dry_run = $true
        started_at_utc = $startedAt.ToUniversalTime().ToString("o")
        repo_root = $repoRoot
        smoke_script = $smokeScript
        listener_exe = $ListenerExe
        requested_port = $Port
        trigger = "desktop-cancel"
        checks = @(
            "PowerShell parser accepts wrapper and smoke script",
            "Wrapper will resolve COMx at hardware execution time",
            "Smoke trigger uses desktop capsule cancel click; serial is only start and cleanup",
            "Report must show PASS, desktop_cancel_report.clicked=true, no transcript/inserted text, and BLE capture cancel log"
        )
    }
    $summaryJson = Convert-ValidationJson $summary
    Set-Content -LiteralPath $summaryPath -Value $summaryJson -Encoding UTF8
    Write-Output $summaryJson
    Write-Output "ble_recording_cancel_matrix_result_json=$summaryJson"
    Write-Output "ble_recording_cancel_matrix_report=$summaryPath"
    exit 0
}

if (-not (Test-Path -LiteralPath $ListenerExe)) {
    throw "Listener executable not found: $ListenerExe. Build it outside the hardware lock before running this validation."
}

$resolvedPort = Resolve-UniqueSerialPort -RequestedPort $Port
Write-Host "ble-recording-cancel-matrix: using port=$resolvedPort device=$DeviceName out=$resolvedOutDir"

$smokeArgs = @(
    "-NoProfile",
    "-File", $smokeScript,
    "-TriggerMode", "desktop-cancel",
    "-Port", $resolvedPort,
    "-DeviceName", $DeviceName,
    "-BluetoothAddress", $BluetoothAddress,
    "-SilentAudio",
    "-ExpectNoText",
    "-SilentAudioMs", ([string]$SilentAudioMs),
    "-PreRecordDelayMs", "250",
    "-PostPlaybackRecordMs", "250",
    "-TimeoutMs", ([string]$TimeoutMs),
    "-RecordingStartTimeoutMs", "5000",
    "-NoNotificationTimeoutSeconds", "12",
    "-MaxMissingPackets", "0",
    "-FailOnMissingPackets",
    "-VerifyInsertion",
    "-OutDir", $resolvedOutDir,
    "-ListenerExe", $ListenerExe
)

$smokeOutput = & pwsh @smokeArgs 2>&1
$smokeExit = $LASTEXITCODE
$smokeOutput | ForEach-Object { Write-Output $_ }

$smokeReport = Get-ChildItem -LiteralPath $resolvedOutDir -Filter "ble-stream-smoke.*.json" |
    Sort-Object LastWriteTimeUtc -Descending |
    Select-Object -First 1

if (-not $smokeReport) {
    throw "BLE stream smoke did not produce a report under $resolvedOutDir"
}

$report = Get-Content -LiteralPath $smokeReport.FullName -Raw | ConvertFrom-Json
$errors = New-Object System.Collections.Generic.List[string]

if ($smokeExit -ne 0) {
    $errors.Add("smoke process exited with code $smokeExit")
}
if ([string]$report.status -ne "PASS") {
    $errors.Add("smoke report status is $($report.status)")
}
if ([string]$report.trigger -ne "desktop-cancel") {
    $errors.Add("smoke trigger is $($report.trigger), expected desktop-cancel")
}
if (-not [bool]$report.expect_no_text) {
    $errors.Add("smoke report did not run with ExpectNoText")
}
if (-not $report.desktop_cancel_report -or -not [bool]$report.desktop_cancel_report.clicked) {
    $errors.Add("desktop capsule cancel click was not recorded")
}
if (-not $report.serial_report -or -not [bool]$report.serial_report.recording_start_seen) {
    $errors.Add("firmware recording start was not confirmed")
}
if (-not $report.serial_report -or -not [bool]$report.serial_report.cancel_completed) {
    $errors.Add("firmware cleanup cancel was not confirmed")
}
if (-not [string]::IsNullOrWhiteSpace([string]$report.expected_stream_failure)) {
    $errors.Add("BLE stream failed instead of returning Ok after desktop cancel: $($report.expected_stream_failure)")
}
if (-not [string]::IsNullOrWhiteSpace([string]$report.final_text)) {
    $errors.Add("final text was produced after cancel")
}
if (-not [string]::IsNullOrWhiteSpace([string]$report.inserted_text)) {
    $errors.Add("target insertion received text after cancel")
}
if ($report.verification_errors -and $report.verification_errors.Count -gt 0) {
    $errors.Add("smoke verification errors: $([string]::Join('; ', @($report.verification_errors)))")
}

$summaryStatus = if ($errors.Count -eq 0) { "PASS" } else { "FAIL" }
$summary = [ordered]@{
    report_schema = "ble_recording_cancel_matrix"
    status = $summaryStatus
    dry_run = $false
    started_at_utc = $startedAt.ToUniversalTime().ToString("o")
    finished_at_utc = Get-ValidationUtcNow
    repo_root = $repoRoot
    port = $resolvedPort
    device_name = $DeviceName
    bluetooth_address = $BluetoothAddress
    listener_exe = $ListenerExe
    smoke_report_path = $smokeReport.FullName
    smoke_status = [string]$report.status
    smoke_trigger = [string]$report.trigger
    desktop_cancel_report = $report.desktop_cancel_report
    serial_report = $report.serial_report
    pcm_bytes = [int]$report.pcm_bytes
    missing_packets = [int]$report.missing_packets
    expected_stream_failure = [string]$report.expected_stream_failure
    final_text = [string]$report.final_text
    inserted_text = [string]$report.inserted_text
    verification_errors = @($errors)
}
$summaryJson = Convert-ValidationJson $summary
Set-Content -LiteralPath $summaryPath -Value $summaryJson -Encoding UTF8
Write-Output $summaryJson
Write-Output "ble_recording_cancel_matrix_result_json=$summaryJson"
Write-Output "ble_recording_cancel_matrix_report=$summaryPath"

if ($errors.Count -gt 0) {
    throw "BLE recording cancel matrix failed: $([string]::Join('; ', @($errors)))"
}
