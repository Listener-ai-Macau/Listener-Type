param(
    [ValidateSet("serial-toggle", "serial-cancel", "manual-key")]
    [string]$TriggerMode = "serial-toggle",
    [string]$Port = "COM3",
    [string]$DeviceName = "listener",
    [string]$BluetoothAddress = "DCB4D91112CE",
    [int]$TimeoutMs = 45000,
    [int]$NotifyReadyTimeoutSeconds = 20,
    [double]$NotifySettleSeconds = 0.8,
    [int]$NoNotificationTimeoutSeconds = 8,
    [int]$PlaybackCount = 1,
    [int]$RecordPlaybackIndex = 1,
    [int]$RandomSentenceCount = 1,
    [int]$SilentAudioMs = 2500,
    [int]$PreRecordDelayMs = 300,
    [int]$RecordingStartTimeoutMs = 2500,
    [int]$ManualTriggerReadyDelayMs = 20000,
    [int]$PostPlaybackRecordMs = 500,
    [ValidateSet("normal", "fast", "low-volume", "fast-low-volume", "noisy", "punctuation")]
    [string]$AudioProfile = "normal",
    [string]$ExpectedText,
    [string]$Sentence,
    [string]$WavPath,
    [string]$VoiceName = "Microsoft Huihui Desktop",
    [int]$TtsRate = 0,
    [double]$TtsGain = 4.0,
    [string]$ListenerExe,
    [string]$FirmwareRepo,
    [string]$OutDir = "artifacts\embedded_stream_smoke",
    [int]$MaxMissingPackets = 0,
    [switch]$FailOnMissingPackets,
    [switch]$SilentAudio,
    [switch]$ExpectNoText,
    [switch]$NoResetBeforeCapture,
    [switch]$SkipEnsureBle,
    [switch]$SkipCapsuleVisibleGate,
    [switch]$VerifyInsertion,
    [switch]$VerifyHistory
)

$ErrorActionPreference = "Stop"

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

function Write-SmokeTrace {
    param([string]$Message)
    if ([string]::IsNullOrWhiteSpace($script:TraceLogPath)) {
        return
    }
    Add-Content -Path $script:TraceLogPath -Value "$(Get-Date -Format o) $Message" -Encoding UTF8
}

function Get-SmokeUtcNow {
    return (Get-Date).ToUniversalTime().ToString("o")
}

function Get-SmokeReportSchema {
    return [ordered]@{
        name = "ble_stream_smoke_report"
        schema_version = 1
        required = @(
            "status",
            "trigger",
            "audio_profile",
            "expected_text",
            "transcript",
            "final_text",
            "partial_preview_count",
            "last_partial_preview",
            "asr_text_update_count",
            "asr_text_updates",
            "inserted_text",
            "history_session",
            "recording_archive_path",
            "timeline",
            "started_at_utc",
            "log_path"
        )
        optional = @(
            "history_session.embeddedAudioStats",
            "history_session.insertStatus",
            "serial_report",
            "serial_log_path",
            "insertion_target_path",
            "expected_stream_failure",
            "error"
        )
        diagnostic = @(
            "normalized_expected",
            "normalized_transcript",
            "cer",
            "accuracy",
            "accuracy_threshold",
            "accuracy_warning_only",
            "wav_path",
            "tts_rate",
            "tts_gain",
            "random_sentence_count",
            "pcm_bytes",
            "missing_packets",
            "verification_errors"
        )
    }
}

function Get-SmokeAudioProfile {
    param([string]$Name)
    switch ($Name) {
        "fast" {
            return [ordered]@{ name = "fast"; tts_rate = 3; tts_gain = 4.0; minimum_accuracy = 0.78; warning_only = $false }
        }
        "low-volume" {
            return [ordered]@{ name = "low-volume"; tts_rate = 0; tts_gain = 1.8; minimum_accuracy = 0.72; warning_only = $false }
        }
        "fast-low-volume" {
            return [ordered]@{ name = "fast-low-volume"; tts_rate = 3; tts_gain = 1.8; minimum_accuracy = 0.65; warning_only = $true }
        }
        "noisy" {
            return [ordered]@{ name = "noisy"; tts_rate = 0; tts_gain = 4.0; minimum_accuracy = 0.70; warning_only = $false }
        }
        "punctuation" {
            return [ordered]@{ name = "punctuation"; tts_rate = 0; tts_gain = 4.0; minimum_accuracy = 0.60; warning_only = $true }
        }
        default {
            return [ordered]@{ name = "normal"; tts_rate = 0; tts_gain = 4.0; minimum_accuracy = 0.85; warning_only = $false }
        }
    }
}

