[CmdletBinding(PositionalBinding = $false)]
param(
    [string]$OutputDir = "",
    [string]$StepId = "",
    [int]$TimeoutSeconds = 0,
    [switch]$ListSteps,
    [switch]$NoPrompt
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $OutputDir = Join-Path $repoRoot ".cache\validation\ble-focused-human-review-$stamp"
}
$OutputDir = [System.IO.Path]::GetFullPath($OutputDir)
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

$steps = @(
    [ordered]@{
        id = "manual-delete-no-autopair-led"
        title = "1. 手动删除不抢回"
        prompt = "动作：Type 保持开启，在 Windows 蓝牙里删除当前 Listener。`r`n期望：Type 不自动抢回配对；灯效不显示 Type-ready/已连接，不出现已连接未连接循环。`r`n只填写失败现象；通过可直接点通过。"
    },
    [ordered]@{
        id = "type-exit-led-clears"
        title = "2. 退出 Type 灯效"
        prompt = "动作：退出 Listener Type，等待 12 秒。`r`n期望：固件不再显示 Type-ready/连接 Type 的灯效；功能层可以保持 HID 已连接，但灯不能骗用户 Type 还在。`r`n填写退出后蓝牙灯/EC11 灯现象。"
    },
    [ordered]@{
        id = "no-type-clean-pair-led"
        title = "3. 无 Type 干净配对"
        prompt = "动作：确认 Type 已退出，只用 Windows 原生蓝牙连接 Listener。`r`n期望：键盘蓝牙能连上；灯效不是一直 Type-ready，也不是已连接/未连接乱跳。`r`n填写 Windows 状态和灯效。"
    },
    [ordered]@{
        id = "ec11-single-not-repair"
        title = "4. EC11 单击不重配"
        prompt = "动作：单击一次 EC11 旋钮。`r`n期望：只触发单击/白色本地反馈，不打开蓝牙重配，不弹 Windows 连接，也不进入双击重配灯效。`r`n填写是否误判成双击。"
    },
    [ordered]@{
        id = "ec11-double-repair"
        title = "5. EC11 双击重配"
        prompt = "动作：快速双击 EC11 旋钮。`r`n期望：不要进录音；出现已验收的蓝色重配三次双闪；Windows/Type 能完成重新连接，不长时间卡已连接/未连接。`r`n填写灯效、Windows 状态和 Type 胶囊。"
    },
    [ordered]@{
        id = "type-restart-recovers-led"
        title = "6. Type 重启恢复"
        prompt = "动作：重新打开 Listener Type，等待它自动接管。`r`n期望：Type 恢复 BLE 音频通道；灯效从 HID connected 找 Type 过渡到 Type-ready，不乱闪。`r`n填写恢复速度和灯效。"
    },
    [ordered]@{
        id = "long-press-idle-led"
        title = "7. 长按/idle 灯效"
        prompt = "动作：长按 EC11 到关机确认灯效出现，然后松开；如方便，再观察一次 idle/断电恢复。`r`n期望：长按先清掉白色按下反馈，进入稳定的琥珀确认；松开后不残留全亮/错色/蓝牙干扰。`r`n填写灯效。"
    }
)

if ($ListSteps.IsPresent) {
    foreach ($step in $steps) {
        Write-Output ("{0}`t{1}" -f $step.id, $step.title)
    }
    exit 0
}

if (-not [string]::IsNullOrWhiteSpace($StepId)) {
    $selectedSteps = @($steps | Where-Object { $_.id -eq $StepId })
    if ($selectedSteps.Count -eq 0) {
        throw "Unknown StepId '$StepId'. Use -ListSteps to see valid steps."
    }
    $steps = $selectedSteps
}

function Save-Result {
    param(
        [Parameter(Mandatory = $true)][string]$StepId,
        [Parameter(Mandatory = $true)][string]$Result,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Notes
    )
    $record = [ordered]@{
        step = $StepId
        result = $Result
        notes = $Notes
        at = (Get-Date).ToString("o")
    }
    $path = Join-Path $OutputDir ("{0}.json" -f $StepId)
    $record | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $path -Encoding UTF8
}

