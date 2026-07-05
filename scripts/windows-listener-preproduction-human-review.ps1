[CmdletBinding(PositionalBinding = $false)]
param(
    [string]$OutputDir = "",
    [string]$StepId = "",
    [string]$DeviceName = "listener",
    [string]$RandomName = "",
    [switch]$ListSteps,
    [switch]$NoPrompt,
    [switch]$NoSound
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $OutputDir = Join-Path $repoRoot ".cache\validation\preproduction-human-review-$stamp"
} elseif (-not [System.IO.Path]::IsPathRooted($OutputDir)) {
    $OutputDir = Join-Path $repoRoot $OutputDir
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$OutputDir = (Resolve-Path -LiteralPath $OutputDir).Path

$sessionPath = Join-Path $OutputDir "preproduction-human-review-session.jsonl"
$summaryPath = Join-Path $OutputDir "preproduction-human-review-summary.md"
$summaryJsonPath = Join-Path $OutputDir "preproduction-human-review-summary.json"
$startInfoPath = Join-Path $OutputDir "preproduction-human-review-start.txt"

$singleInstanceCreated = $false
$singleInstanceMutex = [System.Threading.Mutex]::new(
    $true,
    "Global\Denzic.Listener.PreproductionHumanReview",
    [ref]$singleInstanceCreated)
if (-not $singleInstanceCreated) {
    Write-Warning "Another Listener preproduction review window is already running."
    exit 3
}

if ([string]::IsNullOrWhiteSpace($RandomName)) {
    $suffix = -join ((48..57 + 65..90) | Get-Random -Count 4 | ForEach-Object { [char]$_ })
    $RandomName = "listener-$suffix"
}

function Join-Text {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Lines)
    return ($Lines -join [Environment]::NewLine)
}

function Convert-ReviewText {
    param([AllowNull()][string]$Text)
    if ($null -eq $Text) { return "" }
    return (($Text -replace "\\r\\n", [Environment]::NewLine) -replace "\\n", [Environment]::NewLine)
}

function New-ReviewStep {
    param(
        [Parameter(Mandatory = $true)][string]$Id,
        [Parameter(Mandatory = $true)][string]$Title,
        [Parameter(Mandatory = $true)][string]$Action,
        [Parameter(Mandatory = $true)][string]$Expected,
        [string]$ObservationTemplate = "",
        [string]$EvidenceHint = "",
        [string]$ClipboardText = ""
    )

    [PSCustomObject]@{
        id = $Id
        title = $Title
        action = $Action
        expected = $Expected
        observation_template = $ObservationTemplate
        evidence_hint = $EvidenceHint
        clipboard_text = $ClipboardText
    }
}

function Get-TypeProcessSnapshot {
    try {
        @(Get-Process | Where-Object {
            $_.ProcessName -match "listener|type" -or $_.MainWindowTitle -match "Listener Type|com.listener.type"
        } | Select-Object Id, ProcessName, MainWindowTitle, Responding, StartTime)
    } catch {
        @([PSCustomObject]@{ error = $_.Exception.Message })
    }
}

function Get-BluetoothDeviceSnapshot {
    try {
        @(Get-PnpDevice -Class Bluetooth -ErrorAction Stop |
            Where-Object {
                $_.FriendlyName -match "listener|Billy|Blistener|keyboard|HID" -or
                $_.InstanceId -match "BTH|BLE|HID"
            } |
            Select-Object Status, Class, FriendlyName, InstanceId |
            Sort-Object FriendlyName, InstanceId)
    } catch {
        @([PSCustomObject]@{ error = $_.Exception.Message })
    }
}

function Save-Snapshot {
    param(
        [Parameter(Mandatory = $true)][int]$Index,
        [Parameter(Mandatory = $true)][string]$StepId,
        [Parameter(Mandatory = $true)][string]$Phase
    )

    $safeStep = $StepId -replace "[^A-Za-z0-9_.-]", "_"
    $safePhase = $Phase -replace "[^A-Za-z0-9_.-]", "_"
    $devicePath = Join-Path $OutputDir ("step-{0:D2}-{1}-{2}-bluetooth.txt" -f $Index, $safeStep, $safePhase)
    $processPath = Join-Path $OutputDir ("step-{0:D2}-{1}-{2}-process.txt" -f $Index, $safeStep, $safePhase)

    Get-BluetoothDeviceSnapshot | Format-Table -AutoSize | Out-String |
        Set-Content -LiteralPath $devicePath -Encoding UTF8
    Get-TypeProcessSnapshot | Format-Table -AutoSize | Out-String |
        Set-Content -LiteralPath $processPath -Encoding UTF8

    [ordered]@{
        bluetooth = $devicePath
        process = $processPath
    }
}