function ConvertTo-SmokeJsonString {
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

function ConvertTo-SmokeJsonValue {
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
    if ($Value -is [System.Collections.IDictionary] -or
        $Value -is [System.Collections.Specialized.OrderedDictionary]) {
        $parts = New-Object System.Collections.Generic.List[string]
        foreach ($key in $Value.Keys) {
            $parts.Add((ConvertTo-SmokeJsonString ([string]$key)) + ":" + (ConvertTo-SmokeJsonValue $Value[$key]))
        }
        return "{" + ([string]::Join(",", $parts)) + "}"
    }
    if ($Value -is [System.Collections.IEnumerable] -and -not ($Value -is [string])) {
        $parts = New-Object System.Collections.Generic.List[string]
        foreach ($item in $Value) {
            $parts.Add((ConvertTo-SmokeJsonValue $item))
        }
        return "[" + ([string]::Join(",", $parts)) + "]"
    }
    return ConvertTo-SmokeJsonString ([string]$Value)
}

function Get-LatestAsrTranscriptFromLog {
    param([string]$Text)

    return [string](Get-AsrTranscriptSummaryFromLog -Text $Text).final_text
}

function Get-AsrTranscriptSummaryFromLog {
    param([string]$Text)

    $updates = New-Object System.Collections.Generic.List[string]
    $latest = ""
    foreach ($line in ($Text -split "(`r`n|`n)")) {
        $marker = "server JSON:"
        $index = $line.IndexOf($marker)
        if ($index -lt 0) {
            continue
        }
        $jsonText = $line.Substring($index + $marker.Length).Trim()
        if ([string]::IsNullOrWhiteSpace($jsonText)) {
            continue
        }
        try {
            $payload = $jsonText | ConvertFrom-Json
            if ($payload.result -and -not [string]::IsNullOrWhiteSpace([string]$payload.result.text)) {
                $latest = [string]$payload.result.text
                if ($updates.Count -eq 0 -or $updates[$updates.Count - 1] -ne $latest) {
                    $updates.Add($latest)
                }
                continue
            }
        } catch {
        }
        $match = [regex]::Match($jsonText, '"text"\s*:\s*"((?:\\.|[^"\\])*)"')
        if ($match.Success) {
            try {
                $candidate = [string](('"' + $match.Groups[1].Value + '"') | ConvertFrom-Json)
            } catch {
                $candidate = [string]$match.Groups[1].Value
            }
            if (-not [string]::IsNullOrWhiteSpace($candidate)) {
                $latest = $candidate
                if ($updates.Count -eq 0 -or $updates[$updates.Count - 1] -ne $latest) {
                    $updates.Add($latest)
                }
            }
        }
    }
    $partialCount = [Math]::Max(0, $updates.Count - 1)
    $lastPartial = ""
    if ($partialCount -gt 0) {
        $lastPartial = $updates[$partialCount - 1]
    }
    return [ordered]@{
        final_text = $latest
        asr_text_update_count = $updates.Count
        partial_preview_count = $partialCount
        last_partial_preview = $lastPartial
        text_updates = @($updates)
    }
}

function Normalize-AccuracyText {
    param([string]$Text)
    if ($null -eq $Text) {
        return ""
    }
    $normalized = $Text.Normalize([System.Text.NormalizationForm]::FormKC).ToLowerInvariant()
    $builder = [System.Text.StringBuilder]::new()
    foreach ($ch in $normalized.ToCharArray()) {
        if ([char]::IsLetterOrDigit($ch)) {
            [void]$builder.Append($ch)
        }
    }
    return $builder.ToString()
}

function Get-EditDistance {
    param(
        [string]$Expected,
        [string]$Actual
    )
    if ($Expected -eq $Actual) {
        return 0
    }
    $previous = New-Object int[] ($Actual.Length + 1)
    for ($i = 0; $i -le $Actual.Length; $i++) {
        $previous[$i] = $i
    }
    for ($i = 1; $i -le $Expected.Length; $i++) {
        $current = New-Object int[] ($Actual.Length + 1)
        $current[0] = $i
        for ($j = 1; $j -le $Actual.Length; $j++) {
            $cost = if ($Expected[$i - 1] -eq $Actual[$j - 1]) { 0 } else { 1 }
            $insertCost = $current[$j - 1] + 1
            $deleteCost = $previous[$j] + 1
            $replaceCost = $previous[$j - 1] + $cost
            $current[$j] = [Math]::Min([Math]::Min($insertCost, $deleteCost), $replaceCost)
        }
        $previous = $current
    }
    return $previous[$Actual.Length]
}

function Measure-TranscriptAccuracy {
    param(
        [string]$Expected,
        [string]$Transcript
    )
    $expectedNormalized = Normalize-AccuracyText -Text $Expected
    $transcriptNormalized = Normalize-AccuracyText -Text $Transcript
    $distance = Get-EditDistance -Expected $expectedNormalized -Actual $transcriptNormalized
    if ($expectedNormalized.Length -eq 0) {
        $cer = if ($transcriptNormalized.Length -eq 0) { 0.0 } else { 1.0 }
    } else {
        $cer = $distance / [double]$expectedNormalized.Length
    }
    $accuracy = [Math]::Max(0.0, 1.0 - $cer)
    return [ordered]@{
        expected_text = $Expected
        normalized_expected = $expectedNormalized
        normalized_transcript = $transcriptNormalized
        edit_distance = $distance
        reference_length = $expectedNormalized.Length
        cer = [Math]::Round($cer, 6)
        accuracy = [Math]::Round($accuracy, 6)
    }
}

function Get-WavDurationMilliseconds {
    param([string]$Path)

    $stream = [System.IO.File]::OpenRead($Path)
    $reader = [System.IO.BinaryReader]::new($stream)
    try {
        $riff = [System.Text.Encoding]::ASCII.GetString($reader.ReadBytes(4))
        [void]$reader.ReadUInt32()
        $wave = [System.Text.Encoding]::ASCII.GetString($reader.ReadBytes(4))
        if ($riff -ne "RIFF" -or $wave -ne "WAVE") {
            return 5000
        }

        $byteRate = 0
        $dataBytes = 0
        while ($stream.Position -le ($stream.Length - 8)) {
            $chunkId = [System.Text.Encoding]::ASCII.GetString($reader.ReadBytes(4))
            $chunkSize = [int]$reader.ReadUInt32()
            $chunkStart = $stream.Position

            if ($chunkId -eq "fmt ") {
                [void]$reader.ReadUInt16()
                [void]$reader.ReadUInt16()
                [void]$reader.ReadUInt32()
                $byteRate = [int]$reader.ReadUInt32()
            } elseif ($chunkId -eq "data") {
                $dataBytes = $chunkSize
            }

            $nextChunk = $chunkStart + $chunkSize
            if (($chunkSize % 2) -eq 1) {
                $nextChunk += 1
            }
            if ($nextChunk -gt $stream.Length) {
                break
            }
            $stream.Position = $nextChunk
            if ($byteRate -gt 0 -and $dataBytes -gt 0) {
                break
            }
        }

        if ($byteRate -le 0 -or $dataBytes -le 0) {
            return 5000
        }
        return [int][System.Math]::Ceiling(($dataBytes * 1000.0) / $byteRate)
    } finally {
        $reader.Dispose()
        $stream.Dispose()
    }
}

function Send-SerialToggle {
    param([string]$PortName)
    Send-SerialCommand -PortName $PortName -Command "~VREC:TOGGLE"
}

function Quote-ProcessArgument {
    param([string]$Value)
    return '"' + ($Value -replace '"', '\"') + '"'
}

function Ensure-WindowInterop {
    if ("ListenerSmokeWindow" -as [type]) {
        return
    }
    Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;

public struct ListenerSmokeRect {
    public int Left;
    public int Top;
    public int Right;
    public int Bottom;
}

public delegate bool ListenerSmokeEnumWindowsProc(IntPtr hWnd, IntPtr lParam);

public static class ListenerSmokeWindow {
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr FindWindow(string lpClassName, string lpWindowName);

    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool IsWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool EnumWindows(ListenerSmokeEnumWindowsProc lpEnumFunc, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetWindowText(IntPtr hWnd, StringBuilder lpString, int nMaxCount);

    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out ListenerSmokeRect lpRect);
}
"@
}

function Get-WindowTitleByHandle {
    param([IntPtr]$Handle)

    if ($Handle -eq [IntPtr]::Zero) {
        return ""
    }
    $titleBuilder = [System.Text.StringBuilder]::new(256)
    [void][ListenerSmokeWindow]::GetWindowText($Handle, $titleBuilder, $titleBuilder.Capacity)
    return $titleBuilder.ToString()
}

function Get-ForegroundWindowSnapshot {
    Ensure-WindowInterop
    $handle = [ListenerSmokeWindow]::GetForegroundWindow()
    [uint32]$processId = 0
    if ($handle -ne [IntPtr]::Zero) {
        [void][ListenerSmokeWindow]::GetWindowThreadProcessId($handle, [ref]$processId)
    }
    return [pscustomobject]@{
        Handle = $handle
        ProcessId = [int]$processId
        Title = Get-WindowTitleByHandle -Handle $handle
        Visible = ($handle -ne [IntPtr]::Zero -and [ListenerSmokeWindow]::IsWindowVisible($handle))
    }
}

function Write-ForegroundTrace {
    param(
        [string]$Label,
        $Snapshot
    )
    if (-not $Snapshot) {
        Write-SmokeTrace "$Label foreground=<null>"
        return
    }
    Write-SmokeTrace "$Label foreground_pid=$($Snapshot.ProcessId) foreground_visible=$($Snapshot.Visible) foreground_title=$($Snapshot.Title)"
}

function Restore-ForegroundWindow {
    param(
        $Snapshot,
        [string]$Label
    )

    if (-not $Snapshot -or $Snapshot.Handle -eq [IntPtr]::Zero) {
        Write-SmokeTrace "$Label foreground_restore_skipped reason=no_snapshot"
        return $false
    }
    Ensure-WindowInterop
    if (-not [ListenerSmokeWindow]::IsWindow($Snapshot.Handle)) {
        Write-SmokeTrace "$Label foreground_restore_skipped reason=stale_window"
        return $false
    }
    for ($attempt = 1; $attempt -le 5; $attempt++) {
        [void][ListenerSmokeWindow]::ShowWindow($Snapshot.Handle, 9)
        [void][ListenerSmokeWindow]::SetForegroundWindow($Snapshot.Handle)
        Start-Sleep -Milliseconds (80 * $attempt)
        $current = Get-ForegroundWindowSnapshot
        Write-ForegroundTrace -Label "$Label`_after_restore_$attempt" -Snapshot $current
        if ($current.Handle -eq $Snapshot.Handle) {
            return $true
        }
        if ($current.ProcessId -eq $Snapshot.ProcessId -and $current.Visible) {
            return $true
        }
    }
    try {
        Add-Type -AssemblyName Microsoft.VisualBasic -ErrorAction SilentlyContinue
        [void][Microsoft.VisualBasic.Interaction]::AppActivate([int]$Snapshot.ProcessId)
        Start-Sleep -Milliseconds 160
        $current = Get-ForegroundWindowSnapshot
        Write-ForegroundTrace -Label "$Label`_after_appactivate" -Snapshot $current
        if ($current.Handle -eq $Snapshot.Handle) {
            return $true
        }
        if ($current.ProcessId -eq $Snapshot.ProcessId -and $current.Visible) {
            return $true
        }
    } catch {
        Write-SmokeTrace "$Label foreground_restore_appactivate_failed error=$($_.Exception.Message)"
    }
    return $false
}

function Test-CapsuleWindowVisible {
    param([int]$ProcessId = 0)

    Ensure-WindowInterop
    $handle = [ListenerSmokeWindow]::FindWindow($null, "Listener Type Capsule")
    if ($handle -ne [IntPtr]::Zero -and [ListenerSmokeWindow]::IsWindowVisible($handle)) {
        return $true
    }
    if ($ProcessId -le 0) {
        return $false
    }

    $found = $false
    $callback = [ListenerSmokeEnumWindowsProc]{
        param([IntPtr]$hWnd, [IntPtr]$lParam)

        [uint32]$windowPid = 0
        [void][ListenerSmokeWindow]::GetWindowThreadProcessId($hWnd, [ref]$windowPid)
        if ($windowPid -ne [uint32]$ProcessId -or -not [ListenerSmokeWindow]::IsWindowVisible($hWnd)) {
            return $true
        }

        $title = Get-WindowTitleByHandle -Handle $hWnd
        $rect = New-Object ListenerSmokeRect
        if (-not [ListenerSmokeWindow]::GetWindowRect($hWnd, [ref]$rect)) {
            return $true
        }
        $width = [int]($rect.Right - $rect.Left)
        $height = [int]($rect.Bottom - $rect.Top)
        Write-SmokeTrace "window_visible pid=$windowPid title=$title rect=$($rect.Left),$($rect.Top),$width,$height"
        if ($title -match "Capsule" -or ($width -ge 180 -and $width -le 420 -and $height -ge 50 -and $height -le 190)) {
            $script:CapsuleWindowFound = $true
            return $false
        }
        return $true
    }
    $script:CapsuleWindowFound = $false
    [void][ListenerSmokeWindow]::EnumWindows($callback, [IntPtr]::Zero)
    $found = [bool]$script:CapsuleWindowFound
    $script:CapsuleWindowFound = $false
    return $found
}

function Wait-CapsuleWindowVisible {
    param(
        [int]$TimeoutMs = 1800,
        [int]$ProcessId = 0
    )

    $deadline = (Get-Date).AddMilliseconds($TimeoutMs)
    while ((Get-Date) -lt $deadline) {
        if (Test-CapsuleWindowVisible -ProcessId $ProcessId) {
            return $true
        }
        Start-Sleep -Milliseconds 80
    }
    return $false
}

function Focus-ProcessWindow {
    param(
        [System.Diagnostics.Process]$Process,
        [int]$Retries = 30
    )

    if (-not $Process) {
        return $false
    }
    Ensure-WindowInterop
    for ($i = 0; $i -lt $Retries; $i++) {
        if ($Process.HasExited) {
            return $false
        }
        $Process.Refresh()
        $handle = $Process.MainWindowHandle
        if ($handle -ne [IntPtr]::Zero) {
            [void][ListenerSmokeWindow]::ShowWindow($handle, 9)
            [void][ListenerSmokeWindow]::SetForegroundWindow($handle)
            Start-Sleep -Milliseconds 150
            return $true
        }
        Start-Sleep -Milliseconds 200
    }
    return $false
}

function Start-InsertionTarget {
    param([string]$Path)

    $parent = Split-Path -Parent $Path
    New-Item -ItemType Directory -Force -Path $parent | Out-Null
    Set-Content -Path $Path -Value "" -Encoding UTF8
    $targetScript = Join-Path ([System.IO.Path]::GetTempPath()) "listener_ble_insert_target_$PID.ps1"
    $targetStdout = "$Path.target.stdout.log"
    $targetStderr = "$Path.target.stderr.log"
    $targetScriptBody = @'
param([string]$Path)

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$form = [System.Windows.Forms.Form]::new()
$form.Text = "Listener BLE Smoke Target"
$form.Width = 900
$form.Height = 320
$form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
$form.TopMost = $true

$textBox = [System.Windows.Forms.TextBox]::new()
$textBox.Multiline = $true
$textBox.Dock = [System.Windows.Forms.DockStyle]::Fill
$textBox.AcceptsReturn = $true
$textBox.Font = [System.Drawing.Font]::new("Microsoft YaHei UI", 14)
$form.Controls.Add($textBox)

$timer = [System.Windows.Forms.Timer]::new()
$timer.Interval = 200
$timer.Add_Tick({
    [System.IO.File]::WriteAllText($Path, $textBox.Text, [System.Text.Encoding]::UTF8)
})
$form.Add_Shown({
    $textBox.Focus()
})
$form.Add_FormClosed({
    [System.IO.File]::WriteAllText($Path, $textBox.Text, [System.Text.Encoding]::UTF8)
    $timer.Stop()
    $timer.Dispose()
})

$timer.Start()
[System.Windows.Forms.Application]::Run($form)
'@
    Set-Content -Path $targetScript -Value $targetScriptBody -Encoding UTF8
    $process = Start-Process -FilePath "powershell.exe" `
        -ArgumentList @(
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-STA",
            "-File",
            (Quote-ProcessArgument $targetScript),
            "-Path",
            (Quote-ProcessArgument $Path)
        ) `
        -RedirectStandardOutput $targetStdout `
        -RedirectStandardError $targetStderr `
        -PassThru
    [void](Focus-ProcessWindow -Process $process)
    return [ordered]@{
        Process = $process
        Path = $Path
        ScriptPath = $targetScript
        StdoutPath = $targetStdout
        StderrPath = $targetStderr
    }
}

function Read-InsertionTargetText {
    param($Target)

    if (-not $Target) {
        return $null
    }
    Start-Sleep -Milliseconds 500
    if (-not (Test-Path $Target.Path)) {
        return ""
    }
    $text = Get-Content -Path $Target.Path -Raw -ErrorAction SilentlyContinue
    if ($null -eq $text) {
        return ""
    }
    return $text
}

function Stop-InsertionTarget {
    param($Target)

    if (-not $Target) {
        return
    }
    try {
        [void](Read-InsertionTargetText -Target $Target)
    } catch {
    }
    if ($Target.Process -and -not $Target.Process.HasExited) {
        try {
            [void]$Target.Process.CloseMainWindow()
            if (-not $Target.Process.WaitForExit(2000)) {
                $Target.Process.Kill()
            }
        } catch {
        }
    }
    if ($Target.ScriptPath) {
        Remove-Item -LiteralPath $Target.ScriptPath -Force -ErrorAction SilentlyContinue
    }
}

function Get-HistoryPath {
    if ($env:APPDATA) {
        return Join-Path $env:APPDATA "Listener Type\history.json"
    }
    return $null
}

function Read-SmokeHistorySessions {
    $historyPath = Get-HistoryPath
    if (-not $historyPath -or -not (Test-Path $historyPath)) {
        return @()
    }
    $raw = Get-Content -Path $historyPath -Raw -ErrorAction SilentlyContinue
    if ([string]::IsNullOrWhiteSpace($raw)) {
        return @()
    }
    $parsed = $raw | ConvertFrom-Json
    if ($null -eq $parsed) {
        return @()
    }
    if ($parsed -is [System.Array]) {
        return $parsed
    }
    return @($parsed)
}

function Get-RecordingsRoot {
    if ($env:APPDATA) {
        return Join-Path $env:APPDATA "Listener Type\recordings"
    }
    return $null
}

function Find-LatestRecordingAfter {
    param([datetime]$StartedAt)

    $root = Get-RecordingsRoot
    if (-not $root -or -not (Test-Path $root)) {
        return $null
    }
    $threshold = $StartedAt.ToUniversalTime().AddMinutes(-1)
    $candidate = Get-ChildItem -Path $root -Filter "*.wav" -File -ErrorAction SilentlyContinue |
        Where-Object { $_.LastWriteTimeUtc -ge $threshold } |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1
    if (-not $candidate) {
        return $null
    }
    return $candidate.FullName
}

function Find-SmokeHistorySession {
    param(
        [datetime]$StartedAt,
        [string]$Transcript,
        [int]$ExpectedPcmBytes = 0
    )

    $sessions = @(Read-SmokeHistorySessions)
    $threshold = $StartedAt.ToUniversalTime().AddMinutes(-2)
    $candidates = @()
    foreach ($session in $sessions) {
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
        $matchesTranscript = $false
        if (-not [string]::IsNullOrWhiteSpace($Transcript)) {
            $rawTranscript = [string]$session.rawTranscript
            $finalText = [string]$session.finalText
            $matchesTranscript = $rawTranscript.Contains($Transcript) -or
                $finalText.Contains($Transcript) -or
                ((-not [string]::IsNullOrWhiteSpace($rawTranscript)) -and $Transcript.Contains($rawTranscript)) -or
                ((-not [string]::IsNullOrWhiteSpace($finalText)) -and $Transcript.Contains($finalText))
        }
        $hasEmbeddedStats = $null -ne $session.embeddedAudioStats
        $pcmMatches = $false
        if ($hasEmbeddedStats -and $ExpectedPcmBytes -gt 0) {
            $pcmMatches = ([int64]$session.embeddedAudioStats.receivedPcmBytes -eq [int64]$ExpectedPcmBytes) -or
                ([int64]$session.embeddedAudioStats.reconstructedPcmBytes -eq [int64]$ExpectedPcmBytes)
        }
        $score = 0
        if ($pcmMatches) {
            $score += 4
        }
        if ($hasEmbeddedStats) {
            $score += 2
        }
        if ($matchesTranscript) {
            $score += 1
        }
        if ($score -gt 0) {
            $candidates += [pscustomobject]@{
                Created = $created
                Score = $score
                Session = $session
            }
        }
    }
    if ($candidates.Count -eq 0) {
        return $null
    }
    return ($candidates | Sort-Object Score, Created -Descending | Select-Object -First 1).Session
}

function Find-SmokeHistorySessionById {
    param(
        [string]$SessionId
    )

    if ([string]::IsNullOrWhiteSpace($SessionId)) {
        return $null
    }
    $sessions = @(Read-SmokeHistorySessions)
    foreach ($session in $sessions) {
        if ([string]$session.id -eq $SessionId) {
            return $session
        }
    }
    return $null
}

function Wait-SmokeHistorySession {
    param(
        [datetime]$StartedAt,
        [string]$Transcript,
        [int]$ExpectedPcmBytes = 0,
        [int]$TimeoutSeconds = 15
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        $session = Find-SmokeHistorySession `
            -StartedAt $StartedAt `
            -Transcript $Transcript `
            -ExpectedPcmBytes $ExpectedPcmBytes
        if ($session) {
            return $session
        }
        Start-Sleep -Milliseconds 300
    } while ((Get-Date) -lt $deadline)
    return $null
}

function Convert-SerialReportForJson {
    param($Report)
    if (-not $Report) {
        return $null
    }
    return [pscustomobject]@{
        serial_log_path = [string]$Report.serial_log_path
        serial_line_count = [int]$Report.serial_line_count
        notify_enabled = [bool]$Report.notify_enabled
        stream_ready = [bool]$Report.stream_ready
        streaming_queued = [bool]$Report.streaming_queued
        transport_not_ready = [bool]$Report.transport_not_ready
        record_start_rejected = [bool]$Report.record_start_rejected
        recording_start_seen = [bool]$Report.recording_start_seen
        recording_stop_seen = [bool]$Report.recording_stop_seen
        recording_cancel_seen = [bool]$Report.recording_cancel_seen
        cancel_requested = [bool]$Report.cancel_requested
        cancel_completed = [bool]$Report.cancel_completed
    }
}

function Convert-HistorySessionForJson {
    param($Session)
    if (-not $Session) {
        return $null
    }
    $stats = $Session.embeddedAudioStats
    $statsReport = $null
    if ($stats) {
        $statsReport = [ordered]@{
            receivedPcmBytes = [int64]$stats.receivedPcmBytes
            reconstructedPcmBytes = [int64]$stats.reconstructedPcmBytes
            missingPacketCount = [int64]$stats.missingPacketCount
            expectedPacketCount = $stats.expectedPacketCount
            terminalReceived = [bool]$stats.terminalReceived
        }
    }
    return [ordered]@{
        id = [string]$Session.id
        createdAt = [string]$Session.createdAt
        rawTranscript = [string]$Session.rawTranscript
        finalText = [string]$Session.finalText
        insertStatus = [string]$Session.insertStatus
        embeddedAudioStats = $statsReport
    }
}

function Send-SerialCommand {
    param(
        [string]$PortName,
        [string]$Command
    )

    $python = @'
import serial
import sys
import time

port = sys.argv[1]
command = sys.argv[2]
last_error = None

for attempt in range(12):
    ser = serial.Serial()
    ser.port = port
    ser.baudrate = 115200
    ser.timeout = 0.05
    ser.write_timeout = 2
    ser.dsrdtr = False
    ser.rtscts = False
    ser.dtr = False
    ser.rts = False
    try:
        ser.open()
        ser.setDTR(False)
        ser.setRTS(False)
        ser.write((command + "\n").encode("ascii"))
        ser.flush()
        time.sleep(0.05)
        ser.close()
        sys.exit(0)
    except Exception as exc:
        last_error = exc
        try:
            if ser.is_open:
                ser.close()
        finally:
            time.sleep(0.5)

raise RuntimeError(f"unable to send {command!r} to {port}: {last_error!r}")
'@

    $tempScript = Join-Path ([System.IO.Path]::GetTempPath()) "listener_ble_serial_trigger_$PID.py"
    try {
        Set-Content -Path $tempScript -Value $python -Encoding UTF8
        & python $tempScript $PortName $Command
        if ($LASTEXITCODE -ne 0) {
            throw "serial trigger command failed: $Command on $PortName"
        }
    } finally {
        Remove-Item -LiteralPath $tempScript -Force -ErrorAction SilentlyContinue
    }
}

function Reset-SerialTarget {
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
ser.setDTR(False)
ser.setRTS(True)
time.sleep(0.1)
ser.setRTS(False)
time.sleep(0.2)
ser.close()
'@

    $tempScript = Join-Path ([System.IO.Path]::GetTempPath()) "listener_ble_serial_reset_$PID.py"
    try {
        Set-Content -Path $tempScript -Value $python -Encoding UTF8
        & python $tempScript $PortName
        if ($LASTEXITCODE -ne 0) {
            throw "serial reset failed on $PortName"
        }
    } finally {
        Remove-Item -LiteralPath $tempScript -Force -ErrorAction SilentlyContinue
    }
}

function Start-SerialRecordingWindow {
    param(
        [string]$PortName,
        [int]$MaxRecordMs,
        [string]$SerialLogPath,
        [string]$StartSignalPath,
        [string]$StopSignalPath,
        [string]$RecordingStartedSignalPath,
        [int]$RecordingStartTimeoutMs,
        [ValidateSet("toggle", "cancel")]
        [string]$EndCommand = "toggle",
        [int]$MaxWaitSeconds = 120
    )

    $python = @'
import json
import pathlib
import serial
import sys
import time

port = sys.argv[1]
max_record_ms = int(sys.argv[2])
log_path = pathlib.Path(sys.argv[3])
start_signal_path = pathlib.Path(sys.argv[4])
stop_signal_path = pathlib.Path(sys.argv[5])
recording_started_signal_path = pathlib.Path(sys.argv[6])
recording_start_timeout_ms = int(sys.argv[7])
end_command = sys.argv[8]
max_wait_seconds = int(sys.argv[9])
lines = []
buffer = bytearray()

def poll_lines(ser):
    waiting = ser.in_waiting
    if waiting <= 0:
        return
    data = ser.read(waiting)
    if not data:
        return
    buffer.extend(data)
    while b"\n" in buffer:
        raw, _, rest = buffer.partition(b"\n")
        buffer[:] = rest
        line = raw.decode("utf-8", errors="ignore").strip()
        if line:
            lines.append(line)

def poll_until(ser, deadline):
    while time.monotonic() < deadline:
        poll_lines(ser)
        time.sleep(0.05)

def send_command(ser, command):
    ser.write((command + "\n").encode("ascii"))
    ser.flush()
    time.sleep(0.05)
    poll_lines(ser)

def wait_for_start_signal(ser):
    deadline = time.monotonic() + max_wait_seconds
    while time.monotonic() < deadline:
        poll_lines(ser)
        if start_signal_path.exists():
            try:
                start_signal_path.unlink()
            except OSError:
                pass
            return
        time.sleep(0.05)
    raise RuntimeError(f"timed out waiting for start signal: {start_signal_path}")

def contains(text):
    return any(text in line for line in lines)

def wait_for_recording_start(ser):
    deadline = time.monotonic() + (recording_start_timeout_ms / 1000.0)
    while time.monotonic() < deadline:
        poll_lines(ser)
        if contains("recording start source="):
            recording_started_signal_path.write_text("recording_start_seen", encoding="ascii")
            return
        time.sleep(0.02)
    raise RuntimeError("timed out waiting for firmware recording start log")

def wait_for_stop_signal(ser):
    deadline = time.monotonic() + (max_record_ms / 1000.0)
    while time.monotonic() < deadline:
        poll_lines(ser)
        if stop_signal_path.exists():
            try:
                stop_signal_path.unlink()
            except OSError:
                pass
            return
        time.sleep(0.02)
    raise RuntimeError(f"timed out waiting for stop signal: {stop_signal_path}")

ser = serial.Serial()
ser.port = port
ser.baudrate = 115200
ser.timeout = 0.05
ser.write_timeout = 2
ser.dsrdtr = False
ser.rtscts = False
ser.dtr = False
ser.rts = False

try:
    ser.open()
    ser.setDTR(False)
    ser.setRTS(False)
    ser.reset_input_buffer()
    wait_for_start_signal(ser)
    send_command(ser, "~VREC:TOGGLE")
    wait_for_recording_start(ser)
    wait_for_stop_signal(ser)
    if end_command == "cancel":
        send_command(ser, "~VREC:CANCEL")
    else:
        send_command(ser, "~VREC:TOGGLE")
    poll_until(ser, time.monotonic() + 1.5)
finally:
    try:
        if ser.is_open:
            ser.close()
    finally:
        log_path.parent.mkdir(parents=True, exist_ok=True)
        log_path.write_text("\n".join(lines), encoding="utf-8")

notify_enabled = any(
    ("audio notify subscription changed" in line and "notify=1" in line)
    or "audio notify subscription restored before connect" in line
    or "audio notify subscribed:" in line
    or ("audio transport state:" in line and "notify=1" in line)
    for line in lines
)

summary = {
    "serial_log_path": str(log_path),
    "serial_line_count": len(lines),
    "notify_enabled": notify_enabled,
    "stream_ready": any("stream_ready" in line for line in lines),
    "streaming_queued": contains("session_start_queued") or contains("stream session start queued"),
    "transport_not_ready": contains("BLE audio transport not ready"),
    "record_start_rejected": contains("record session start rejected"),
    "recording_start_seen": contains("recording start source="),
    "recording_stop_seen": contains("recording stop source="),
    "recording_cancel_seen": contains("recording cancel source="),
    "cancel_requested": contains("record session cancel requested"),
    "cancel_completed": (
        contains("record session canceled")
        or contains("recording cancel source=")
        or contains("record session canceled before activation")
    ),
}
print(json.dumps(summary, ensure_ascii=False), flush=True)
'@

    $tempScript = Join-Path ([System.IO.Path]::GetTempPath()) "listener_ble_serial_window_$PID.py"
    Set-Content -Path $tempScript -Value $python -Encoding UTF8

    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = "python"
    $psi.Arguments = @(
        Quote-ProcessArgument $tempScript
        Quote-ProcessArgument $PortName
        Quote-ProcessArgument ([string]$MaxRecordMs)
        Quote-ProcessArgument $SerialLogPath
        Quote-ProcessArgument $StartSignalPath
        Quote-ProcessArgument $StopSignalPath
        Quote-ProcessArgument $RecordingStartedSignalPath
        Quote-ProcessArgument ([string]$RecordingStartTimeoutMs)
        Quote-ProcessArgument $EndCommand
        Quote-ProcessArgument ([string]$MaxWaitSeconds)
    ) -join " "
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $process = [System.Diagnostics.Process]::Start($psi)

    return [pscustomobject]@{
        Process = $process
        ScriptPath = $tempScript
        SerialLogPath = $SerialLogPath
        RecordingStartedSignalPath = $RecordingStartedSignalPath
    }
}

function Wait-RecordingStartedSignal {
    param(
        $Window,
        [int]$TimeoutMs
    )

    $deadline = (Get-Date).AddMilliseconds($TimeoutMs)
    while ((Get-Date) -lt $deadline) {
        if ($Window -and $Window.Process.HasExited) {
            $stderr = $Window.Process.StandardError.ReadToEnd()
            throw "serial recording window exited before recording start: $stderr"
        }
        if ($Window -and (Test-Path $Window.RecordingStartedSignalPath)) {
            return
        }
        Start-Sleep -Milliseconds 25
    }
    throw "Timed out waiting for firmware recording start confirmation"
}

function Wait-SerialRecordingWindow {
    param($Window)

    if (-not $Window) {
        return $null
    }

    [void]$Window.Process.WaitForExit()
    $stdout = $Window.Process.StandardOutput.ReadToEnd()
    $stderr = $Window.Process.StandardError.ReadToEnd()
    Remove-Item -LiteralPath $Window.ScriptPath -Force -ErrorAction SilentlyContinue

    if ($Window.Process.ExitCode -ne 0) {
        throw "serial recording window failed: $stderr"
    }

    $jsonLine = $stdout -split "(`r`n|`n)" |
        Where-Object { $_.Trim().Length -gt 0 } |
        Select-Object -Last 1
    if (-not $jsonLine) {
        return [pscustomobject]@{
            serial_log_path = $Window.SerialLogPath
            stdout = $stdout
            stderr = $stderr
        }
    }
    return $jsonLine | ConvertFrom-Json
}

function Stop-SerialRecordingWindow {
    param($Window)

    if (-not $Window) {
        return
    }
    if (-not $Window.Process.HasExited) {
        try {
            $Window.Process.Kill()
        } catch {
        }
    }
    Remove-Item -LiteralPath $Window.ScriptPath -Force -ErrorAction SilentlyContinue
}

function New-RandomSentence {
    param([int]$Count = 1)

    $sentences = @(
        "火山识别和蓝牙传输正在接受测试。",
        "蓝牙音频正在发送到火山识别，请检查文本结果。",
        "蓝牙手动按键测试成功。",
        "请把这句话写到当前光标位置。",
        "减少人工参与测试正在进行。",
        "这次测试会检查识别结果和光标输出。",
        "蓝牙数据传输完成后会插入文字。"
    )
    if ($Count -le 1) {
        return Get-Random -InputObject $sentences
    }

    $selected = New-Object System.Collections.Generic.List[string]
    $pool = @($sentences)
    for ($index = 0; $index -lt $Count; $index++) {
        if ($pool.Count -eq 0) {
            $pool = @($sentences)
        }
        $choice = Get-Random -InputObject $pool
        $selected.Add([string]$choice)
        $pool = @($pool | Where-Object { $_ -ne $choice })
    }
    return [string]::Join("", $selected)
}

function New-TtsWave {
    param(
        [string]$Text,
        [string]$Path,
        [string]$PreferredVoice,
        [int]$Rate = 0
    )
    Add-Type -AssemblyName System.Speech
    $synth = [System.Speech.Synthesis.SpeechSynthesizer]::new()
    try {
        $synth.Volume = 100
        $synth.Rate = [Math]::Max(-10, [Math]::Min(10, $Rate))
        $voices = @($synth.GetInstalledVoices() | ForEach-Object { $_.VoiceInfo.Name })
        if ($voices -contains $PreferredVoice) {
            $synth.SelectVoice($PreferredVoice)
        } else {
            $zhVoice = $synth.GetInstalledVoices() |
                Where-Object { $_.VoiceInfo.Culture.Name -eq "zh-CN" } |
                Select-Object -First 1
            if ($zhVoice) {
                $synth.SelectVoice($zhVoice.VoiceInfo.Name)
            }
        }
        $format = [System.Speech.AudioFormat.SpeechAudioFormatInfo]::new(
            16000,
            [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen,
            [System.Speech.AudioFormat.AudioChannel]::Mono
        )
        $synth.SetOutputToWaveFile($Path, $format)
        $synth.Speak($Text)
    } finally {
        $synth.Dispose()
    }
}

function New-SilenceWave {
    param(
        [string]$Path,
        [int]$DurationMs
    )

    $sampleRate = 16000
    $bytesPerSample = 2
    $sampleCount = [int][System.Math]::Max(1, [System.Math]::Ceiling($sampleRate * ($DurationMs / 1000.0)))
    $dataSize = $sampleCount * $bytesPerSample
    $stream = [System.IO.File]::Open($Path, [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write)
    $writer = [System.IO.BinaryWriter]::new($stream)
    try {
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes("RIFF"))
        $writer.Write([uint32](36 + $dataSize))
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes("WAVE"))
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes("fmt "))
        $writer.Write([uint32]16)
        $writer.Write([uint16]1)
        $writer.Write([uint16]1)
        $writer.Write([uint32]$sampleRate)
        $writer.Write([uint32]($sampleRate * $bytesPerSample))
        $writer.Write([uint16]$bytesPerSample)
        $writer.Write([uint16]16)
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes("data"))
        $writer.Write([uint32]$dataSize)
        $zeros = New-Object byte[] $dataSize
        $writer.Write($zeros)
    } finally {
        $writer.Dispose()
        $stream.Dispose()
    }
}

