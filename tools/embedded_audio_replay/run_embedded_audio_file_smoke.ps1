param(
    [Parameter(Mandatory=$true)]
    [string]$WavPath,
    [int]$TimeoutMs = 30000,
    [string]$Sentence,
    [string]$ExpectedText,
    [string]$ListenerExe,
    [string]$OutDir = "artifacts\embedded_file_smoke",
    [int]$MaxMissingPackets = 0,
    [double]$MinimumAccuracy = 0.85,
    [switch]$AccuracyWarningOnly,
    [switch]$FailOnMissingPackets,
    [switch]$VerifyInsertion,
    [switch]$VerifyHistory,
    [switch]$UseExistingInstance
)

$ErrorActionPreference = "Stop"

function Resolve-RepoPath {
    param([string]$Path)
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return $Path
    }
    return Join-Path $RepoRoot $Path
}

function Quote-ProcessArgument {
    param([string]$Value)
    return '"' + ($Value -replace '"', '\"') + '"'
}

function Read-NewLogText {
    param([string]$Path, [ref]$Offset)
    if (-not (Test-Path $Path)) { return "" }
    $stream = [System.IO.File]::Open($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    try {
        if ($stream.Length -lt $Offset.Value) { $Offset.Value = 0 }
        [void]$stream.Seek($Offset.Value, [System.IO.SeekOrigin]::Begin)
        $remaining = [int]($stream.Length - $stream.Position)
        if ($remaining -le 0) { return "" }
        $bytes = New-Object byte[] $remaining
        $read = $stream.Read($bytes, 0, $remaining)
        $Offset.Value = $stream.Position
        return [System.Text.Encoding]::UTF8.GetString($bytes, 0, $read)
    } finally {
        $stream.Dispose()
    }
}

function Get-LatestAsrTranscriptFromLog {
    param([string]$Text)
    $latestAsr = ""
    $latestCapsule = ""
    foreach ($line in ($Text -split "(`r`n|`n)")) {
        $marker = "server JSON:"
        $index = $line.IndexOf($marker)
        if ($index -lt 0) { continue }
        $jsonText = $line.Substring($index + $marker.Length).Trim()
        try {
            $payload = $jsonText | ConvertFrom-Json
            if ($payload.result -and -not [string]::IsNullOrWhiteSpace([string]$payload.result.text)) {
                $latestAsr = [string]$payload.result.text
            }
        } catch {
            $textMatch = [regex]::Match($jsonText, '"text"\s*:\s*"(?<text>(?:\\.|[^"\\])*)"')
            if ($textMatch.Success) {
                $fallbackText = [regex]::Unescape([string]$textMatch.Groups["text"].Value)
                if (-not [string]::IsNullOrWhiteSpace($fallbackText)) {
                    $latestAsr = $fallbackText
                }
            }
        }
    }
    foreach ($match in [regex]::Matches($Text, "source=backend\.capsule event=emit_request .*? message=(?<message>.+)(?:`r?`n|$)")) {
        $message = [string]$match.Groups["message"].Value
        if (-not [string]::IsNullOrWhiteSpace($message) -and $message -ne "-") {
            $latestCapsule = $message.Trim()
        }
    }
    if (-not [string]::IsNullOrWhiteSpace($latestAsr)) {
        return $latestAsr
    }
    return $latestCapsule
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

function Ensure-WindowInterop {
    if ("ListenerSmokeWindow" -as [type]) { return }
    Add-Type @"
using System;
using System.Runtime.InteropServices;

public static class ListenerSmokeWindow {
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
}
"@
}

function Focus-ProcessWindow {
    param([System.Diagnostics.Process]$Process, [int]$Retries = 30)
    if (-not $Process) { return $false }
    Ensure-WindowInterop
    for ($i = 0; $i -lt $Retries; $i++) {
        if ($Process.HasExited) { return $false }
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
    $targetScript = Join-Path ([System.IO.Path]::GetTempPath()) "listener_file_insert_target_$PID.ps1"
    $targetScriptBody = @'
param([string]$Path)
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$form = [System.Windows.Forms.Form]::new()
$form.Text = "Listener File Smoke Target"
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
$timer.Add_Tick({ [System.IO.File]::WriteAllText($Path, $textBox.Text, [System.Text.Encoding]::UTF8) })
$form.Add_Shown({ $textBox.Focus() })
$form.Add_FormClosed({
    [System.IO.File]::WriteAllText($Path, $textBox.Text, [System.Text.Encoding]::UTF8)
    $timer.Stop()
    $timer.Dispose()
})
$timer.Start()
[System.Windows.Forms.Application]::Run($form)
'@
    Set-Content -Path $targetScript -Value $targetScriptBody -Encoding UTF8
    $process = Start-Process -FilePath "pwsh.exe" `
        -ArgumentList @("-NoProfile", "-STA", "-File", (Quote-ProcessArgument $targetScript), "-Path", (Quote-ProcessArgument $Path)) `
        -PassThru
    [void](Focus-ProcessWindow -Process $process)
    return [pscustomobject]@{ Process = $process; Path = $Path; ScriptPath = $targetScript }
}

function Read-InsertionTargetText {
    param($Target)
    if (-not $Target) { return $null }
    Start-Sleep -Milliseconds 500
    if (-not (Test-Path $Target.Path)) { return "" }
    $text = Get-Content -Path $Target.Path -Raw -ErrorAction SilentlyContinue
    if ($null -eq $text) { return "" }
    return $text
}

function Stop-InsertionTarget {
    param($Target)
    if (-not $Target) { return }
    try { [void](Read-InsertionTargetText -Target $Target) } catch {}
    if ($Target.Process -and -not $Target.Process.HasExited) {
        try {
            [void]$Target.Process.CloseMainWindow()
            if (-not $Target.Process.WaitForExit(2000)) { $Target.Process.Kill() }
        } catch {}
    }
    if ($Target.ScriptPath) {
        Remove-Item -LiteralPath $Target.ScriptPath -Force -ErrorAction SilentlyContinue
    }
}

function Get-HistoryPath {
    if ($env:APPDATA) { return Join-Path $env:APPDATA "Listener Type\history.json" }
    return $null
}

function Find-SmokeHistorySession {
    param([datetime]$StartedAt, [string]$Transcript, [int]$ExpectedPcmBytes = 0)
    $historyPath = Get-HistoryPath
    if (-not $historyPath -or -not (Test-Path $historyPath)) { return $null }
    $raw = Get-Content -Path $historyPath -Raw -ErrorAction SilentlyContinue
    if ([string]::IsNullOrWhiteSpace($raw)) { return $null }
    $sessions = @($raw | ConvertFrom-Json)
    $threshold = $StartedAt.ToUniversalTime().AddSeconds(-10)
    $candidates = @()
    foreach ($session in $sessions) {
        if (-not $session.createdAt) { continue }
        try { $created = ([datetime]::Parse([string]$session.createdAt)).ToUniversalTime() } catch { continue }
        if ($created -lt $threshold) { continue }
        $rawTranscript = [string]$session.rawTranscript
        $finalText = [string]$session.finalText
        $stats = $session.embeddedAudioStats
        $score = 0
        if ($stats) { $score += 2 }
        if ($ExpectedPcmBytes -gt 0 -and $stats) {
            if ([int64]$stats.receivedPcmBytes -eq [int64]$ExpectedPcmBytes -or [int64]$stats.reconstructedPcmBytes -eq [int64]$ExpectedPcmBytes) {
                $score += 4
            }
        }
        if (-not [string]::IsNullOrWhiteSpace($Transcript)) {
            $matchesTranscript =
                ((-not [string]::IsNullOrWhiteSpace($rawTranscript)) -and $rawTranscript.Contains($Transcript)) -or
                ((-not [string]::IsNullOrWhiteSpace($finalText)) -and $finalText.Contains($Transcript)) -or
                ((-not [string]::IsNullOrWhiteSpace($rawTranscript)) -and $Transcript.Contains($rawTranscript)) -or
                ((-not [string]::IsNullOrWhiteSpace($finalText)) -and $Transcript.Contains($finalText))
            if ($matchesTranscript) {
                $score += 1
            }
        }
        if ($score -gt 0) {
            $candidates += [pscustomobject]@{ Created = $created; Score = $score; Session = $session }
        }
    }
    if ($candidates.Count -eq 0) { return $null }
    return ($candidates | Sort-Object Score, Created -Descending | Select-Object -First 1).Session
}

function Wait-SmokeHistorySession {
    param([datetime]$StartedAt, [string]$Transcript, [int]$ExpectedPcmBytes = 0, [int]$TimeoutSeconds = 15)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        $session = Find-SmokeHistorySession -StartedAt $StartedAt -Transcript $Transcript -ExpectedPcmBytes $ExpectedPcmBytes
        if ($session) { return $session }
        Start-Sleep -Milliseconds 300
    } while ((Get-Date) -lt $deadline)
    return $null
}

$ScriptDir = Split-Path -Parent $PSCommandPath
$RepoRoot = Resolve-Path (Join-Path $ScriptDir "..\..")
if (-not $ListenerExe) {
    $ListenerExe = Join-Path $RepoRoot "src-tauri\target\debug\listener-type.exe"
}
$OutDir = Resolve-RepoPath $OutDir
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$WavPath = Resolve-RepoPath $WavPath
if (-not (Test-Path $WavPath)) { throw "WAV not found: $WavPath" }
if (-not (Test-Path $ListenerExe)) {
    $frontendDist = Join-Path $RepoRoot "dist"
    if (-not (Test-Path $frontendDist)) {
        throw "Listener executable not found at $ListenerExe and Tauri frontend dist is missing at $frontendDist. Build the frontend first or pass -ListenerExe <existing listener-type.exe>."
    }
    $cargoBin = Join-Path $HOME ".cargo\bin"
    if (Test-Path -LiteralPath $cargoBin) {
        $env:PATH = "$cargoBin;$env:PATH"
    }
    $runningLt = Get-Process listener-type -ErrorAction SilentlyContinue
    if ($runningLt) {
        Write-Host "Stopping running Listener Type before cargo build..."
        $runningLt | Stop-Process -Force
        Start-Sleep -Milliseconds 600
    }
    cargo build --manifest-path (Join-Path $RepoRoot "src-tauri\Cargo.toml")
}
if (-not (Test-Path $ListenerExe)) {
    throw "Listener executable not found after cargo build: $ListenerExe"
}

$RunStamp = Get-Date -Format "yyyyMMdd-HHmmss"
$logPath = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
$logDir = Split-Path -Parent $logPath
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
$logOffsetValue = if (Test-Path $logPath) { (Get-Item $logPath).Length } else { 0 }
$logOffset = [ref]$logOffsetValue
$capturedLog = ""
$process = $null
$insertionTarget = $null
$insertedText = $null
$historySession = $null
$smokeStartedAt = Get-Date

$resolvedListenerExe = (Resolve-Path $ListenerExe).Path
if ($UseExistingInstance) {
    $existingInstance = Get-Process listener-type -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $resolvedListenerExe } |
        Select-Object -First 1
    if (-not $existingInstance) {
        throw "UseExistingInstance requested, but no running Listener Type process matches $resolvedListenerExe"
    }
} else {
    Get-Process listener-type -ErrorAction SilentlyContinue | Stop-Process -Force
}

try {
    if ($VerifyInsertion) {
        $insertionTargetPath = Join-Path $OutDir "embedded-file-smoke-$RunStamp.target.txt"
        $insertionTarget = Start-InsertionTarget -Path $insertionTargetPath
        # The app captures its insertion target when the command begins.
        if (-not (Focus-ProcessWindow -Process $insertionTarget.Process)) {
            throw "Could not focus the insertion target before starting the embedded audio command"
        }
        Start-Sleep -Milliseconds 150
    }

    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $resolvedListenerExe
    $psi.Arguments = "--suppress-capsule-window --force-raw-output --submit-embedded-audio-wav-stream " + (Quote-ProcessArgument $WavPath)
    $psi.WorkingDirectory = $RepoRoot
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.EnvironmentVariables["LISTENER_TYPE_HIDE_MAIN_ON_START"] = "1"
    $psi.EnvironmentVariables["LISTENER_TYPE_SUPPRESS_CAPSULE_WINDOW"] = "1"
    $psi.EnvironmentVariables["LISTENER_TYPE_DISABLE_BACKGROUND_BLE"] = "1"
    $psi.EnvironmentVariables["LISTENER_TYPE_FORCE_RAW_OUTPUT"] = "1"
    $process = [System.Diagnostics.Process]::Start($psi)

    $doneDeadline = (Get-Date).AddMilliseconds($TimeoutMs + 10000)
    $doneMatch = $null
    while ((Get-Date) -lt $doneDeadline) {
        Start-Sleep -Milliseconds 300
        $capturedLog += Read-NewLogText -Path $logPath -Offset $logOffset
        $doneMatch = [regex]::Match(
            $capturedLog,
            "submit-embedded-audio-streaming-file done: pcm_bytes=(\d+) missing_packets=(\d+)",
            [System.Text.RegularExpressions.RegexOptions]::RightToLeft
        )
        if ($doneMatch.Success) { break }
        if ($capturedLog -match "submit-embedded-audio-streaming-file failed") {
            throw "Listener-Type embedded audio file stream failed"
        }
        if (-not $UseExistingInstance -and $process.HasExited -and -not $doneMatch.Success) {
            throw "Listener-Type exited before embedded audio file stream completed"
        }
    }
    if (-not $doneMatch -or -not $doneMatch.Success) {
        throw "Timed out waiting for Listener-Type embedded audio file stream completion"
    }

    Start-Sleep -Milliseconds 200
    $capturedLog += Read-NewLogText -Path $logPath -Offset $logOffset
    $streamTranscript = Get-LatestAsrTranscriptFromLog -Text $capturedLog
    $transcript = $streamTranscript
    $missingPackets = [int]$doneMatch.Groups[2].Value
    $pcmBytes = [int]$doneMatch.Groups[1].Value
    $verificationErrors = @()
    $status = if ($missingPackets -gt $MaxMissingPackets) { "WARNING" } else { "PASS" }
    if ($FailOnMissingPackets -and $missingPackets -gt $MaxMissingPackets) {
        $verificationErrors += "missing packets exceeded threshold: missing=$missingPackets threshold=$MaxMissingPackets"
    }
    if ($VerifyHistory -or $VerifyInsertion) {
        $historySession = Wait-SmokeHistorySession -StartedAt $smokeStartedAt -Transcript $transcript -ExpectedPcmBytes $pcmBytes -TimeoutSeconds 15
    }
    if ($historySession -and [string]::IsNullOrWhiteSpace($transcript)) {
        if (-not [string]::IsNullOrWhiteSpace([string]$historySession.rawTranscript)) {
            $transcript = [string]$historySession.rawTranscript
        } elseif (-not [string]::IsNullOrWhiteSpace([string]$historySession.finalText)) {
            $transcript = [string]$historySession.finalText
        }
    }
    if ($VerifyHistory) {
        if (-not $historySession) { $verificationErrors += "history session was not written for this file smoke" }
        elseif (-not $historySession.embeddedAudioStats) { $verificationErrors += "history session does not include embeddedAudioStats" }
    }
    if ($VerifyInsertion) {
        $insertedText = Read-InsertionTargetText -Target $insertionTarget
        $insertionExpectedText = if ($historySession -and -not [string]::IsNullOrWhiteSpace([string]$historySession.finalText)) { [string]$historySession.finalText } else { $transcript }
        if ([string]::IsNullOrWhiteSpace($insertionExpectedText)) { $verificationErrors += "no transcript/final text available for insertion verification" }
        elseif (-not ([string]$insertedText).Contains($insertionExpectedText)) { $verificationErrors += "target editor does not contain final text" }
    }
    $expectedTextForAccuracy = if (-not [string]::IsNullOrWhiteSpace($ExpectedText)) { $ExpectedText } else { $Sentence }
    $finalText = if ($historySession -and -not [string]::IsNullOrWhiteSpace([string]$historySession.finalText)) {
        [string]$historySession.finalText
    } elseif (-not [string]::IsNullOrWhiteSpace($transcript)) {
        $transcript
    } else {
        ""
    }
    $accuracyReport = Measure-TranscriptAccuracy -Expected $expectedTextForAccuracy -Transcript $finalText
    $accuracyWarning = $false
    $accuracyWarningMessage = $null
    if (-not [string]::IsNullOrWhiteSpace($expectedTextForAccuracy)) {
        if ([double]$accuracyReport.accuracy -lt $MinimumAccuracy) {
            $accuracyWarningMessage = "transcript accuracy below threshold: accuracy={0:0.######} threshold={1:0.######}" -f ([double]$accuracyReport.accuracy), $MinimumAccuracy
            if ($AccuracyWarningOnly) {
                $accuracyWarning = $true
                if ($status -ne "FAIL") {
                    $status = "WARNING"
                }
            } else {
                $verificationErrors += $accuracyWarningMessage
            }
        }
    }
    if ($verificationErrors.Count -gt 0) {
        $status = "FAIL"
    }
    $report = [pscustomobject]@{
        status = $status
        trigger = "existing-wav"
        sentence = $Sentence
        expected_text = $expectedTextForAccuracy
        wav_path = $WavPath
        transcript = $transcript
        stream_transcript = $streamTranscript
        final_text = $finalText
        normalized_expected = $accuracyReport.normalized_expected
        normalized_transcript = $accuracyReport.normalized_transcript
        cer = $accuracyReport.cer
        accuracy = $accuracyReport.accuracy
        accuracy_threshold = $MinimumAccuracy
        accuracy_warning_only = [bool]$AccuracyWarningOnly
        accuracy_warning = [bool]$accuracyWarning
        accuracy_warning_message = $accuracyWarningMessage
        pcm_bytes = $pcmBytes
        missing_packets = $missingPackets
        max_missing_packets = $MaxMissingPackets
        verify_insertion = [bool]$VerifyInsertion
        verify_history = [bool]$VerifyHistory
        use_existing_instance = [bool]$UseExistingInstance
        insertion_target_path = if ($insertionTarget) { $insertionTarget.Path } else { $null }
        inserted_text = $insertedText
        history_path = Get-HistoryPath
        history_session = $historySession
        verification_errors = $verificationErrors
        started_at_utc = $smokeStartedAt.ToUniversalTime().ToString("o")
        log_path = $logPath
    }
    $pretty = $report | ConvertTo-Json -Depth 8
    $compact = $report | ConvertTo-Json -Depth 8 -Compress
    $reportPath = Join-Path $OutDir "embedded-file-smoke.$((Get-Date).ToString('yyyyMMdd-HHmmss')).json"
    Set-Content -Path $reportPath -Value $pretty -Encoding UTF8
    Write-Output $pretty
    Write-Output "ble_stream_smoke_result_json=$compact"
    if ($status -eq "FAIL") { exit 1 }
} catch {
    $capturedLog += Read-NewLogText -Path $logPath -Offset $logOffset
    if ($VerifyInsertion -and $insertionTarget) {
        try { $insertedText = Read-InsertionTargetText -Target $insertionTarget } catch {}
    }
    $report = [pscustomobject]@{
        status = "FAIL"
        trigger = "existing-wav"
        sentence = $Sentence
        wav_path = $WavPath
        verify_insertion = [bool]$VerifyInsertion
        verify_history = [bool]$VerifyHistory
        use_existing_instance = [bool]$UseExistingInstance
        insertion_target_path = if ($insertionTarget) { $insertionTarget.Path } else { $null }
        inserted_text = $insertedText
        history_path = Get-HistoryPath
        error = $_.Exception.Message
        started_at_utc = $smokeStartedAt.ToUniversalTime().ToString("o")
        log_path = $logPath
    }
    $pretty = $report | ConvertTo-Json -Depth 8
    $compact = $report | ConvertTo-Json -Depth 8 -Compress
    $reportPath = Join-Path $OutDir "embedded-file-smoke.$((Get-Date).ToString('yyyyMMdd-HHmmss')).json"
    Set-Content -Path $reportPath -Value $pretty -Encoding UTF8
    Write-Output $pretty
    Write-Output "ble_stream_smoke_result_json=$compact"
    exit 1
} finally {
    Stop-InsertionTarget -Target $insertionTarget
    if (-not $UseExistingInstance -and $process -and -not $process.HasExited) {
        try {
            $process.Kill()
            [void]$process.WaitForExit(2000)
        } catch {}
    }
}
