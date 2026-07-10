param(
    [string]$Port = "COM5",
    [string]$DeviceName = "listener",
    [string]$BluetoothAddress = "E80B41BCC9A1",
    [string]$ListenerExe,
    [string]$FirmwareRepo,
    [string]$OutDir = "tests\artifacts\ble_reconnect_recovery",
    [ValidateSet("validation-injected", "serial-reset", "windows-bluetooth-restart")]
    [string]$DisconnectMode = "validation-injected",
    [int]$InitialReadyTimeoutSeconds = 20,
    [int]$ReconnectReadyTimeoutSeconds = 5,
    [ValidateRange(1, 10)]
    [int]$CycleCount = 1,
    [int]$PostRestartSettleMilliseconds = 0,
    [int]$PostReconnectStreamSmokeAttempts = 3,
    [int]$PostReconnectStreamSmokeRetryDelaySeconds = 3,
    [switch]$SkipBluetoothRestart,
    [switch]$SkipStreamSmoke,
    [switch]$KeepAppRunning
)

$ErrorActionPreference = "Stop"

function ConvertTo-JsonString {
    param([string]$Text)
    if ($null -eq $Text) {
        return "null"
    }
    $builder = [System.Text.StringBuilder]::new()
    [void]$builder.Append('"')
    foreach ($ch in $Text.ToCharArray()) {
        $code = [int][char]$ch
        switch ($code) {
            8 { [void]$builder.Append('\b'); break }
            9 { [void]$builder.Append('\t'); break }
            10 { [void]$builder.Append('\n'); break }
            12 { [void]$builder.Append('\f'); break }
            13 { [void]$builder.Append('\r'); break }
            34 { [void]$builder.Append('\"'); break }
            92 { [void]$builder.Append('\\'); break }
            default {
                if ($code -lt 32) {
                    [void]$builder.Append('\u')
                    [void]$builder.Append($code.ToString('x4', [System.Globalization.CultureInfo]::InvariantCulture))
                } else {
                    [void]$builder.Append($ch)
                }
            }
        }
    }
    [void]$builder.Append('"')
    return $builder.ToString()
}

function ConvertTo-JsonValue {
    param($Value)
    if ($null -eq $Value) {
        return "null"
    }
    if ($Value -is [bool]) {
        if ($Value) { return "true" }
        return "false"
    }
    if ($Value -is [byte] -or $Value -is [sbyte] -or
        $Value -is [int16] -or $Value -is [uint16] -or
        $Value -is [int32] -or $Value -is [uint32] -or
        $Value -is [int64] -or $Value -is [uint64] -or
        $Value -is [single] -or $Value -is [double] -or $Value -is [decimal]) {
        return [System.Convert]::ToString($Value, [System.Globalization.CultureInfo]::InvariantCulture)
    }
    if ($Value -is [datetime]) {
        return ConvertTo-JsonString $Value.ToUniversalTime().ToString("o")
    }
    if ($Value -is [System.Management.Automation.PSCustomObject]) {
        $parts = New-Object System.Collections.Generic.List[string]
        foreach ($property in $Value.PSObject.Properties) {
            $parts.Add((ConvertTo-JsonString ([string]$property.Name)) + ":" + (ConvertTo-JsonValue $property.Value))
        }
        return "{" + ([string]::Join(",", $parts)) + "}"
    }
    if ($Value -is [System.Collections.IDictionary] -or
        $Value -is [System.Collections.Specialized.OrderedDictionary]) {
        $parts = New-Object System.Collections.Generic.List[string]
        foreach ($key in $Value.Keys) {
            $parts.Add((ConvertTo-JsonString ([string]$key)) + ":" + (ConvertTo-JsonValue $Value[$key]))
        }
        return "{" + ([string]::Join(",", $parts)) + "}"
    }
    if ($Value -is [System.Collections.IEnumerable] -and -not ($Value -is [string])) {
        $parts = New-Object System.Collections.Generic.List[string]
        foreach ($item in $Value) {
            $parts.Add((ConvertTo-JsonValue $item))
        }
        return "[" + ([string]::Join(",", $parts)) + "]"
    }
    return ConvertTo-JsonString ([string]$Value)
}

function Get-UtcNowText {
    return (Get-Date).ToUniversalTime().ToString("o")
}

function Resolve-RepoPath {
    param([string]$Path)
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return $Path
    }
    return Join-Path $RepoRoot $Path
}

