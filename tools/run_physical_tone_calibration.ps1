[CmdletBinding(PositionalBinding = $false)]
param(
    [string]$Port = "COM3",
    [ValidateRange(1, 30)]
    [int]$DurationSeconds = 10,
    [ValidateRange(100, 4000)]
    [int]$ToneHz = 1000,
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

$typeRoot = Split-Path -Parent $PSScriptRoot
$validationRoot = [IO.Path]::GetFullPath((Join-Path $typeRoot ".cache\validation"))
$resolvedOutDir = [IO.Path]::GetFullPath($OutDir)
$resolvedLedgerPath = [IO.Path]::GetFullPath($TrialLedgerPath)
if (-not $resolvedOutDir.StartsWith($validationRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "OutDir must be under $validationRoot"
}
if (-not $resolvedLedgerPath.StartsWith($validationRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "TrialLedgerPath must be under $validationRoot"
}

$firmwareSerialTool = Join-Path (Split-Path -Parent $typeRoot) "Listener-Firmware\tools\send_serial_and_capture.ps1"
if (-not (Test-Path -LiteralPath $firmwareSerialTool)) {
    throw "Missing Firmware serial capture tool: $firmwareSerialTool"
}

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
        try {
            $mutex.ReleaseMutex()
        } catch {
        }
        $mutex.Dispose()
    }
}

function Get-IntegerMetric {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Line,
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    $match = [regex]::Match($Line, "(?:^|\s)$Name=(\d+)")
    if ($match.Success) {
        return [int64]$match.Groups[1].Value
    }
    return $null
}

function Write-Pcm16ToneWave {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [int]$Seconds,
        [Parameter(Mandatory = $true)]
        [int]$FrequencyHz
    )

    $sampleRate = 16000
    $sampleCount = $sampleRate * $Seconds
    $pcm = New-Object byte[] ($sampleCount * 2)
    for ($sampleIndex = 0; $sampleIndex -lt $sampleCount; $sampleIndex++) {
        $sample = [int16][Math]::Round(
            28000 * [Math]::Sin(2 * [Math]::PI * $FrequencyHz * $sampleIndex / $sampleRate)
        )
        $sampleBytes = [BitConverter]::GetBytes($sample)
        $pcm[$sampleIndex * 2] = $sampleBytes[0]
        $pcm[($sampleIndex * 2) + 1] = $sampleBytes[1]
    }

    $header = New-Object byte[] 44
    [Text.Encoding]::ASCII.GetBytes("RIFF").CopyTo($header, 0)
    [BitConverter]::GetBytes(36 + $pcm.Length).CopyTo($header, 4)
    [Text.Encoding]::ASCII.GetBytes("WAVEfmt ").CopyTo($header, 8)
    [BitConverter]::GetBytes(16).CopyTo($header, 16)
    [BitConverter]::GetBytes([int16]1).CopyTo($header, 20)
    [BitConverter]::GetBytes([int16]1).CopyTo($header, 22)
    [BitConverter]::GetBytes($sampleRate).CopyTo($header, 24)
    [BitConverter]::GetBytes($sampleRate * 2).CopyTo($header, 28)
    [BitConverter]::GetBytes([int16]2).CopyTo($header, 32)
    [BitConverter]::GetBytes([int16]16).CopyTo($header, 34)
    [Text.Encoding]::ASCII.GetBytes("data").CopyTo($header, 36)
    [BitConverter]::GetBytes($pcm.Length).CopyTo($header, 40)

    $stream = [IO.File]::Open($Path, [IO.FileMode]::Create, [IO.FileAccess]::Write)
    try {
        $stream.Write($header, 0, $header.Length)
        $stream.Write($pcm, 0, $pcm.Length)
    } finally {
        $stream.Dispose()
    }
}

function Write-ToneCalibrationLedger {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SummaryPath
    )

    $summary = Get-Content -LiteralPath $SummaryPath -Raw | ConvertFrom-Json
    $ledger = if (Test-Path -LiteralPath $resolvedLedgerPath) {
        Get-Content -LiteralPath $resolvedLedgerPath -Raw | ConvertFrom-Json
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

    $entry = [ordered]@{
        trial_id = $TrialId
        hypothesis = $Hypothesis
        candidate_identity = $CandidateIdentity
        machine_status = $summary.status
        mode = $summary.mode
        stimulus_sha256 = $null
        stimulus_duration_ms = $DurationSeconds * 1000
        firmware = $summary.firmware
        type = $null
        coverage = $null
        evidence_gaps = @($summary.evidence_gaps)
        artifacts = [ordered]@{
            calibration_summary = [ordered]@{
                filename = [IO.Path]::GetFileName($SummaryPath)
                sha256 = (Get-FileHash -LiteralPath $SummaryPath -Algorithm SHA256).Hash
            }
        }
        device_disposition = "No firmware or installed Type replacement; temporary non-speech calibration waveform deleted."
    }
    $ledger.trials = @($ledger.trials) + [pscustomobject]$entry
    $ledger.updated_at = [DateTime]::UtcNow.ToString("o")
    $ledger | ConvertTo-Json -Depth 9 | Set-Content -LiteralPath $resolvedLedgerPath -Encoding utf8
}

