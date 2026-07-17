[CmdletBinding(PositionalBinding = $false)]
param(
    [string]$Port = "COM3",
    [int]$CaptureSeconds = 180,
    [int]$PromptTimeoutSeconds = 150,
    [int]$CaptureWaitAfterPromptSeconds = 45,
    [int]$CaptureGraceSeconds = 20,
    [string]$OutputRoot = "",
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$typeRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$listenerRoot = Resolve-Path (Join-Path $typeRoot "..")
$firmwareRoot = Join-Path $listenerRoot "Listener-Firmware"
$sendSerial = Join-Path $firmwareRoot "tools\send_serial_and_capture.ps1"
$checker = Join-Path $typeRoot "scripts\check-recording-consumption-evidence.mjs"
$installedType = "C:\Program Files\Listener Type\listener-type.exe"
$workflowRoot = if ($env:AI_WORKFLOW_REPO -and (Test-Path (Join-Path $env:AI_WORKFLOW_REPO "docs\agent_quickstart.md"))) {
    $env:AI_WORKFLOW_REPO
} else {
    "C:\Users\Billy\Desktop\Denzic\ai-collaboration-workflow"
}
$aiw = Join-Path $workflowRoot "scripts\aiw.ps1"
$typeLog = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
$capsuleLog = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\capsule-timeline.log"

if ([string]::IsNullOrWhiteSpace($OutputRoot)) {
    $OutputRoot = Join-Path $typeRoot ".cache\validation\recording-consumption-20260717"
}

$missing = @()
foreach ($path in @($firmwareRoot, $sendSerial, $checker, $installedType, $aiw)) {
    if (-not (Test-Path -LiteralPath $path)) {
        $missing += $path
    }
}
if ($missing.Count -gt 0) {
    throw "Missing required path(s): $($missing -join '; ')"
}

$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$base = "physical-long-utterance-msys-credit-fix-runner-$stamp"
$artifactDir = Join-Path $OutputRoot $base
$serialLog = Join-Path $artifactDir "$base.log"
$captureStdout = Join-Path $artifactDir "$base.capture.stdout.log"
$captureStderr = Join-Path $artifactDir "$base.capture.stderr.log"
$promptNote = Join-Path $artifactDir "$base.operator-note.txt"
$promptResult = Join-Path $artifactDir "$base.operator-prompt.stdout.json"
$statusLog = Join-Path $artifactDir "$base.preflight-device-status.log"
$powerLog = Join-Path $artifactDir "$base.preflight-power-status.log"
$readyLog = Join-Path $artifactDir "$base.preflight-ready.log"
$machineCheck = Join-Path $artifactDir "$base.machine-check.json"
$runnerResult = Join-Path $artifactDir "$base.runner-result.json"

$spokenText = "请用自然语速朗读这一段，目标是确认 Listener 在长时间说话时，录音生产速度和蓝牙消费速度能够持续匹配。我现在模拟一条真实工作消息：短录音不能代表长录音稳定，因为短样本可能还没有暴露队列堆积、蓝牙通知重试或者缓存压力。接下来我会连续说一段完整的话，中间可以有自然停顿，也可以像平时说话一样稍微换几个词。系统应该在我按下录音键之后立刻开始收音，持续发送每一包音频，不应该把数据越攒越多，不应该出现丢包，也不应该因为缓存紧张让最终文本等很久。这里我再补充一个场景：如果我正在聊天窗口里讲一段比较长的想法，Listener 应该像稳定的话筒一样把这段话完整传给 Type，然后让识别结果正常收尾。读到最后，请再说一句：这次验证关注的是四十五秒以上的真实人声、真实设备按键、安装版 Type、固件蓝牙传输和最终非空文本是否能一起闭环。"

$plan = [ordered]@{
    status = "DRY_RUN"
    artifact_dir = $artifactDir
    installed_type = $installedType
    firmware_root = $firmwareRoot
    port = $Port
    capture_seconds = $CaptureSeconds
    prompt_timeout_seconds = $PromptTimeoutSeconds
    capture_grace_seconds = $CaptureGraceSeconds
    formal_capture_uses_command_read_ms = $false
    serial_log = $serialLog
    prompt_result = $promptResult
    machine_check = $machineCheck
    preflight_power_log = $powerLog
}

if ($DryRun.IsPresent) {
    $plan | ConvertTo-Json -Depth 6
    exit 0
}

New-Item -ItemType Directory -Force -Path $artifactDir | Out-Null

$typeProcess = Get-CimInstance Win32_Process |
    Where-Object { $_.Name -eq "listener-type.exe" -and $_.ExecutablePath -eq $installedType } |
    Select-Object -First 1
if ($null -eq $typeProcess) {
    Start-Process -FilePath $installedType -WindowStyle Hidden | Out-Null
    Start-Sleep -Seconds 6
    $typeProcess = Get-CimInstance Win32_Process |
        Where-Object { $_.Name -eq "listener-type.exe" -and $_.ExecutablePath -eq $installedType } |
        Select-Object -First 1
}
if ($null -eq $typeProcess) {
    throw "Installed Listener Type process is not running from $installedType"
}

$mutex = New-Object System.Threading.Mutex($false, "Global\Listener_COM3")
$hasMutex = $false
$captureProcess = $null
$captureStartIso = $null
$captureEndIso = $null
$promptJson = $null
$checkerExit = $null
$runStatus = "NO_GO"
$errorText = $null

try {
    $hasMutex = $mutex.WaitOne([TimeSpan]::FromSeconds(8))
    if (-not $hasMutex) {
        throw "COM3 mutex acquisition timed out"
    }

    pwsh -NoProfile -File $sendSerial -Port $Port -Command "~DEVICE:STATUS" -CommandReadMs 1800 -OutputPath $statusLog
    $statusText = Get-Content -Raw -LiteralPath $statusLog
    if ($statusText -notmatch "ec11_fast_recording=1") {
        throw "Preflight failed: ec11_fast_recording=1 was not found in device status"
    }

    pwsh -NoProfile -File $sendSerial -Port $Port -Command "~POWER:STATUS" -CommandReadMs 2500 -OutputPath $powerLog
    $powerText = Get-Content -Raw -LiteralPath $powerLog
    if ($powerText -notmatch "state=CONNECTED_IDLE") {
        throw "Preflight failed: state=CONNECTED_IDLE was not found in power status"
    }

    pwsh -NoProfile -File $sendSerial -Port $Port -CaptureSeconds 12 -OutputPath $readyLog
    $readyText = Get-Content -Raw -LiteralPath $readyLog
    if ($readyText -notmatch "TYPE:HB") {
        throw "Preflight failed: TYPE:HB evidence was not present"
    }

    $captureStartTimeUtc = (Get-Date).ToUniversalTime()
    $captureStartIso = $captureStartTimeUtc.ToString("o")
    $captureProcess = Start-Process -FilePath "pwsh" -ArgumentList @(
        "-NoProfile",
        "-File",
        $sendSerial,
        "-Port",
        $Port,
        "-CaptureSeconds",
        ([string]$CaptureSeconds),
        "-OutputPath",
        $serialLog
    ) -WorkingDirectory $firmwareRoot -RedirectStandardOutput $captureStdout -RedirectStandardError $captureStderr -PassThru -WindowStyle Hidden

    Start-Sleep -Milliseconds 800
    $promptArgs = @(
        "-NoProfile",
        "-File",
        $aiw,
        "operator-prompt",
        "-ReviewStyle",
        "-Input",
        "-Json",
        "-Title",
        "Listener 录音长语音验证",
        "-ProgressText",
        "1/1",
        "-ScopeText",
        "录音平台消费速度；窗口只记录动作完成，不代表机器 PASS",
        "-Message",
        "请现在操作 Listener 实物：1. 单按设备录音键开始录音；2. 按下方原文自然朗读 45-60 秒；3. 读完后再单按录音键停止；4. 等 Type 胶囊完成或显示结果后点击【已完成】。如果没有开始、断连、无法停止、文字为空，或你没来得及操作，请点【失败/中止】并写备注。",
        "-ExpectedText",
        "机器 PASS 需要串口和 Type 日志同时证明：真实物理 EC11 按键，45-60 秒真实语音，missing_packets=0，audio_sent 等于 expected_packet_count，queue/pool/audio failure 为 0，pool_high_water_pct <=20，mbuf/ENOMEM retry 各 <=1% audio_sent，且 ASR final 非空。",
        "-SpokenText",
        $spokenText,
        "-Buttons",
        "已完成,失败,中止",
        "-OutputPath",
        $promptNote,
        "-Width",
        "780",
        "-Height",
        "720",
        "-TimeoutSeconds",
        ([string]$PromptTimeoutSeconds)
    )
    $promptJson = & pwsh @promptArgs
    $promptJson | Set-Content -LiteralPath $promptResult -Encoding UTF8

    $captureDeadlineUtc = $captureStartTimeUtc.AddSeconds($CaptureSeconds + $CaptureGraceSeconds)
    $minimumPostPromptWaitMs = [Math]::Max(1, $CaptureWaitAfterPromptSeconds) * 1000
    $remainingCaptureMs = [int][Math]::Max(
        1,
        ($captureDeadlineUtc - (Get-Date).ToUniversalTime()).TotalMilliseconds)
    $waitAfterPromptMs = [Math]::Max($minimumPostPromptWaitMs, $remainingCaptureMs)
    if ($captureProcess -and -not $captureProcess.HasExited) {
        [void]$captureProcess.WaitForExit($waitAfterPromptMs)
    }
    if ($captureProcess -and -not $captureProcess.HasExited) {
        Stop-Process -Id $captureProcess.Id -Force
        throw "serial capture did not exit before timeout"
    }
    $captureEndIso = (Get-Date).ToUniversalTime().ToString("o")

    & node $checker `
        --serial-log $serialLog `
        --prompt-json $promptResult `
        --type-log $typeLog `
        --capsule-log $capsuleLog `
        --capture-start-iso $captureStartIso `
        --capture-end-iso $captureEndIso `
        --output-json $machineCheck
    $checkerExit = $LASTEXITCODE
    $runStatus = if ($checkerExit -eq 0) { "PASS" } else { "NO_GO" }
} catch {
    $errorText = $_.Exception.Message
    $runStatus = "NO_GO"
} finally {
    if ($hasMutex) {
        $mutex.ReleaseMutex() | Out-Null
    }
    $mutex.Dispose()
}

$result = [ordered]@{
    status = $runStatus
    error = $errorText
    artifact_dir = $artifactDir
    installed_type_pid = $typeProcess.ProcessId
    installed_type_path = $typeProcess.ExecutablePath
    preflight_status_log = $statusLog
    preflight_power_log = $powerLog
    preflight_ready_log = $readyLog
    capture_start_iso = $captureStartIso
    capture_end_iso = $captureEndIso
    serial_log = $serialLog
    prompt_result = $promptResult
    prompt_note = $promptNote
    machine_check = $machineCheck
    checker_exit_code = $checkerExit
}
$result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $runnerResult -Encoding UTF8
Get-Content -Raw -LiteralPath $runnerResult
exit $(if ($runStatus -eq "PASS") { 0 } else { 1 })
