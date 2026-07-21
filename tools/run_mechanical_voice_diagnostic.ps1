[CmdletBinding(PositionalBinding = $false)]
param(
    [Parameter(Mandatory = $true)]
    [string]$StimulusText,
    [string]$Port = "COM3",
    [string]$VoiceName = "Microsoft Huihui Desktop",
    [ValidateRange(-10, 10)]
    [int]$Rate = 0,
    [ValidateRange(0.5, 3.0)]
    [double]$PlaybackSpeedMultiplier = 1.0,
    [ValidateRange(0, 120)]
    [int]$TargetDurationSeconds = 0,
    [ValidateRange(3, 20)]
    [int]$PostStopCaptureSeconds = 8,
    [ValidateRange(0, 120)]
    [int]$LiveCaptureSeconds = 0,
    [ValidateSet("SyntheticEc11", "UsbRecordingToggle")]
    [string]$ControlMode = "SyntheticEc11",
    [Parameter(Mandatory = $true)]
    [string]$OutDir,
    [Parameter(Mandatory = $true)]
    [ValidatePattern("^[A-Za-z0-9][A-Za-z0-9._-]{2,127}$")]
    [string]$TrialId,
    [Parameter(Mandatory = $true)]
    [ValidateLength(8, 512)]
    [string]$Hypothesis,
    [Parameter(Mandatory = $true)]
    [ValidateLength(3, 512)]
    [string]$CandidateIdentity,
    [Parameter(Mandatory = $true)]
    [string]$TrialLedgerPath
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$maxVisiblePreviewGapMs = 4000
$minWholeTextCoverageRatio = 0.89
$enforceInteractivePreviewCadence = $PlaybackSpeedMultiplier -le 1.0
# A physical-microphone fixture that only captures room noise cannot attribute a
# short provider result to streaming, stop timing, or transcript handling.
$minAfeOutputPeak = 500
$maxAfeOutputPeakExclusive = 32768
$minStreamingAgcVoicedChunks = 90

$typeRoot = Split-Path -Parent $PSScriptRoot
$validationRoot = [IO.Path]::GetFullPath((Join-Path $typeRoot ".cache\validation"))
$resolvedTrialLedgerPath = [IO.Path]::GetFullPath($TrialLedgerPath)
if (-not $resolvedTrialLedgerPath.StartsWith($validationRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "TrialLedgerPath must be under $validationRoot"
}
$firmwareSerialTool = Join-Path (Split-Path -Parent $typeRoot) "Listener-Firmware\tools\send_serial_and_capture.ps1"
if (-not (Test-Path -LiteralPath $firmwareSerialTool)) {
    throw "Missing Firmware serial capture tool: $firmwareSerialTool"
}

$resolvedOutDir = [IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Force -Path $resolvedOutDir | Out-Null

function Invoke-ListenerSerialCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Command,
        [Parameter(Mandatory = $true)]
        [string]$OutputPath,
        [int]$CaptureSeconds = 0
    )

    $mutex = [Threading.Mutex]::new($false, "Global\Listener_$Port")
    try {
        if (-not $mutex.WaitOne(10000)) {
            throw "Timed out waiting for Global\Listener_$Port"
        }
        $arguments = @(
            "-NoProfile",
            "-File",
            $firmwareSerialTool,
            "-Port",
            $Port,
            "-Command",
            $Command,
            "-OutputPath",
            $OutputPath
        )
        if ($CaptureSeconds -gt 0) {
            $arguments += @("-CaptureSeconds", $CaptureSeconds)
        }
        & pwsh @arguments
        if ($LASTEXITCODE -ne 0) {
            throw "Firmware serial capture failed with exit code $LASTEXITCODE"
        }
    } finally {
        if ($null -ne $mutex) {
            try {
                $mutex.ReleaseMutex()
            } catch {
            }
            $mutex.Dispose()
        }
    }
}

function Start-ListenerSerialObservation {
    param(
        [Parameter(Mandatory = $true)]
        [int]$CaptureSeconds,
        [Parameter(Mandatory = $true)]
        [string]$OutputPath
    )

    if ($CaptureSeconds -le 0) {
        return $null
    }

    $readyEventName = "ListenerSerialObservation_" + [Guid]::NewGuid().ToString("N")
    $readyEvent = [Threading.EventWaitHandle]::new(
        $false,
        [Threading.EventResetMode]::ManualReset,
        $readyEventName
    )
    try {
        $job = Start-Job -ScriptBlock {
            param(
                [string]$FirmwareSerialTool,
                [string]$Port,
                [int]$CaptureSeconds,
                [string]$OutputPath,
                [string]$ReadyEventName
            )

            $mutex = [Threading.Mutex]::new($false, "Global\Listener_$Port")
            $readyEvent = $null
            try {
                if (-not $mutex.WaitOne(10000)) {
                    throw "Timed out waiting for Global\Listener_$Port"
                }
                $readyEvent = [Threading.EventWaitHandle]::OpenExisting($ReadyEventName)
                $readyEvent.Set()
                & pwsh -NoProfile -File $FirmwareSerialTool -Port $Port -CaptureSeconds $CaptureSeconds -OutputPath $OutputPath
                if ($LASTEXITCODE -ne 0) {
                    throw "Firmware serial observation failed with exit code $LASTEXITCODE"
                }
            } finally {
                if ($null -ne $readyEvent) {
                    $readyEvent.Dispose()
                }
                if ($null -ne $mutex) {
                    try {
                        $mutex.ReleaseMutex()
                    } catch {
                    }
                    $mutex.Dispose()
                }
            }
        } -ArgumentList $firmwareSerialTool, $Port, $CaptureSeconds, $OutputPath, $readyEventName

        if (-not $readyEvent.WaitOne(12000)) {
            Receive-Job -Job $job -Keep | Out-Null
            throw "Timed out starting command-free firmware serial observation"
        }
        return $job
    } finally {
        $readyEvent.Dispose()
    }
}

function Complete-ListenerSerialObservation {
    param(
        [AllowNull()]
        [System.Management.Automation.Job]$Job
    )

    if ($null -eq $Job) {
        return
    }

    try {
        Wait-Job -Job $Job -Timeout 30000 | Out-Null
        $jobOutput = Receive-Job -Job $Job
        if ($Job.State -ne "Completed") {
            $details = ($jobOutput | Out-String).Trim()
            throw "Firmware serial observation did not complete: $details"
        }
    } finally {
        Remove-Job -Job $Job -Force -ErrorAction SilentlyContinue
    }
}

function Start-ListenerScheduledRecording {
    param(
        [Parameter(Mandatory = $true)]
        [string]$StartCommand,
        [Parameter(Mandatory = $true)]
        [string]$StopCommand,
        [Parameter(Mandatory = $true)]
        [int]$DurationMs,
        [Parameter(Mandatory = $true)]
        [int]$PostStopCaptureSeconds,
        [Parameter(Mandatory = $true)]
        [string]$OutputPath
    )

    $eventName = "ListenerMechanicalRecording_" + [Guid]::NewGuid().ToString("N")
    $readyEvent = [Threading.EventWaitHandle]::new(
        $false,
        [Threading.EventResetMode]::AutoReset,
        $eventName
    )
    $job = Start-Job -ScriptBlock {
        param(
            [string]$FirmwareSerialTool,
            [string]$SerialPort,
            [string]$Start,
            [string]$Stop,
            [int]$PlaybackDurationMs,
            [int]$PostStopSeconds,
            [string]$SessionLog,
            [string]$ReadyEventName
        )

        $mutex = [Threading.Mutex]::new($false, "Global\Listener_$SerialPort")
        try {
            if (-not $mutex.WaitOne(10000)) {
                throw "Timed out waiting for Global\Listener_$SerialPort"
            }
            & pwsh -NoProfile -File $FirmwareSerialTool `
                -Port $SerialPort `
                -Command $Start `
                -OutputPath $SessionLog `
                -SignalEventName $ReadyEventName `
                -SignalEventOnOutput "stream session start queued" `
                -ScheduledCommandAfterEventMs $PlaybackDurationMs `
                -ScheduledCommand $Stop `
                -ScheduledCommandReadMs ($PostStopSeconds * 1000)
            if ($LASTEXITCODE -ne 0) {
                throw "Scheduled firmware serial recording failed with exit code $LASTEXITCODE"
            }
        } finally {
            try {
                $mutex.ReleaseMutex()
            } catch {
            }
            $mutex.Dispose()
        }
    } -ArgumentList $firmwareSerialTool, $Port, $StartCommand, $StopCommand, $DurationMs, $PostStopCaptureSeconds, $OutputPath, $eventName

    try {
        if (-not $readyEvent.WaitOne(12000)) {
            $details = (Receive-Job -Job $job -Keep | Out-String).Trim()
            throw "Timed out waiting for firmware recording start: $details"
        }
        return [pscustomobject]@{ Job = $job }
    } catch {
        Stop-Job -Job $job -ErrorAction SilentlyContinue
        Remove-Job -Job $job -Force -ErrorAction SilentlyContinue
        throw
    } finally {
        $readyEvent.Dispose()
    }
}

function Complete-ListenerScheduledRecording {
    param(
        [AllowNull()]
        [pscustomobject]$Recording
    )

    if ($null -eq $Recording) {
        return
    }

    try {
        Wait-Job -Job $Recording.Job -Timeout 30000 | Out-Null
        $jobOutput = Receive-Job -Job $Recording.Job
        if ($Recording.Job.State -ne "Completed") {
            $details = ($jobOutput | Out-String).Trim()
            throw "Scheduled firmware serial recording did not complete: $details"
        }
    } finally {
        Remove-Job -Job $Recording.Job -Force -ErrorAction SilentlyContinue
    }
}

function Get-IntegerMetric {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Line,
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    $escapedName = [regex]::Escape($Name)
    $match = [regex]::Match($Line, "(?:^|\s)$escapedName=(\d+)")
    if (-not $match.Success) {
        $match = [regex]::Match($Line, "`"$escapedName`":(\d+)")
    }
    if (-not $match.Success) {
        return $null
    }
    return [int64]$match.Groups[1].Value
}

function Get-AfeOutputLevelSummary {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$LogPaths
    )

    $samples = @()
    $sessionSummaries = @()
    foreach ($path in $LogPaths) {
        if (-not (Test-Path -LiteralPath $path)) {
            continue
        }
        foreach ($line in Get-Content -LiteralPath $path) {
            $match = [regex]::Match(
                $line,
                "PDM AFE level: stage=output block=\d+ peak=(\d+) mean_abs=(\d+)"
            )
            if ($match.Success) {
                $samples += [pscustomobject]@{
                    peak = [int]$match.Groups[1].Value
                    mean_abs = [int]$match.Groups[2].Value
                }
            }
            $sessionMatch = [regex]::Match(
                $line,
                "PDM AFE session signal:\s+session_id=(\d+).*?\soutput_frames=(\d+).*?\soutput_peak=(\d+).*?\soutput_mean_abs=(\d+)"
            )
            if ($sessionMatch.Success) {
                $sessionSummaries += [pscustomobject]@{
                    session_id = [int]$sessionMatch.Groups[1].Value
                    output_frames = [int]$sessionMatch.Groups[2].Value
                    peak = [int]$sessionMatch.Groups[3].Value
                    mean_abs = [int]$sessionMatch.Groups[4].Value
                }
            }
        }
    }

    $latestSession = @($sessionSummaries | Select-Object -Last 1)
    $latestSession = if ($latestSession.Count -eq 1) { $latestSession[0] } else { $null }
    $peakMax = $null
    $peakAverage = $null
    $meanAbsAverage = $null
    $meanAbsMax = $null
    if ($samples.Count -gt 0) {
        $peaks = $samples | Measure-Object -Property peak -Maximum -Average
        $means = $samples | Measure-Object -Property mean_abs -Maximum -Average
        $peakMax = [int]$peaks.Maximum
        $peakAverage = [math]::Round($peaks.Average, 3)
        $meanAbsAverage = [math]::Round($means.Average, 3)
        $meanAbsMax = [int]$means.Maximum
    }

    [pscustomobject]@{
        sample_count = $samples.Count
        peak_max = $peakMax
        peak_average = $peakAverage
        mean_abs_average = $meanAbsAverage
        mean_abs_max = $meanAbsMax
        session_summary_count = $sessionSummaries.Count
        session_id = if ($null -ne $latestSession) { $latestSession.session_id } else { $null }
        session_output_frames = if ($null -ne $latestSession) { $latestSession.output_frames } else { $null }
        session_peak = if ($null -ne $latestSession) { $latestSession.peak } else { $null }
        session_mean_abs = if ($null -ne $latestSession) { $latestSession.mean_abs } else { $null }
        acoustic_gate_source = if ($null -ne $latestSession) { "session_summary" } elseif ($samples.Count -gt 0) { "early_block_samples" } else { $null }
        acoustic_gate_peak = if ($null -ne $latestSession) { $latestSession.peak } else { $peakMax }
        acoustic_gate_mean_abs = if ($null -ne $latestSession) { $latestSession.mean_abs } else { $meanAbsAverage }
    }
}

function Get-FirmwareControlTimeline {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$LogPaths
    )

    # Preserve numeric control evidence without persisting the spoken fixture.
    $startedSessions = @()
    $transportSummaries = @()
    $cancelEvents = @()
    $lineNumber = 0
    foreach ($path in $LogPaths) {
        if (-not (Test-Path -LiteralPath $path)) {
            continue
        }
        foreach ($line in Get-Content -LiteralPath $path) {
            $lineNumber++
            if ($line.Contains("stream session start queued")) {
                $sessionId = Get-IntegerMetric -Line $line -Name "session_id"
                if ($null -ne $sessionId) {
                    $startedSessions += [pscustomobject]@{
                        session_id = [int]$sessionId
                        line_number = $lineNumber
                    }
                }
            }
            if ($line.Contains("audio session transport summary:")) {
                $sessionId = Get-IntegerMetric -Line $line -Name "session"
                if ($null -ne $sessionId) {
                    $transportSummaries += [pscustomobject]@{
                        session_id = [int]$sessionId
                        line_number = $lineNumber
                    }
                }
            }
            if ($line.Contains("VREC:CANCEL") -or $line.Contains("recording cancel requested") -or $line.Contains("recording cancel source=")) {
                $cancelEvents += [pscustomobject]@{
                    line_number = $lineNumber
                }
            }
        }
    }

    [pscustomobject]@{
        started_session_ids = @($startedSessions | ForEach-Object { $_.session_id })
        transport_summary_session_ids = @($transportSummaries | ForEach-Object { $_.session_id })
        started_sessions = @($startedSessions)
        transport_summaries = @($transportSummaries)
        cancel_event_count = $cancelEvents.Count
        cancel_events = @($cancelEvents)
    }
}

function ConvertTo-ObservationEvent {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Line
    )

    if (-not $Line.Contains("[obs-v1]")) {
        return $null
    }

    $payloadStart = $Line.IndexOf("{")
    if ($payloadStart -lt 0) {
        return $null
    }

    try {
        return $Line.Substring($payloadStart) | ConvertFrom-Json -ErrorAction Stop
    } catch {
        return $null
    }
}

function ConvertTo-NormalizedText {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Text
    )

    return [regex]::Replace(
        $Text.Normalize([Text.NormalizationForm]::FormKC),
        "[^\p{L}\p{Nd}]",
        ""
    )
}

function Assert-NaturalSinglePassageFixture {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Text
    )

    $sentences = @(
        [regex]::Split($Text.Trim(), "(?<=[。！？!?])") |
            ForEach-Object { $_.Trim() } |
            Where-Object { $_.Length -gt 0 }
    )
    if ($sentences.Count -lt 4) {
        throw "Mechanical stimulus must be one natural multi-sentence passage, not a repeated short phrase"
    }

    $normalizedSentences = @(
        $sentences | ForEach-Object { ConvertTo-NormalizedText -Text $_ } | Where-Object { $_.Length -gt 0 }
    )
    if (($normalizedSentences | Select-Object -Unique).Count -ne $normalizedSentences.Count) {
        throw "Mechanical stimulus must not repeat a sentence"
    }

    $normalizedPassage = ConvertTo-NormalizedText -Text $Text
    for ($offset = 0; $offset + 24 -le $normalizedPassage.Length; $offset += 12) {
        $phrase = $normalizedPassage.Substring($offset, 12)
        $first = $normalizedPassage.IndexOf($phrase, [StringComparison]::Ordinal)
        $last = $normalizedPassage.LastIndexOf($phrase, [StringComparison]::Ordinal)
        if ($first -ne $last) {
            throw "Mechanical stimulus must not loop a short phrase"
        }
    }
}

function Get-LongestCommonSubsequenceLength {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Expected,
        [Parameter(Mandatory = $true)]
        [string]$Actual
    )

    if ($Expected.Length -eq 0 -or $Actual.Length -eq 0) {
        return 0
    }

    $previous = New-Object int[] ($Actual.Length + 1)
    for ($expectedIndex = 1; $expectedIndex -le $Expected.Length; $expectedIndex++) {
        $current = New-Object int[] ($Actual.Length + 1)
        $expectedChar = $Expected[$expectedIndex - 1]
        for ($actualIndex = 1; $actualIndex -le $Actual.Length; $actualIndex++) {
            if ($expectedChar -eq $Actual[$actualIndex - 1]) {
                $current[$actualIndex] = $previous[$actualIndex - 1] + 1
            } else {
                $current[$actualIndex] = [Math]::Max($previous[$actualIndex], $current[$actualIndex - 1])
            }
        }
        $previous = $current
    }

    return $previous[$Actual.Length]
}

function Get-TrailingMatchLength {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Expected,
        [Parameter(Mandatory = $true)]
        [string]$Actual
    )

    $matched = 0
    while ($matched -lt $Expected.Length -and $matched -lt $Actual.Length) {
        if ($Expected[$Expected.Length - 1 - $matched] -ne $Actual[$Actual.Length - 1 - $matched]) {
            break
        }
        $matched++
    }
    return $matched
}

function Get-MechanicalFinalCoverage {
    param(
        [Parameter(Mandatory = $true)]
        [int]$EmbeddedSessionId,
        [Parameter(Mandatory = $true)]
        [string]$ExpectedText,
        [Parameter(Mandatory = $true)]
        [DateTimeOffset]$SessionStartedAt
    )

    $historyPath = Join-Path $env:APPDATA "Listener Type\history.json"
    if (-not (Test-Path -LiteralPath $historyPath)) {
        throw "Installed Type history is unavailable: $historyPath"
    }

    $entries = @(Get-Content -Raw -LiteralPath $historyPath | ConvertFrom-Json)
    $matches = @($entries | Where-Object {
        $null -ne $_.embeddedAudioStats -and
            $_.embeddedAudioStats.sessionId -eq $EmbeddedSessionId -and
            -not [string]::IsNullOrWhiteSpace([string]$_.createdAt) -and
            [DateTimeOffset]::Parse([string]$_.createdAt) -ge $SessionStartedAt
    } | Sort-Object { [DateTimeOffset]::Parse([string]$_.createdAt) })
    if ($matches.Count -eq 0) {
        throw "Installed Type history has no final result for embedded session $EmbeddedSessionId"
    }

    $entry = $matches[-1]
    $expected = ConvertTo-NormalizedText -Text $ExpectedText
    $final = ConvertTo-NormalizedText -Text ([string]$entry.finalText)
    if ($expected.Length -eq 0) {
        throw "Mechanical expected text normalized to empty"
    }

    $tailWindowChars = [Math]::Min(12, $expected.Length)
    $tailExpected = $expected.Substring($expected.Length - $tailWindowChars, $tailWindowChars)
    $tailActual = if ($final.Length -le $tailWindowChars) {
        $final
    } else {
        $final.Substring($final.Length - $tailWindowChars, $tailWindowChars)
    }
    $lcsLength = Get-LongestCommonSubsequenceLength -Expected $expected -Actual $final
    $tailMatchChars = Get-TrailingMatchLength -Expected $tailExpected -Actual $tailActual

    [ordered]@{
        expected_normalized_chars = $expected.Length
        final_text_chars = ([string]$entry.finalText).Length
        final_normalized_chars = $final.Length
        whole_text_coverage_ratio = [math]::Round($lcsLength / $expected.Length, 4)
        tail_window_chars = $tailWindowChars
        tail_match_chars = $tailMatchChars
        tail_coverage_ratio = [math]::Round($tailMatchChars / $tailWindowChars, 4)
    }
}

function Get-MechanicalFinalCoverageWithRetry {
    param(
        [Parameter(Mandatory = $true)]
        [int]$EmbeddedSessionId,
        [Parameter(Mandatory = $true)]
        [string]$ExpectedText,
        [Parameter(Mandatory = $true)]
        [DateTimeOffset]$SessionStartedAt
    )

    $lastError = $null
    for ($attempt = 1; $attempt -le 8; $attempt++) {
        try {
            return Get-MechanicalFinalCoverage `
                -EmbeddedSessionId $EmbeddedSessionId `
                -ExpectedText $ExpectedText `
                -SessionStartedAt $SessionStartedAt
        } catch {
            $lastError = $_
            if ($attempt -lt 8) {
                Start-Sleep -Milliseconds 250
            }
        }
    }
    throw $lastError
}

function Read-Pcm16Wave {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $bytes = [IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 44 -or [Text.Encoding]::ASCII.GetString($bytes, 0, 4) -ne "RIFF" -or [Text.Encoding]::ASCII.GetString($bytes, 8, 4) -ne "WAVE") {
        throw "Mechanical stimulus is not a RIFF/WAVE file: $Path"
    }

    $offset = 12
    $formatSeen = $false
    while ($offset + 8 -le $bytes.Length) {
        $chunkName = [Text.Encoding]::ASCII.GetString($bytes, $offset, 4)
        $chunkLength = [int][BitConverter]::ToUInt32($bytes, $offset + 4)
        $chunkDataOffset = $offset + 8
        if ($chunkLength -lt 0 -or $chunkDataOffset + $chunkLength -gt $bytes.Length) {
            throw "Mechanical stimulus contains an invalid WAVE chunk"
        }
        if ($chunkName -eq "fmt ") {
            if ($chunkLength -lt 16) {
                throw "Mechanical stimulus fmt chunk is incomplete"
            }
            $formatTag = [BitConverter]::ToUInt16($bytes, $chunkDataOffset)
            $channels = [BitConverter]::ToUInt16($bytes, $chunkDataOffset + 2)
            $sampleRate = [BitConverter]::ToUInt32($bytes, $chunkDataOffset + 4)
            $bitsPerSample = [BitConverter]::ToUInt16($bytes, $chunkDataOffset + 14)
            if ($formatTag -ne 1 -or $channels -ne 1 -or $sampleRate -ne 16000 -or $bitsPerSample -ne 16) {
                throw "Mechanical stimulus must be 16 kHz 16-bit mono PCM"
            }
            $formatSeen = $true
        } elseif ($chunkName -eq "data") {
            if (-not $formatSeen) {
                throw "Mechanical stimulus data precedes its fmt chunk"
            }
            $pcm = New-Object byte[] $chunkLength
            [Array]::Copy($bytes, $chunkDataOffset, $pcm, 0, $chunkLength)
            return [pscustomobject]@{ Pcm = $pcm }
        }
        $offset = $chunkDataOffset + $chunkLength + ($chunkLength % 2)
    }
    throw "Mechanical stimulus has no PCM data chunk"
}

function Write-Pcm16Wave {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [byte[]]$Pcm
    )

    $stream = [IO.File]::Open($Path, [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::None)
    $writer = [IO.BinaryWriter]::new($stream)
    try {
        $writer.Write([Text.Encoding]::ASCII.GetBytes("RIFF"))
        $writer.Write([uint32](36 + $Pcm.Length))
        $writer.Write([Text.Encoding]::ASCII.GetBytes("WAVEfmt "))
        $writer.Write([uint32]16)
        $writer.Write([uint16]1)
        $writer.Write([uint16]1)
        $writer.Write([uint32]16000)
        $writer.Write([uint32]32000)
        $writer.Write([uint16]2)
        $writer.Write([uint16]16)
        $writer.Write([Text.Encoding]::ASCII.GetBytes("data"))
        $writer.Write([uint32]$Pcm.Length)
        $writer.Write($Pcm)
    } finally {
        $writer.Dispose()
    }
}

function Get-MechanicalTypeSummary {
    param(
        [Parameter(Mandatory = $true)]
        [int]$EmbeddedSessionId,
        [Parameter(Mandatory = $true)]
        [string]$ExpectedText
    )

    $typeLogPath = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
    if (-not (Test-Path -LiteralPath $typeLogPath)) {
        throw "Installed Type log is unavailable: $typeLogPath"
    }

    $lines = [Collections.Generic.List[string]](Get-Content -LiteralPath $typeLogPath)
    $startMarker = "embedded audio streaming dictation started (embedded_session_id=$EmbeddedSessionId,"
    $startIndex = -1
    for ($index = $lines.Count - 1; $index -ge 0; $index--) {
        if ($lines[$index].Contains($startMarker)) {
            $startIndex = $index
            break
        }
    }
    if ($startIndex -lt 0) {
        throw "Installed Type did not start embedded session $EmbeddedSessionId"
    }

    $endIndex = -1
    for ($index = $startIndex + 1; $index -lt $lines.Count; $index++) {
        if ($lines[$index].Contains("background listener ready for next BLE session")) {
            $endIndex = $index
            break
        }
    }
    $evidenceGaps = [Collections.Generic.List[string]]::new()
    if ($endIndex -lt 0) {
        $endIndex = $lines.Count - 1
        [void]$evidenceGaps.Add("listener_completion")
    }

    $observationStartLine = $null
    $observationSearchStart = [math]::Max(0, $startIndex - 20)
    for ($index = $startIndex - 1; $index -ge $observationSearchStart; $index--) {
        $observation = ConvertTo-ObservationEvent -Line $lines[$index]
        if ($null -ne $observation -and $observation.event -eq "embedded_audio_started") {
            $observationStartLine = $lines[$index]
            break
        }
    }
    $observationCorrelation = if ($null -ne $observationStartLine) {
        Get-IntegerMetric -Line $observationStartLine -Name "correlation_id"
    } else {
        $null
    }
    if ($null -eq $observationCorrelation) {
        [void]$evidenceGaps.Add("observability_correlation")
    }

    $sessionLines = $lines.GetRange($startIndex, $endIndex - $startIndex + 1)
    $sessionStartedAt = [DateTimeOffset]::Parse(($sessionLines[0] -split " ", 2)[0])
    $streamingAgcSummaryLine = @(
        $sessionLines | Where-Object {
            $_.Contains("embedded audio streaming AGC summary")
        } | Select-Object -Last 1
    )
    $streamingAgcVoicedChunks = if ($streamingAgcSummaryLine.Count -eq 1) {
        Get-IntegerMetric -Line $streamingAgcSummaryLine[0] -Name "voiced_chunks"
    } else {
        [void]$evidenceGaps.Add("streaming_agc_summary")
        $null
    }
    $streamingAgcQuietChunks = if ($streamingAgcSummaryLine.Count -eq 1) {
        Get-IntegerMetric -Line $streamingAgcSummaryLine[0] -Name "quiet_chunks"
    } else {
        $null
    }
    $streamingAgcClippedSamples = if ($streamingAgcSummaryLine.Count -eq 1) {
        Get-IntegerMetric -Line $streamingAgcSummaryLine[0] -Name "clipped_samples"
    } else {
        $null
    }
    $sessionObservations = @(
        foreach ($line in $sessionLines) {
            $observation = ConvertTo-ObservationEvent -Line $line
            if ($null -eq $observation -or $null -eq $observationCorrelation -or $observation.correlation_id -ne $observationCorrelation) {
                continue
            }
            [pscustomobject]@{
                line = $line
                event = [string]$observation.event
                event_sequence = [int64]$observation.event_sequence
                monotonic_ms = [int64]$observation.monotonic_ms
                ble_lifecycle_state = [string]$observation.ble_lifecycle_state
                timing_value_ms = [int64]$observation.timing_value_ms
            }
        }
    )
    $firstPcmLine = $sessionLines | Where-Object {
        $_.Contains("event=pcm embedded_session_id=$EmbeddedSessionId packet_sequence=0")
    } | Select-Object -First 1
    $firstPreviewObservation = @(
        $sessionObservations | Where-Object {
            $_.event -match '^embedded_audio_preview_first_(?:provider_stream|final_supplement)$'
        } | Sort-Object event_sequence, monotonic_ms | Select-Object -First 1
    )
    $finalObservation = @(
        $sessionObservations | Where-Object {
            $_.event -eq "embedded_audio_final"
        } | Sort-Object event_sequence, monotonic_ms | Select-Object -First 1
    )
    $completionLine = $sessionLines | Where-Object {
        $_.Contains("background session completed while keeping notify open")
    } | Select-Object -First 1

    if ($null -eq $firstPcmLine) { [void]$evidenceGaps.Add("first_pcm") }
    if ($firstPreviewObservation.Count -ne 1) { [void]$evidenceGaps.Add("provider_preview") }
    if ($finalObservation.Count -ne 1) { [void]$evidenceGaps.Add("provider_final") }
    if ($null -eq $completionLine) { [void]$evidenceGaps.Add("listener_completion") }

    $firstPcmAt = if ($null -ne $firstPcmLine) {
        [DateTimeOffset]::Parse(($firstPcmLine -split " ", 2)[0])
    } else {
        $null
    }
    $firstPreviewAt = if ($firstPreviewObservation.Count -eq 1) {
        [DateTimeOffset]::Parse(($firstPreviewObservation[0].line -split " ", 2)[0])
    } else {
        $null
    }
    $firstPreviewLatency = if ($firstPreviewObservation.Count -eq 1) {
        $firstPreviewObservation[0].timing_value_ms
    } else {
        $null
    }
    # Only an observation emitted after the capsule state transition has been
    # accepted proves that a changed preview was actually published. Actor
    # commands are intentionally not used here: duplicate candidates may be
    # rejected before they reach the capsule.
    $publishedPreviewEvents = @(
        $sessionObservations | Where-Object {
            $_.event -match '^embedded_audio_preview(?:_first)?_(?:provider_stream|final_supplement)$'
        }
    )
    $activePublishedPreviewEvents = @(
        $publishedPreviewEvents | Where-Object {
            $_.ble_lifecycle_state -eq "recording"
        } | Sort-Object event_sequence, monotonic_ms
    )
    $postStopPublishedPreviewEvents = @(
        $publishedPreviewEvents | Where-Object {
            $_.ble_lifecycle_state -eq "connected_idle"
        }
    )
    if ($activePublishedPreviewEvents.Count -lt 2) {
        [void]$evidenceGaps.Add("published_preview_timeline")
    }
    $visiblePreviewGapsMs = [Collections.Generic.List[double]]::new()
    for ($index = 1; $index -lt $activePublishedPreviewEvents.Count; $index++) {
        [void]$visiblePreviewGapsMs.Add(
            [math]::Round($activePublishedPreviewEvents[$index].monotonic_ms - $activePublishedPreviewEvents[$index - 1].monotonic_ms)
        )
    }
    $visiblePreviewMaxGapMs = if ($visiblePreviewGapsMs.Count -gt 0) {
        ($visiblePreviewGapsMs | Measure-Object -Maximum).Maximum
    } else {
        $null
    }
    $finalTranscription = if ($finalObservation.Count -eq 1) {
        $finalObservation[0].timing_value_ms
    } else {
        $null
    }
    $missingPackets = if ($null -ne $completionLine) {
        Get-IntegerMetric -Line $completionLine -Name "missing_packets"
    } else {
        $null
    }

    $providerProgress = foreach ($line in $sessionLines) {
        if (-not $line.Contains("server metadata:")) {
            continue
        }
        $duration = [regex]::Match($line, '"audio_duration_ms":(\d+)')
        if (-not $duration.Success) {
            continue
        }
        $at = [DateTimeOffset]::Parse(($line -split " ", 2)[0])
        $elapsed = [math]::Round(($at - $sessionStartedAt).TotalMilliseconds)
        $audioDuration = [int64]$duration.Groups[1].Value
        [pscustomobject]@{
            elapsed_ms = $elapsed
            audio_duration_ms = $audioDuration
            provider_audio_lag_ms = $elapsed - $audioDuration
            is_final = $line.Contains('"has_final_frame":true')
            provider_result_chars = Get-IntegerMetric -Line $line -Name "result_chars"
        }
    }
    $providerFinalProgress = @($providerProgress | Where-Object { $_.is_final } | Select-Object -Last 1)
    if (@($providerProgress).Count -eq 0) {
        [void]$evidenceGaps.Add("provider_audio_progress")
    }
    if ($providerFinalProgress.Count -ne 1) {
        [void]$evidenceGaps.Add("provider_final_audio_duration")
    }
    $finalCoverage = [pscustomobject]@{
        expected_normalized_chars = $null
        final_text_chars = $null
        final_normalized_chars = $null
        whole_text_coverage_ratio = $null
        tail_window_chars = $null
        tail_match_chars = $null
        tail_coverage_ratio = $null
    }
    try {
        $finalCoverage = Get-MechanicalFinalCoverageWithRetry -EmbeddedSessionId $EmbeddedSessionId -ExpectedText $ExpectedText -SessionStartedAt $sessionStartedAt
    } catch {
        [void]$evidenceGaps.Add("history_final")
    }
    $deliverySummaryLine = $sessionLines | Where-Object {
        $_.Contains("pending_send_high_water_frames=")
    } | Select-Object -Last 1
    $stopLine = $sessionLines | Where-Object {
        $_.Contains("event=stop embedded_session_id=$EmbeddedSessionId expected_packets=")
    } | Select-Object -Last 1
    $stopExpectedPackets = if ($null -ne $stopLine) {
        Get-IntegerMetric -Line $stopLine -Name "expected_packets"
    } else {
        [void]$evidenceGaps.Add("stop_boundary")
        $null
    }
    $tailPacketsAfterStop = @(
        $sessionLines | Where-Object {
            $_.Contains("event=pcm embedded_session_id=$EmbeddedSessionId") -and $_.Contains("after_stop=true")
        }
    ).Count
    $completedPcmBytes = if ($null -ne $completionLine) {
        Get-IntegerMetric -Line $completionLine -Name "pcm_bytes"
    } else {
        $null
    }
    $pendingSendHighWaterFrames = if ($null -ne $deliverySummaryLine) {
        Get-IntegerMetric -Line $deliverySummaryLine -Name "pending_send_high_water_frames"
    } else {
        [void]$evidenceGaps.Add("asr_delivery_summary")
        $null
    }
    $asrCapturedAudioBytes = if ($null -ne $deliverySummaryLine) {
        $capturedBytesMatch = [regex]::Match($deliverySummaryLine, '(\d+) captured bytes')
        if ($capturedBytesMatch.Success) {
            [int64]$capturedBytesMatch.Groups[1].Value
        } else {
            [void]$evidenceGaps.Add("asr_captured_audio_bytes")
            $null
        }
    } else {
        $null
    }

    $providerMaxLagMs = if (@($providerProgress).Count -gt 0) {
        (@($providerProgress | Measure-Object -Property provider_audio_lag_ms -Maximum).Maximum)
    } else {
        $null
    }
    $providerFinalLagMs = if (@($providerProgress).Count -gt 0) {
        (@($providerProgress | Select-Object -Last 1).provider_audio_lag_ms)
    } else {
        $null
    }
    $providerStreamPreviewCandidateEvents = @(
        $sessionObservations | Where-Object {
            $_.event -eq "embedded_audio_preview_candidate_provider_stream" -and $_.ble_lifecycle_state -eq "recording"
        } | Sort-Object event_sequence, monotonic_ms
    )
    $providerStreamPreviewCandidateSource = "observability_candidate_event"
    if ($providerStreamPreviewCandidateEvents.Count -lt 2) {
        # The current provider callback logs a count-only partial-update line
        # before the actor publishes a visible preview. Older builds emitted a
        # separate candidate observation; retain that path above, then use the
        # privacy-safe provider log when the old event is absent.
        $providerStreamPreviewCandidateEvents = @(
            for ($index = 0; $index -lt $sessionLines.Count; $index++) {
                $line = $sessionLines[$index]
                $match = [regex]::Match(
                    $line,
                    "\[asr\].* partial update chars=\d+ final=false elapsed_ms=\d+"
                )
                if (-not $match.Success) {
                    continue
                }
                $at = [DateTimeOffset]::Parse(($line -split " ", 2)[0])
                [pscustomobject]@{
                    event_sequence = $index + 1
                    monotonic_ms = [math]::Round(($at - $sessionStartedAt).TotalMilliseconds)
                }
            }
        )
        $providerStreamPreviewCandidateSource = "provider_partial_update_log"
    }
    $finalSupplementPreviewCandidateEvents = @(
        $sessionObservations | Where-Object {
            $_.event -eq "embedded_audio_preview_candidate_final_supplement"
        } | Sort-Object event_sequence, monotonic_ms
    )
    if ($providerStreamPreviewCandidateEvents.Count -lt 2) {
        [void]$evidenceGaps.Add("provider_preview_candidate_timeline")
    }
    $providerPreviewCandidateGapsMs = [Collections.Generic.List[double]]::new()
    for ($index = 1; $index -lt $providerStreamPreviewCandidateEvents.Count; $index++) {
        [void]$providerPreviewCandidateGapsMs.Add(
            [math]::Round($providerStreamPreviewCandidateEvents[$index].monotonic_ms - $providerStreamPreviewCandidateEvents[$index - 1].monotonic_ms)
        )
    }
    $providerPreviewCandidateMaxGapMs = if ($providerPreviewCandidateGapsMs.Count -gt 0) {
        ($providerPreviewCandidateGapsMs | Measure-Object -Maximum).Maximum
    } else {
        $null
    }

    $lastAcceptedPacketSequence = if ($null -ne $stopExpectedPackets -and $missingPackets -eq 0 -and $stopExpectedPackets -gt 0) {
        $stopExpectedPackets - 1
    } else {
        $null
    }

    [ordered]@{
        embedded_session_id = $EmbeddedSessionId
        observability_correlation_id = $observationCorrelation
        first_pcm_after_session_start_ms = if ($null -ne $firstPcmAt) { [math]::Round(($firstPcmAt - $sessionStartedAt).TotalMilliseconds) } else { $null }
        first_preview_latency_ms = $firstPreviewLatency
        first_pcm_to_visible_preview_ms = if ($null -ne $firstPreviewAt -and $null -ne $firstPcmAt) { [math]::Round(($firstPreviewAt - $firstPcmAt).TotalMilliseconds) } else { $null }
        visible_preview_updates = $publishedPreviewEvents.Count
        visible_preview_active_updates = $activePublishedPreviewEvents.Count
        visible_preview_post_stop_updates = $postStopPublishedPreviewEvents.Count
        provider_preview_candidate_updates = $providerStreamPreviewCandidateEvents.Count
        provider_preview_candidate_source = $providerStreamPreviewCandidateSource
        final_supplement_preview_candidate_updates = $finalSupplementPreviewCandidateEvents.Count
        provider_preview_candidate_max_gap_ms = $providerPreviewCandidateMaxGapMs
        visible_preview_max_gap_ms = $visiblePreviewMaxGapMs
        published_preview_monotonic_timeline = @(
            $activePublishedPreviewEvents | ForEach-Object {
                [ordered]@{
                    event_sequence = $_.event_sequence
                    monotonic_ms = $_.monotonic_ms
                }
            }
        )
        provider_preview_candidate_monotonic_timeline = @(
            $providerStreamPreviewCandidateEvents | ForEach-Object {
                [ordered]@{
                    event_sequence = $_.event_sequence
                    monotonic_ms = $_.monotonic_ms
                }
            }
        )
        stop_expected_packets = $stopExpectedPackets
        last_accepted_packet_sequence = $lastAcceptedPacketSequence
        tail_packets_after_stop = $tailPacketsAfterStop
        type_completed_pcm_bytes = $completedPcmBytes
        asr_captured_audio_bytes = $asrCapturedAudioBytes
        provider_progress_samples = @($providerProgress).Count
        provider_audio_max_lag_ms = $providerMaxLagMs
        provider_audio_final_lag_ms = $providerFinalLagMs
        provider_final_audio_duration_ms = if ($providerFinalProgress.Count -eq 1) { $providerFinalProgress[0].audio_duration_ms } else { $null }
        provider_final_result_chars = if ($providerFinalProgress.Count -eq 1) { $providerFinalProgress[0].provider_result_chars } else { $null }
        pending_send_high_water_frames = $pendingSendHighWaterFrames
        final_transcription_ms = $finalTranscription
        final_chars = $finalCoverage.final_text_chars
        missing_packets = $missingPackets
        streaming_agc = [ordered]@{
            voiced_chunks = $streamingAgcVoicedChunks
            quiet_chunks = $streamingAgcQuietChunks
            clipped_samples = $streamingAgcClippedSamples
        }
        final_coverage = $finalCoverage
        evidence_gaps = @($evidenceGaps | Select-Object -Unique)
    }
}

function Get-MechanicalDiagnosticFailureCategory {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Message
    )

    if ($Message -match "firmware transport summary") {
        return "firmware_transport_summary_missing"
    }
    if ($Message -match "firmware session discontinuity") {
        return "firmware_session_discontinuity"
    }
    if ($Message -match "firmware session id") {
        return "firmware_session_id_missing"
    }
    if ($Message -match "capture PCM duration") {
        return "firmware_capture_duration_missing"
    }
    if ($Message -match "Installed Type") {
        return "installed_type_evidence_missing"
    }
    if ($Message -match "Firmware serial capture") {
        return "firmware_serial_capture_failed"
    }
    if ($Message -match "Global\\Listener_") {
        return "firmware_serial_lock_timeout"
    }
    return "mechanical_diagnostic_incomplete"
}

