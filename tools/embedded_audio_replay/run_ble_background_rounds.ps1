param(
    [Parameter(Mandatory = $true)]
    [string]$Port,
    [ValidateSet("generated-key3")]
    [string]$TriggerMode = "generated-key3",
    [string]$DeviceName = "listener",
    [string]$BluetoothAddress = "",
    [string]$ListenerExe = "",
    [string]$FirmwareRepo = "",
    [Parameter(Mandatory = $true)]
    [string]$RoundSpecJson,
    [string]$OutDir = "artifacts\embedded_stream_background_rounds",
    [int]$NotifyReadyTimeoutSeconds = 35,
    [int]$RecordingStartTimeoutMs = 2500,
    [int]$PreRecordDelayMs = 300,
    [int]$PostPlaybackRecordMs = 500,
    [double]$MaxHiddenToVisibleSeconds = 1.0,
    [switch]$SkipAccuracyGate,
    [switch]$SkipEnsureBle
)

$ErrorActionPreference = "Stop"

function Get-UtcNow {
    return (Get-Date).ToUniversalTime().ToString("o")
}

function Resolve-RepoPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path $RepoRoot $Path))
}

function Write-Utf8NoBomText {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Text
    )
    $encoding = [System.Text.UTF8Encoding]::new($false)
    [System.IO.File]::WriteAllText($Path, $Text, $encoding)
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

function Initialize-CapsuleWindowProbe {
    if ("ListenerTypeBackgroundRoundWindow" -as [type]) {
        return
    }
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class ListenerTypeBackgroundRoundWindow
{
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetWindowText(IntPtr hWnd, StringBuilder lpString, int nMaxCount);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetWindowTextLength(IntPtr hWnd);
}
'@
}

function Test-CapsuleWindowVisible {
    Initialize-CapsuleWindowProbe
    $script:CapsuleVisible = $false
    $callback = [ListenerTypeBackgroundRoundWindow+EnumWindowsProc]{
        param([IntPtr]$hWnd, [IntPtr]$lParam)
        if (-not [ListenerTypeBackgroundRoundWindow]::IsWindowVisible($hWnd)) {
            return $true
        }
        $length = [ListenerTypeBackgroundRoundWindow]::GetWindowTextLength($hWnd)
        if ($length -le 0) {
            return $true
        }
        $buffer = [System.Text.StringBuilder]::new($length + 1)
        [void][ListenerTypeBackgroundRoundWindow]::GetWindowText($hWnd, $buffer, $buffer.Capacity)
        if ($buffer.ToString() -like "*Listener Type Capsule*") {
            $script:CapsuleVisible = $true
            return $false
        }
        return $true
    }
    [void][ListenerTypeBackgroundRoundWindow]::EnumWindows($callback, [IntPtr]::Zero)
    return [bool]$script:CapsuleVisible
}

function Wait-CapsuleVisible {
    param([int]$TimeoutMs)
    $deadline = (Get-Date).AddMilliseconds($TimeoutMs)
    do {
        if (Test-CapsuleWindowVisible) {
            return Get-Date
        }
        Start-Sleep -Milliseconds 25
    } while ((Get-Date) -lt $deadline)
    return $null
}

function Wait-CapsuleHidden {
    param([int]$TimeoutMs = 8000)
    $started = Get-Date
    $deadline = $started.AddMilliseconds($TimeoutMs)
    do {
        if (-not (Test-CapsuleWindowVisible)) {
            return [pscustomobject]@{
                hidden = $true
                hidden_at = Get-Date
                wait_ms = [int]((Get-Date) - $started).TotalMilliseconds
            }
        }
        Start-Sleep -Milliseconds 50
    } while ((Get-Date) -lt $deadline)
    return [pscustomobject]@{
        hidden = $false
        hidden_at = $null
        wait_ms = [int]((Get-Date) - $started).TotalMilliseconds
    }
}

