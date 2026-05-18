param(
    [ValidateSet("serial-toggle", "manual-key")]
    [string]$TriggerMode = "serial-toggle",
    [string]$Port = "COM3",
    [string]$DeviceName = "listener",
    [string]$BluetoothAddress = "DCB4D91112CE",
    [int]$TimeoutMs = 45000,
    [int]$NotifyReadyTimeoutSeconds = 20,
    [double]$NotifySettleSeconds = 2.0,
    [int]$NoNotificationTimeoutSeconds = 8,
    [int]$PlaybackCount = 1,
    [int]$RecordPlaybackIndex = 1,
    [int]$PreRecordDelayMs = 300,
    [int]$ManualTriggerReadyDelayMs = 700,
    [int]$PostPlaybackRecordMs = 500,
    [string]$Sentence,
    [string]$WavPath,
    [string]$VoiceName = "Microsoft Huihui Desktop",
    [double]$TtsGain = 2.5,
    [string]$ListenerExe,
    [string]$FirmwareRepo,
    [string]$OutDir = "artifacts\embedded_stream_smoke",
    [int]$MaxMissingPackets = 0,
    [switch]$FailOnMissingPackets,
    [switch]$NoResetBeforeCapture,
    [switch]$SkipEnsureBle,
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

function Get-LatestAsrTranscriptFromLog {
    param([string]$Text)

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
            }
        } catch {
        }
    }
    return $latest
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
    $process = Start-Process -FilePath "notepad.exe" `
        -ArgumentList (Quote-ProcessArgument $Path) `
        -PassThru
    [void](Focus-ProcessWindow -Process $process)
    return [pscustomobject]@{
        Process = $process
        Path = $Path
    }
}

function Read-InsertionTargetText {
    param($Target)

    if (-not $Target) {
        return $null
    }
    Add-Type -AssemblyName System.Windows.Forms
    [void](Focus-ProcessWindow -Process $Target.Process)
    [System.Windows.Forms.SendKeys]::SendWait("^s")
    Start-Sleep -Milliseconds 700
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
}

function Get-HistoryPath {
    if ($env:APPDATA) {
        return Join-Path $env:APPDATA "Listener Type\history.json"
    }
    return $null
}

function Find-SmokeHistorySession {
    param(
        [datetime]$StartedAt,
        [string]$Transcript,
        [int]$ExpectedPcmBytes = 0
    )

    $historyPath = Get-HistoryPath
    if (-not $historyPath -or -not (Test-Path $historyPath)) {
        return $null
    }
    $raw = Get-Content -Path $historyPath -Raw -ErrorAction SilentlyContinue
    if ([string]::IsNullOrWhiteSpace($raw)) {
        return $null
    }
    $sessions = @($raw | ConvertFrom-Json)
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
        [int]$DurationMs,
        [string]$SerialLogPath,
        [string]$StartSignalPath,
        [int]$MaxWaitSeconds = 120
    )

    $python = @'
import json
import pathlib
import serial
import sys
import time

port = sys.argv[1]
duration_ms = int(sys.argv[2])
log_path = pathlib.Path(sys.argv[3])
start_signal_path = pathlib.Path(sys.argv[4])
max_wait_seconds = int(sys.argv[5])
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
    poll_until(ser, time.monotonic() + (duration_ms / 1000.0))
    send_command(ser, "~VREC:TOGGLE")
    poll_until(ser, time.monotonic() + 1.5)
finally:
    try:
        if ser.is_open:
            ser.close()
    finally:
        log_path.parent.mkdir(parents=True, exist_ok=True)
        log_path.write_text("\n".join(lines), encoding="utf-8")