function Write-MechanicalTrialLedger {
    param(
        [Parameter(Mandatory = $true)]
        [string]$LedgerPath,
        [Parameter(Mandatory = $true)]
        [string]$SummaryPath,
        [Parameter(Mandatory = $true)]
        [string]$MetadataPath
    )

    if (-not (Test-Path -LiteralPath $SummaryPath)) {
        throw "Mechanical summary is missing, cannot record trial ledger: $SummaryPath"
    }

    $summary = Get-Content -LiteralPath $SummaryPath -Raw | ConvertFrom-Json
    $summaryTypeProperty = $summary.PSObject.Properties["type"]
    $summaryType = if ($null -ne $summaryTypeProperty) { $summaryTypeProperty.Value } else { $null }
    $summaryTailCoverageProperty = $summary.PSObject.Properties["tail_coverage"]
    $summaryTailCoverage = if ($null -ne $summaryTailCoverageProperty) { $summaryTailCoverageProperty.Value } else { $null }
    $metadata = if (Test-Path -LiteralPath $MetadataPath) {
        Get-Content -LiteralPath $MetadataPath -Raw | ConvertFrom-Json
    } else {
        $null
    }
    $ledger = if (Test-Path -LiteralPath $LedgerPath) {
        Get-Content -LiteralPath $LedgerPath -Raw | ConvertFrom-Json
    } else {
        [pscustomobject]@{
            schema_version = 1
            purpose = "Privacy-preserving Listener recording trial ledger: numeric evidence and rollback disposition only."
            updated_at = $null
            trials = @()
        }
    }
    if (@($ledger.trials | Where-Object { $_.trial_id -eq $TrialId }).Count -ne 0) {
        throw "Trial ledger already contains trial id '$TrialId'"
    }

    $installedTypeRoot = "C:\Program Files\Listener Type"
    $installedExe = Join-Path $installedTypeRoot "listener-type.exe"
    $installedDll = Join-Path $installedTypeRoot "listener_type_lib.dll"
    $summarySha256 = (Get-FileHash -LiteralPath $SummaryPath -Algorithm SHA256).Hash
    $metadataSha256 = if (Test-Path -LiteralPath $MetadataPath) {
        (Get-FileHash -LiteralPath $MetadataPath -Algorithm SHA256).Hash
    } else {
        $null
    }
    $entry = [ordered]@{
        trial_id = $TrialId
        hypothesis = $Hypothesis
        candidate_identity = $CandidateIdentity
        machine_status = $summary.status
        mode = $summary.mode
        control_mode = $summary.control_mode
        stimulus_sha256 = if ($null -ne $metadata) { $metadata.stimulus_sha256 } else { $null }
        stimulus_duration_ms = if ($null -ne $metadata) { $metadata.stimulus_duration_ms } else { $null }
        playback_speed_multiplier = if ($null -ne $metadata) { $metadata.playback_speed_multiplier } else { $null }
        firmware = $summary.firmware
        type = [ordered]@{
            installed_exe_sha256 = if (Test-Path -LiteralPath $installedExe) { (Get-FileHash -LiteralPath $installedExe -Algorithm SHA256).Hash } else { $null }
            installed_dll_sha256 = if (Test-Path -LiteralPath $installedDll) { (Get-FileHash -LiteralPath $installedDll -Algorithm SHA256).Hash } else { $null }
            first_preview_latency_ms = if ($null -ne $summaryType) { $summaryType.first_preview_latency_ms } else { $null }
            visible_preview_max_gap_ms = if ($null -ne $summaryType) { $summaryType.visible_preview_max_gap_ms } else { $null }
            missing_packets = if ($null -ne $summaryType) { $summaryType.missing_packets } else { $null }
            pending_send_high_water_frames = if ($null -ne $summaryType) { $summaryType.pending_send_high_water_frames } else { $null }
        }
        coverage = [ordered]@{
            provider_final_audio_duration_ms = if ($null -ne $summaryTailCoverage) { $summaryTailCoverage.provider_final_audio_duration_ms } else { $null }
            whole_text_coverage_ratio = if ($null -ne $summaryTailCoverage) { $summaryTailCoverage.whole_text_coverage_ratio } else { $null }
            tail_coverage_ratio = if ($null -ne $summaryTailCoverage) { $summaryTailCoverage.tail_coverage_ratio } else { $null }
        }
        evidence_gaps = @(
            @($summary.evidence_gaps) |
                Where-Object { $null -ne $_ -and -not [string]::IsNullOrWhiteSpace([string]$_) }
        )
        artifacts = [ordered]@{
            mechanical_summary = [ordered]@{
                filename = [IO.Path]::GetFileName($SummaryPath)
                sha256 = $summarySha256
            }
            stimulus_metadata = if ($null -ne $metadataSha256) {
                [ordered]@{
                    filename = [IO.Path]::GetFileName($MetadataPath)
                    sha256 = $metadataSha256
                }
            } else {
                $null
            }
        }
        device_disposition = "recorded by mechanical diagnostic; invoking harness must record candidate rollback separately when required"
    }
    $ledger.trials = @($ledger.trials) + [pscustomobject]$entry
    $ledger.updated_at = [DateTime]::UtcNow.ToString("o")
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $LedgerPath) | Out-Null
    $ledger | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $LedgerPath -Encoding utf8
}

