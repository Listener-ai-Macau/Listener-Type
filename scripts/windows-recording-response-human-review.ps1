[CmdletBinding(PositionalBinding = $false)]
param(
    [string]$OutputDir = "",
    [string]$LogPath = "",
    [switch]$NoPrompt
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $OutputDir = Join-Path $repoRoot ".cache\validation\recording-response-human-review-$stamp"
} elseif (-not [System.IO.Path]::IsPathRooted($OutputDir)) {
    $OutputDir = Join-Path $repoRoot $OutputDir
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$OutputDir = (Resolve-Path -LiteralPath $OutputDir).Path

if ([string]::IsNullOrWhiteSpace($LogPath)) {
    $LogPath = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
}

$sessionPath = Join-Path $OutputDir "recording-response-session.jsonl"
$summaryPath = Join-Path $OutputDir "recording-response-summary.md"
$summaryJsonPath = Join-Path $OutputDir "recording-response-summary.json"

$mutexCreated = $false
$mutex = [System.Threading.Mutex]::new(
    $true,
    "Global\Denzic.Listener.RecordingResponseHumanReview",
    [ref]$mutexCreated)
if (-not $mutexCreated) {
    Write-Warning "Another recording response review window is already running."
    exit 3
}

function Convert-PromptText {
    param([AllowNull()][string]$Text)
    if ($null -eq $Text) {
        return ""
    }
    return (($Text -replace "\\r\\n", "`r`n") -replace "/r/n", "`r`n") -replace "\\n", "`r`n"
}

function Ensure-FormsLoaded {
    if ($NoPrompt.IsPresent) {
        return
    }
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -AssemblyName System.Drawing
    [System.Windows.Forms.Application]::EnableVisualStyles()
}

function Invoke-ReviewSound {
    if ($NoPrompt.IsPresent) {
        return
    }
    try {
        [System.Media.SystemSounds]::Exclamation.Play()
    } catch {
        try {
            [Console]::Beep(880, 160)
        } catch {
        }
    }
}

function Get-LogLength {
    if (Test-Path -LiteralPath $LogPath) {
        return (Get-Item -LiteralPath $LogPath).Length
    }
    return 0
}

function Get-LogTailFromOffset {
    param([long]$Offset)
    if (-not (Test-Path -LiteralPath $LogPath)) {
        return ""
    }
    $stream = [System.IO.File]::Open($LogPath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    try {
        if ($Offset -gt $stream.Length) {
            $Offset = 0
        }
        $stream.Seek($Offset, [System.IO.SeekOrigin]::Begin) | Out-Null
        $reader = [System.IO.StreamReader]::new($stream, [System.Text.Encoding]::UTF8, $true, 4096, $true)
        try {
            return $reader.ReadToEnd()
        } finally {
            $reader.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

function Show-Intro {
    if ($NoPrompt.IsPresent) {
        return $true
    }
    Ensure-FormsLoaded
    Invoke-ReviewSound
    $message = @"
这次验收刚刷进去的录音响应和旋钮录音灯，不改设置。

流程：
1. 确认 Type 已在托盘运行、Listener 蓝牙已连接。
2. 按一次录音键，观察白色按下灯是否被录音灯平滑接管。
3. 再按一次停止，观察 AI/转写状态多久出现。
4. 连续再做一轮，看第一轮和第二轮是否都稳定。

通过=这个现象正常；失败=请直接填你看到的问题。脚本会记录时间点和 Type 日志 offset，方便我对日志。
"@
    $result = [System.Windows.Forms.MessageBox]::Show(
        (Convert-PromptText $message),
        "Listener 录音响应验收",
        [System.Windows.Forms.MessageBoxButtons]::OKCancel,
        [System.Windows.Forms.MessageBoxIcon]::Information)
    return $result -eq [System.Windows.Forms.DialogResult]::OK
}

function Show-Step {
    param(
        [Parameter(Mandatory = $true)][int]$Index,
        [Parameter(Mandatory = $true)][int]$Total,
        [Parameter(Mandatory = $true)][string]$Title,
        [Parameter(Mandatory = $true)][string]$Instruction,
        [string]$DefaultObservation = ""
    )

    $startAt = Get-Date
    $logOffset = Get-LogLength

    if ($NoPrompt.IsPresent) {
        return [PSCustomObject]@{
            index = $Index
            title = $Title
            result = "SKIP"
            observation = "NoPrompt mode."
            started_at = $startAt.ToString("o")
            ended_at = (Get-Date).ToString("o")
            log_offset = $logOffset
            log_tail = ""
        }
    }

    Ensure-FormsLoaded
    Invoke-ReviewSound
    $form = [System.Windows.Forms.Form]::new()
    try {
        $form.Text = "录音响应验收 $Index/$Total"
        $form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
        $form.TopMost = $true
        $form.ShowInTaskbar = $true
        $form.AutoScaleMode = [System.Windows.Forms.AutoScaleMode]::Dpi
        $form.ClientSize = [System.Drawing.Size]::new(660, 420)
        $form.MinimumSize = [System.Drawing.Size]::new(600, 380)
        $form.Font = [System.Drawing.Font]::new("Microsoft YaHei UI", 9)

        $titleLabel = [System.Windows.Forms.Label]::new()
        $titleLabel.Text = "$Index/$Total  $Title"
        $titleLabel.Left = 14
        $titleLabel.Top = 14
        $titleLabel.Width = $form.ClientSize.Width - 28
        $titleLabel.Height = 28
        $titleLabel.Font = [System.Drawing.Font]::new("Microsoft YaHei UI", 11, [System.Drawing.FontStyle]::Bold)
        $titleLabel.Anchor = "Top,Left,Right"
        $form.Controls.Add($titleLabel)

        $instructionBox = [System.Windows.Forms.TextBox]::new()
        $instructionBox.Left = 14
        $instructionBox.Top = 50
        $instructionBox.Width = $form.ClientSize.Width - 28
        $instructionBox.Height = 145
        $instructionBox.Multiline = $true
        $instructionBox.ReadOnly = $true
        $instructionBox.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
        $instructionBox.Text = Convert-PromptText $Instruction
        $instructionBox.Anchor = "Top,Left,Right"
        $form.Controls.Add($instructionBox)

        $label = [System.Windows.Forms.Label]::new()
        $label.Text = "你看到的现象："
        $label.Left = 14
        $label.Top = 208
        $label.Width = 180
        $label.Height = 22
        $form.Controls.Add($label)

        $observation = [System.Windows.Forms.TextBox]::new()
        $observation.Left = 14
        $observation.Top = 232
        $observation.Width = $form.ClientSize.Width - 28
        $observation.Height = 105
        $observation.Multiline = $true
        $observation.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
        $observation.Text = Convert-PromptText $DefaultObservation
        $observation.Anchor = "Top,Bottom,Left,Right"
        $form.Controls.Add($observation)

        $holder = @{ result = "ABORT" }

        $passButton = [System.Windows.Forms.Button]::new()
        $passButton.Text = "通过"
        $passButton.Width = 96
        $passButton.Height = 32
        $passButton.Left = $form.ClientSize.Width - 316
        $passButton.Top = $form.ClientSize.Height - 48
        $passButton.Anchor = "Right,Bottom"
        $passButton.Add_Click({ $holder.result = "PASS"; $form.Close() })
        $form.Controls.Add($passButton)

        $failButton = [System.Windows.Forms.Button]::new()
        $failButton.Text = "失败"
        $failButton.Width = 96
        $failButton.Height = 32
        $failButton.Left = $form.ClientSize.Width - 210
        $failButton.Top = $form.ClientSize.Height - 48
        $failButton.Anchor = "Right,Bottom"
        $failButton.Add_Click({ $holder.result = "FAIL"; $form.Close() })
        $form.Controls.Add($failButton)

        $skipButton = [System.Windows.Forms.Button]::new()
        $skipButton.Text = "跳过"
        $skipButton.Width = 96
        $skipButton.Height = 32
        $skipButton.Left = $form.ClientSize.Width - 104
        $skipButton.Top = $form.ClientSize.Height - 48
        $skipButton.Anchor = "Right,Bottom"
        $skipButton.Add_Click({ $holder.result = "SKIP"; $form.Close() })
        $form.Controls.Add($skipButton)
        $form.CancelButton = $skipButton

        $form.Add_Shown({
            $form.WindowState = [System.Windows.Forms.FormWindowState]::Normal
            $form.Activate()
            $form.BringToFront()
            $observation.Focus()
        })
        [void]$form.ShowDialog()

        $endAt = Get-Date
        $logTail = Get-LogTailFromOffset -Offset $logOffset
        return [PSCustomObject]@{
            index = $Index
            title = $Title
            result = $holder.result
            observation = $observation.Text
            started_at = $startAt.ToString("o")
            ended_at = $endAt.ToString("o")
            log_offset = $logOffset
            log_tail = $logTail
        }
    } finally {
        $form.Dispose()
    }
}

try {
    if (-not (Show-Intro)) {
        Write-Host "recording_response_status=ABORTED output_dir=$OutputDir"
        exit 2
    }

    $steps = @(
        [PSCustomObject]@{
            title = "准备状态"
            instruction = "确认 Type 已在托盘运行；Windows 蓝牙里 Listener 显示已连接；设备不要处于 OTA/重新配对/低功耗。`r`n`r`n看一下蓝牙灯和 Type 状态是否像已连接。"
            default = ""
        },
        [PSCustomObject]@{
            title = "第一次开始录音"
            instruction = "点本窗口后，不用关 Type。马上按一次录音键。`r`n`r`n重点观察：旋钮白色按下灯应该很快被金色录音灯覆盖；录音灯不要中途灭一下、不要重新启动一次。胶囊也应很快进入录音。"
            default = "按下到胶囊/REC灯：约  秒；白灯切录音灯：平滑/灭一下/重启；现象："
        },
        [PSCustomObject]@{
            title = "第一次停止录音"
            instruction = "现在再按一次录音键停止。`r`n`r`n重点观察：从停止到 AI/转写状态出现多久？录音灯是否直接切到处理灯，不要先灭一段再亮。"
            default = "停止到AI/转写：约  秒；现象："
        },
        [PSCustomObject]@{
            title = "第二次开始录音"
            instruction = "再按一次录音键开始第二轮。`r`n`r`n重点观察：第二轮也要和第一轮一样稳定；不要出现只有第二轮正常、第一轮灭一下的情况。"
            default = "第二轮开始响应：约  秒；和第一轮：都稳定/第一轮差/第二轮差；现象："
        },
        [PSCustomObject]@{
            title = "第二次停止录音"
            instruction = "再按一次录音键停止第二轮。`r`n`r`n重点观察：是否能稳定转写；胶囊状态、REC/AI 灯是否顺；有没有 Type 断连/重连提示。"
            default = "第二轮停止响应：约  秒；现象："
        }
    )

    $records = [System.Collections.Generic.List[object]]::new()
    for ($i = 0; $i -lt $steps.Count; $i++) {
        $step = $steps[$i]
        $record = Show-Step `
            -Index ($i + 1) `
            -Total $steps.Count `
            -Title $step.title `
            -Instruction $step.instruction `
            -DefaultObservation $step.default
        $records.Add($record) | Out-Null
        ($record | ConvertTo-Json -Depth 8 -Compress) | Add-Content -LiteralPath $sessionPath -Encoding UTF8
        if ($record.result -eq "ABORT") {
            break
        }
    }

    $failCount = @($records | Where-Object { $_.result -eq "FAIL" }).Count
    $skipCount = @($records | Where-Object { $_.result -eq "SKIP" -or $_.result -eq "ABORT" }).Count
    $status = if ($failCount -gt 0) {
        "HUMAN_REVIEW_FAIL"
    } elseif ($skipCount -gt 0) {
        "HUMAN_REVIEW_INCOMPLETE"
    } else {
        "HUMAN_REVIEW_PASS"
    }

    [ordered]@{
        status = $status
        generated_at = (Get-Date).ToString("o")
        output_dir = $OutputDir
        session_jsonl = $sessionPath
        log_path = $LogPath
        records = @($records)
    } | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $summaryJsonPath -Encoding UTF8

    $lines = [System.Collections.Generic.List[string]]::new()
    $lines.Add("# Recording Response Human Review") | Out-Null
    $lines.Add("") | Out-Null
    $lines.Add("- Status: $status") | Out-Null
    $lines.Add("- Log: $LogPath") | Out-Null
    $lines.Add("- Session: $sessionPath") | Out-Null
    $lines.Add("") | Out-Null
    $lines.Add("| # | Step | Result | Observation | Started | Ended |") | Out-Null
    $lines.Add("|---:|---|---|---|---|---|") | Out-Null
    foreach ($record in $records) {
        $obs = (($record.observation -replace "\|", "/") -replace "`r?`n", " ")
        $lines.Add(("| {0} | {1} | {2} | {3} | {4} | {5} |" -f $record.index, $record.title, $record.result, $obs, $record.started_at, $record.ended_at)) | Out-Null
    }
    $lines | Set-Content -LiteralPath $summaryPath -Encoding UTF8

    Invoke-ReviewSound
    if (-not $NoPrompt.IsPresent) {
        [void][System.Windows.Forms.MessageBox]::Show(
            "录音响应验收已记录。`r`n`r`n结果：$status`r`n目录：$OutputDir",
            "Listener 录音响应验收完成",
            [System.Windows.Forms.MessageBoxButtons]::OK,
            [System.Windows.Forms.MessageBoxIcon]::Information)
    }

    Write-Host "recording_response_status=$status"
    Write-Host "summary=$summaryPath"
    Write-Host "session=$sessionPath"
    if ($status -eq "HUMAN_REVIEW_FAIL") {
        exit 1
    }
} finally {
    if ($mutexCreated) {
        $mutex.ReleaseMutex() | Out-Null
    }
    $mutex.Dispose()
}