def contains(text):
    return any(text in line for line in lines)

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
        Quote-ProcessArgument ([string]$DurationMs)
        Quote-ProcessArgument $SerialLogPath
        Quote-ProcessArgument $StartSignalPath
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
    }
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
    $sentences = @(
        "火山识别和蓝牙传输正在接受测试。",
        "蓝牙音频正在发送到火山识别，请检查文本结果。",
        "这是一段新的合成语音，用来检查蓝牙传输。",
        "如果这句话能被识别出来，说明流式上传已经连通。",
        "自动化测试正在模拟按键录音，并验证端到端听写链路。"
    )
    return Get-Random -InputObject $sentences
}

function New-TtsWave {
    param(
        [string]$Text,
        [string]$Path,
        [string]$PreferredVoice
    )
    Add-Type -AssemblyName System.Speech
    $synth = [System.Speech.Synthesis.SpeechSynthesizer]::new()
    try {
        $synth.Volume = 100
        $synth.Rate = -1
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

    for ($i = $dataOffset; $i + 1 -lt ($dataOffset + $dataSize); $i += 2) {
        $sample = [BitConverter]::ToInt16($bytes, $i)
        $scaled = [int][Math]::Round($sample * $Gain)
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
if ($NoNotificationTimeoutSeconds -lt 1) {
    throw "NoNotificationTimeoutSeconds must be >= 1"
}

$RunStamp = Get-Date -Format "yyyyMMdd-HHmmss"
if (-not $Sentence) {
    $Sentence = New-RandomSentence
}
if (-not $WavPath) {
    $WavPath = Join-Path $OutDir "ble-stream-smoke-$RunStamp.wav"
}
$WavPath = Resolve-RepoPath $WavPath
if (-not (Test-Path $WavPath)) {
    New-TtsWave -Text $Sentence -Path $WavPath -PreferredVoice $VoiceName
    Boost-WavPcm16 -Path $WavPath -Gain $TtsGain
}
$wavDurationMs = Get-WavDurationMilliseconds -Path $WavPath
$recordingWindowMs = [System.Math]::Max(
    1000,
    $PreRecordDelayMs + $wavDurationMs + $PostPlaybackRecordMs
)
$serialLogPath = Join-Path $OutDir "ble-stream-smoke-$RunStamp.serial.log"
$serialStartSignalPath = Join-Path $OutDir "ble-stream-smoke-$RunStamp.start.signal"
Remove-Item -LiteralPath $serialStartSignalPath -Force -ErrorAction SilentlyContinue

if (-not (Test-Path $ListenerExe)) {
    $env:PATH = "C:\Users\Billy\.cargo\bin;$env:PATH"
    cargo build --manifest-path (Join-Path $RepoRoot "src-tauri\Cargo.toml")
}

if (-not $NoResetBeforeCapture) {
    Reset-SerialTarget -PortName $Port
    Start-Sleep -Seconds 2
} else {
    Send-SerialCommand -PortName $Port -Command "~VREC:CANCEL"
    Start-Sleep -Milliseconds 500
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
}

$logPath = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
$logDir = Split-Path -Parent $logPath
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
$logOffsetValue = if (Test-Path $logPath) { (Get-Item $logPath).Length } else { 0 }
$logOffset = [ref]$logOffsetValue
$capturedLog = ""

Get-Process listener-type -ErrorAction SilentlyContinue | Stop-Process -Force

$process = $null
$serialWindow = $null
$serialReport = $null
$recordingStarted = $false
$insertionTarget = $null
$insertedText = $null
$historySession = $null
$smokeStartedAt = Get-Date
try {
    if ($VerifyInsertion) {
        $insertionTargetPath = Join-Path $OutDir "ble-stream-smoke-$RunStamp.target.txt"
        $insertionTarget = Start-InsertionTarget -Path $insertionTargetPath
    }

    if ($TriggerMode -eq "serial-toggle") {
        $serialWindow = Start-SerialRecordingWindow `
            -PortName $Port `
            -DurationMs $recordingWindowMs `
            -SerialLogPath $serialLogPath `
            -StartSignalPath $serialStartSignalPath
    }

    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = (Resolve-Path $ListenerExe).Path
    $psi.Arguments = "--submit-embedded-audio-ble-stream $TimeoutMs"
    $psi.WorkingDirectory = $RepoRoot
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.EnvironmentVariables["LISTENER_TYPE_HIDE_MAIN_ON_START"] = "1"
    $process = [System.Diagnostics.Process]::Start($psi)

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
    Start-Sleep -Milliseconds ([int]($NotifySettleSeconds * 1000))

    $streamStartedPattern = "embedded audio streaming dictation started"
    Add-Type -AssemblyName System.Windows.Forms
    $player = [System.Media.SoundPlayer]::new($WavPath)
    $player.Load()

    for ($index = 1; $index -le $PlaybackCount; $index++) {
        if ($index -eq $RecordPlaybackIndex) {
            if ($VerifyInsertion -and $insertionTarget) {
                [void](Focus-ProcessWindow -Process $insertionTarget.Process)
                Start-Sleep -Milliseconds 150
            }
            if ($TriggerMode -eq "serial-toggle") {
                Set-Content -Path $serialStartSignalPath -Value "start" -Encoding ASCII
                $recordingStarted = $true
            } else {
                Write-Output "manual_trigger_ready=1"
                Write-Output "manual_trigger_hint=press KEY1 while the playback sentence is audible"
                Start-Sleep -Milliseconds $ManualTriggerReadyDelayMs
            }
            Start-Sleep -Milliseconds $PreRecordDelayMs
        }
        $player.PlaySync()
        if ($index -eq $RecordPlaybackIndex) {
            if ($TriggerMode -eq "serial-toggle") {
                $serialReport = Wait-SerialRecordingWindow -Window $serialWindow
                $serialWindow = $null
                $recordingStarted = $false
            } else {
                Write-Output "manual_trigger_playback_done=1"
            }
        } elseif ($index -lt $PlaybackCount) {
            Start-Sleep -Milliseconds 700
        }
    }

    $streamStarted = $capturedLog -match $streamStartedPattern
    $noNotificationDeadline = (Get-Date).AddSeconds($NoNotificationTimeoutSeconds)
    $doneDeadline = (Get-Date).AddMilliseconds($TimeoutMs + 10000)
    $doneMatch = $null
    while ((Get-Date) -lt $doneDeadline) {
        Start-Sleep -Milliseconds 300
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
            break
        }
        if ($capturedLog -match "submit-embedded-audio-ble-stream failed") {
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
    if (-not $doneMatch -or -not $doneMatch.Success) {
        throw "Timed out waiting for Listener-Type BLE stream completion"
    }
    Start-Sleep -Milliseconds 1200
    $capturedLog += Read-NewLogText -Path $logPath -Offset $logOffset

    $transcript = Get-LatestAsrTranscriptFromLog -Text $capturedLog
    $missingPackets = [int]$doneMatch.Groups[2].Value
    $pcmBytes = [int]$doneMatch.Groups[1].Value
    $status = if ($missingPackets -gt $MaxMissingPackets) { "WARNING" } else { "PASS" }
    if ($FailOnMissingPackets -and $missingPackets -gt $MaxMissingPackets) {
        $status = "FAIL"
    }
    $verificationErrors = @()
    if ($VerifyHistory -or $VerifyInsertion) {
        $historySession = Wait-SmokeHistorySession `
            -StartedAt $smokeStartedAt `
            -Transcript $transcript `
            -ExpectedPcmBytes $pcmBytes `
            -TimeoutSeconds 15
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
    if ($VerifyHistory) {
        if (-not $historySession) {
            $verificationErrors += "history session was not written for this BLE smoke"
        } elseif (-not $historySession.embeddedAudioStats) {
            $verificationErrors += "history session does not include embeddedAudioStats"
        }
    }
    if ($VerifyInsertion) {
        $insertedText = Read-InsertionTargetText -Target $insertionTarget
        $expectedText = ""
        if ($historySession -and -not [string]::IsNullOrWhiteSpace([string]$historySession.finalText)) {
            $expectedText = [string]$historySession.finalText
        } elseif (-not [string]::IsNullOrWhiteSpace($transcript)) {
            $expectedText = $transcript
        }
        if ([string]::IsNullOrWhiteSpace($expectedText)) {
            $verificationErrors += "no transcript/final text available for insertion verification"
        } elseif (-not ([string]$insertedText).Contains($expectedText)) {
            $verificationErrors += "target editor does not contain final text"
        }
    }
    if ($verificationErrors.Count -gt 0) {
        $status = "FAIL"
    }

    $report = [pscustomobject]@{
        status = $status
        trigger = $TriggerMode
        port = $Port
        sentence = $Sentence
        transcript = $transcript
        wav_path = $WavPath
        tts_gain = $TtsGain
        playback_count = $PlaybackCount
        record_playback_index = $RecordPlaybackIndex
        wav_duration_ms = $wavDurationMs
        recording_window_ms = $recordingWindowMs
        pre_record_delay_ms = $PreRecordDelayMs
        post_playback_record_ms = $PostPlaybackRecordMs
        no_notification_timeout_seconds = $NoNotificationTimeoutSeconds
        manual_trigger_ready_delay_ms = $ManualTriggerReadyDelayMs
        serial_log_path = if ($serialReport) { $serialReport.serial_log_path } else { $serialLogPath }
        serial_report = $serialReport
        pcm_bytes = $pcmBytes
        missing_packets = $missingPackets
        max_missing_packets = $MaxMissingPackets
        verify_insertion = [bool]$VerifyInsertion
        verify_history = [bool]$VerifyHistory
        insertion_target_path = if ($insertionTarget) { $insertionTarget.Path } else { $null }
        inserted_text = $insertedText
        history_path = Get-HistoryPath
        history_session = $historySession
        verification_errors = $verificationErrors
        log_path = $logPath
    }
    $pretty = $report | ConvertTo-Json -Depth 8
    $compact = $report | ConvertTo-Json -Depth 8 -Compress
    $reportPath = Join-Path $OutDir "ble-stream-smoke.$((Get-Date).ToString('yyyyMMdd-HHmmss')).json"
    Set-Content -Path $reportPath -Value $pretty -Encoding UTF8
    Write-Output $pretty
    Write-Output "ble_stream_smoke_result_json=$compact"
    if ($status -eq "FAIL") {
        exit 1
    }
} catch {
    $caughtError = $_
    if ($recordingStarted) {
        try {
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
    $report = [pscustomobject]@{
        status = "FAIL"
        trigger = $TriggerMode
        port = $Port
        sentence = $Sentence
        wav_path = $WavPath
        tts_gain = $TtsGain
        wav_duration_ms = $wavDurationMs
        recording_window_ms = $recordingWindowMs
        pre_record_delay_ms = $PreRecordDelayMs
        post_playback_record_ms = $PostPlaybackRecordMs
        no_notification_timeout_seconds = $NoNotificationTimeoutSeconds
        manual_trigger_ready_delay_ms = $ManualTriggerReadyDelayMs
        serial_log_path = if ($serialReport) { $serialReport.serial_log_path } else { $serialLogPath }
        serial_report = $serialReport
        verify_insertion = [bool]$VerifyInsertion
        verify_history = [bool]$VerifyHistory
        insertion_target_path = if ($insertionTarget) { $insertionTarget.Path } else { $null }
        inserted_text = $insertedText
        history_path = Get-HistoryPath
        error = $caughtError.Exception.Message
        log_path = $logPath
    }
    $pretty = $report | ConvertTo-Json -Depth 8
    $compact = $report | ConvertTo-Json -Depth 8 -Compress
    Write-Output $pretty
    Write-Output "ble_stream_smoke_result_json=$compact"
    exit 1
} finally {
    Stop-SerialRecordingWindow -Window $serialWindow
    Stop-InsertionTarget -Target $insertionTarget
    if ($process -and -not $process.HasExited) {
        try {
            $process.Kill()
        } catch {
        }
    }
}