function Read-NewLogText {
    param(
        [string]$Path,
        [ref]$Offset
    )
    if (-not (Test-Path $Path)) {
        return ""
    }
    $stream = [System.IO.File]::Open(
        $Path,
        [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        [System.IO.FileShare]::ReadWrite
    )
    try {
        if ($stream.Length -lt $Offset.Value) {
            $Offset.Value = 0
        }
        [void]$stream.Seek($Offset.Value, [System.IO.SeekOrigin]::Begin)
        $remaining = [int]($stream.Length - $stream.Position)
        if ($remaining -le 0) {
            return ""
        }
        $bytes = New-Object byte[] $remaining
        $read = $stream.Read($bytes, 0, $remaining)
        $Offset.Value = $stream.Position
        return [System.Text.Encoding]::UTF8.GetString($bytes, 0, $read)
    } finally {
        $stream.Dispose()
    }
}

function Invoke-External {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Label,
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [string[]]$Arguments = @(),
        [string]$WorkingDirectory = $RepoRoot,
        [string]$StdoutPath,
        [string]$StderrPath
    )

    $started = Get-Date
    if (-not $StdoutPath) {
        $StdoutPath = Join-Path $RunDir "$Label.stdout.log"
    }
    if (-not $StderrPath) {
        $StderrPath = Join-Path $RunDir "$Label.stderr.log"
    }

    $process = Start-Process -FilePath $FilePath `
        -ArgumentList $Arguments `
        -WorkingDirectory $WorkingDirectory `
        -WindowStyle Hidden `
        -RedirectStandardOutput $StdoutPath `
        -RedirectStandardError $StderrPath `
        -PassThru
    [void]$process.WaitForExit()
    $finished = Get-Date
    return [ordered]@{
        label = $Label
        file = $FilePath
        arguments = @($Arguments)
        exit_code = [int]$process.ExitCode
        started_at_utc = $started.ToUniversalTime().ToString("o")
        finished_at_utc = $finished.ToUniversalTime().ToString("o")
        duration_ms = [int][Math]::Round(($finished - $started).TotalMilliseconds)
        stdout_path = $StdoutPath
        stderr_path = $StderrPath
        stdout_tail = Get-TextTail -Path $StdoutPath -MaxLines 80
        stderr_tail = Get-TextTail -Path $StderrPath -MaxLines 80
    }
}

function Get-TextTail {
    param(
        [string]$Path,
        [int]$MaxLines = 80
    )
    if (-not (Test-Path $Path)) {
        return ""
    }
    $lines = Get-Content -LiteralPath $Path -Tail $MaxLines -ErrorAction SilentlyContinue
    return [string]::Join("`n", @($lines))
}

function Require-Success {
    param($CommandResult)
    if ([int]$CommandResult.exit_code -ne 0) {
        throw "$($CommandResult.label) failed with exit_code=$($CommandResult.exit_code). See $($CommandResult.stdout_path) and $($CommandResult.stderr_path)."
    }
}

function Test-ListenerExeNeedsBuild {
    param([string]$Path)
    if (-not (Test-Path $Path)) {
        return $true
    }
    $exeTime = (Get-Item -LiteralPath $Path).LastWriteTimeUtc
    $newerSource = Get-ChildItem -LiteralPath (Join-Path $RepoRoot "src-tauri\src") -Recurse -Filter "*.rs" -File |
        Where-Object { $_.LastWriteTimeUtc -gt $exeTime } |
        Select-Object -First 1
    return $null -ne $newerSource
}

