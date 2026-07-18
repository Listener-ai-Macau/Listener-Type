[CmdletBinding(PositionalBinding = $false)]
param(
    [Parameter(Mandatory = $true)]
    [string]$StimulusText,
    [string]$Port = "COM3",
    [string]$VoiceName = "Microsoft Huihui Desktop",
    [ValidateRange(-10, 10)]
    [int]$Rate = 9,
    [ValidateRange(0, 120)]
    [int]$TargetDurationSeconds = 0,
    [ValidateRange(3, 20)]
    [int]$PostStopCaptureSeconds = 8,
    [Parameter(Mandatory = $true)]
    [string]$OutDir
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$typeRoot = Split-Path -Parent $PSScriptRoot
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
        [int]$EmbeddedSessionId
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
    if ($endIndex -lt 0) {
        throw "Installed Type did not complete embedded session $EmbeddedSessionId"
    }

    $sessionLines = $lines.GetRange($startIndex, $endIndex - $startIndex + 1)
    $sessionStartedAt = [DateTimeOffset]::Parse(($sessionLines[0] -split " ", 2)[0])
    $firstPcmLine = $sessionLines | Where-Object {
        $_.Contains("event=pcm embedded_session_id=$EmbeddedSessionId packet_sequence=0")
    } | Select-Object -First 1
    $firstPreviewLine = $sessionLines | Where-Object {
        $_.Contains('"event":"embedded_audio_preview_first')
    } | Select-Object -First 1
    $finalLine = $sessionLines | Where-Object {
        $_.Contains('"event":"embedded_audio_final"')
    } | Select-Object -First 1
    $completionLine = $sessionLines | Where-Object {
        $_.Contains("background session completed while keeping notify open")
    } | Select-Object -First 1
    $finalActionLine = $sessionLines | Where-Object {
        $_.Contains("final completion actions session_id=")
    } | Select-Object -First 1

    if ($null -eq $firstPcmLine -or $null -eq $firstPreviewLine -or $null -eq $finalLine -or $null -eq $completionLine -or $null -eq $finalActionLine) {
        throw "Installed Type session $EmbeddedSessionId is missing required PCM, preview, final, completion, or final-action evidence"
    }

    $firstPcmAt = [DateTimeOffset]::Parse(($firstPcmLine -split " ", 2)[0])
    $firstPreviewAt = [DateTimeOffset]::Parse(($firstPreviewLine -split " ", 2)[0])
    $firstPreviewLatency = Get-IntegerMetric -Line $firstPreviewLine -Name "timing_value_ms"
    $finalTranscription = Get-IntegerMetric -Line $finalLine -Name "timing_value_ms"
    $missingPackets = Get-IntegerMetric -Line $completionLine -Name "missing_packets"
    $finalChars = Get-IntegerMetric -Line $finalActionLine -Name "chars"

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
        }
    }
    if (@($providerProgress).Count -eq 0) {
        throw "Installed Type session $EmbeddedSessionId has no provider audio-progress evidence"
    }

    [ordered]@{
        embedded_session_id = $EmbeddedSessionId
        first_pcm_after_session_start_ms = [math]::Round(($firstPcmAt - $sessionStartedAt).TotalMilliseconds)
        first_preview_latency_ms = $firstPreviewLatency
        first_pcm_to_visible_preview_ms = [math]::Round(($firstPreviewAt - $firstPcmAt).TotalMilliseconds)
        visible_preview_updates = @($sessionLines | Where-Object { $_.Contains(" event=asr_partial ") }).Count
        provider_progress_samples = @($providerProgress).Count
        provider_audio_max_lag_ms = (@($providerProgress | Measure-Object -Property provider_audio_lag_ms -Maximum).Maximum)
        provider_audio_final_lag_ms = (@($providerProgress | Select-Object -Last 1).provider_audio_lag_ms)
        final_transcription_ms = $finalTranscription
        final_chars = $finalChars
        missing_packets = $missingPackets
    }
}

$startLog = Join-Path $resolvedOutDir "generated-start.log"
$stopLog = Join-Path $resolvedOutDir "generated-stop.log"
$metadataPath = Join-Path $resolvedOutDir "mechanical-stimulus-metadata.json"
$summaryPath = Join-Path $resolvedOutDir "mechanical-machine-summary.json"