function Boost-WavPcm16 {
    param(
        [string]$Path,
        [double]$Gain
    )

    if ($Gain -le 1.0) {
        return
    }
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 44) {
        return
    }
    $riff = [System.Text.Encoding]::ASCII.GetString($bytes, 0, 4)
    $wave = [System.Text.Encoding]::ASCII.GetString($bytes, 8, 4)
    if ($riff -ne "RIFF" -or $wave -ne "WAVE") {
        return
    }

    $offset = 12
    $bitsPerSample = $null
    $dataOffset = $null
    $dataSize = $null
    while ($offset + 8 -le $bytes.Length) {
        $chunkId = [System.Text.Encoding]::ASCII.GetString($bytes, $offset, 4)
        $chunkSize = [BitConverter]::ToInt32($bytes, $offset + 4)
        $chunkDataOffset = $offset + 8
        if ($chunkDataOffset + $chunkSize -gt $bytes.Length) {
            break
        }
        if ($chunkId -eq "fmt " -and $chunkSize -ge 16) {
            $audioFormat = [BitConverter]::ToInt16($bytes, $chunkDataOffset)
            $bitsPerSample = [BitConverter]::ToInt16($bytes, $chunkDataOffset + 14)
            if ($audioFormat -ne 1) {
                return
            }
        } elseif ($chunkId -eq "data") {
            $dataOffset = $chunkDataOffset
            $dataSize = $chunkSize
        }
        $offset = $chunkDataOffset + $chunkSize
        if (($chunkSize % 2) -eq 1) {
            $offset += 1
        }
    }
    if ($bitsPerSample -ne 16 -or $null -eq $dataOffset -or $null -eq $dataSize) {
        return
    }

    $peak = 0
    for ($i = $dataOffset; $i + 1 -lt ($dataOffset + $dataSize); $i += 2) {
        $sample = [BitConverter]::ToInt16($bytes, $i)
        $abs = [Math]::Abs([int]$sample)
        if ($abs -gt $peak) {
            $peak = $abs
        }
    }
    if ($peak -le 0) {
        return
    }

    $targetPeak = 26000.0
    $effectiveGain = [Math]::Min($Gain, $targetPeak / [double]$peak)
    if ($effectiveGain -le 0.0) {
        return
    }
    Write-SmokeTrace ("wav_gain requested={0:0.###} effective={1:0.###} peak={2} target_peak={3}" -f $Gain, $effectiveGain, $peak, [int]$targetPeak)

    for ($i = $dataOffset; $i + 1 -lt ($dataOffset + $dataSize); $i += 2) {
        $sample = [BitConverter]::ToInt16($bytes, $i)
        $scaled = [int][Math]::Round($sample * $effectiveGain)
        if ($scaled -gt [int16]::MaxValue) {
            $scaled = [int16]::MaxValue
        } elseif ($scaled -lt [int16]::MinValue) {
            $scaled = [int16]::MinValue
        }
        [BitConverter]::GetBytes([int16]$scaled).CopyTo($bytes, $i)
    }
    [System.IO.File]::WriteAllBytes($Path, $bytes)
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