function Open-SerialPort {
    param([Parameter(Mandatory = $true)][string]$PortName)
    $lastError = $null
    for ($attempt = 1; $attempt -le 8; $attempt++) {
        $serial = [System.IO.Ports.SerialPort]::new(
            $PortName,
            115200,
            [System.IO.Ports.Parity]::None,
            8,
            [System.IO.Ports.StopBits]::One
        )
        $serial.ReadTimeout = 100
        $serial.WriteTimeout = 3000
        $serial.DtrEnable = $false
        $serial.RtsEnable = $false
        try {
            $serial.Open()
            $serial.DtrEnable = $false
            $serial.RtsEnable = $false
            return $serial
        } catch {
            $lastError = $_
            try { $serial.Dispose() } catch {}
            Start-Sleep -Milliseconds (200 * $attempt)
        }
    }
    throw "Unable to open serial port $PortName after retries: $($lastError.Exception.Message)"
}

function Read-SerialUntil {
    param(
        [Parameter(Mandatory = $true)]$Serial,
        [Parameter(Mandatory = $true)][datetime]$Deadline,
        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [System.Collections.Generic.List[string]]$Lines,
        [string]$Pattern = ""
    )
    while ((Get-Date) -lt $Deadline) {
        try {
            $line = $Serial.ReadLine().Trim()
            if ($line) {
                $Lines.Add($line)
                if ($Pattern -and $line -match $Pattern) {
                    return $line
                }
            }
        } catch [System.TimeoutException] {
        }
    }
    return $null
}

function Send-SerialCommand {
    param(
        [Parameter(Mandatory = $true)]$Serial,
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [System.Collections.Generic.List[string]]$Lines
    )
    $Lines.Add("> $Command")
    $Serial.Write("$Command`n")
    $Serial.BaseStream.Flush()
}

function Invoke-GeneratedRecordingStop {
    param(
        [Parameter(Mandatory = $true)]$Serial,
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $true)][string]$TriggerMode,
        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [System.Collections.Generic.List[string]]$Lines
    )

    $stopPattern = "recording stop source=|record session stop requested|stream session stop queued"
    $readyPattern = "audio transport state: .* -> stream_ready|audio notify subscription changed: .* notify=1|audio notify subscription restored before connect|connection established"
    $maxAttempts = 3
    for ($attempt = 1; $attempt -le $maxAttempts; $attempt++) {
        if ($attempt -gt 1) {
            $Lines.Add("# retry generated stop attempt=$attempt")
        }
        Send-SerialCommand -Serial $Serial -Command $Command -Lines $Lines
        $stopLine = Read-SerialUntil `
            -Serial $Serial `
            -Deadline (Get-Date).AddMilliseconds(3500) `
            -Lines $Lines `
            -Pattern $stopPattern
        if ($stopLine) {
            return $stopLine
        }
        [void](Read-SerialUntil `
            -Serial $Serial `
            -Deadline (Get-Date).AddMilliseconds(6500) `
            -Lines $Lines `
            -Pattern $readyPattern)
    }
    return $null
}

function Get-GeneratedButtonEvidence {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [string[]]$Lines,
        [Parameter(Mandatory = $true)]
        [string]$TriggerMode
    )

    $logical = "KEY3"
    $ackPattern = "~KEY:GENERATED logical=$logical gesture=single result=ESP_OK"
    $ackCount = @($Lines | Where-Object { $_ -like "*$ackPattern*" }).Count
    $timingPatterns = @(
        "custom key generated single-click queued: logical=KEY3",
        "custom key generated single-click armed: logical=KEY3",
        "custom key generated single-click completed: logical=KEY3",
        "custom key raw transition: logical=KEY3",
        "custom key stable transition: logical=KEY3",
        "custom key release: logical=KEY3"
    )
    $singlePatterns = @(
        "custom key single pending: logical=KEY3",
        "custom key fallback queued: logical=KEY3"
    )
    $timingSeen = [bool](@($Lines | Where-Object {
        $line = $_
        @($timingPatterns | Where-Object { $line -like "*$_*" }).Count -gt 0
    }).Count -gt 0)
    $singleSeen = [bool](@($Lines | Where-Object {
        $line = $_
        @($singlePatterns | Where-Object { $line -like "*$_*" }).Count -gt 0
    }).Count -gt 0)

    return [ordered]@{
        logical = $logical
        ack_count = $ackCount
        start_stop_ack_seen = $ackCount -ge 2
        timing_seen = $timingSeen
        single_seen = $singleSeen
        summary = "KEY3 generated press/release -> custom key debounce/single-click/F15 path"
    }
}