if ($NoPrompt.IsPresent) {
    foreach ($step in $steps) {
        Save-Result -StepId $step.id -Result "SKIP" -Notes "NoPrompt dry run"
    }
} else {
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -AssemblyName System.Drawing
    [System.Windows.Forms.Application]::EnableVisualStyles()

    foreach ($step in $steps) {
        [System.Media.SystemSounds]::Exclamation.Play()

        $form = [System.Windows.Forms.Form]::new()
        $form.Text = "Listener 蓝牙验收"
        $form.StartPosition = "CenterScreen"
        $form.ClientSize = [System.Drawing.Size]::new(620, 380)
        $form.MinimumSize = [System.Drawing.Size]::new(600, 380)
        $form.TopMost = $true
        $form.Font = [System.Drawing.Font]::new("Microsoft YaHei UI", 10)
        $form.AutoScaleMode = [System.Windows.Forms.AutoScaleMode]::Dpi

        $title = [System.Windows.Forms.Label]::new()
        $title.Text = $step.title
        $title.Font = [System.Drawing.Font]::new("Microsoft YaHei UI", 13, [System.Drawing.FontStyle]::Bold)
        $title.AutoSize = $false
        $title.Location = [System.Drawing.Point]::new(18, 14)
        $title.Size = [System.Drawing.Size]::new(580, 34)
        $form.Controls.Add($title)

        $prompt = [System.Windows.Forms.Label]::new()
        $prompt.Text = $step.prompt
        $prompt.AutoSize = $false
        $prompt.Location = [System.Drawing.Point]::new(18, 54)
        $prompt.Size = [System.Drawing.Size]::new(582, 124)
        $form.Controls.Add($prompt)

        $box = [System.Windows.Forms.TextBox]::new()
        $box.Multiline = $true
        $box.ScrollBars = "Vertical"
        $box.Location = [System.Drawing.Point]::new(18, 188)
        $box.Size = [System.Drawing.Size]::new(582, 102)
        $box.Anchor = "Left,Right,Top,Bottom"
        $form.Controls.Add($box)

        $buttonPanel = [System.Windows.Forms.FlowLayoutPanel]::new()
        $buttonPanel.FlowDirection = "RightToLeft"
        $buttonPanel.Location = [System.Drawing.Point]::new(18, 310)
        $buttonPanel.Size = [System.Drawing.Size]::new(582, 44)
        $buttonPanel.Anchor = "Left,Right,Bottom"
        $form.Controls.Add($buttonPanel)

        $result = "SKIP"
        Save-Result -StepId $step.id -Result "STARTED" -Notes $step.prompt
        foreach ($spec in @(
            @{ text = "通过"; value = "PASS" },
            @{ text = "失败"; value = "FAIL" },
            @{ text = "跳过"; value = "SKIP" }
        )) {
            $button = [System.Windows.Forms.Button]::new()
            $button.Text = $spec.text
            $button.Size = [System.Drawing.Size]::new(100, 34)
            $button.Tag = $spec.value
            $button.Add_Click({
                param($sender, $eventArgs)
                $script:focusedReviewResult = [string]$sender.Tag
                $form.Close()
            })
            $buttonPanel.Controls.Add($button)
        }

        $script:focusedReviewResult = "SKIP"
        $timer = $null
        if ($TimeoutSeconds -gt 0) {
            $timer = [System.Windows.Forms.Timer]::new()
            $timer.Interval = [Math]::Max(1000, $TimeoutSeconds * 1000)
            $timer.Add_Tick({
                $timer.Stop()
                $script:focusedReviewResult = "TIMEOUT"
                $form.Close()
            })
            $timer.Start()
        }
        $form.Add_Shown({
            try {
                [Console]::Beep(900, 220)
                [Console]::Beep(1200, 220)
            } catch {
            }
            $form.Activate()
        })
        [void]$form.ShowDialog()
        if ($null -ne $timer) {
            $timer.Dispose()
        }
        $result = $script:focusedReviewResult
        Save-Result -StepId $step.id -Result $result -Notes $box.Text
    }
}

$records = foreach ($step in $steps) {
    $path = Join-Path $OutputDir ("{0}.json" -f $step.id)
    if (Test-Path -LiteralPath $path) {
        Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
    }
}
$failed = @($records | Where-Object { $_.result -eq "FAIL" }).Count
$incomplete = @($records | Where-Object { $_.result -in @("SKIP", "TIMEOUT", "STARTED") }).Count
$status = if ($failed -gt 0) {
    "HUMAN_REVIEW_FAIL"
} elseif ($incomplete -gt 0) {
    "HUMAN_REVIEW_INCOMPLETE"
} else {
    "HUMAN_REVIEW_PASS"
}

$summary = [System.Collections.Generic.List[string]]::new()
$summary.Add("# Listener BLE Focused Human Review") | Out-Null
$summary.Add("") | Out-Null
$summary.Add("Status: $status") | Out-Null
$summary.Add("Output: $OutputDir") | Out-Null
$summary.Add("") | Out-Null
foreach ($record in $records) {
    $summary.Add(("## {0} - {1}" -f $record.step, $record.result)) | Out-Null
    $summary.Add(($record.notes | Out-String).Trim()) | Out-Null
    $summary.Add("") | Out-Null
}
$summaryPath = Join-Path $OutputDir "ble-focused-human-review-summary.md"
$summary | Set-Content -LiteralPath $summaryPath -Encoding UTF8
Write-Output $status
Write-Output "summary=$summaryPath"