if ($PlaybackCount -lt 1) {
    throw "PlaybackCount must be >= 1"
}
if ($RecordPlaybackIndex -lt 1 -or $RecordPlaybackIndex -gt $PlaybackCount) {
    throw "RecordPlaybackIndex must be between 1 and PlaybackCount"
}
if ($RandomSentenceCount -lt 1) {
    throw "RandomSentenceCount must be >= 1"
}
if ($NoNotificationTimeoutSeconds -lt 1) {
    throw "NoNotificationTimeoutSeconds must be >= 1"
}

$RunStamp = Get-Date -Format "yyyyMMdd-HHmmss"
$script:TraceLogPath = Join-Path $OutDir "ble-stream-smoke-$RunStamp.trace.log"
Write-SmokeTrace "start trigger=$TriggerMode port=$Port timeout_ms=$TimeoutMs"
$AudioProfileConfig = Get-SmokeAudioProfile -Name $AudioProfile
if (-not $PSBoundParameters.ContainsKey("TtsRate")) {
    $TtsRate = [int]$AudioProfileConfig.tts_rate
}
if (-not $PSBoundParameters.ContainsKey("TtsGain")) {
    $TtsGain = [double]$AudioProfileConfig.tts_gain
}
if (-not $Sentence) {
    $Sentence = New-RandomSentence -Count $RandomSentenceCount
}
if (-not $ExpectedText -and -not $ExpectNoText) {
    $ExpectedText = $Sentence
}
if (-not $WavPath) {
    $WavPath = Join-Path $OutDir "ble-stream-smoke-$RunStamp.wav"
}
$WavPath = Resolve-RepoPath $WavPath
if (-not (Test-Path $WavPath)) {
    if ($SilentAudio) {
        New-SilenceWave -Path $WavPath -DurationMs $SilentAudioMs
    } else {
        New-TtsWave -Text $Sentence -Path $WavPath -PreferredVoice $VoiceName -Rate $TtsRate
        Boost-WavPcm16 -Path $WavPath -Gain $TtsGain
    }
}
Write-SmokeTrace "wav_ready path=$WavPath sentence=$Sentence profile=$AudioProfile rate=$TtsRate gain=$TtsGain silent=$([bool]$SilentAudio)"
$wavDurationMs = Get-WavDurationMilliseconds -Path $WavPath
$recordingWindowMs = [System.Math]::Max(
    1000,
    $PreRecordDelayMs + $wavDurationMs + $PostPlaybackRecordMs
)
$serialLogPath = Join-Path $OutDir "ble-stream-smoke-$RunStamp.serial.log"
$serialStartSignalPath = Join-Path $OutDir "ble-stream-smoke-$RunStamp.start.signal"
$serialStopSignalPath = Join-Path $OutDir "ble-stream-smoke-$RunStamp.stop.signal"
$serialRecordingStartedSignalPath = Join-Path $OutDir "ble-stream-smoke-$RunStamp.recording-started.signal"
Remove-Item -LiteralPath $serialStartSignalPath -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $serialStopSignalPath -Force -ErrorAction SilentlyContinue