function Invoke-SerialReset {
    param([string]$PortName)

    $python = @'
import serial
import sys
import time

port = sys.argv[1]
ser = serial.Serial()
ser.port = port
ser.baudrate = 115200
ser.timeout = 0.05
ser.write_timeout = 2
ser.dsrdtr = False
ser.rtscts = False
ser.dtr = False
ser.rts = False
ser.open()
try:
    ser.setDTR(False)
    ser.setRTS(True)
    time.sleep(0.1)
    ser.setRTS(False)
    time.sleep(0.2)
finally:
    ser.close()
'@

    $tempScript = Join-Path ([System.IO.Path]::GetTempPath()) "listener_ble_reconnect_serial_reset_$PID.py"
    $stdoutPath = Join-Path $RunDir "serial-reset.stdout.log"
    $stderrPath = Join-Path $RunDir "serial-reset.stderr.log"
    try {
        Set-Content -LiteralPath $tempScript -Value $python -Encoding UTF8
        return Invoke-External `
            -Label "serial-reset" `
            -FilePath "python" `
            -Arguments @($tempScript, $PortName) `
            -StdoutPath $stdoutPath `
            -StderrPath $stderrPath
    } finally {
        Remove-Item -LiteralPath $tempScript -Force -ErrorAction SilentlyContinue
    }
}

function Get-LogPath {
    if (-not $env:LOCALAPPDATA) {
        throw "LOCALAPPDATA is not set."
    }
    return Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
}

function Get-PreferencesPath {
    if (-not $env:APPDATA) {
        throw "APPDATA is not set."
    }
    return Join-Path $env:APPDATA "Listener Type\preferences.json"
}

function Set-ReconnectPreferences {
    param([string]$Path)
    $dir = Split-Path -Parent $Path
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    if (Test-Path $Path) {
        $raw = Get-Content -LiteralPath $Path -Raw
        if ([string]::IsNullOrWhiteSpace($raw)) {
            $prefs = [pscustomobject]@{}
        } else {
            $prefs = $raw | ConvertFrom-Json
        }
    } else {
        $prefs = [pscustomobject]@{}
    }

    $prefs | Add-Member -NotePropertyName "dictationInputSource" -NotePropertyValue "embeddedBle" -Force
    $prefs | Add-Member -NotePropertyName "showCapsule" -NotePropertyValue $true -Force
    $prefs | Add-Member -NotePropertyName "startMinimized" -NotePropertyValue $true -Force
    $prefs | Add-Member -NotePropertyName "recordAudioForDebug" -NotePropertyValue $true -Force
    $prefs | Add-Member -NotePropertyName "streamingInsert" -NotePropertyValue $true -Force
    $prefs | Add-Member -NotePropertyName "streamingInsertDefaultMigrated" -NotePropertyValue $true -Force
    $json = $prefs | ConvertTo-Json -Depth 100
    Set-Content -LiteralPath $Path -Value $json -Encoding UTF8

    $saved = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    if ([string]$saved.dictationInputSource -ne "embeddedBle" -or -not [bool]$saved.showCapsule) {
        throw "Failed to persist reconnect preferences to $Path."
    }
}

function Restore-Preferences {
    param(
        [string]$Path,
        [string]$BackupPath,
        [bool]$HadOriginal
    )
    if ($HadOriginal) {
        Copy-Item -LiteralPath $BackupPath -Destination $Path -Force
    } else {
        Remove-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
    }
}

function Wait-LogPattern {
    param(
        [string]$Pattern,
        [int]$TimeoutSeconds,
        [ref]$Offset,
        [ref]$Buffer
    )
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        Start-Sleep -Milliseconds 150
        $newText = Read-NewLogText -Path $LogPath -Offset $Offset
        if ($newText.Length -gt 0) {
            $Buffer.Value += $newText
        }
        if ($Buffer.Value -match $Pattern) {
            return $true
        }
    } while ((Get-Date) -lt $deadline)
    return $false
}

function Get-MatchingLines {
    param(
        [string]$Text,
        [string]$Pattern
    )
    $matchingLines = New-Object System.Collections.Generic.List[string]
    foreach ($line in ($Text -split "(`r`n|`n)")) {
        if ($line -match $Pattern) {
            $matchingLines.Add($line)
        }
    }
    return @($matchingLines)
}

function Get-FirstLogTimestampUtc {
    param(
        [string]$Text,
        [string]$Pattern
    )
    foreach ($line in ($Text -split "(`r`n|`n)")) {
        if ($line -notmatch $Pattern) {
            continue
        }
        $match = [regex]::Match($line, "^(?<ts>\d{4}-\d{2}-\d{2}T[^\s]+)")
        if (-not $match.Success) {
            return $null
        }
        try {
            return ([datetimeoffset]::Parse($match.Groups["ts"].Value)).UtcDateTime
        } catch {
            return $null
        }
    }
    return $null
}

function Get-LastLogTimestampUtc {
    param(
        [string]$Text,
        [string]$Pattern
    )
    $found = $null
    foreach ($line in ($Text -split "(`r`n|`n)")) {
        if ($line -notmatch $Pattern) {
            continue
        }
        $match = [regex]::Match($line, "^(?<ts>\d{4}-\d{2}-\d{2}T[^\s]+)")
        if (-not $match.Success) {
            continue
        }
        try {
            $found = ([datetimeoffset]::Parse($match.Groups["ts"].Value)).UtcDateTime
        } catch {
        }
    }
    return $found
}