function Get-HistoryPath {
    if ($env:APPDATA) {
        return Join-Path $env:APPDATA "Listener Type\history.json"
    }
    return $null
}

function Get-PreferencesPath {
    if (-not $env:APPDATA) {
        throw "APPDATA is not set."
    }
    return Join-Path $env:APPDATA "Listener Type\preferences.json"
}

function Add-TopLevelJsonField {
    param(
        [Parameter(Mandatory = $true)][string]$Json,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$ValueJson
    )
    $index = $Json.LastIndexOf("}")
    if ($index -lt 0) {
        throw "Cannot add $Name to malformed preferences JSON."
    }
    $prefix = $Json.Substring(0, $index).TrimEnd()
    $suffix = $Json.Substring($index)
    $separator = if ($prefix.EndsWith("{")) { "`n  " } else { ",`n  " }
    return $prefix + $separator + "`"$Name`": $ValueJson`n" + $suffix
}

function Set-JsonStringField {
    param(
        [Parameter(Mandatory = $true)][string]$Json,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Value
    )
    $pattern = "(?m)(^(\s*)`"" + [Regex]::Escape($Name) + "`"\s*:\s*)`"([^`"\\]|\\.)*`""
    $escapedValue = $Value.Replace("\", "\\").Replace("`"", "\`"")
    $regex = [Regex]::new($pattern)
    if ($regex.IsMatch($Json)) {
        return $regex.Replace(
            $Json,
            [System.Text.RegularExpressions.MatchEvaluator]{ param($match) $match.Groups[1].Value + "`"$escapedValue`"" },
            1
        )
    }
    return Add-TopLevelJsonField -Json $Json -Name $Name -ValueJson "`"$escapedValue`""
}

function Set-JsonBoolField {
    param(
        [Parameter(Mandatory = $true)][string]$Json,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][bool]$Value
    )
    $valueJson = if ($Value) { "true" } else { "false" }
    $pattern = "(?m)(^(\s*)`"" + [Regex]::Escape($Name) + "`"\s*:\s*)(true|false)"
    $regex = [Regex]::new($pattern)
    if ($regex.IsMatch($Json)) {
        return $regex.Replace(
            $Json,
            [System.Text.RegularExpressions.MatchEvaluator]{ param($match) $match.Groups[1].Value + $valueJson },
            1
        )
    }
    return Add-TopLevelJsonField -Json $Json -Name $Name -ValueJson $valueJson
}

function Ensure-JsonObjectProperty {
    param(
        [Parameter(Mandatory = $true)]$Object,
        [Parameter(Mandatory = $true)][string]$Name
    )
    if ($null -eq $Object.PSObject.Properties[$Name] -or $null -eq $Object.$Name) {
        $Object | Add-Member -NotePropertyName $Name -NotePropertyValue ([pscustomobject]@{}) -Force
    }
    return $Object.$Name
}

function Set-BackgroundRoundPreferences {
    param([Parameter(Mandatory = $true)][string]$Path)
    $dir = Split-Path -Parent $Path
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    if (Test-Path $Path) {
        $raw = Get-Content -LiteralPath $Path -Raw
        $prefs = if ([string]::IsNullOrWhiteSpace($raw)) { [pscustomobject]@{} } else { $raw | ConvertFrom-Json }
    } else {
        $prefs = [pscustomobject]@{}
    }

    $prefs | Add-Member -NotePropertyName "dictationInputSource" -NotePropertyValue "embeddedBle" -Force
    $prefs | Add-Member -NotePropertyName "dictationInputSourceUserOverridden" -NotePropertyValue $true -Force
    $prefs | Add-Member -NotePropertyName "showCapsule" -NotePropertyValue $true -Force
    $prefs | Add-Member -NotePropertyName "recordAudioForDebug" -NotePropertyValue $true -Force
    $prefs | Add-Member -NotePropertyName "streamingInsert" -NotePropertyValue $true -Force
    $prefs | Add-Member -NotePropertyName "streamingInsertDefaultMigrated" -NotePropertyValue $true -Force
    $prefs | Add-Member -NotePropertyName "deviceCustomKeysDefaultMigrated" -NotePropertyValue $true -Force

    $keys = Ensure-JsonObjectProperty -Object $prefs -Name "deviceCustomKeys"
    $key3 = Ensure-JsonObjectProperty -Object $keys -Name "key3"
    $key3 | Add-Member -NotePropertyName "action" -NotePropertyValue "dictation" -Force
    $key3 | Add-Member -NotePropertyName "appPage" -NotePropertyValue "settingsShortcuts" -Force
    $key3 | Add-Member -NotePropertyName "externalAppPath" -NotePropertyValue "" -Force
    $key3 | Add-Member -NotePropertyName "pasteTemplate" -NotePropertyValue "" -Force
    $key3 | Add-Member -NotePropertyName "shortcut" -NotePropertyValue $null -Force

    $json = $prefs | ConvertTo-Json -Depth 32
    Write-Utf8NoBomText -Path $Path -Text $json

    $saved = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    if (
        [string]$saved.dictationInputSource -ne "embeddedBle" -or
        -not [bool]$saved.showCapsule -or
        [string]$saved.deviceCustomKeys.key3.action -ne "dictation"
    ) {
        throw "Failed to persist background round preferences to $Path."
    }
}

function Restore-Preferences {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$BackupPath,
        [Parameter(Mandatory = $true)][bool]$HadOriginal
    )
    if ($HadOriginal) {
        Copy-Item -LiteralPath $BackupPath -Destination $Path -Force
    } else {
        Remove-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
    }
}

function Stop-ListenerTypeProcesses {
    $existing = @(Get-Process listener-type -ErrorAction SilentlyContinue)
    if ($existing.Count -eq 0) {
        return
    }
    foreach ($process in $existing) {
        try { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue } catch {}
    }
    foreach ($process in $existing) {
        try { Wait-Process -Id $process.Id -Timeout 5 -ErrorAction SilentlyContinue } catch {}
    }
}

function Read-HistorySessions {
    $path = Get-HistoryPath
    if (-not $path -or -not (Test-Path $path)) {
        return @()
    }
    $raw = Get-Content -Path $path -Raw -ErrorAction SilentlyContinue
    if ([string]::IsNullOrWhiteSpace($raw)) {
        return @()
    }
    $parsed = $raw | ConvertFrom-Json
    if ($parsed -is [System.Array]) {
        return @($parsed | ForEach-Object { $_ })
    }
    return @($parsed)
}

function Find-LatestEmbeddedHistorySession {
    param([Parameter(Mandatory = $true)][datetime]$StartedAt)
    $threshold = $StartedAt.ToUniversalTime().AddSeconds(-1)
    $candidates = @()
    foreach ($session in @(Read-HistorySessions)) {
        if (-not $session.createdAt) {
            continue
        }
        try {
            $created = ([datetime]::Parse([string]$session.createdAt)).ToUniversalTime()
        } catch {
            continue
        }
        if ($created -lt $threshold) {
            continue
        }
        $score = 0
        if ($session.embeddedAudioStats) { $score += 4 }
        if (-not [string]::IsNullOrWhiteSpace([string]$session.rawTranscript)) { $score += 2 }
        if (-not [string]::IsNullOrWhiteSpace([string]$session.finalText)) { $score += 2 }
        if ($score -gt 0) {
            $candidates += [pscustomobject]@{
                created = $created
                score = $score
                session = $session
            }
        }
    }
    if ($candidates.Count -eq 0) {
        return $null
    }
    return ($candidates | Sort-Object score, created -Descending | Select-Object -First 1).session
}

function Wait-HistorySession {
    param(
        [Parameter(Mandatory = $true)][datetime]$StartedAt,
        [int]$TimeoutSeconds = 20
    )
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        $session = Find-LatestEmbeddedHistorySession -StartedAt $StartedAt
        if ($session) {
            return [pscustomobject]@{
                session = $session
                found_at = Get-Date
            }
        }
        Start-Sleep -Milliseconds 300
    } while ((Get-Date) -lt $deadline)
    return [pscustomobject]@{
        session = $null
        found_at = Get-Date
    }
}

function Get-PropertyValue {
    param($Object, [string]$Name)
    if ($null -eq $Object) { return $null }
    if ($Object.PSObject.Properties[$Name]) {
        return $Object.$Name
    }
    return $null
}

$ScriptDir = Split-Path -Parent $PSCommandPath
$RepoRoot = Resolve-Path (Join-Path $ScriptDir "..\..")
if (-not $FirmwareRepo) {
    $FirmwareRepo = Join-Path (Split-Path -Parent $RepoRoot) "voice-keyboard-firmware"
}
if (-not $ListenerExe) {
    $ListenerExe = Join-Path $RepoRoot "src-tauri\target\debug\listener-type.exe"
}

$OutDir = Resolve-RepoPath $OutDir
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$roundSpecsRaw = Get-Content -Raw -LiteralPath (Resolve-RepoPath $RoundSpecJson) | ConvertFrom-Json
$roundSpecs = @($roundSpecsRaw | ForEach-Object { $_ })
if ($roundSpecs.Count -lt 1) {
    throw "RoundSpecJson must contain at least one round."
}
if (-not (Test-Path $ListenerExe)) {
    throw "Listener executable not found: $ListenerExe"
}

if (-not $SkipEnsureBle) {
    $ensureScript = Join-Path $FirmwareRepo "tools\ensure_ble_hid_connection.ps1"
    if (-not (Test-Path $ensureScript)) {
        throw "BLE ensure script not found: $ensureScript"
    }
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $ensureScript `
        -DeviceName $DeviceName `
        -BluetoothAddress $BluetoothAddress `
        -DurationSeconds 8 `
        -PollIntervalSeconds 2 `
        -ExitOnReady
    if ($LASTEXITCODE -ne 0) {
        throw "BLE ensure failed before background rounds."
    }
}

$runStamp = Get-Date -Format "yyyyMMdd-HHmmss"
$logPath = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
$logDir = Split-Path -Parent $logPath
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
$logOffsetValue = if (Test-Path $logPath) { (Get-Item $logPath).Length } else { 0 }
$logOffset = [ref]$logOffsetValue
$capturedLog = ""
$listenerStdout = Join-Path $OutDir "listener-background-$runStamp.stdout.log"
$listenerStderr = Join-Path $OutDir "listener-background-$runStamp.stderr.log"
$serialLogPath = Join-Path $OutDir "background-rounds-$runStamp.serial.log"
$capturedLogPath = Join-Path $OutDir "background-rounds-$runStamp.listener.log"
$reportPath = Join-Path $OutDir "background-rounds-$runStamp.json"
$prefsPath = Get-PreferencesPath
$prefsBackup = Join-Path $OutDir "preferences-before-background-rounds-$runStamp.json"
$prefsActiveSnapshot = Join-Path $OutDir "preferences-active-background-rounds-$runStamp.json"
$hadPrefs = Test-Path $prefsPath

Stop-ListenerTypeProcesses
Start-Sleep -Milliseconds 1500

$oldHideMain = $env:LISTENER_TYPE_HIDE_MAIN_ON_START
$oldForceRaw = $env:LISTENER_TYPE_FORCE_RAW_OUTPUT
$oldRecordEmbedded = $env:LISTENER_TYPE_RECORD_EMBEDDED_AUDIO_FOR_DEBUG
$oldForegroundInsert = $env:LISTENER_TYPE_INSERT_INTO_FOREGROUND_FALLBACK
$process = $null
$serial = $null
$roundResults = @()
$scriptFailure = $null
$allSerialLines = [System.Collections.Generic.List[string]]::new()
$previousHiddenAt = $null
$startedAt = Get-Date
$triggerCommand = "~KEY:KEY3:SINGLE"
try {
    if ($hadPrefs) {
        Copy-Item -LiteralPath $prefsPath -Destination $prefsBackup -Force
    }
    Set-BackgroundRoundPreferences -Path $prefsPath
    Copy-Item -LiteralPath $prefsPath -Destination $prefsActiveSnapshot -Force

    $env:LISTENER_TYPE_HIDE_MAIN_ON_START = "1"
    $env:LISTENER_TYPE_FORCE_RAW_OUTPUT = "1"
    $env:LISTENER_TYPE_RECORD_EMBEDDED_AUDIO_FOR_DEBUG = "1"
    $env:LISTENER_TYPE_INSERT_INTO_FOREGROUND_FALLBACK = "1"
    $process = Start-Process -FilePath (Resolve-Path $ListenerExe).Path `
        -WorkingDirectory $RepoRoot `
        -WindowStyle Hidden `
        -RedirectStandardOutput $listenerStdout `
        -RedirectStandardError $listenerStderr `
        -PassThru

    $readyDeadline = (Get-Date).AddSeconds($NotifyReadyTimeoutSeconds)
    $notifyReady = $false
    while ((Get-Date) -lt $readyDeadline) {
        Start-Sleep -Milliseconds 250
        $capturedLog += Read-NewLogText -Path $logPath -Offset $logOffset
        if ($capturedLog -match "\[embedded-ble\] background listener notify ready") {
            $notifyReady = $true
            break
        }
        if ($process.HasExited) {
            throw "Listener-Type exited before background listener was ready."
        }
    }
    if (-not $notifyReady) {
        throw "Timed out waiting for background listener notify ready."
    }

    $serial = Open-SerialPort -PortName $Port
    Start-Sleep -Milliseconds 200
    while ($serial.BytesToRead -gt 0) {
        [void]$serial.ReadExisting()
        Start-Sleep -Milliseconds 20
    }
    Send-SerialCommand -Serial $serial -Command "~VREC:CANCEL" -Lines $allSerialLines
    [void](Read-SerialUntil -Serial $serial -Deadline (Get-Date).AddMilliseconds(500) -Lines $allSerialLines)

    foreach ($spec in $roundSpecs) {
        $roundIndex = [int](Get-PropertyValue $spec "round")
        if ($roundIndex -lt 1) {
            $roundIndex = $roundResults.Count + 1
        }
        $label = [string](Get-PropertyValue $spec "label")
        if ([string]::IsNullOrWhiteSpace($label)) {
            $label = "round$roundIndex"
        }
        $wavPath = Resolve-RepoPath ([string](Get-PropertyValue $spec "wav_path"))
        $expectedText = [string](Get-PropertyValue $spec "sentence")
        $roundSerialLines = [System.Collections.Generic.List[string]]::new()

        $hiddenProbe = $null
        if ($roundResults.Count -gt 0) {
            $hiddenProbe = Wait-CapsuleHidden -TimeoutMs 8000
            if ($hiddenProbe.hidden) {
                $previousHiddenAt = $hiddenProbe.hidden_at
            }
        }

        $roundStartedAt = Get-Date
        Send-SerialCommand -Serial $serial -Command $triggerCommand -Lines $roundSerialLines
        $startLine = Read-SerialUntil `
            -Serial $serial `
            -Deadline (Get-Date).AddMilliseconds($RecordingStartTimeoutMs) `
            -Lines $roundSerialLines `
            -Pattern "recording start source=|session_start_queued|stream session start queued"
        foreach ($line in $roundSerialLines) { $allSerialLines.Add("[${label}] $line") }
        $capsuleVisibleAt = Wait-CapsuleVisible -TimeoutMs 1800

        Start-Sleep -Milliseconds $PreRecordDelayMs
        $player = [System.Media.SoundPlayer]::new($wavPath)
        $player.Load()
        $playbackStartedAt = Get-Date
        $player.PlaySync()
        $playbackDoneAt = Get-Date
        Start-Sleep -Milliseconds $PostPlaybackRecordMs
        $stopLines = [System.Collections.Generic.List[string]]::new()
        $stopLine = Invoke-GeneratedRecordingStop `
            -Serial $serial `
            -Command $triggerCommand `
            -TriggerMode $TriggerMode `
            -Lines $stopLines
        foreach ($line in $stopLines) { $allSerialLines.Add("[${label}] $line") }

        $historyResult = Wait-HistorySession -StartedAt $roundStartedAt -TimeoutSeconds 20
        $roundLogText = Read-NewLogText -Path $logPath -Offset $logOffset
        $capturedLog += $roundLogText
        $session = $historyResult.session
        $stats = if ($session) { $session.embeddedAudioStats } else { $null }
        $rawTranscript = if ($session) { [string]$session.rawTranscript } else { "" }
        $finalText = if ($session) { [string]$session.finalText } else { "" }
        $transcript = if (-not [string]::IsNullOrWhiteSpace($finalText)) { $finalText } else { $rawTranscript }
        $missingPackets = Get-PropertyValue $stats "missingPacketCount"
        $duplicatePackets = Get-PropertyValue $stats "duplicatePacketCount"
        $receivedPackets = Get-PropertyValue $stats "receivedPacketCount"
        $expectedPackets = Get-PropertyValue $stats "expectedPacketCount"
        $hiddenToVisibleSeconds = $null
        if ($previousHiddenAt -and $capsuleVisibleAt) {
            $hiddenToVisibleSeconds = [Math]::Round(($capsuleVisibleAt - $previousHiddenAt).TotalSeconds, 3)
        }

        $roundFailures = @()
        $roundWarnings = @()
        if (-not $startLine) { $roundFailures += "firmware_recording_start_not_seen" }
        if (-not $stopLine) { $roundFailures += "firmware_recording_stop_not_seen" }
        if (-not $capsuleVisibleAt) { $roundFailures += "capsule_not_visible" }
        if (-not $session) { $roundFailures += "history_session_missing" }
        if ($session -and -not $stats) { $roundFailures += "embedded_audio_stats_missing" }
        if ([string]::IsNullOrWhiteSpace($transcript)) {
            if ($SkipAccuracyGate) {
                $roundWarnings += "transcript_missing"
            } else {
                $roundFailures += "transcript_missing"
            }
        }
        if ($null -ne $missingPackets -and [int]$missingPackets -ne 0) { $roundFailures += "missing_packets=$missingPackets" }
        if ($hiddenToVisibleSeconds -ne $null -and $hiddenToVisibleSeconds -gt $MaxHiddenToVisibleSeconds) {
            $roundFailures += "hidden_to_visible_seconds=$hiddenToVisibleSeconds"
        }
        $roundSerialEvidenceLines = @($roundSerialLines + $stopLines)
        $serialText = ($roundSerialEvidenceLines -join "`n")
        $generatedEvidence = Get-GeneratedButtonEvidence -Lines $roundSerialEvidenceLines -TriggerMode $TriggerMode
        if (-not [bool]$generatedEvidence.start_stop_ack_seen) {
            $roundFailures += "generated_button_ack_missing"
        }
        if (-not [bool]$generatedEvidence.timing_seen) {
            $roundFailures += "generated_button_timing_missing"
        }
        if (-not [bool]$generatedEvidence.single_seen) {
            $roundFailures += "generated_button_single_missing"
        }
        $errorText = ($serialText + "`n" + $roundLogText)
        if ($errorText -match "QueueFull|SessionError|session storm") {
            $roundFailures += "error_marker"
        }
        if (-not $startLine -and $errorText -match "transport_not_ready") {
            $roundFailures += "transport_not_ready"
        }

        $roundResults += [ordered]@{
            round = $roundIndex
            label = $label
            trigger = $TriggerMode
            trigger_command = $triggerCommand
            status = if ($roundFailures.Count -eq 0) { "PASS" } else { "FAIL" }
            failures = @($roundFailures)
            warnings = @($roundWarnings)
            expected_text = $expectedText
            transcript = $transcript
            accuracy_gate_skipped = [bool]$SkipAccuracyGate
            transcript_gate_skipped = [bool]$SkipAccuracyGate
            started_at_utc = $roundStartedAt.ToUniversalTime().ToString("o")
            capsule_visible_at_utc = if ($capsuleVisibleAt) { $capsuleVisibleAt.ToUniversalTime().ToString("o") } else { $null }
            previous_capsule_hidden_at_utc = if ($previousHiddenAt) { $previousHiddenAt.ToUniversalTime().ToString("o") } else { $null }
            hidden_to_visible_seconds = $hiddenToVisibleSeconds
            playback_started_at_utc = $playbackStartedAt.ToUniversalTime().ToString("o")
            playback_done_at_utc = $playbackDoneAt.ToUniversalTime().ToString("o")
            history_wait_done_at_utc = $historyResult.found_at.ToUniversalTime().ToString("o")
            history_session_id = if ($session) { [string]$session.id } else { $null }
            insert_status = if ($session) { [string]$session.insertStatus } else { $null }
            expected_packet_count = $expectedPackets
            received_packet_count = $receivedPackets
            missing_packet_count = $missingPackets
            duplicate_packet_count = $duplicatePackets
            serial_start_line = $startLine
            serial_stop_line = $stopLine
            generated_button_evidence = $generatedEvidence
            listener_log_excerpt_chars = [Math]::Min(4000, $roundLogText.Length)
            hidden_probe = $hiddenProbe
            wav_path = $wavPath
        }
    }
} catch {
    $scriptFailure = $_
} finally {
    if ($serial -and $serial.IsOpen) {
        try { $serial.Close() } catch {}
    }
    if ($process -and -not $process.HasExited) {
        try {
            $process.Kill()
            [void]$process.WaitForExit(2000)
        } catch {}
    }
    if ($process) { try { $process.Dispose() } catch {} }
    if ($null -eq $oldHideMain) { Remove-Item Env:LISTENER_TYPE_HIDE_MAIN_ON_START -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_HIDE_MAIN_ON_START = $oldHideMain }
    if ($null -eq $oldForceRaw) { Remove-Item Env:LISTENER_TYPE_FORCE_RAW_OUTPUT -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_FORCE_RAW_OUTPUT = $oldForceRaw }
    if ($null -eq $oldRecordEmbedded) { Remove-Item Env:LISTENER_TYPE_RECORD_EMBEDDED_AUDIO_FOR_DEBUG -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_RECORD_EMBEDDED_AUDIO_FOR_DEBUG = $oldRecordEmbedded }
    if ($null -eq $oldForegroundInsert) { Remove-Item Env:LISTENER_TYPE_INSERT_INTO_FOREGROUND_FALLBACK -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_INSERT_INTO_FOREGROUND_FALLBACK = $oldForegroundInsert }
    Restore-Preferences -Path $prefsPath -BackupPath $prefsBackup -HadOriginal $hadPrefs
    Set-Content -Path $serialLogPath -Value @($allSerialLines) -Encoding UTF8
    Set-Content -Path $capturedLogPath -Value $capturedLog -Encoding UTF8
}