if (-not (Test-Path $ListenerExe)) {
    $env:PATH = "C:\Users\Billy\.cargo\bin;$env:PATH"
    cargo build --manifest-path (Join-Path $RepoRoot "src-tauri\Cargo.toml")
}

if (-not $NoResetBeforeCapture) {
    Write-SmokeTrace "serial_reset_start"
    Reset-SerialTarget -PortName $Port
    Start-Sleep -Seconds 2
    Write-SmokeTrace "serial_reset_done"
} else {
    Write-SmokeTrace "serial_cancel_start"
    Send-SerialCommand -PortName $Port -Command "~VREC:CANCEL"
    Start-Sleep -Milliseconds 500
    Write-SmokeTrace "serial_cancel_done"
}

if (-not $SkipEnsureBle) {
    Write-SmokeTrace "ensure_ble_start"
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
    Write-SmokeTrace "ensure_ble_done"
}

$logPath = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
$logDir = Split-Path -Parent $logPath
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
$logOffsetValue = if (Test-Path $logPath) { (Get-Item $logPath).Length } else { 0 }
$logOffset = [ref]$logOffsetValue
$capturedLog = ""
$listenerChildStdoutLog = Join-Path $OutDir "listener-type-child-$RunStamp.stdout.log"
$listenerChildStderrLog = Join-Path $OutDir "listener-type-child-$RunStamp.stderr.log"

$foregroundBeforeListenerStart = Get-ForegroundWindowSnapshot
Write-ForegroundTrace -Label "before_listener_start" -Snapshot $foregroundBeforeListenerStart
Get-Process listener-type -ErrorAction SilentlyContinue | Stop-Process -Force