function Write-MarkdownReport {
    param($Report)
    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add("# BLE reconnect recovery validation")
    $lines.Add("")
    $lines.Add("- Status: ``$($Report.status)``")
    $lines.Add("- Started: ``$($Report.started_at_utc)``")
    $lines.Add("- Finished: ``$($Report.finished_at_utc)``")
    $lines.Add("- Port/address: ``$($Report.port)`` / ``$($Report.bluetooth_address)``")
    $lines.Add("- Deterministic disconnect: ``$($Report.deterministic_disconnect.method)``; $($Report.deterministic_disconnect.justification)")
    $lines.Add("- Disconnect-to-ready: ``$($Report.reconnect.disconnect_to_ready_ms) ms`` (limit ``$($Report.reconnect.limit_ms) ms``)")
    $lines.Add("- Reconnect cycles: ``$(@($Report.reconnect_cycles).Count)``")
    $lines.Add("- OTA identity after reconnect: ``$($Report.ota_after.status)``")
    $lines.Add("- Audio stream after reconnect: ``$($Report.stream.status)``")
    $lines.Add("- UI capsule evidence: reconnecting lines ``$(@($Report.ui_capsule.reconnecting_lines).Count)``, reconnected lines ``$(@($Report.ui_capsule.reconnected_lines).Count)``")
    $lines.Add("")
    $lines.Add("## Artifacts")
    $lines.Add("")
    $lines.Add("- JSON: ``$($Report.json_path)``")
    $lines.Add("- App log excerpt: ``$($Report.app_log_excerpt_path)``")
    $lines.Add("- Stream smoke report: ``$($Report.stream.report_path)``")
    $lines.Add("")
    $lines.Add("## UI Capsule Log Evidence")
    $lines.Add("")
    foreach ($line in @($Report.ui_capsule.reconnecting_lines + $Report.ui_capsule.reconnected_lines)) {
        $lines.Add("- ``$line``")
    }
    if (@($Report.ui_capsule.reconnecting_lines + $Report.ui_capsule.reconnected_lines).Count -eq 0) {
        $lines.Add("- No recovery capsule lines captured.")
    }
    if (@($Report.reconnect_cycles).Count -gt 1) {
        $lines.Add("")
        $lines.Add("## Reconnect Cycles")
        $lines.Add("")
        foreach ($cycle in @($Report.reconnect_cycles)) {
            $lines.Add("- Cycle ``$($cycle.cycle)`` disconnect-to-ready=``$($cycle.disconnect_to_ready_ms) ms`` within_limit=``$($cycle.within_limit)``")
        }
    }
    $lines.Add("")
    $lines.Add("## Command Results")
    $lines.Add("")
    foreach ($command in @($Report.commands)) {
        $lines.Add("- ``$($command.label)`` exit=``$($command.exit_code)`` duration=``$($command.duration_ms) ms``")
    }
    $lines.Add("")
    $lines.Add("## Errors")
    $lines.Add("")
    if (@($Report.errors).Count -eq 0) {
        $lines.Add("- None")
    } else {
        foreach ($errorText in @($Report.errors)) {
            $lines.Add("- $errorText")
        }
    }
    Set-Content -LiteralPath $Report.markdown_path -Value ([string]::Join("`n", $lines)) -Encoding UTF8
}

$ScriptDir = Split-Path -Parent $PSCommandPath
$RepoRoot = Resolve-Path (Join-Path $ScriptDir "..")
if (-not $FirmwareRepo) {
    $FirmwareRepo = Join-Path (Split-Path -Parent $RepoRoot) "voice-keyboard-firmware"
}
if (-not $ListenerExe) {
    $ListenerExe = Join-Path $RepoRoot "src-tauri\target\debug\listener-type.exe"
}
$OutRoot = Resolve-RepoPath $OutDir
$RunStamp = Get-Date -Format "yyyyMMdd-HHmmss"
$RunDir = Join-Path $OutRoot $RunStamp
New-Item -ItemType Directory -Force -Path $RunDir | Out-Null

$LogPath = Get-LogPath
$LogDir = Split-Path -Parent $LogPath
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$logOffsetValue = if (Test-Path $LogPath) { (Get-Item $LogPath).Length } else { 0 }
$logOffset = [ref]$logOffsetValue
$capturedLog = ""
$commands = New-Object System.Collections.Generic.List[object]
$errors = New-Object System.Collections.Generic.List[string]
$appProcess = $null
$prefsPath = Get-PreferencesPath
$prefsBackup = Join-Path $RunDir "preferences.before.json"
$hadPrefs = Test-Path $prefsPath
$jsonPath = Join-Path $RunDir "ble-reconnect-recovery.json"
$markdownPath = Join-Path $RunDir "ble-reconnect-recovery.md"
$appLogExcerptPath = Join-Path $RunDir "listener-type.reconnect.log"
$validationDisconnectSignalPath = Join-Path $RunDir "validation-disconnect.signal"