function Ensure-FormsLoaded {
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -AssemblyName System.Drawing
    [System.Windows.Forms.Application]::EnableVisualStyles()
}

function Invoke-NoticeSound {
    if ($NoSound.IsPresent) { return }
    try {
        [System.Media.SystemSounds]::Exclamation.Play()
        [Console]::Beep(880, 160)
        [Console]::Beep(1175, 180)
    } catch {
    }
}

function Show-ReviewStep {
    param(
        [Parameter(Mandatory = $true)][int]$Index,
        [Parameter(Mandatory = $true)][int]$Total,
        [Parameter(Mandatory = $true)][pscustomobject]$Step
    )

    $startedAt = Get-Date
    $before = Save-Snapshot -Index $Index -StepId $Step.id -Phase "before"

    if (-not [string]::IsNullOrWhiteSpace($Step.clipboard_text)) {
        try {
            Ensure-FormsLoaded
            [System.Windows.Forms.Clipboard]::SetText($Step.clipboard_text)
        } catch {
        }
    }

    if ($NoPrompt.IsPresent) {
        return [ordered]@{
            index = $Index
            id = $Step.id
            title = $Step.title
            result = "SKIP"
            observation = "NoPrompt dry run"
            started_at = $startedAt.ToString("o")
            ended_at = (Get-Date).ToString("o")
            before = $before
            after = Save-Snapshot -Index $Index -StepId $Step.id -Phase "after"
        }
    }

    Ensure-FormsLoaded
    Invoke-NoticeSound

    $form = [System.Windows.Forms.Form]::new()
    $form.Text = "Listener 1.0.2 准量产验收"
    $form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
    $form.ClientSize = [System.Drawing.Size]::new(660, 500)
    $form.MinimumSize = [System.Drawing.Size]::new(620, 460)
    $form.MaximizeBox = $false
    $form.TopMost = $true
    $form.Font = [System.Drawing.Font]::new("Microsoft YaHei UI", 10)
    $form.AutoScaleMode = [System.Windows.Forms.AutoScaleMode]::Dpi

    $title = [System.Windows.Forms.Label]::new()
    $title.Text = "$Index/$Total  $($Step.title)"
    $title.Font = [System.Drawing.Font]::new("Microsoft YaHei UI", 12, [System.Drawing.FontStyle]::Bold)
    $title.AutoSize = $false
    $title.Location = [System.Drawing.Point]::new(16, 12)
    $title.Size = [System.Drawing.Size]::new(628, 30)
    $form.Controls.Add($title)

    $actionLabel = [System.Windows.Forms.Label]::new()
    $actionLabel.Text = "你现在做"
    $actionLabel.AutoSize = $false
    $actionLabel.Location = [System.Drawing.Point]::new(16, 50)
    $actionLabel.Size = [System.Drawing.Size]::new(628, 22)
    $form.Controls.Add($actionLabel)

    $actionBox = [System.Windows.Forms.TextBox]::new()
    $actionBox.Multiline = $true
    $actionBox.ReadOnly = $true
    $actionBox.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
    $actionBox.Location = [System.Drawing.Point]::new(16, 74)
    $actionBox.Size = [System.Drawing.Size]::new(628, 96)
    $actionBox.Text = Convert-ReviewText $Step.action
    $form.Controls.Add($actionBox)

    $expectedLabel = [System.Windows.Forms.Label]::new()
    $expectedLabel.Text = "通过标准"
    $expectedLabel.AutoSize = $false
    $expectedLabel.Location = [System.Drawing.Point]::new(16, 178)
    $expectedLabel.Size = [System.Drawing.Size]::new(628, 22)
    $form.Controls.Add($expectedLabel)

    $expectedBox = [System.Windows.Forms.TextBox]::new()
    $expectedBox.Multiline = $true
    $expectedBox.ReadOnly = $true
    $expectedBox.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
    $expectedBox.Location = [System.Drawing.Point]::new(16, 202)
    $expectedBox.Size = [System.Drawing.Size]::new(628, 80)
    $expectedBox.Text = Convert-ReviewText $Step.expected
    $form.Controls.Add($expectedBox)

    $obsLabel = [System.Windows.Forms.Label]::new()
    $obsLabel.Text = "填写现象"
    $obsLabel.AutoSize = $false
    $obsLabel.Location = [System.Drawing.Point]::new(16, 290)
    $obsLabel.Size = [System.Drawing.Size]::new(628, 22)
    $form.Controls.Add($obsLabel)

    $observation = [System.Windows.Forms.TextBox]::new()
    $observation.Multiline = $true
    $observation.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
    $observation.Location = [System.Drawing.Point]::new(16, 314)
    $observation.Size = [System.Drawing.Size]::new(628, 88)
    $observation.Anchor = [System.Windows.Forms.AnchorStyles]::Left -bor [System.Windows.Forms.AnchorStyles]::Right -bor [System.Windows.Forms.AnchorStyles]::Top -bor [System.Windows.Forms.AnchorStyles]::Bottom
    $observation.Text = Convert-ReviewText $Step.observation_template
    $form.Controls.Add($observation)

    $buttonPanel = [System.Windows.Forms.FlowLayoutPanel]::new()
    $buttonPanel.FlowDirection = [System.Windows.Forms.FlowDirection]::RightToLeft
    $buttonPanel.WrapContents = $false
    $buttonPanel.Location = [System.Drawing.Point]::new(16, 420)
    $buttonPanel.Size = [System.Drawing.Size]::new(628, 46)
    $buttonPanel.Anchor = [System.Windows.Forms.AnchorStyles]::Left -bor [System.Windows.Forms.AnchorStyles]::Right -bor [System.Windows.Forms.AnchorStyles]::Bottom
    $form.Controls.Add($buttonPanel)

    $script:preproductionReviewResult = "SKIP"
    foreach ($spec in @(
        @{ text = "通过"; value = "PASS" },
        @{ text = "失败"; value = "FAIL" },
        @{ text = "跳过"; value = "SKIP" },
        @{ text = "中止"; value = "ABORT" }
    )) {
        $button = [System.Windows.Forms.Button]::new()
        $button.Text = $spec.text
        $button.Tag = $spec.value
        $button.Size = [System.Drawing.Size]::new(96, 34)
        $button.Add_Click({
            param($sender, $eventArgs)
            $script:preproductionReviewResult = [string]$sender.Tag
            $form.Close()
        })
        $buttonPanel.Controls.Add($button)
    }

    $form.Add_Shown({
        $form.Activate()
        $observation.Focus()
    })
    [void]$form.ShowDialog()

    $endedAt = Get-Date
    $after = Save-Snapshot -Index $Index -StepId $Step.id -Phase "after"

    [ordered]@{
        index = $Index
        id = $Step.id
        title = $Step.title
        result = $script:preproductionReviewResult
        observation = $observation.Text
        action = $Step.action
        expected = $Step.expected
        evidence_hint = $Step.evidence_hint
        started_at = $startedAt.ToString("o")
        ended_at = $endedAt.ToString("o")
        before = $before
        after = $after
    }
}