$failed = @($roundResults | Where-Object { $_.status -ne "PASS" })
if ($scriptFailure) {
    $failed += [ordered]@{
        round = $null
        status = "FAIL"
        failures = @("script_error")
    }
}
$payload = [ordered]@{
    schema = "listener_type_ble_background_rounds.v1"
    status = if ($failed.Count -eq 0) { "PASS" } else { "FAIL" }
    started_at_utc = $startedAt.ToUniversalTime().ToString("o")
    finished_at_utc = Get-UtcNow
    port = $Port
    trigger = $TriggerMode
    trigger_command = $triggerCommand
    device_name = $DeviceName
    bluetooth_address = $BluetoothAddress
    listener_exe = $ListenerExe
    listener_stdout = $listenerStdout
    listener_stderr = $listenerStderr
    listener_log = $logPath
    captured_listener_log_path = $capturedLogPath
    serial_log_path = $serialLogPath
    script_error = if ($scriptFailure) { [string]$scriptFailure.Exception.Message } else { $null }
    temporary_preferences = [ordered]@{
        path = $prefsPath
        backup_path = if ($hadPrefs) { $prefsBackup } else { $null }
        active_snapshot_path = $prefsActiveSnapshot
        dictation_input_source = "embeddedBle"
        key3_single_click_action = "dictation"
        restored_after_run = $true
    }
    max_hidden_to_visible_seconds = $MaxHiddenToVisibleSeconds
    accuracy_gate_skipped_for_short_recordings = [bool]$SkipAccuracyGate
    report_path = $reportPath
    rounds = @($roundResults)
}
$json = $payload | ConvertTo-Json -Depth 20 -Compress
Set-Content -Path $reportPath -Value $json -Encoding UTF8
Write-Output $json
Write-Output "ble_background_rounds_result_json=$json"
if ($failed.Count -gt 0) {
    exit 1
}