$report = [ordered]@{
    report_schema = [ordered]@{
        name = "ble_reconnect_recovery_report"
        schema_version = 1
    }
    status = "FAIL"
    started_at_utc = Get-UtcNowText
    finished_at_utc = $null
    repo_root = [string]$RepoRoot
    firmware_repo = $FirmwareRepo
    listener_exe = $ListenerExe
    port = $Port
    device_name = $DeviceName
    bluetooth_address = ($BluetoothAddress -replace "[^0-9A-Fa-f]", "").ToUpperInvariant()
    deterministic_disconnect = [ordered]@{
        method = $DisconnectMode
        justification = switch ($DisconnectMode) {
            "validation-injected" {
                "A validation-only signal file asks the active WinRT notify wait to emit the same Disconnected/transport_not_ready signal used by real device/GATT status handlers. This deterministically exercises Listener-Type's background listener loss, retry, reconnect, capsule, and ready code path; the run then verifies OTA identity and audio streaming against the real BLE hardware."
            }
            "serial-reset" {
                "Pulsing the device serial RTS reset line reboots the BLE peripheral, causing a real BLE link loss while keeping the test deterministic and local."
            }
            default {
                "Restarting Windows bthserv drops WinRT BLE/GATT sessions deterministically, exercising Listener-Type connection status and GATT session loss handlers without relying on manual range walking."
            }
        }
        skipped = [bool]$SkipBluetoothRestart
        signal_path = $null
    }
    reconnect = [ordered]@{
        limit_ms = $ReconnectReadyTimeoutSeconds * 1000
        disconnect_at_utc = $null
        disconnect_observed_at_utc = $null
        ready_at_utc = $null
        disconnect_to_ready_ms = $null
        within_limit = $false
    }
    reconnect_cycles = @()
    ota_before = $null
    ota_after = $null
    stream = [ordered]@{
        status = "SKIP"
        report_path = $null
        pcm_bytes = $null
        missing_packets = $null
        history_embedded_audio_stats = $null
    }
    ui_capsule = [ordered]@{
        evidence_type = "listener-type.log recovery capsule lines emitted from the same code path that sends capsule:state to the capsule webview"
        reconnecting_lines = @()
        reconnected_lines = @()
        show_lines = @()
    }
    commands = @()
    errors = @()
    json_path = $jsonPath
    markdown_path = $markdownPath
    app_log_excerpt_path = $appLogExcerptPath
}