$steps = @(
    New-ReviewStep `
        -Id "baseline-type-tray-ui" `
        -Title "Type 托盘和窗口基线" `
        -Action (Join-Text @(
            "打开最新 Listener Type，确认托盘里是最新版本。"
            "从托盘打开主窗口。不要继续下一步，直到窗口不是空白 WebView。")) `
        -Expected (Join-Text @(
            "主窗口能打开，界面不空白。"
            "Windows 蓝牙和 Type 状态稳定，不在已连接/未连接之间循环。")) `
        -ObservationTemplate "窗口：正常/空白；托盘：正常/异常；Windows蓝牙状态：；灯效："
    New-ReviewStep `
        -Id "same-name-write-no-repair" `
        -Title "同名写入不重配" `
        -Action (Join-Text @(
            "在 Type 蓝牙名称里填当前 Windows 正在显示的同一个名字。"
            "点击写入。这个步骤是同名写入，不是改名。")) `
        -Expected (Join-Text @(
            "不能触发重新配对。"
            "不能弹 Windows 添加设备通知。"
            "Type 保持连接，录音通道不被破坏。")) `
        -ObservationTemplate "当前名字：；写入后是否断连/弹窗/重配："
    New-ReviewStep `
        -Id "random-name-exact-cache-refresh" `
        -Title "随机名改名和缓存刷新" `
        -Action (Join-Text @(
            "在 Type 蓝牙名称里填这个随机名字并写入：$RandomName"
            "名字已复制到剪贴板。"
            "如果 Windows 需要原生连接，只处理一次，然后等它稳定。")) `
        -Expected (Join-Text @(
            "任意 1-29 个可见 ASCII 名字都能写入。"
            "Windows 最终显示精确新名字：$RandomName。"
            "不能继续显示旧缓存名，不能无限弹添加设备，Type 最终能恢复。")) `
        -ObservationTemplate "写入名字：$RandomName；Windows最终显示：；弹窗次数：；Type恢复耗时：约  秒；现象：" `
        -ClipboardText $RandomName
    New-ReviewStep `
        -Id "restore-default-listener" `
        -Title "恢复默认名字 listener" `
        -Action (Join-Text @(
            "在 Type 蓝牙名称里填 listener 并写入。"
            "如果 Windows 要求连接，只处理一次，然后等它稳定。")) `
        -Expected (Join-Text @(
            "默认名字 listener 能恢复。"
            "Windows 最终显示 listener，Type 恢复连接。"
            "不能截断名字，不能缓存成旧名。")) `
        -ObservationTemplate "Windows最终显示：；Type状态：；现象：" `
        -ClipboardText "listener"
    New-ReviewStep `
        -Id "manual-windows-delete-no-type-autopair" `
        -Title "有 Type 时手动删除不抢回" `
        -Action (Join-Text @(
            "保持 Type 打开。"
            "在 Windows 设置 > 蓝牙和其他设备里删除当前 Listener 设备。"
            "删除后等待 20 秒，不要点击添加设备。")) `
        -Expected (Join-Text @(
            "Type 不能自动把旧电脑配回来。"
            "Windows 不能自己重新出现已连接。"
            "可以提示用户手动恢复，但不能自动 PairAsync 抢回，这用于换电脑。")) `
        -ObservationTemplate "删除结果：；20秒后Windows状态：；有没有自动恢复/弹窗：；Type提示："
    New-ReviewStep `
        -Id "no-type-native-pairing" `
        -Title "没有 Type 的原生配对" `
        -Action (Join-Text @(
            "退出 Listener Type。"
            "确认托盘里没有 Type。"
            "在 Windows 蓝牙里删除旧 Listener，然后用 Windows 添加设备 > 蓝牙连接 Listener。")) `
        -Expected (Join-Text @(
            "没有 Type 的电脑也能把 Listener 当蓝牙键盘连上。"
            "不能一直显示请尝试重新连接设备。"
            "不能卡在已连接/未连接循环。")) `
        -ObservationTemplate "删除旧设备：成功/失败；添加看到名字：；最终状态：；灯效："
    New-ReviewStep `
        -Id "type-takeover-no-forced-repair" `
        -Title "Type 接管已配对设备" `
        -Action (Join-Text @(
            "在上一项原生配对成功后，重新打开最新 Type。"
            "不要改名，不要重新配对，等待 15 秒。")) `
        -Expected (Join-Text @(
            "Type 应该接管已配对设备并恢复 BLE 控制/录音通道。"
            "不应该强制重新配对。"
            "不应该重复弹 Windows 添加设备通知。")) `
        -ObservationTemplate "Type恢复：成功/失败；耗时：约  秒；有没有弹窗：；现象："
    New-ReviewStep `
        -Id "ec11-single-not-double" `
        -Title "EC11 单击不误判双击" `
        -Action (Join-Text @(
            "单击一次 EC11 旋钮。"
            "不要快速双击。观察 Windows、Type 和蓝牙灯。")) `
        -Expected (Join-Text @(
            "只触发单击/本地反馈。"
            "不能打开重配流程，不能弹 Windows 连接通知。"
            "EC11 的按键行为要和 key1-key4 的单/双击窗口一致。")) `
        -ObservationTemplate "是否误触发双击：是/否；灯效：；Windows/Type现象："
    New-ReviewStep `
        -Id "ec11-double-repair-with-type" `
        -Title "EC11 双击重配和恢复" `
        -Action (Join-Text @(
            "保持 Type 打开。快速双击 EC11 旋钮。"
            "如果 Windows 右下角只出现一次连接通知，可以点一次；如果没有通知，打开蓝牙页观察。")) `
        -Expected (Join-Text @(
            "双击进入明确重新配对/恢复流程。"
            "不能长时间卡在已连接/未连接循环。"
            "灯效时机正确：重配提示和找 Type 提示不能互相错用。")) `
        -ObservationTemplate "弹窗：有/无；点连接结果：；Windows状态：；Type恢复：；灯效："
    New-ReviewStep `
        -Id "computer-switch-product-flow" `
        -Title "换电脑产品流程" `
        -Action (Join-Text @(
            "模拟换电脑：旧电脑如果已经手动删除，Type 不能抢回。"
            "在没有 Type 的电脑/场景里，只走 Windows 原生配对。"
            "有 Type 的新电脑打开 Type 后，再接管已配对设备。")) `
        -Expected (Join-Text @(
            "旧电脑缓存不会被旧 Type 自动抢回。"
            "无 Type 场景仍然能连键盘。"
            "有 Type 场景能恢复 BLE 控制和录音。")) `
        -ObservationTemplate "旧电脑：；无Type配对：；有Type接管：；问题："
    New-ReviewStep `
        -Id "recording-response-and-led-priority" `
        -Title "录音响应和灯效优先级" `
        -Action (Join-Text @(
            "在 Type 已连接后，按一次录音键开始，再按一次停止。"
            "重复两轮。重点看第一轮，不只看第二轮。")) `
        -Expected (Join-Text @(
            "录音胶囊快速出现。"
            "录音灯要优先覆盖旋钮白灯，不能先亮白灯很久。"
            "录音中不能中途灭一下再亮。"
            "key 灯不能出现之前类似未开 DMA 的红/绿乱闪。")) `
        -ObservationTemplate "第一轮开始响应：约  秒；第二轮：；白灯覆盖：；中途断灯：；key灯："
    New-ReviewStep `
        -Id "ble-audio-type-link" `
        -Title "BLE 音频链路和 Type-ready" `
        -Action (Join-Text @(
            "保持 Type 打开并已连接。"
            "做一次正常录音，确认 Type 收到音频/转写或明确错误。"
            "观察蓝牙灯不要显示假连接。")) `
        -Expected (Join-Text @(
            "Windows 已连接不足以通过，Type 必须真的到达 BLE 音频 GATT。"
            "录音数据能传输，Type-ready 灯效和实际 Type 连接一致。"
            "不能出现 Windows 已连接但蓝牙灯一直是未连接/配对状态。")) `
        -ObservationTemplate "Type BLE状态：；录音结果：；蓝牙灯：；日志/错误："
    New-ReviewStep `
        -Id "led-independent-contract" `
        -Title "灯效独立和防回退" `
        -Action (Join-Text @(
            "观察录音、蓝牙、EC11、key1-key4、OTA/idle 这些灯效。"
            "已验收的灯效不要重新调样式，只看是否被其它状态干扰。")) `
        -Expected (Join-Text @(
            "状态灯、旋钮灯、按键灯、边框灯相互独立。"
            "key 灯不能被 EC11/录音/蓝牙状态带着闪。"
            "已确认的蓝色双闪、找 Type、录音金色、shutdown/idle 灯效不能回退。")) `
        -ObservationTemplate "独立性：；key灯：；EC11：；BLE：；录音：；其它："
    New-ReviewStep `
        -Id "ota-wireless-smoke" `
        -Title "无线 OTA smoke" `
        -Action (Join-Text @(
            "在 Type 里选择最新 v1.0.2 OTA 包做一次 OTA preflight/探测，必要时做一次实际 OTA。"
            "如果不做完整传输，至少确认 OTA v2 GATT 可发现。")) `
        -Expected (Join-Text @(
            "OTA v2 服务和控制特征可发现。"
            "OTA 过程中设备有 OTA 灯效。"
            "界面状态、设备灯效和日志一致；不能报旧版本/低版本误判。")) `
        -ObservationTemplate "OTA包：；preflight/GATT：；传输：；灯效：；错误："
    New-ReviewStep `
        -Id "wired-flash-smoke" `
        -Title "有线刷机 smoke" `
        -Action (Join-Text @(
            "用 repo 的工具链做一次有线刷机或最短有线刷机 smoke。"
            "只能用 tools\\build.ps1 / tools\\idf.ps1 / release gate 包装脚本，不要裸跑 idf.py。")) `
        -Expected (Join-Text @(
            "有线刷机能完成，设备能重启到当前版本。"
            "不能再次出现 ESP-IDF shell 缺 esp_idf_monitor 的环境问题。"
            "刷机后蓝牙/录音/灯效仍然通过。")) `
        -ObservationTemplate "端口：；刷机结果：；启动版本：；回归现象："
    New-ReviewStep `
        -Id "release-package-final-check" `
        -Title "发布包最终检查" `
        -Action (Join-Text @(
            "只在前面全部通过后做。"
            "确认 Denzic 根目录只保留最新 Type MSI 和 Firmware OTA zip。"
            "不要保留 Type portable zip。")) `
        -Expected (Join-Text @(
            "Type MSI 是最新 v1.0.2。"
            "Firmware OTA zip 是最新 v1.0.2。"
            "根目录没有旧包或 portable 包。")) `
        -ObservationTemplate "MSI：；Firmware zip：；根目录旧包：；是否可发布："
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

@(
    "Listener 1.0.2 preproduction human review"
    "Generated: $((Get-Date).ToString('o'))"
    "Repo: $repoRoot"
    "Output: $OutputDir"
    "DeviceName: $DeviceName"
    "RandomName: $RandomName"
    "NoPrompt: $($NoPrompt.IsPresent)"
) | Set-Content -LiteralPath $startInfoPath -Encoding UTF8

$records = [System.Collections.Generic.List[object]]::new()
try {
    for ($i = 0; $i -lt $steps.Count; $i++) {
        $record = Show-ReviewStep -Index ($i + 1) -Total $steps.Count -Step $steps[$i]
        $records.Add($record) | Out-Null
        ($record | ConvertTo-Json -Depth 10 -Compress) | Add-Content -LiteralPath $sessionPath -Encoding UTF8
        if ($record.result -eq "ABORT") {
            break
        }
    }
} finally {
    if ($null -ne $singleInstanceMutex) {
        try { $singleInstanceMutex.ReleaseMutex() | Out-Null } catch {}
        $singleInstanceMutex.Dispose()
    }
}

$failCount = @($records | Where-Object { $_.result -eq "FAIL" }).Count
$incompleteCount = @($records | Where-Object { $_.result -in @("SKIP", "ABORT") }).Count
$status = if ($failCount -gt 0) {
    "HUMAN_REVIEW_FAIL"
} elseif ($incompleteCount -gt 0 -or $records.Count -ne $steps.Count) {
    "HUMAN_REVIEW_INCOMPLETE"
} else {
    "HUMAN_REVIEW_PASS"
}

[ordered]@{
    schema_version = 1
    status = $status
    generated_at = (Get-Date).ToString("o")
    repo_root = $repoRoot
    output_dir = $OutputDir
    session_jsonl = $sessionPath
    device_name = $DeviceName
    random_name = $RandomName
    records = @($records)
} | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $summaryJsonPath -Encoding UTF8

$lines = [System.Collections.Generic.List[string]]::new()
$lines.Add("# Listener 1.0.2 Preproduction Human Review") | Out-Null
$lines.Add("") | Out-Null
$lines.Add("- Status: $status") | Out-Null
$lines.Add("- Output: $OutputDir") | Out-Null
$lines.Add("- Session: $sessionPath") | Out-Null
$lines.Add("- Random BLE name: $RandomName") | Out-Null
$lines.Add("") | Out-Null
$lines.Add("| # | StepId | Step | Result | Observation |") | Out-Null
$lines.Add("|---:|---|---|---|---|") | Out-Null
foreach ($record in $records) {
    $obs = (($record.observation -replace "\|", "/") -replace "`r?`n", " ")
    $lines.Add("| $($record.index) | $($record.id) | $($record.title) | $($record.result) | $obs |") | Out-Null
}
$lines.Add("") | Out-Null
$lines.Add("Release rule: only HUMAN_REVIEW_PASS can be used as final physical acceptance evidence for publishing v1.0.2.") | Out-Null
$lines | Set-Content -LiteralPath $summaryPath -Encoding UTF8

Write-Host "preproduction_human_review_status=$status"
Write-Host "summary=$summaryPath"
Write-Host "session=$sessionPath"
if ($status -eq "HUMAN_REVIEW_PASS") {
    exit 0
}
exit 2