New-Item -ItemType Directory -Force -Path $resolvedOutDir | Out-Null
$startLogPath = Join-Path $resolvedOutDir "serial-start.log"
$stopLogPath = Join-Path $resolvedOutDir "serial-stop.log"
$summaryPath = Join-Path $resolvedOutDir "calibration-summary.json"
$tonePath = Join-Path $env:TEMP ("listener-tone-" + [Guid]::NewGuid().ToString("N") + ".wav")
$started = $false
$ledgerWritten = $false

try {
    Add-Type -AssemblyName System.Windows.Extensions
    Write-Pcm16ToneWave -Path $tonePath -Seconds $DurationSeconds -FrequencyHz $ToneHz

    Invoke-ListenerSerialCommand -Command "~VREC:TOGGLE" -OutputPath $startLogPath
    $started = $true
    $player = [System.Media.SoundPlayer]::new($tonePath)
    $player.Load()
    $player.PlaySync()
    Invoke-ListenerSerialCommand -Command "~VREC:STOP" -OutputPath $stopLogPath -CaptureSeconds 5
    $started = $false

    $signalLine = Select-String -LiteralPath $stopLogPath -Pattern "PDM AFE session signal:" |
        Select-Object -Last 1
    $transportLine = Select-String -LiteralPath $stopLogPath -Pattern "audio session transport summary:" |
        Select-Object -Last 1
    if ($null -eq $signalLine -or $null -eq $transportLine) {
        throw "Missing calibrated tone session telemetry"
    }

    $outputPeak = Get-IntegerMetric -Line $signalLine.Line -Name "output_peak"
    $outputMeanAbs = Get-IntegerMetric -Line $signalLine.Line -Name "output_mean_abs"
    $fixturePass = $null -ne $outputPeak -and $null -ne $outputMeanAbs -and
        $outputPeak -ge 500 -and $outputMeanAbs -ge 100
    $serialClosed = ((Get-Content -LiteralPath $startLogPath -Raw) +
        (Get-Content -LiteralPath $stopLogPath -Raw)) -match "serial_closed"
    $summary = [ordered]@{
        schema_version = 1
        mode = "physical_sine_tone_fixture_calibration_not_recording_acceptance"
        privacy = "numeric values only; temporary non-speech waveform removed"
        status = if ($fixturePass) { "PASS_FIXTURE_CALIBRATION" } else { "NO_GO" }
        evidence_gaps = @(
            if ($fixturePass) { @() } else { "fixture_acoustic_level_below_minimum" }
        )
        firmware = [ordered]@{
            session_id = Get-IntegerMetric -Line $signalLine.Line -Name "session_id"
            input_peak = Get-IntegerMetric -Line $signalLine.Line -Name "input_peak"
            input_mean_abs = Get-IntegerMetric -Line $signalLine.Line -Name "input_mean_abs"
            output_peak = $outputPeak
            output_mean_abs = $outputMeanAbs
            raw_selected_peak = Get-IntegerMetric -Line $signalLine.Line -Name "raw_selected_peak"
            raw_selected_mean_abs = Get-IntegerMetric -Line $signalLine.Line -Name "raw_selected_mean_abs"
            raw_alternate_peak = Get-IntegerMetric -Line $signalLine.Line -Name "raw_alternate_peak"
            raw_alternate_mean_abs = Get-IntegerMetric -Line $signalLine.Line -Name "raw_alternate_mean_abs"
            raw_slot_equal_permille = Get-IntegerMetric -Line $signalLine.Line -Name "raw_slot_equal_permille"
        }
        fixture_gate = [ordered]@{
            required_output_peak = 500
            required_output_mean_abs = 100
            pass = $fixturePass
        }
        transport = [ordered]@{
            expected_packets = Get-IntegerMetric -Line $transportLine.Line -Name "expected_packet_count"
            audio_sent = Get-IntegerMetric -Line $transportLine.Line -Name "audio_sent"
            notify_failed = Get-IntegerMetric -Line $transportLine.Line -Name "notify_failed"
            queue_full = Get-IntegerMetric -Line $transportLine.Line -Name "queue_full"
        }
        serial_closed = $serialClosed
    }
    $summary | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $summaryPath -Encoding utf8
    Write-ToneCalibrationLedger -SummaryPath $summaryPath
    $ledgerWritten = $true
    Write-Output "tone_calibration_status=$($summary.status)"
} finally {
    if ($started) {
        try {
            Invoke-ListenerSerialCommand -Command "~VREC:STOP" -OutputPath (Join-Path $resolvedOutDir "serial-recovery-stop.log") -CaptureSeconds 2
        } catch {
        }
    }
    if (-not $ledgerWritten -and (Test-Path -LiteralPath $summaryPath)) {
        Write-ToneCalibrationLedger -SummaryPath $summaryPath
    }
    Remove-Item -LiteralPath $tonePath -Force -ErrorAction SilentlyContinue
}