$process = $null
$serialWindow = $null
$serialReport = $null
$recordingStarted = $false
$insertionTarget = $null
$insertedText = $null
$historySession = $null
$historyLookupSkipped = $false
$recordingArchivePath = $null
$recordPlaybackDurationMs = $null
$recordPlaybackStarted = $false
$smokeStartedAt = Get-Date
$timeline = [ordered]@{
    smoke_started_at_utc = $smokeStartedAt.ToUniversalTime().ToString("o")
}
$scriptExitCode = 0
$usesSerialSignal = @("serial-toggle", "serial-cancel") -contains $TriggerMode
$serialEndCommand = if ($TriggerMode -eq "serial-cancel") { "cancel" } else { "toggle" }
$expectedStreamFailure = $null
try {
    if ($VerifyInsertion) {
        $insertionTargetPath = Join-Path $OutDir "ble-stream-smoke-$RunStamp.target.txt"
        $insertionTarget = Start-InsertionTarget -Path $insertionTargetPath
        Write-SmokeTrace "insertion_target_started path=$insertionTargetPath"
    }

    if ($usesSerialSignal) {
        $serialWindow = Start-SerialRecordingWindow `
            -PortName $Port `
            -MaxRecordMs ([System.Math]::Max(15000, $recordingWindowMs + 10000)) `
            -SerialLogPath $serialLogPath `
            -StartSignalPath $serialStartSignalPath `
            -StopSignalPath $serialStopSignalPath `
            -RecordingStartedSignalPath $serialRecordingStartedSignalPath `
            -RecordingStartTimeoutMs $RecordingStartTimeoutMs `
            -EndCommand $serialEndCommand
        Write-SmokeTrace "serial_window_started path=$serialLogPath"
    }

    $oldHideMain = $env:LISTENER_TYPE_HIDE_MAIN_ON_START
    $oldDisableBle = $env:LISTENER_TYPE_DISABLE_BACKGROUND_BLE
    $oldForceRaw = $env:LISTENER_TYPE_FORCE_RAW_OUTPUT
    $oldRecordEmbedded = $env:LISTENER_TYPE_RECORD_EMBEDDED_AUDIO_FOR_DEBUG
    $oldForegroundInsert = $env:LISTENER_TYPE_INSERT_INTO_FOREGROUND_FALLBACK
    try {
        $env:LISTENER_TYPE_HIDE_MAIN_ON_START = "1"
        $env:LISTENER_TYPE_DISABLE_BACKGROUND_BLE = "1"
        $env:LISTENER_TYPE_FORCE_RAW_OUTPUT = "1"
        $env:LISTENER_TYPE_RECORD_EMBEDDED_AUDIO_FOR_DEBUG = "1"
        $env:LISTENER_TYPE_INSERT_INTO_FOREGROUND_FALLBACK = "1"
        $process = Start-Process -FilePath (Resolve-Path $ListenerExe).Path `
            -ArgumentList @("--submit-embedded-audio-ble-stream", ([string]$TimeoutMs)) `
            -WorkingDirectory $RepoRoot `
            -WindowStyle Hidden `
            -RedirectStandardOutput $listenerChildStdoutLog `
            -RedirectStandardError $listenerChildStderrLog `
            -PassThru
        $timeline["listener_started_at_utc"] = Get-SmokeUtcNow
        Write-SmokeTrace "listener_started pid=$($process.Id)"
    } finally {
        if ($null -eq $oldHideMain) { Remove-Item Env:LISTENER_TYPE_HIDE_MAIN_ON_START -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_HIDE_MAIN_ON_START = $oldHideMain }
        if ($null -eq $oldDisableBle) { Remove-Item Env:LISTENER_TYPE_DISABLE_BACKGROUND_BLE -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_DISABLE_BACKGROUND_BLE = $oldDisableBle }
        if ($null -eq $oldForceRaw) { Remove-Item Env:LISTENER_TYPE_FORCE_RAW_OUTPUT -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_FORCE_RAW_OUTPUT = $oldForceRaw }
        if ($null -eq $oldRecordEmbedded) { Remove-Item Env:LISTENER_TYPE_RECORD_EMBEDDED_AUDIO_FOR_DEBUG -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_RECORD_EMBEDDED_AUDIO_FOR_DEBUG = $oldRecordEmbedded }
        if ($null -eq $oldForegroundInsert) { Remove-Item Env:LISTENER_TYPE_INSERT_INTO_FOREGROUND_FALLBACK -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_INSERT_INTO_FOREGROUND_FALLBACK = $oldForegroundInsert }
    }

    $readyDeadline = (Get-Date).AddSeconds($NotifyReadyTimeoutSeconds)
    $notifyReady = $false
    while ((Get-Date) -lt $readyDeadline) {
        Start-Sleep -Milliseconds 250
        $capturedLog += Read-NewLogText -Path $logPath -Offset $logOffset
        if ($capturedLog -match "\[embedded-ble\] ValueChanged handler registered") {
            $notifyReady = $true
            break
        }
        if ($capturedLog -match "submit-embedded-audio-ble-stream failed") {
            throw "Listener-Type BLE stream failed before notify was ready"
        }
        if ($process.HasExited) {
            throw "Listener-Type exited before BLE notify was ready"
        }
    }
    if (-not $notifyReady) {
        throw "Timed out waiting for BLE notify registration"
    }
    $timeline["notify_ready_at_utc"] = Get-SmokeUtcNow
    Write-SmokeTrace "notify_ready"
    Start-Sleep -Milliseconds ([int]($NotifySettleSeconds * 1000))
    $foregroundAfterListenerReady = Get-ForegroundWindowSnapshot
    Write-ForegroundTrace -Label "after_listener_ready" -Snapshot $foregroundAfterListenerReady
    if (
        $process -and
        $foregroundAfterListenerReady.ProcessId -eq $process.Id -and
        $foregroundBeforeListenerStart.ProcessId -ne $process.Id
    ) {
        $restoredForeground = Restore-ForegroundWindow -Snapshot $foregroundBeforeListenerStart -Label "listener_start_focus"
        if (-not $restoredForeground) {
            $timeline["listener_start_focus_stolen_at_utc"] = Get-SmokeUtcNow
            throw "Listener-Type startup stole foreground focus before playback; aborting before audio playback"
        }
    }

    $streamStartedPattern = "embedded audio streaming dictation started"
    Add-Type -AssemblyName System.Windows.Forms
    $player = [System.Media.SoundPlayer]::new($WavPath)
    $player.Load()

    for ($index = 1; $index -le $PlaybackCount; $index++) {
        if ($index -eq $RecordPlaybackIndex) {
            if ($VerifyInsertion -and $insertionTarget) {
                [void](Focus-ProcessWindow -Process $insertionTarget.Process)
                Start-Sleep -Milliseconds 150
                Write-SmokeTrace "insertion_target_focused"
            }
            $foregroundBeforeCapsule = Get-ForegroundWindowSnapshot
            Write-ForegroundTrace -Label "before_capsule" -Snapshot $foregroundBeforeCapsule
            if ($usesSerialSignal) {
                Set-Content -Path $serialStartSignalPath -Value "start" -Encoding ASCII
                $recordingStarted = $true
                $timeline["serial_start_signal_at_utc"] = Get-SmokeUtcNow
                Write-SmokeTrace "serial_start_signal_written"
                Wait-RecordingStartedSignal -Window $serialWindow -TimeoutMs $RecordingStartTimeoutMs
                $timeline["firmware_recording_started_at_utc"] = Get-SmokeUtcNow
                Write-SmokeTrace "firmware_recording_start_seen"
            } else {
                Write-Output "manual_trigger_ready=1"
                Write-Output "manual_trigger_hint=press KEY1 once to start recording; playback begins after the capsule appears"
                $timeline["manual_start_ready_at_utc"] = Get-SmokeUtcNow
                if (-not $SkipCapsuleVisibleGate) {
                    if (-not (Wait-CapsuleWindowVisible -TimeoutMs $ManualTriggerReadyDelayMs -ProcessId $process.Id)) {
                        $timeline["capsule_visible_failed_at_utc"] = Get-SmokeUtcNow
                        throw "Recording capsule did not become visible after manual KEY1 start; aborting before audio playback"
                    }
                    $timeline["manual_start_capsule_visible_at_utc"] = Get-SmokeUtcNow
                    Write-SmokeTrace "manual_start_capsule_visible"
                } else {
                    Start-Sleep -Milliseconds $ManualTriggerReadyDelayMs
                }
            }
            if (-not $SkipCapsuleVisibleGate) {
                if (-not (Wait-CapsuleWindowVisible -TimeoutMs 1800 -ProcessId $process.Id)) {
                    $timeline["capsule_visible_failed_at_utc"] = Get-SmokeUtcNow
                    throw "Recording capsule did not become visible before playback; aborting before audio playback"
                }
                $timeline["capsule_visible_at_utc"] = Get-SmokeUtcNow
                Write-SmokeTrace "capsule_visible"
                Start-Sleep -Milliseconds 80
                $foregroundAfterCapsule = Get-ForegroundWindowSnapshot
                Write-ForegroundTrace -Label "after_capsule" -Snapshot $foregroundAfterCapsule
                if (
                    $foregroundAfterCapsule.ProcessId -eq $process.Id -and
                    $foregroundBeforeCapsule.ProcessId -ne $process.Id
                ) {
                    $timeline["capsule_focus_stolen_at_utc"] = Get-SmokeUtcNow
                    throw "Recording capsule stole foreground focus before playback; aborting before audio playback"
                }
            }
            Start-Sleep -Milliseconds $PreRecordDelayMs
        }
        Write-SmokeTrace "playback_start index=$index"
        $playbackStartedAt = Get-Date
        if ($index -eq $RecordPlaybackIndex) {
            $recordPlaybackStarted = $true
            $timeline["record_playback_started_at_utc"] = $playbackStartedAt.ToUniversalTime().ToString("o")
        }
        $player.PlaySync()
        $playbackElapsedMs = [int][System.Math]::Round(((Get-Date) - $playbackStartedAt).TotalMilliseconds)
        if ($index -eq $RecordPlaybackIndex) {
            $timeline["record_playback_done_at_utc"] = Get-SmokeUtcNow
        }
        Write-SmokeTrace "playback_done index=$index actual_ms=$playbackElapsedMs"
        if ($index -eq $RecordPlaybackIndex) {
            $recordPlaybackDurationMs = $playbackElapsedMs
            if ($usesSerialSignal) {
                Start-Sleep -Milliseconds $PostPlaybackRecordMs
                Set-Content -Path $serialStopSignalPath -Value "stop" -Encoding ASCII
                $timeline["serial_stop_signal_at_utc"] = Get-SmokeUtcNow
                Write-SmokeTrace "serial_stop_signal_written"
                $serialReport = Wait-SerialRecordingWindow -Window $serialWindow
                $serialWindow = $null
                $recordingStarted = $false
                $timeline["serial_window_done_at_utc"] = Get-SmokeUtcNow
                Write-SmokeTrace "serial_window_done"
                if ($VerifyInsertion -and $insertionTarget) {
                    [void](Focus-ProcessWindow -Process $insertionTarget.Process -Retries 5)
                    Write-SmokeTrace "insertion_target_refocused_after_serial"
                }
            } else {
                Write-Output "manual_trigger_playback_done=1"
                Write-Output "manual_trigger_stop_hint=press KEY1 once to stop recording now"
                $timeline["manual_stop_ready_at_utc"] = Get-SmokeUtcNow
            }
        } elseif ($index -lt $PlaybackCount) {
            Start-Sleep -Milliseconds 700
        }
    }

    $streamStarted = $capturedLog -match $streamStartedPattern
    $noNotificationDeadline = (Get-Date).AddSeconds($NoNotificationTimeoutSeconds)
    $doneDeadline = (Get-Date).AddMilliseconds($TimeoutMs + 10000)
    $doneMatch = $null
    $lastInsertionTargetFocusAt = Get-Date
    while ((Get-Date) -lt $doneDeadline) {
        Start-Sleep -Milliseconds 300
        if ($VerifyInsertion -and $insertionTarget -and (((Get-Date) - $lastInsertionTargetFocusAt).TotalMilliseconds -ge 1000)) {
            [void](Focus-ProcessWindow -Process $insertionTarget.Process -Retries 3)
            $lastInsertionTargetFocusAt = Get-Date
            Write-SmokeTrace "insertion_target_refocused"
        }
        $capturedLog += Read-NewLogText -Path $logPath -Offset $logOffset
        if (-not $streamStarted -and $capturedLog -match $streamStartedPattern) {
            $streamStarted = $true
        }
        $doneMatch = [regex]::Match(
            $capturedLog,
            "submit-embedded-audio-ble-stream done: pcm_bytes=(\d+) missing_packets=(\d+)",
            [System.Text.RegularExpressions.RegexOptions]::RightToLeft
        )
        if ($doneMatch.Success) {
            $timeline["stream_done_at_utc"] = Get-SmokeUtcNow
            Write-SmokeTrace "done_match"
            break
        }
        $failedMatch = [regex]::Match(
            $capturedLog,
            "submit-embedded-audio-ble-stream failed: (.+)",
            [System.Text.RegularExpressions.RegexOptions]::RightToLeft
        )
        if ($failedMatch.Success) {
            if ($ExpectNoText) {
                $expectedStreamFailure = $failedMatch.Groups[1].Value.Trim()
                Write-SmokeTrace "expected_stream_failure=$expectedStreamFailure"
                break
            }
            throw "Listener-Type BLE stream failed after triggered playback"
        }
        if (-not $streamStarted -and (Get-Date) -ge $noNotificationDeadline) {
            $hint = ""
            if ($serialReport -and $serialReport.transport_not_ready) {
                $hint = "; firmware serial reported BLE audio transport not ready"
            } elseif ($serialReport -and $serialReport.record_start_rejected) {
                $hint = "; firmware serial reported record start rejected"
            }
            throw "No BLE audio notifications arrived within $NoNotificationTimeoutSeconds seconds after triggered playback$hint"
        }
    }
    if ((-not $doneMatch -or -not $doneMatch.Success) -and -not $expectedStreamFailure) {
        throw "Timed out waiting for Listener-Type BLE stream completion"
    }
    Start-Sleep -Milliseconds 200
    $capturedLog += Read-NewLogText -Path $logPath -Offset $logOffset
    if (Test-Path $listenerChildStdoutLog) {
        $capturedLog += "`n"
        $capturedLog += Get-Content -Path $listenerChildStdoutLog -Raw -ErrorAction SilentlyContinue
    }

    Write-SmokeTrace "parse_transcript_start"
    $asrSummary = Get-AsrTranscriptSummaryFromLog -Text $capturedLog
    $transcript = [string]$asrSummary.final_text
    $missingPackets = if ($doneMatch -and $doneMatch.Success) { [int]$doneMatch.Groups[2].Value } else { 0 }
    $pcmBytes = if ($doneMatch -and $doneMatch.Success) { [int]$doneMatch.Groups[1].Value } else { 0 }
    $timeline["transcript_parsed_at_utc"] = Get-SmokeUtcNow
    Write-SmokeTrace "parse_transcript_done transcript_len=$($transcript.Length) pcm=$pcmBytes missing=$missingPackets"
    $status = if ($missingPackets -gt $MaxMissingPackets) { "WARNING" } else { "PASS" }
    if ($FailOnMissingPackets -and $missingPackets -gt $MaxMissingPackets) {
        $status = "FAIL"
    }
    $verificationErrors = @()
    $needsHistoryLookup = $VerifyHistory -and -not $VerifyInsertion
    if ($VerifyInsertion -and [string]::IsNullOrWhiteSpace($transcript)) {
        $needsHistoryLookup = $true
    }
    if ($needsHistoryLookup) {
        $timeline["history_wait_started_at_utc"] = Get-SmokeUtcNow
        Write-SmokeTrace "history_wait_start"
        $historySession = Wait-SmokeHistorySession `
            -StartedAt $smokeStartedAt `
            -Transcript $transcript `
            -ExpectedPcmBytes $pcmBytes `
            -TimeoutSeconds 3
        $timeline["history_wait_done_at_utc"] = Get-SmokeUtcNow
        Write-SmokeTrace "history_wait_done found=$([bool]$historySession)"
    } elseif ($VerifyHistory -or $VerifyInsertion) {
        $historyLookupSkipped = $true
        Write-SmokeTrace "history_wait_skipped transcript_len=$($transcript.Length) verify_insertion=$VerifyInsertion verify_history=$VerifyHistory"
    }
    if ($recordPlaybackStarted -or $recordingStarted -or $pcmBytes -gt 0) {
        $recordingArchivePath = Find-LatestRecordingAfter -StartedAt $smokeStartedAt
    } else {
        Write-SmokeTrace "recording_archive_lookup_skipped reason=no_record_playback"
    }
    if (-not $historySession -and $recordingArchivePath) {
        $recordingSessionId = [System.IO.Path]::GetFileNameWithoutExtension($recordingArchivePath)
        $historySession = Find-SmokeHistorySessionById -SessionId $recordingSessionId
        if ($historySession) {
            $timeline["history_recording_id_fallback_at_utc"] = Get-SmokeUtcNow
            Write-SmokeTrace "history_recording_id_fallback found=1 session_id=$recordingSessionId"
        }
    }
    if ($historySession -and [string]::IsNullOrWhiteSpace($transcript)) {
        if (-not [string]::IsNullOrWhiteSpace([string]$historySession.rawTranscript)) {
            $transcript = [string]$historySession.rawTranscript
        } elseif (-not [string]::IsNullOrWhiteSpace([string]$historySession.finalText)) {
            $transcript = [string]$historySession.finalText
        }
    } elseif ($historySession -and -not [string]::IsNullOrWhiteSpace([string]$historySession.rawTranscript)) {
        $historyTranscript = [string]$historySession.rawTranscript
        if ($historyTranscript.Length -gt $transcript.Length) {
            $transcript = $historyTranscript
        }
    }
    if ($usesSerialSignal) {
        if (-not $serialReport) {
            $verificationErrors += "serial recording report missing"
        } else {
            if (-not [bool]$serialReport.recording_start_seen) {
                $verificationErrors += "firmware recording start was not confirmed before playback"
            }
            if ($TriggerMode -eq "serial-cancel") {
                if (-not [bool]$serialReport.cancel_completed) {
                    $verificationErrors += "firmware recording cancel was not confirmed after playback"
                }
            } else {
                if (-not [bool]$serialReport.recording_stop_seen) {
                    $verificationErrors += "firmware recording stop was not confirmed after playback"
                }
            }
        }
    }
    $insertionVerified = $false
    if ($VerifyInsertion) {
        $timeline["insertion_read_started_at_utc"] = Get-SmokeUtcNow
        Write-SmokeTrace "insertion_read_start"
        $insertedText = Read-InsertionTargetText -Target $insertionTarget
        $timeline["insertion_read_done_at_utc"] = Get-SmokeUtcNow
        Write-SmokeTrace "insertion_read_done len=$(([string]$insertedText).Length)"
        $expectedText = ""
        if ($historySession -and -not [string]::IsNullOrWhiteSpace([string]$historySession.finalText)) {
            $expectedText = [string]$historySession.finalText
        } elseif (-not [string]::IsNullOrWhiteSpace($transcript)) {
            $expectedText = $transcript
        }
        if ($ExpectNoText) {
            if (-not [string]::IsNullOrWhiteSpace([string]$insertedText)) {
                $verificationErrors += "target editor contains text during no-text expectation"
            }
        } elseif ([string]::IsNullOrWhiteSpace($expectedText)) {
            $verificationErrors += "no transcript/final text available for insertion verification"
        } elseif (-not ([string]$insertedText).Contains($expectedText)) {
            $verificationErrors += "target editor does not contain final text"
        } else {
            $insertionVerified = $true
        }
    }
    if ($ExpectNoText) {
        $historyRaw = if ($historySession) { [string]$historySession.rawTranscript } else { "" }
        $historyFinal = if ($historySession) { [string]$historySession.finalText } else { "" }
        if (-not [string]::IsNullOrWhiteSpace($transcript)) {
            $verificationErrors += "transcript was produced during no-text expectation"
        }
        if (-not [string]::IsNullOrWhiteSpace($historyRaw) -or -not [string]::IsNullOrWhiteSpace($historyFinal)) {
            $verificationErrors += "history text was produced during no-text expectation"
        }
    } elseif ($VerifyHistory) {
        if (-not $historySession) {
            if (-not $insertionVerified) {
                $verificationErrors += "history session was not written for this BLE smoke"
            }
        } elseif (-not $historySession.embeddedAudioStats) {
            if (-not $insertionVerified) {
                $verificationErrors += "history session does not include embeddedAudioStats"
            }
        }
    }
    if ($verificationErrors.Count -gt 0) {
        $status = "FAIL"
    }

    Write-SmokeTrace "report_object_start"
    $serialReportJson = Convert-SerialReportForJson -Report $serialReport
    $historySessionJson = Convert-HistorySessionForJson -Session $historySession
    $finalText = if (-not [string]::IsNullOrWhiteSpace($transcript)) { $transcript } else { "" }
    $accuracyReport = Measure-TranscriptAccuracy -Expected $ExpectedText -Transcript $finalText
    $insertStatus = if ($historySessionJson -and -not [string]::IsNullOrWhiteSpace([string]$historySessionJson.insertStatus)) {
        [string]$historySessionJson.insertStatus
    } elseif ($insertionVerified) {
        "inserted"
    } else {
        $null
    }
    $report = [ordered]@{
        report_schema = Get-SmokeReportSchema
        status = $status
        trigger = $TriggerMode
        port = $Port
        audio_profile = $AudioProfile
        sentence = $Sentence
        expected_text = $ExpectedText
        transcript = $transcript
        final_text = $finalText
        partial_preview_count = [int]$asrSummary.partial_preview_count
        last_partial_preview = [string]$asrSummary.last_partial_preview
        asr_text_update_count = [int]$asrSummary.asr_text_update_count
        asr_text_updates = @($asrSummary.text_updates)
        normalized_expected = $accuracyReport.normalized_expected
        normalized_transcript = $accuracyReport.normalized_transcript
        cer = $accuracyReport.cer
        accuracy = $accuracyReport.accuracy
        accuracy_threshold = [double]$AudioProfileConfig.minimum_accuracy
        accuracy_warning_only = [bool]$AudioProfileConfig.warning_only
        wav_path = $WavPath
        tts_rate = $TtsRate
        tts_gain = $TtsGain
        random_sentence_count = $RandomSentenceCount
        silent_audio = [bool]$SilentAudio
        silent_audio_ms = $SilentAudioMs
        expect_no_text = [bool]$ExpectNoText
        expected_stream_failure = $expectedStreamFailure
        playback_count = $PlaybackCount
        record_playback_index = $RecordPlaybackIndex
        record_playback_actual_ms = $recordPlaybackDurationMs
        wav_duration_ms = $wavDurationMs
        recording_window_ms = $recordingWindowMs
        pre_record_delay_ms = $PreRecordDelayMs
        recording_start_timeout_ms = $RecordingStartTimeoutMs
        post_playback_record_ms = $PostPlaybackRecordMs
        no_notification_timeout_seconds = $NoNotificationTimeoutSeconds
        manual_trigger_ready_delay_ms = $ManualTriggerReadyDelayMs
        serial_log_path = if ($serialReport) { $serialReport.serial_log_path } else { $serialLogPath }
        serial_report = $serialReportJson
        pcm_bytes = $pcmBytes
        missing_packets = $missingPackets
        max_missing_packets = $MaxMissingPackets
        verify_insertion = [bool]$VerifyInsertion
        verify_history = [bool]$VerifyHistory
        history_lookup_skipped = [bool]$historyLookupSkipped
        insert_status = $insertStatus
        insertion_verified = [bool]$insertionVerified
        insertion_target_used = [bool]$insertionTarget
        insertion_target_path = if ($insertionTarget) { $insertionTarget.Path } else { $null }
        inserted_text = $insertedText
        history_path = Get-HistoryPath
        recording_archive_path = $recordingArchivePath
        history_session = $historySessionJson
        verification_errors = $verificationErrors
        timeline = $timeline
        started_at_utc = $smokeStartedAt.ToUniversalTime().ToString("o")
        log_path = $logPath
    }
    Write-SmokeTrace "report_object_done"
    Write-SmokeTrace "json_convert_start"
    $compact = ConvertTo-SmokeJsonValue $report
    $pretty = $compact
    Write-SmokeTrace "json_convert_done"
    $reportPath = Join-Path $OutDir "ble-stream-smoke.$((Get-Date).ToString('yyyyMMdd-HHmmss')).json"
    $timeline["report_write_started_at_utc"] = Get-SmokeUtcNow
    Write-SmokeTrace "report_write_start path=$reportPath status=$status"
    Set-Content -Path $reportPath -Value $pretty -Encoding UTF8
    Write-Output $pretty
    Write-Output "ble_stream_smoke_result_json=$compact"
    Write-SmokeTrace "report_write_done"
    if ($status -eq "FAIL") {
        $scriptExitCode = 1
    }
} catch {
    $caughtError = $_
    Write-SmokeTrace "catch error=$($caughtError.Exception.Message)"
    if ($recordingStarted) {
        try {
            Set-Content -Path $serialStopSignalPath -Value "stop" -Encoding ASCII
            Write-SmokeTrace "catch_serial_stop_signal_written"
            $serialReport = Wait-SerialRecordingWindow -Window $serialWindow
            $serialWindow = $null
        } catch {
            Write-Warning "Failed to finish serial recording window: $_"
        }
    }
    $capturedLog += Read-NewLogText -Path $logPath -Offset $logOffset
    if ($VerifyInsertion -and $insertionTarget) {
        try {
            $insertedText = Read-InsertionTargetText -Target $insertionTarget
        } catch {
        }
    }
    if ($recordPlaybackStarted -or $recordingStarted) {
        $recordingArchivePath = Find-LatestRecordingAfter -StartedAt $smokeStartedAt
    } else {
        Write-SmokeTrace "recording_archive_lookup_skipped reason=no_record_playback"
    }
    $serialReportJson = Convert-SerialReportForJson -Report $serialReport
    $asrSummary = Get-AsrTranscriptSummaryFromLog -Text $capturedLog
    $transcript = [string]$asrSummary.final_text
    $finalText = if (-not [string]::IsNullOrWhiteSpace($transcript)) { $transcript } else { "" }
    $accuracyReport = Measure-TranscriptAccuracy -Expected $ExpectedText -Transcript $finalText
    $report = [ordered]@{
        report_schema = Get-SmokeReportSchema
        status = "FAIL"
        trigger = $TriggerMode
        port = $Port
        audio_profile = $AudioProfile
        sentence = $Sentence
        expected_text = $ExpectedText
        transcript = $transcript
        final_text = $finalText
        partial_preview_count = [int]$asrSummary.partial_preview_count
        last_partial_preview = [string]$asrSummary.last_partial_preview
        asr_text_update_count = [int]$asrSummary.asr_text_update_count
        asr_text_updates = @($asrSummary.text_updates)
        normalized_expected = $accuracyReport.normalized_expected
        normalized_transcript = $accuracyReport.normalized_transcript
        cer = $accuracyReport.cer
        accuracy = $accuracyReport.accuracy
        accuracy_threshold = [double]$AudioProfileConfig.minimum_accuracy
        accuracy_warning_only = [bool]$AudioProfileConfig.warning_only
        wav_path = $WavPath
        tts_rate = $TtsRate
        tts_gain = $TtsGain
        random_sentence_count = $RandomSentenceCount
        silent_audio = [bool]$SilentAudio
        silent_audio_ms = $SilentAudioMs
        expect_no_text = [bool]$ExpectNoText
        expected_stream_failure = $expectedStreamFailure
        record_playback_actual_ms = $recordPlaybackDurationMs
        wav_duration_ms = $wavDurationMs
        recording_window_ms = $recordingWindowMs
        pre_record_delay_ms = $PreRecordDelayMs
        recording_start_timeout_ms = $RecordingStartTimeoutMs
        post_playback_record_ms = $PostPlaybackRecordMs
        no_notification_timeout_seconds = $NoNotificationTimeoutSeconds
        manual_trigger_ready_delay_ms = $ManualTriggerReadyDelayMs
        serial_log_path = if ($serialReport) { $serialReport.serial_log_path } else { $serialLogPath }
        serial_report = $serialReportJson
        verify_insertion = [bool]$VerifyInsertion
        verify_history = [bool]$VerifyHistory
        insert_status = $null
        insertion_target_used = [bool]$insertionTarget
        insertion_target_path = if ($insertionTarget) { $insertionTarget.Path } else { $null }
        inserted_text = $insertedText
        history_path = Get-HistoryPath
        history_session = $null
        recording_archive_path = $recordingArchivePath
        error = $caughtError.Exception.Message
        timeline = $timeline
        started_at_utc = $smokeStartedAt.ToUniversalTime().ToString("o")
        log_path = $logPath
    }
    $compact = ConvertTo-SmokeJsonValue $report
    $pretty = $compact
    $reportPath = Join-Path $OutDir "ble-stream-smoke.$((Get-Date).ToString('yyyyMMdd-HHmmss')).json"
    $timeline["catch_report_write_started_at_utc"] = Get-SmokeUtcNow
    Write-SmokeTrace "catch_report_write_start path=$reportPath"
    Set-Content -Path $reportPath -Value $pretty -Encoding UTF8
    Write-Output $pretty
    Write-Output "ble_stream_smoke_result_json=$compact"
    Write-SmokeTrace "catch_report_write_done"
    $scriptExitCode = 1
} finally {
    Write-SmokeTrace "finally_start"
    Stop-SerialRecordingWindow -Window $serialWindow
    Stop-InsertionTarget -Target $insertionTarget
    if ($process -and -not $process.HasExited) {
        try {
            $process.Kill()
            [void]$process.WaitForExit(2000)
        } catch {
        }
    }
    if ($process) { try { $process.Dispose() } catch {} }
    Write-SmokeTrace "finally_done"
}

if ($scriptExitCode -ne 0) {
    exit $scriptExitCode
}