try {
    if ($hadPrefs) {
        Copy-Item -LiteralPath $prefsPath -Destination $prefsBackup -Force
    }
    Set-ReconnectPreferences -Path $prefsPath
    $report["temporary_preferences"] = [ordered]@{
        path = $prefsPath
        backup_path = if ($hadPrefs) { $prefsBackup } else { $null }
        dictation_input_source = "embeddedBle"
        show_capsule = $true
    }

    if (Test-ListenerExeNeedsBuild -Path $ListenerExe) {
        $frontendDist = Join-Path $RepoRoot "dist"
        if (-not (Test-Path $frontendDist)) {
            throw "Listener executable not found at $ListenerExe and frontend dist is missing at $frontendDist. Run npm run build first or pass -ListenerExe."
        }
        $buildResult = Invoke-External `
            -Label "cargo-build-listener" `
            -FilePath "cargo" `
            -Arguments @("build", "--manifest-path", (Join-Path $RepoRoot "src-tauri\Cargo.toml")) `
            -WorkingDirectory $RepoRoot
        $commands.Add($buildResult)
        Require-Success $buildResult
    }

    $ensureScript = Join-Path $FirmwareRepo "tools\ensure_ble_hid_connection.ps1"
    $restartScript = Join-Path $FirmwareRepo "tools\restart_windows_bluetooth.ps1"
    $otaScript = Join-Path $FirmwareRepo "tools\verify_ble_ota_gatt_discovery.ps1"
    foreach ($path in @($ensureScript, $restartScript, $otaScript)) {
        if (-not (Test-Path $path)) {
            throw "Required firmware helper not found: $path"
        }
    }

    $initialEnsure = Invoke-External `
        -Label "initial-ensure-ble" `
        -FilePath "pwsh" `
        -Arguments @(
            "-NoProfile", "-File", $ensureScript,
            "-DeviceName", $DeviceName,
            "-BluetoothAddress", $BluetoothAddress,
            "-DurationSeconds", ([string]$InitialReadyTimeoutSeconds),
            "-PollIntervalSeconds", "1",
            "-ExitOnReady"
        )
    $commands.Add($initialEnsure)
    Require-Success $initialEnsure

    $otaBefore = Invoke-External `
        -Label "ota-identity-before" `
        -FilePath "pwsh" `
        -Arguments @(
            "-NoProfile", "-File", $otaScript,
            "-DeviceName", $DeviceName,
            "-BluetoothAddress", $BluetoothAddress,
            "-TimeoutSeconds", "20"
        )
    $commands.Add($otaBefore)
    Require-Success $otaBefore
    $report.ota_before = [ordered]@{
        status = "PASS"
        stdout_path = $otaBefore.stdout_path
        stdout_tail = $otaBefore.stdout_tail
    }

    Get-Process listener-type -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    $logOffsetValue = if (Test-Path $LogPath) { (Get-Item $LogPath).Length } else { 0 }
    $logOffset = [ref]$logOffsetValue
    $capturedLog = ""
    $oldHideMain = $env:LISTENER_TYPE_HIDE_MAIN_ON_START
    $oldShowMain = $env:LISTENER_TYPE_SHOW_MAIN_ON_START
    $oldRecordEmbedded = $env:LISTENER_TYPE_RECORD_EMBEDDED_AUDIO_FOR_DEBUG
    $oldDisconnectSignal = $env:LISTENER_TYPE_BLE_VALIDATION_DISCONNECT_SIGNAL_FILE
    try {
        $env:LISTENER_TYPE_HIDE_MAIN_ON_START = "1"
        Remove-Item Env:LISTENER_TYPE_SHOW_MAIN_ON_START -ErrorAction SilentlyContinue
        $env:LISTENER_TYPE_RECORD_EMBEDDED_AUDIO_FOR_DEBUG = "1"
        if ($DisconnectMode -eq "validation-injected") {
            Remove-Item -LiteralPath $validationDisconnectSignalPath -Force -ErrorAction SilentlyContinue
            $env:LISTENER_TYPE_BLE_VALIDATION_DISCONNECT_SIGNAL_FILE = $validationDisconnectSignalPath
            $report.deterministic_disconnect.signal_path = $validationDisconnectSignalPath
        } else {
            Remove-Item Env:LISTENER_TYPE_BLE_VALIDATION_DISCONNECT_SIGNAL_FILE -ErrorAction SilentlyContinue
        }
        $appStdout = Join-Path $RunDir "listener-type-app.stdout.log"
        $appStderr = Join-Path $RunDir "listener-type-app.stderr.log"
        $appProcess = Start-Process -FilePath (Resolve-Path $ListenerExe).Path `
            -WorkingDirectory $RepoRoot `
            -WindowStyle Hidden `
            -RedirectStandardOutput $appStdout `
            -RedirectStandardError $appStderr `
            -PassThru
    } finally {
        if ($null -eq $oldHideMain) { Remove-Item Env:LISTENER_TYPE_HIDE_MAIN_ON_START -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_HIDE_MAIN_ON_START = $oldHideMain }
        if ($null -eq $oldShowMain) { Remove-Item Env:LISTENER_TYPE_SHOW_MAIN_ON_START -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_SHOW_MAIN_ON_START = $oldShowMain }
        if ($null -eq $oldRecordEmbedded) { Remove-Item Env:LISTENER_TYPE_RECORD_EMBEDDED_AUDIO_FOR_DEBUG -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_RECORD_EMBEDDED_AUDIO_FOR_DEBUG = $oldRecordEmbedded }
        if ($null -eq $oldDisconnectSignal) { Remove-Item Env:LISTENER_TYPE_BLE_VALIDATION_DISCONNECT_SIGNAL_FILE -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_BLE_VALIDATION_DISCONNECT_SIGNAL_FILE = $oldDisconnectSignal }
    }

    if (-not (Wait-LogPattern -Pattern "\[embedded-ble\].*background listener notify ready" -TimeoutSeconds $InitialReadyTimeoutSeconds -Offset $logOffset -Buffer ([ref]$capturedLog))) {
        throw "Timed out waiting for Listener-Type background BLE listener ready before disconnect."
    }
    $postReadyLogOffsetValue = if (Test-Path $LogPath) { (Get-Item $LogPath).Length } else { $logOffset.Value }
    $logOffset = [ref]$postReadyLogOffsetValue

    if ($CycleCount -gt 1 -and ($DisconnectMode -ne "validation-injected" -or $SkipBluetoothRestart)) {
        throw "-CycleCount greater than 1 requires validation-injected disconnects without -SkipBluetoothRestart."
    }

    $disconnectPattern = "transport_not_ready|connection status changed to Disconnected|GATT session status changed"
    $readyPattern = "\[embedded-ble\] recovery capsule state=reconnected emitted=true|\[embedded-ble\].*background listener notify ready"

    for ($cycleIndex = 1; $cycleIndex -le $CycleCount; $cycleIndex++) {
        if ($appProcess -and $appProcess.HasExited) {
            throw "Listener-Type exited before reconnect cycle $cycleIndex could run."
        }
        $cycle = [ordered]@{
            cycle = $cycleIndex
            limit_ms = $ReconnectReadyTimeoutSeconds * 1000
            disconnect_at_utc = $null
            disconnect_observed_at_utc = $null
            ready_at_utc = $null
            disconnect_to_ready_ms = $null
            within_limit = $false
        }
        $capturedLog += Read-NewLogText -Path $LogPath -Offset $logOffset

        if ($SkipBluetoothRestart) {
            $cycle.disconnect_at_utc = Get-UtcNowText
        } else {
            $disconnectStarted = Get-Date
            $cycle.disconnect_at_utc = $disconnectStarted.ToUniversalTime().ToString("o")
            if ($DisconnectMode -eq "validation-injected") {
                Set-Content -LiteralPath $validationDisconnectSignalPath -Value "disconnect" -Encoding ASCII
                Start-Sleep -Milliseconds 1000
                Remove-Item -LiteralPath $validationDisconnectSignalPath -Force -ErrorAction SilentlyContinue
            } elseif ($DisconnectMode -eq "serial-reset") {
                $resetResult = Invoke-SerialReset -PortName $Port
                $commands.Add($resetResult)
                Require-Success $resetResult
            } else {
                $restartResult = Invoke-External `
                    -Label "restart-windows-bluetooth" `
                    -FilePath "pwsh" `
                    -Arguments @("-NoProfile", "-File", $restartScript, "-RestartPanAdapter")
                $commands.Add($restartResult)
                Require-Success $restartResult
            }
        }

        if ($PostRestartSettleMilliseconds -gt 0) {
            Start-Sleep -Milliseconds $PostRestartSettleMilliseconds
        }

        $observeDisconnectDeadline = (Get-Date).AddSeconds([Math]::Max(30, $ReconnectReadyTimeoutSeconds))
        $disconnectObservedAt = $null
        $postDisconnectLog = ""
        while ((Get-Date) -lt $observeDisconnectDeadline) {
            $newLogText = Read-NewLogText -Path $LogPath -Offset $logOffset
            if ($newLogText.Length -gt 0) {
                $capturedLog += $newLogText
                $postDisconnectLog += $newLogText
                $disconnectObservedAt = Get-FirstLogTimestampUtc -Text $postDisconnectLog -Pattern $disconnectPattern
                if ($disconnectObservedAt) {
                    break
                }
            }
            Start-Sleep -Milliseconds 100
        }
        if (-not $disconnectObservedAt) {
            throw "Listener-Type did not log a BLE/GATT disconnect after deterministic disconnect trigger cycle $cycleIndex."
        }
        $cycle.disconnect_observed_at_utc = $disconnectObservedAt.ToUniversalTime().ToString("o")

        $readyDeadline = (Get-Date).AddSeconds($ReconnectReadyTimeoutSeconds)
        $ready = $false
        $readyAt = $null
        while ((Get-Date) -lt $readyDeadline) {
            $newLogText = Read-NewLogText -Path $LogPath -Offset $logOffset
            if ($newLogText.Length -gt 0) {
                $capturedLog += $newLogText
                $postDisconnectLog += $newLogText
            }
            $readyAt = Get-LastLogTimestampUtc -Text $postDisconnectLog -Pattern $readyPattern
            if ($readyAt -and $readyAt -ge $disconnectObservedAt) {
                $ready = $true
                break
            }
            Start-Sleep -Milliseconds 100
        }
        $capturedLog += Read-NewLogText -Path $LogPath -Offset $logOffset
        if (-not $ready) {
            throw "BLE reconnect cycle $cycleIndex did not return to notify-ready within $ReconnectReadyTimeoutSeconds seconds."
        }
        $cycle.ready_at_utc = $readyAt.ToUniversalTime().ToString("o")
        $cycle.disconnect_to_ready_ms = [int][Math]::Round(($readyAt.ToUniversalTime() - $disconnectObservedAt.ToUniversalTime()).TotalMilliseconds)
        $cycle.within_limit = ([int]$cycle.disconnect_to_ready_ms -le [int]$cycle.limit_ms)
        if (-not [bool]$cycle.within_limit) {
            throw "Disconnect-to-ready timing exceeded $($cycle.limit_ms) ms in cycle ${cycleIndex}: $($cycle.disconnect_to_ready_ms) ms."
        }
        $report["reconnect"] = $cycle
        $report["reconnect_cycles"] = @($report["reconnect_cycles"] + $cycle)
        $postReadyLogOffsetValue = if (Test-Path $LogPath) { (Get-Item $LogPath).Length } else { $logOffset.Value }
        $logOffset = [ref]$postReadyLogOffsetValue
    }

    $otaAfter = Invoke-External `
        -Label "ota-identity-after" `
        -FilePath "pwsh" `
        -Arguments @(
            "-NoProfile", "-File", $otaScript,
            "-DeviceName", $DeviceName,
            "-BluetoothAddress", $BluetoothAddress,
            "-TimeoutSeconds", "20"
        )
    $commands.Add($otaAfter)
    Require-Success $otaAfter
    $report.ota_after = [ordered]@{
        status = "PASS"
        stdout_path = $otaAfter.stdout_path
        stdout_tail = $otaAfter.stdout_tail
    }

    if (-not $KeepAppRunning -and $appProcess -and -not $appProcess.HasExited) {
        Stop-Process -Id $appProcess.Id -Force -ErrorAction SilentlyContinue
        $appProcess = $null
        Start-Sleep -Milliseconds 500
    }

    if (-not $SkipStreamSmoke) {
        $streamOut = Join-Path $RunDir "stream-smoke"
        $attemptCount = [Math]::Max(1, $PostReconnectStreamSmokeAttempts)
        $streamResult = $null
        for ($attempt = 1; $attempt -le $attemptCount; $attempt++) {
            if ($attempt -gt 1) {
                Start-Sleep -Seconds $PostReconnectStreamSmokeRetryDelaySeconds
            }
            $attemptOut = Join-Path $streamOut "attempt-$attempt"
            $streamResult = Invoke-External `
                -Label "post-reconnect-stream-smoke-attempt-$attempt" `
                -FilePath "pwsh" `
                -Arguments @(
                    "-NoProfile", "-File", (Join-Path $RepoRoot "tools\run_ble_stream_smoke.ps1"),
                    "-Port", $Port,
                    "-DeviceName", $DeviceName,
                    "-BluetoothAddress", $BluetoothAddress,
                    "-OutDir", $attemptOut,
                    "-TimeoutMs", "90000",
                    "-NotifyReadyTimeoutSeconds", "35",
                    "-NoResetBeforeCapture",
                    "-VerifyHistory",
                    "-FailOnMissingPackets",
                    "-AudioProfile", "punctuation",
                    "-Sentence", "你好，开始测试。",
                    "-ExpectedText", "你好，开始测试。",
                    "-PlaybackVolumePercent", "80"
                ) `
                -WorkingDirectory $RepoRoot
            $commands.Add($streamResult)
            if ([int]$streamResult.exit_code -eq 0) {
                break
            }
        }
        Require-Success $streamResult
        $streamReportPath = Get-ChildItem -LiteralPath $streamOut -Filter "ble-stream-smoke.*.json" -File -Recurse |
            Sort-Object LastWriteTimeUtc -Descending |
            Select-Object -First 1
        if (-not $streamReportPath) {
            throw "Post-reconnect stream smoke passed but no JSON report was found under $streamOut."
        }
        $streamJson = Get-Content -LiteralPath $streamReportPath.FullName -Raw | ConvertFrom-Json
        $report.stream.status = [string]$streamJson.status
        $report.stream.report_path = $streamReportPath.FullName
        $report.stream.pcm_bytes = [int64]$streamJson.pcm_bytes
        $report.stream.missing_packets = [int64]$streamJson.missing_packets
        $report.stream.history_embedded_audio_stats = $streamJson.history_session.embeddedAudioStats
        if ([string]$streamJson.status -eq "FAIL") {
            throw "Post-reconnect stream smoke JSON status is FAIL."
        }
        if ([int64]$streamJson.pcm_bytes -le 0) {
            throw "Post-reconnect stream smoke did not capture PCM audio."
        }
    }

    $capturedLog += Read-NewLogText -Path $LogPath -Offset $logOffset
    $report.ui_capsule.reconnecting_lines = @(Get-MatchingLines -Text $capturedLog -Pattern "\[embedded-ble\] recovery capsule state=reconnecting")
    $report.ui_capsule.reconnected_lines = @(Get-MatchingLines -Text $capturedLog -Pattern "\[embedded-ble\] recovery capsule state=reconnected")
    $report.ui_capsule.show_lines = @(Get-MatchingLines -Text $capturedLog -Pattern "\[capsule\] show request state=Recording")
    if (@($report.ui_capsule.reconnecting_lines).Count -eq 0) {
        throw "No reconnecting recovery capsule log line was captured."
    }
    if (@($report.ui_capsule.reconnected_lines).Count -eq 0) {
        throw "No reconnected recovery capsule log line was captured."
    }

    $report.status = "PASS"
} catch {
    $errors.Add($_.Exception.Message)
} finally {
    try {
        $capturedLog += Read-NewLogText -Path $LogPath -Offset $logOffset
    } catch {
    }
    Set-Content -LiteralPath $appLogExcerptPath -Value $capturedLog -Encoding UTF8
    if (-not $KeepAppRunning -and $appProcess -and -not $appProcess.HasExited) {
        Stop-Process -Id $appProcess.Id -Force -ErrorAction SilentlyContinue
    }
    try {
        Restore-Preferences -Path $prefsPath -BackupPath $prefsBackup -HadOriginal $hadPrefs
    } catch {
        $errors.Add("Failed to restore preferences: $($_.Exception.Message)")
    }
    $report["finished_at_utc"] = Get-UtcNowText
    $report.ui_capsule.reconnecting_lines = @(Get-MatchingLines -Text $capturedLog -Pattern "\[embedded-ble\] recovery capsule state=reconnecting")
    $report.ui_capsule.reconnected_lines = @(Get-MatchingLines -Text $capturedLog -Pattern "\[embedded-ble\] recovery capsule state=reconnected")
    $report.ui_capsule.show_lines = @(Get-MatchingLines -Text $capturedLog -Pattern "\[capsule\] show request state=Recording")
    $report["commands"] = @($commands.ToArray())
    $report["errors"] = @($errors.ToArray())
    if (@($errors).Count -gt 0 -and $report["status"] -ne "PASS") {
        $report["status"] = "FAIL"
    }
    $compact = ConvertTo-JsonValue $report
    Set-Content -LiteralPath $jsonPath -Value $compact -Encoding UTF8
    Write-MarkdownReport -Report $report
    Write-Output $compact
    Write-Output "ble_reconnect_recovery_result_json=$compact"
}

if ($report["status"] -ne "PASS") {
    exit 1
}