$sessionLog = Join-Path $resolvedOutDir "generated-session.log"
$metadataPath = Join-Path $resolvedOutDir "mechanical-stimulus-metadata.json"
$summaryPath = Join-Path $resolvedOutDir "mechanical-machine-summary.json"
$providerFinalTailAllowanceMs = 250
$startedFirmwareSessionId = $null
$firmwareSessionId = $null
$firmwareControlTimeline = $null
$recordingStartCommand = if ($ControlMode -eq "UsbRecordingToggle") { "~VREC:TOGGLE" } else { "~KEY:EC11:SINGLE" }
$recordingStopCommand = if ($ControlMode -eq "UsbRecordingToggle") { "~VREC:STOP" } else { "~KEY:EC11:SINGLE" }
$recordingControlLabel = if ($ControlMode -eq "UsbRecordingToggle") { "USB recording toggle" } else { "Generated EC11" }
$scheduledRecording = $null
$trialLedgerWritten = $false

Add-Type -AssemblyName System.Speech
$synth = [System.Speech.Synthesis.SpeechSynthesizer]::new()
$speakProgress = [System.Collections.Generic.List[object]]::new()
$speakProgressHandler = [System.EventHandler[System.Speech.Synthesis.SpeakProgressEventArgs]]{
    param($sender, $eventArgs)
    [void]$speakProgress.Add([pscustomobject]@{
        character_position = $eventArgs.CharacterPosition
        audio_position_ms = [int]$eventArgs.AudioPosition.TotalMilliseconds
    })
}
$sourceWavePath = Join-Path $env:TEMP ("listener-mechanical-source-" + [guid]::NewGuid().ToString("N") + ".wav")
$speedAdjustedWavePath = $null
$fixedWavePath = $null
$player = $null
$hash = [Convert]::ToHexString(
    [Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($StimulusText))
).ToLowerInvariant()
try {
    Assert-NaturalSinglePassageFixture -Text $StimulusText
    $synth.SelectVoice($VoiceName)
    $synth.Rate = $Rate
    $synth.Volume = 100
    $synth.add_SpeakProgress($speakProgressHandler)

    # Rendering before the EC11 start keeps TTS initialization out of the recorded silence.
    $waveFormat = [System.Speech.AudioFormat.SpeechAudioFormatInfo]::new(
        16000,
        [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen,
        [System.Speech.AudioFormat.AudioChannel]::Mono
    )
    $synth.SetOutputToWaveFile($sourceWavePath, $waveFormat)
    $synth.Speak($StimulusText)
    $synth.SetOutputToNull()
    $sourceWave = Read-Pcm16Wave -Path $sourceWavePath
    $ttsSourceRenderedDurationMs = [int]($sourceWave.Pcm.Length / 32)
    if ($PlaybackSpeedMultiplier -ne 1.0) {
        $ffmpeg = Get-Command ffmpeg -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($null -eq $ffmpeg) {
            throw "PlaybackSpeedMultiplier requires ffmpeg on PATH"
        }
        $speedAdjustedWavePath = Join-Path $env:TEMP ("listener-mechanical-speed-" + [guid]::NewGuid().ToString("N") + ".wav")
        $tempo = $PlaybackSpeedMultiplier.ToString("0.0", [Globalization.CultureInfo]::InvariantCulture)
        & $ffmpeg.Source -hide_banner -loglevel error -nostdin -y -i $sourceWavePath -filter:a "atempo=$tempo" -ar 16000 -ac 1 -c:a pcm_s16le $speedAdjustedWavePath
        if ($LASTEXITCODE -ne 0) {
            throw "ffmpeg failed to render playback speed multiplier $tempo"
        }
        $sourceWave = Read-Pcm16Wave -Path $speedAdjustedWavePath
        foreach ($progress in $speakProgress) {
            $progress.audio_position_ms = [int][Math]::Round($progress.audio_position_ms / $PlaybackSpeedMultiplier)
        }
    }
    $sourceRenderedDurationMs = [int]($sourceWave.Pcm.Length / 32)
    $playbackWavePath = $sourceWavePath
    $stimulusDurationMs = $sourceRenderedDurationMs
    $expectedText = $StimulusText
    $expectedTextAudioBoundaryMs = $sourceRenderedDurationMs
    if ($TargetDurationSeconds -gt 0) {
        $targetDurationMs = $TargetDurationSeconds * 1000
        $targetBytes = $TargetDurationSeconds * 32000
        if ($sourceWave.Pcm.Length -lt $targetBytes) {
            throw "Rendered mechanical stimulus is shorter than the requested fixed duration"
        }
        $firstTruncatedWord = @($speakProgress | Where-Object {
            $_.audio_position_ms -gt $targetDurationMs
        } | Select-Object -First 1)
        $lastCompleteWordBoundary = @($speakProgress | Where-Object {
            $_.audio_position_ms -lt $targetDurationMs
        } | Select-Object -Last 1)
        if ($firstTruncatedWord.Count -ne 1 -or $lastCompleteWordBoundary.Count -ne 1 -or $lastCompleteWordBoundary[0].character_position -le 0) {
            throw "Rendered mechanical stimulus has no complete word boundary before the requested fixed duration"
        }
        $expectedText = $StimulusText.Substring(0, $lastCompleteWordBoundary[0].character_position)
        $expectedTextAudioBoundaryMs = $lastCompleteWordBoundary[0].audio_position_ms
        $fixedPcm = New-Object byte[] $targetBytes
        $completeWordBytes = [Math]::Min($targetBytes, $expectedTextAudioBoundaryMs * 32)
        [Array]::Copy($sourceWave.Pcm, 0, $fixedPcm, 0, $completeWordBytes)
        $fixedWavePath = Join-Path $env:TEMP ("listener-mechanical-fixed-" + [guid]::NewGuid().ToString("N") + ".wav")
        Write-Pcm16Wave -Path $fixedWavePath -Pcm $fixedPcm
        $playbackWavePath = $fixedWavePath
        $stimulusDurationMs = $TargetDurationSeconds * 1000
    }
    $player = [System.Media.SoundPlayer]::new($playbackWavePath)
    $player.Load()

    if ($LiveCaptureSeconds -gt 0) {
        throw "LiveCaptureSeconds is incompatible with the single-connection mechanical recording session"
    }

    $scheduledRecording = Start-ListenerScheduledRecording `
        -StartCommand $recordingStartCommand `
        -StopCommand $recordingStopCommand `
        -DurationMs $stimulusDurationMs `
        -PostStopCaptureSeconds $PostStopCaptureSeconds `
        -OutputPath $sessionLog

    $timer = [Diagnostics.Stopwatch]::StartNew()
    $player.Play()
    Start-Sleep -Milliseconds $stimulusDurationMs
    $timer.Stop()

    Complete-ListenerScheduledRecording -Recording $scheduledRecording
    $scheduledRecording = $null

    $sessionEvidenceLogs = @($sessionLog)
    $firmwareControlTimeline = Get-FirmwareControlTimeline -LogPaths $sessionEvidenceLogs
    $startedSessions = @($firmwareControlTimeline.started_sessions)
    if ($startedSessions.Count -eq 0) {
        throw "$recordingControlLabel start did not queue an audio session"
    }
    if ($startedSessions.Count -ne 1) {
        throw "$recordingControlLabel produced firmware session discontinuity: $($startedSessions.Count) starts"
    }
    $startedFirmwareSessionId = $startedSessions[0].session_id

    $transportMatch = @(
        $sessionEvidenceLogs |
            ForEach-Object { Select-String -LiteralPath $_ -Pattern "audio session transport summary:" } |
            Where-Object { $_.Line -match "(?:^|\s)session=$startedFirmwareSessionId(?:\s|$)" } |
            Select-Object
    )
    if ($transportMatch.Count -ne 1) {
        if (@($firmwareControlTimeline.transport_summaries).Count -gt 1) {
            throw "$recordingControlLabel produced firmware session discontinuity: transport summaries span $(@($firmwareControlTimeline.transport_summaries).Count) sessions"
        }
        throw "$recordingControlLabel stop did not produce a firmware transport summary"
    }

    $transportLine = $transportMatch[0].Line
    $firmwareSessionId = Get-IntegerMetric -Line $transportLine -Name "session"
    if ($null -eq $firmwareSessionId) {
        throw "$recordingControlLabel stop did not expose a firmware session id"
    }
    $singleFirmwareSession =
        $startedFirmwareSessionId -eq $firmwareSessionId -and
        @($firmwareControlTimeline.started_sessions).Count -eq 1 -and
        @($firmwareControlTimeline.transport_summaries).Count -eq 1
    # The installed Type logger flushes provider and capsule events asynchronously.
    # Read after a bounded settle window so a trailing log batch cannot turn a
    # continuous visible-preview timeline into a false gap.
    Start-Sleep -Seconds 3
    $typeSummary = Get-MechanicalTypeSummary -EmbeddedSessionId ([int]$firmwareSessionId) -ExpectedText $expectedText
    $expectedPackets = Get-IntegerMetric -Line $transportLine -Name "expected_packet_count"
    $sentPackets = Get-IntegerMetric -Line $transportLine -Name "audio_sent"
    $notifyFailed = Get-IntegerMetric -Line $transportLine -Name "notify_failed"
    $queueFull = Get-IntegerMetric -Line $transportLine -Name "queue_full"
    $poolAllocFailed = Get-IntegerMetric -Line $transportLine -Name "pool_alloc_failed"
    $audioPcmBytes = Get-IntegerMetric -Line $transportLine -Name "audio_pcm_bytes"
    $audioFailed = Get-IntegerMetric -Line $transportLine -Name "audio_failed"
    $poolHighWaterPct = Get-IntegerMetric -Line $transportLine -Name "pool_high_water_pct"
    $retryMbuf = Get-IntegerMetric -Line $transportLine -Name "retry_mbuf"
    $retryEnomem = Get-IntegerMetric -Line $transportLine -Name "retry_enomem"
    $captureIntegrityMatch = @(
        $sessionEvidenceLogs |
            ForEach-Object { Select-String -LiteralPath $_ -Pattern "record session capture integrity:" } |
            Where-Object { $_.Line -match "(?:^|\s)session_id=$startedFirmwareSessionId(?:\s|$)" } |
            Select-Object -Last 1
    )
    $captureIntegrityLine = if ($captureIntegrityMatch.Count -eq 1) {
        $captureIntegrityMatch[0].Line
    } else {
        $null
    }
    if ([string]::IsNullOrWhiteSpace($captureIntegrityLine)) {
        throw "$recordingControlLabel stop did not expose capture PCM duration"
    }
    $pcmMs = Get-IntegerMetric -Line $captureIntegrityLine -Name "pcm_ms"
    $fixedDurationCaptured = $TargetDurationSeconds -eq 0 -or
        ($null -ne $pcmMs -and $pcmMs -ge ($TargetDurationSeconds * 1000))
    $steadyConsumptionBps = $null
    if ($null -ne $audioPcmBytes -and $null -ne $pcmMs -and $pcmMs -gt 0) {
        $steadyConsumptionBps = [math]::Round(($audioPcmBytes * 1000.0) / $pcmMs, 3)
    }
    $mbufRetryPct = $null
    $enomemRetryPct = $null
    if ($null -ne $retryMbuf -and $null -ne $sentPackets -and $sentPackets -gt 0) {
        $mbufRetryPct = [math]::Round(($retryMbuf * 100.0) / $sentPackets, 4)
    }
    if ($null -ne $retryEnomem -and $null -ne $sentPackets -and $sentPackets -gt 0) {
        $enomemRetryPct = [math]::Round(($retryEnomem * 100.0) / $sentPackets, 4)
    }
    $afeOutputLevel = Get-AfeOutputLevelSummary -LogPaths $sessionEvidenceLogs
    $fixtureAcousticLevelSufficient =
        $null -ne $afeOutputLevel.acoustic_gate_peak -and
        $null -ne $typeSummary.streaming_agc.voiced_chunks -and
        $null -ne $typeSummary.streaming_agc.clipped_samples -and
        $afeOutputLevel.acoustic_gate_peak -ge $minAfeOutputPeak -and
        $afeOutputLevel.acoustic_gate_peak -lt $maxAfeOutputPeakExclusive -and
        $typeSummary.streaming_agc.voiced_chunks -ge $minStreamingAgcVoicedChunks -and
        $typeSummary.streaming_agc.clipped_samples -eq 0
    $providerFinalTailGapMs = $null
    $providerFinalTailCovered = $false
    if ($null -ne $typeSummary.provider_final_audio_duration_ms -and $null -ne $pcmMs) {
        $providerFinalTailGapMs = [math]::Max(0, $pcmMs - $typeSummary.provider_final_audio_duration_ms)
        $providerFinalTailCovered = $typeSummary.provider_final_audio_duration_ms -ge ($pcmMs - $providerFinalTailAllowanceMs)
    }
    $evidenceGaps = @($typeSummary.evidence_gaps)
    if (-not $singleFirmwareSession) {
        $evidenceGaps += "firmware_session_discontinuity"
    }
    if ($null -eq $afeOutputLevel.acoustic_gate_peak) {
        $evidenceGaps += "afe_output_level"
    } elseif ($null -eq $typeSummary.streaming_agc.voiced_chunks -or $null -eq $typeSummary.streaming_agc.clipped_samples) {
        $evidenceGaps += "streaming_agc_summary"
    } elseif (-not $fixtureAcousticLevelSufficient) {
        $evidenceGaps += "fixture_acoustic_signal_below_minimum"
    }
    $machinePass = $null -ne $typeSummary.first_preview_latency_ms -and
        $null -ne $typeSummary.visible_preview_max_gap_ms -and
        $null -ne $typeSummary.final_transcription_ms -and
        $null -ne $typeSummary.missing_packets -and
        $null -ne $typeSummary.final_chars -and
        $null -ne $typeSummary.pending_send_high_water_frames -and
        $null -ne $expectedPackets -and
        $null -ne $sentPackets -and
        $null -ne $notifyFailed -and
        $null -ne $queueFull -and
        $null -ne $poolAllocFailed -and
        $null -ne $steadyConsumptionBps -and
        $null -ne $audioFailed -and
        $null -ne $poolHighWaterPct -and
        $null -ne $mbufRetryPct -and
        $null -ne $enomemRetryPct -and
        $null -ne $providerFinalTailGapMs -and
        $evidenceGaps.Count -eq 0 -and
        $singleFirmwareSession -and
        $fixedDurationCaptured -and
        $fixtureAcousticLevelSufficient -and
        (-not $enforceInteractivePreviewCadence -or $typeSummary.first_preview_latency_ms -le 5000) -and
        (-not $enforceInteractivePreviewCadence -or $typeSummary.visible_preview_max_gap_ms -le $maxVisiblePreviewGapMs) -and
        $typeSummary.missing_packets -eq 0 -and
        $typeSummary.final_chars -gt 0 -and
        $providerFinalTailCovered -and
        $typeSummary.final_coverage.whole_text_coverage_ratio -ge $minWholeTextCoverageRatio -and
        $typeSummary.final_coverage.tail_coverage_ratio -eq 1 -and
        $expectedPackets -eq $sentPackets -and
        $notifyFailed -eq 0 -and
        $queueFull -eq 0 -and
        $poolAllocFailed -eq 0 -and
        $steadyConsumptionBps -ge 32000 -and
        $audioFailed -eq 0 -and
        $poolHighWaterPct -le 20 -and
        $mbufRetryPct -le 1 -and
        $enomemRetryPct -le 1

    [ordered]@{
        status = if ($machinePass) { "PASS" } else { "NO_GO" }
        mode = "mechanical_voice_physical_microphone_diagnostic"
        control_mode = $ControlMode
        physical_ec11_or_human_acceptance = $false
        evidence_gaps = $evidenceGaps
        firmware = [ordered]@{
            started_session_id = $startedFirmwareSessionId
            session_id = $firmwareSessionId
            single_session_continuity = $singleFirmwareSession
            expected_packets = $expectedPackets
            sent_packets = $sentPackets
            notify_failed = $notifyFailed
            queue_full = $queueFull
            pool_alloc_failed = $poolAllocFailed
            audio_pcm_bytes = $audioPcmBytes
            pcm_ms = $pcmMs
            steady_consumption_bytes_per_s = $steadyConsumptionBps
            audio_failed = $audioFailed
            pool_high_water_pct = $poolHighWaterPct
            retry_mbuf = $retryMbuf
            retry_enomem = $retryEnomem
            retry_mbuf_pct = $mbufRetryPct
            retry_enomem_pct = $enomemRetryPct
            fixed_duration_captured = $fixedDurationCaptured
            afe_output_level = $afeOutputLevel
            fixture_acoustic_level_sufficient = $fixtureAcousticLevelSufficient
            control_timeline = $firmwareControlTimeline
        }
        tail_coverage = [ordered]@{
            provider_final_tail_allowance_ms = $providerFinalTailAllowanceMs
            expected_text_audio_boundary_ms = $expectedTextAudioBoundaryMs
            provider_final_audio_duration_ms = $typeSummary.provider_final_audio_duration_ms
            terminal_pcm_ms = $pcmMs
            provider_final_tail_gap_ms = $providerFinalTailGapMs
            provider_final_tail_covered = $providerFinalTailCovered
            whole_text_coverage_ratio = $typeSummary.final_coverage.whole_text_coverage_ratio
            tail_window_chars = $typeSummary.final_coverage.tail_window_chars
            tail_match_chars = $typeSummary.final_coverage.tail_match_chars
            tail_coverage_ratio = $typeSummary.final_coverage.tail_coverage_ratio
        }
        acceptance_thresholds = [ordered]@{
            preview_cadence_required = $enforceInteractivePreviewCadence
            preview_cadence_scope = "normal_rate_only"
            preview_cadence_phase = "recording_only"
            first_visible_preview_ms = 5000
            max_visible_preview_gap_ms = $maxVisiblePreviewGapMs
            whole_text_coverage_ratio = $minWholeTextCoverageRatio
            tail_coverage_ratio = 1
            min_afe_output_peak = $minAfeOutputPeak
            max_afe_output_peak_exclusive = $maxAfeOutputPeakExclusive
            min_streaming_agc_voiced_chunks = $minStreamingAgcVoicedChunks
            streaming_agc_clipped_samples = 0
        }
        type = $typeSummary
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $summaryPath -Encoding utf8

    [ordered]@{
        mode = "mechanical_voice_physical_microphone_diagnostic"
        stimulus_sha256 = $hash
        stimulus_duration_ms = $stimulusDurationMs
        requested_duration_ms = $TargetDurationSeconds * 1000
        source_rendered_duration_ms = $sourceRenderedDurationMs
        tts_source_rendered_duration_ms = $ttsSourceRenderedDurationMs
        expected_text_audio_boundary_ms = $expectedTextAudioBoundaryMs
        playback_elapsed_ms = [int]$timer.ElapsedMilliseconds
        voice = $VoiceName
        rate = $Rate
        playback_speed_multiplier = $PlaybackSpeedMultiplier
        control_mode = $ControlMode
        live_capture_seconds = $LiveCaptureSeconds
        scheduled_recording_log = [IO.Path]::GetFileName($sessionLog)
        synthetic_ec11 = $ControlMode -eq "SyntheticEc11"
        physical_ec11_or_human_acceptance = $false
    } | ConvertTo-Json | Set-Content -LiteralPath $metadataPath -Encoding utf8

    Write-Output "mechanical_stimulus_duration_ms=$stimulusDurationMs"
    Write-Output "mechanical_playback_elapsed_ms=$($timer.ElapsedMilliseconds)"
    Write-Output "stimulus_sha256=$hash"
} catch {
    $failureCategory = Get-MechanicalDiagnosticFailureCategory -Message $_.Exception.Message
    $evidenceGaps = [Collections.Generic.List[string]]::new()
    [void]$evidenceGaps.Add("machine_run_incomplete")
    [void]$evidenceGaps.Add($failureCategory)
    if ($null -ne $startedFirmwareSessionId -and $null -ne $firmwareSessionId -and $startedFirmwareSessionId -ne $firmwareSessionId) {
        $failureCategory = "firmware_session_discontinuity"
        $evidenceGaps.Clear()
        [void]$evidenceGaps.Add("machine_run_incomplete")
        [void]$evidenceGaps.Add($failureCategory)
    } elseif ($null -ne $startedFirmwareSessionId -and $null -eq $firmwareSessionId) {
        [void]$evidenceGaps.Add("firmware_session_continuity_unproven")
    }
    [ordered]@{
        status = "NO_GO"
        mode = "mechanical_voice_physical_microphone_diagnostic"
        control_mode = $ControlMode
        physical_ec11_or_human_acceptance = $false
        failure_category = $failureCategory
        evidence_gaps = @($evidenceGaps)
        firmware = [ordered]@{
            started_session_id = $startedFirmwareSessionId
            session_id = $firmwareSessionId
            single_session_continuity = $false
            control_timeline = $firmwareControlTimeline
        }
        artifacts = [ordered]@{
            scheduled_recording_log = if (Test-Path -LiteralPath $sessionLog) { [IO.Path]::GetFileName($sessionLog) } else { $null }
            stimulus_metadata = if (Test-Path -LiteralPath $metadataPath) { [IO.Path]::GetFileName($metadataPath) } else { $null }
        }
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $summaryPath -Encoding utf8
    throw
} finally {
    if (-not $trialLedgerWritten) {
        Write-MechanicalTrialLedger `
            -LedgerPath $resolvedTrialLedgerPath `
            -SummaryPath $summaryPath `
            -MetadataPath $metadataPath
        $trialLedgerWritten = $true
        Write-Output "mechanical_trial_ledger=$resolvedTrialLedgerPath"
    }
    if ($null -ne $scheduledRecording) {
        Stop-Job -Job $scheduledRecording.Job -ErrorAction SilentlyContinue
        Remove-Job -Job $scheduledRecording.Job -Force -ErrorAction SilentlyContinue
    }
    if ($null -ne $speakProgressHandler) {
        $synth.remove_SpeakProgress($speakProgressHandler)
    }
    if ($null -ne $player) {
        $player.Dispose()
    }
    $synth.Dispose()
    if (Test-Path -LiteralPath $sourceWavePath) {
        [IO.File]::Delete($sourceWavePath)
    }
    if ($null -ne $speedAdjustedWavePath -and (Test-Path -LiteralPath $speedAdjustedWavePath)) {
        [IO.File]::Delete($speedAdjustedWavePath)
    }
    if ($null -ne $fixedWavePath -and (Test-Path -LiteralPath $fixedWavePath)) {
        [IO.File]::Delete($fixedWavePath)
    }
}