Add-Type -AssemblyName System.Speech
$synth = [System.Speech.Synthesis.SpeechSynthesizer]::new()
$sourceWavePath = Join-Path $env:TEMP ("listener-mechanical-source-" + [guid]::NewGuid().ToString("N") + ".wav")
$fixedWavePath = $null
$player = $null
$hash = [Convert]::ToHexString(
    [Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($StimulusText))
).ToLowerInvariant()
try {
    $synth.SelectVoice($VoiceName)
    $synth.Rate = $Rate
    $synth.Volume = 100

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
    $sourceRenderedDurationMs = [int]($sourceWave.Pcm.Length / 32)
    $playbackWavePath = $sourceWavePath
    $stimulusDurationMs = $sourceRenderedDurationMs
    if ($TargetDurationSeconds -gt 0) {
        $targetBytes = $TargetDurationSeconds * 32000
        if ($sourceWave.Pcm.Length -lt $targetBytes) {
            throw "Rendered mechanical stimulus is shorter than the requested fixed duration"
        }
        $fixedPcm = New-Object byte[] $targetBytes
        [Array]::Copy($sourceWave.Pcm, 0, $fixedPcm, 0, $targetBytes)
        $fixedWavePath = Join-Path $env:TEMP ("listener-mechanical-fixed-" + [guid]::NewGuid().ToString("N") + ".wav")
        Write-Pcm16Wave -Path $fixedWavePath -Pcm $fixedPcm
        $playbackWavePath = $fixedWavePath
        $stimulusDurationMs = $TargetDurationSeconds * 1000
    }
    $player = [System.Media.SoundPlayer]::new($playbackWavePath)
    $player.Load()

    Invoke-ListenerSerialCommand -Command "~KEY:EC11:SINGLE" -OutputPath $startLog
    if (-not (Select-String -LiteralPath $startLog -Pattern "stream session start queued" -Quiet)) {
        throw "Generated EC11 start did not queue an audio session"
    }

    $timer = [Diagnostics.Stopwatch]::StartNew()
    $player.PlaySync()
    $timer.Stop()

    Invoke-ListenerSerialCommand -Command "~KEY:EC11:SINGLE" -OutputPath $stopLog -CaptureSeconds $PostStopCaptureSeconds

    if (-not (Select-String -LiteralPath $stopLog -Pattern "audio session transport summary:" -Quiet)) {
        throw "Generated EC11 stop did not produce a firmware transport summary"
    }

    $transportLine = (Select-String -LiteralPath $stopLog -Pattern "audio session transport summary:" | Select-Object -Last 1).Line
    $firmwareSessionId = Get-IntegerMetric -Line $transportLine -Name "session"
    if ($null -eq $firmwareSessionId) {
        throw "Generated EC11 stop did not expose a firmware session id"
    }
    Start-Sleep -Milliseconds 250
    $typeSummary = Get-MechanicalTypeSummary -EmbeddedSessionId ([int]$firmwareSessionId)
    $expectedPackets = Get-IntegerMetric -Line $transportLine -Name "expected_packet_count"
    $sentPackets = Get-IntegerMetric -Line $transportLine -Name "audio_sent"
    $notifyFailed = Get-IntegerMetric -Line $transportLine -Name "notify_failed"
    $queueFull = Get-IntegerMetric -Line $transportLine -Name "queue_full"
    $poolAllocFailed = Get-IntegerMetric -Line $transportLine -Name "pool_alloc_failed"
    $machinePass = $null -ne $typeSummary.first_preview_latency_ms -and
        $null -ne $typeSummary.final_transcription_ms -and
        $null -ne $typeSummary.missing_packets -and
        $null -ne $typeSummary.final_chars -and
        $null -ne $expectedPackets -and
        $null -ne $sentPackets -and
        $null -ne $notifyFailed -and
        $null -ne $queueFull -and
        $null -ne $poolAllocFailed -and
        $typeSummary.first_preview_latency_ms -le 5000 -and
        $typeSummary.missing_packets -eq 0 -and
        $typeSummary.final_chars -gt 0 -and
        $expectedPackets -eq $sentPackets -and
        $notifyFailed -eq 0 -and
        $queueFull -eq 0 -and
        $poolAllocFailed -eq 0

    [ordered]@{
        status = if ($machinePass) { "PASS" } else { "NO_GO" }
        mode = "mechanical_voice_physical_microphone_diagnostic"
        physical_ec11_or_human_acceptance = $false
        firmware = [ordered]@{
            session_id = $firmwareSessionId
            expected_packets = $expectedPackets
            sent_packets = $sentPackets
            notify_failed = $notifyFailed
            queue_full = $queueFull
            pool_alloc_failed = $poolAllocFailed
        }
        type = $typeSummary
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $summaryPath -Encoding utf8

    [ordered]@{
        mode = "mechanical_voice_physical_microphone_diagnostic"
        stimulus_sha256 = $hash
        stimulus_duration_ms = $stimulusDurationMs
        requested_duration_ms = $TargetDurationSeconds * 1000
        source_rendered_duration_ms = $sourceRenderedDurationMs
        playback_elapsed_ms = [int]$timer.ElapsedMilliseconds
        voice = $VoiceName
        rate = $Rate
        synthetic_ec11 = $true
        physical_ec11_or_human_acceptance = $false
    } | ConvertTo-Json | Set-Content -LiteralPath $metadataPath -Encoding utf8

    Write-Output "mechanical_stimulus_duration_ms=$stimulusDurationMs"
    Write-Output "mechanical_playback_elapsed_ms=$($timer.ElapsedMilliseconds)"
    Write-Output "stimulus_sha256=$hash"
} finally {
    if ($null -ne $player) {
        $player.Dispose()
    }
    $synth.Dispose()
    if (Test-Path -LiteralPath $sourceWavePath) {
        [IO.File]::Delete($sourceWavePath)
    }
    if ($null -ne $fixedWavePath -and (Test-Path -LiteralPath $fixedWavePath)) {
        [IO.File]::Delete($fixedWavePath)
    }
}
