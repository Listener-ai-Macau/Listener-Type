param(
    [ValidateSet("WithTypeDoubleRepair", "NoTypeNativePairing", "Rename", "All")]
    [string[]]$Scenario = @("WithTypeDoubleRepair"),
    [string]$OutputDir
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
if (-not $OutputDir) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $OutputDir = Join-Path $repoRoot ".cache\validation\ble-human-review-$stamp"
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

$sessionPath = Join-Path $OutputDir "human-review-session.jsonl"
$summaryPath = Join-Path $OutputDir "human-review-summary.json"

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()

function New-Scenario {
    param(
        [string]$Id,
        [string]$Title,
        [string]$Instruction
    )
    [pscustomobject]@{
        id = $Id
        title = $Title
        instruction = $Instruction
    }
}

$scenarioMap = [ordered]@{
    WithTypeDoubleRepair = New-Scenario `
        -Id "with_type_double_repair" `
        -Title "有 Type 时双击重配" `
        -Instruction "前提：Listener Type 已在托盘运行，Listener 当前可以被 Type 看到。`r`n`r`n操作：双击 Listener 旋钮/EC11。不要手动点 Windows 蓝牙连接，除非窗口明确要求。`r`n`r`n期望：Windows 右下角不要出现“添加设备/点击以设置 listener”的 Swift Pair 通知；Type 自动清理旧缓存并恢复；最后 Windows 蓝牙显示 listener 已连接，Type 日志恢复 notify ready。`r`n`r`n请在下面写实际现象，例如有没有弹 Windows 通知、有没有连接失败、灯效是否正常。"
    NoTypeNativePairing = New-Scenario `
        -Id "no_type_native_pairing" `
        -Title "没有 Type 时原生配对" `
        -Instruction "前提：先退出 Listener Type，只保留 Windows 蓝牙。`r`n`r`n操作：双击 Listener 旋钮/EC11，让设备进入重新配对；从 Windows 蓝牙原生入口添加 listener。`r`n`r`n期望：没有 Type 的电脑仍可按 Windows 原生流程连上键盘；如果出现 Windows Swift Pair 通知，点击连接后应成功，而不是一直“请尝试重新连接设备”。`r`n`r`n完成后请重新启动 Listener Type。请记录实际现象。"
    Rename = New-Scenario `
        -Id "rename" `
        -Title "蓝牙重命名" `
        -Instruction "前提：Listener Type 已运行并能连接 Listener。`r`n`r`n操作：在 Type 里把蓝牙名改成一个随机 ASCII 名字；再写入一次同名。`r`n`r`n期望：不同名写入后 Windows 蓝牙显示的新名字和 Type 输入一致；同名写入不应触发重新配对；默认名字 listener 仍可恢复。`r`n`r`n请记录实际现象，尤其是旧缓存名、重复弹窗、同名重配等问题。"
}

if ($Scenario -contains "All") {
    $steps = @($scenarioMap.Values)
} else {
    $steps = foreach ($name in $Scenario) { $scenarioMap[$name] }
}

function Show-HumanStep {
    param(
        [pscustomobject]$Step,
        [int]$Index,
        [int]$Total
    )

    [System.Media.SystemSounds]::Exclamation.Play()
    try { [Console]::Beep(880, 180) } catch {}

    $form = [System.Windows.Forms.Form]::new()
    $form.Text = "Listener 蓝牙人工验收 ($Index/$Total)"
    $form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
    $form.TopMost = $true
    $form.ClientSize = [System.Drawing.Size]::new(640, 450)
    $form.MinimumSize = [System.Drawing.Size]::new(620, 430)
    $form.Font = [System.Drawing.Font]::new("Microsoft YaHei UI", 10)

    $title = [System.Windows.Forms.Label]::new()
    $title.Text = $Step.Title
    $title.Font = [System.Drawing.Font]::new("Microsoft YaHei UI", 13, [System.Drawing.FontStyle]::Bold)
    $title.AutoSize = $false
    $title.Location = [System.Drawing.Point]::new(18, 16)
    $title.Size = [System.Drawing.Size]::new(600, 28)
    $form.Controls.Add($title)

    $instructions = [System.Windows.Forms.TextBox]::new()
    $instructions.Multiline = $true
    $instructions.ReadOnly = $true
    $instructions.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
    $instructions.Text = $Step.Instruction
    $instructions.Location = [System.Drawing.Point]::new(18, 54)
    $instructions.Size = [System.Drawing.Size]::new(604, 176)
    $instructions.Anchor = "Top,Left,Right"
    $form.Controls.Add($instructions)

    $notesLabel = [System.Windows.Forms.Label]::new()
    $notesLabel.Text = "现象记录"
    $notesLabel.AutoSize = $true
    $notesLabel.Location = [System.Drawing.Point]::new(18, 244)
    $form.Controls.Add($notesLabel)

    $notes = [System.Windows.Forms.TextBox]::new()
    $notes.Multiline = $true
    $notes.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
    $notes.Location = [System.Drawing.Point]::new(18, 270)
    $notes.Size = [System.Drawing.Size]::new(604, 98)
    $notes.Anchor = "Top,Left,Right"
    $form.Controls.Add($notes)

    $buttonY = 390
    $buttons = @(
        @{ text = "通过"; result = "PASS"; x = 138 },
        @{ text = "失败"; result = "FAIL"; x = 250 },
        @{ text = "跳过"; result = "SKIP"; x = 362 },
        @{ text = "终止"; result = "ABORT"; x = 474 }
    )
    foreach ($buttonSpec in $buttons) {
        $button = [System.Windows.Forms.Button]::new()
        $button.Text = $buttonSpec.text
        $button.Tag = $buttonSpec.result
        $button.Size = [System.Drawing.Size]::new(96, 34)
        $button.Location = [System.Drawing.Point]::new($buttonSpec.x, $buttonY)
        $button.Anchor = "Bottom,Right"
        $button.Add_Click({
            $form.Tag = $this.Tag
            $form.Close()
        })
        $form.Controls.Add($button)
    }

    $form.Add_Shown({ $form.Activate(); $notes.Focus() })
    [void]$form.ShowDialog()
    if (-not $form.Tag) {
        $form.Tag = "ABORT"
    }

    [pscustomobject]@{
        timestamp = (Get-Date).ToString("o")
        id = $Step.Id
        title = $Step.Title
        result = [string]$form.Tag
        notes = $notes.Text
    }
}

$results = New-Object System.Collections.Generic.List[object]
for ($i = 0; $i -lt $steps.Count; $i++) {
    $result = Show-HumanStep -Step $steps[$i] -Index ($i + 1) -Total $steps.Count
    $results.Add($result) | Out-Null
    ($result | ConvertTo-Json -Compress) | Add-Content -LiteralPath $sessionPath -Encoding UTF8
    if ($result.result -eq "ABORT") {
        break
    }
}

$status = if ($results | Where-Object { $_.result -eq "FAIL" }) {
    "FAIL"
} elseif ($results | Where-Object { $_.result -eq "ABORT" }) {
    "ABORT"
} elseif ($results | Where-Object { $_.result -eq "SKIP" }) {
    "SKIP"
} else {
    "PASS"
}

$summary = [pscustomobject]@{
    status = $status
    output_dir = $OutputDir
    session = $sessionPath
    results = @($results.ToArray())
}
$summary | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $summaryPath -Encoding UTF8

Write-Host "human_review_status=$status"
Write-Host "human_review_output_dir=$OutputDir"
Write-Host "human_review_summary=$summaryPath"
