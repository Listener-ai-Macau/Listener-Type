[CmdletBinding(PositionalBinding = $false)]
param(
    [string]$OutputDir = "",
    [string]$StepId = "",
    [string[]]$StepIds = @(),
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

$requestedStepIds = [System.Collections.Generic.List[string]]::new()
if (-not [string]::IsNullOrWhiteSpace($StepId)) {
    $requestedStepIds.Add($StepId.Trim()) | Out-Null
}
foreach ($rawStepIds in $StepIds) {
    foreach ($rawStepId in ([string]$rawStepIds -split ",")) {
        $trimmedStepId = $rawStepId.Trim()
        if (-not [string]::IsNullOrWhiteSpace($trimmedStepId)) {
            $requestedStepIds.Add($trimmedStepId) | Out-Null
        }
    }
}

$resumeExistingFullReview = (
    $requestedStepIds.Count -eq 0 -and
    -not $NoPrompt.IsPresent -and
    (Test-Path -LiteralPath $sessionPath) -and
    -not (Test-Path -LiteralPath $summaryJsonPath)
)

if (-not $resumeExistingFullReview) {
    foreach ($staleOutput in @($sessionPath, $summaryPath, $summaryJsonPath, $startInfoPath)) {
        Remove-Item -LiteralPath $staleOutput -Force -ErrorAction SilentlyContinue
    }
}

if ([string]::IsNullOrWhiteSpace($RandomName)) {
    $suffix = -join ((48..57 + 65..90) | Get-Random -Count 4 | ForEach-Object { [char]$_ })
    $RandomName = "listener-$suffix"
}

function Get-GitHeadInfo {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        return [PSCustomObject]@{
            path = $Path
            exists = $false
            head = ""
            commit_time = ""
            subject = ""
            error = "path not found"
        }
    }

    try {
        $head = (& git -C $Path rev-parse HEAD 2>$null)
        $commitTime = (& git -C $Path log -1 --format=%cI 2>$null)
        $subject = (& git -C $Path log -1 --format=%s 2>$null)
        return [PSCustomObject]@{
            path = $Path
            exists = $true
            head = [string]$head
            commit_time = [string]$commitTime
            subject = [string]$subject
            error = ""
        }
    } catch {
        return [PSCustomObject]@{
            path = $Path
            exists = $true
            head = ""
            commit_time = ""
            subject = ""
            error = $_.Exception.Message
        }
    }
}

$listenerRoot = (Split-Path -Parent $repoRoot)
$firmwareRoot = Join-Path $listenerRoot "Listener-Firmware"
$typeHeadInfo = Get-GitHeadInfo -Path $repoRoot
$firmwareHeadInfo = Get-GitHeadInfo -Path $firmwareRoot

function Join-Text {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Lines)
    return ($Lines -join [Environment]::NewLine)
}

function Convert-ReviewText {
    param([AllowNull()][string]$Text)
    if ($null -eq $Text) { return "" }
    return (($Text -replace "\\r\\n", [Environment]::NewLine) -replace "\\n", [Environment]::NewLine)
}

function Format-MarkdownCell {
    param([AllowNull()][string]$Text)
    if ($null -eq $Text) { return "" }
    return (($Text -replace "\|", "/") -replace "`r?`n", " ").Trim()
}

function New-ReviewStep {
    param(
        [Parameter(Mandatory = $true)][string]$Id,
        [Parameter(Mandatory = $true)][string]$Title,
        [Parameter(Mandatory = $true)][string]$Action,
        [Parameter(Mandatory = $true)][string]$Expected,
        [string]$EvidenceHint = "",
        [string]$ClipboardText = ""
    )

    [PSCustomObject]@{
        id = $Id
        title = $Title
        action = $Action
        expected = $Expected
        evidence_hint = $EvidenceHint
        clipboard_text = $ClipboardText
    }
}

function Get-CanonicalScenarioIds {
    $path = Join-Path $PSScriptRoot "listener-preproduction-scenarios.json"
    if (-not (Test-Path -LiteralPath $path)) {
        throw "Canonical preproduction scenario manifest is missing: $path"
    }
    $manifest = Get-Content -Raw -LiteralPath $path | ConvertFrom-Json
    $ids = @($manifest.scenarios | ForEach-Object { [string]$_.id })
    if ($ids.Count -eq 0) {
        throw "Canonical preproduction scenario manifest has no scenarios: $path"
    }
    return $ids
}

function Assert-StepsMatchCanonicalScenarios {
    param([Parameter(Mandatory = $true)][object[]]$ReviewSteps)

    $canonicalIds = @(Get-CanonicalScenarioIds)
    $reviewIds = @($ReviewSteps | ForEach-Object { [string]$_.id })
    $missing = @($canonicalIds | Where-Object { $reviewIds -notcontains $_ })
    $extra = @($reviewIds | Where-Object { $canonicalIds -notcontains $_ })
    $duplicates = @($reviewIds | Group-Object | Where-Object { $_.Count -gt 1 } | ForEach-Object { $_.Name })

    if ($missing.Count -gt 0 -or $extra.Count -gt 0 -or $duplicates.Count -gt 0 -or $canonicalIds.Count -ne $reviewIds.Count) {
        throw ("Human review scenario drift from canonical manifest. missing=[{0}] extra=[{1}] duplicates=[{2}] canonical_count={3} human_count={4}" -f `
            ($missing -join ","),
            ($extra -join ","),
            ($duplicates -join ","),
            $canonicalIds.Count,
            $reviewIds.Count)
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

function Get-UsbAndPortSnapshot {
    try {
        $pnpPorts = @(Get-PnpDevice -Class Ports -ErrorAction SilentlyContinue |
            Select-Object Status, Class, FriendlyName, InstanceId)
        $serialPorts = @(Get-CimInstance Win32_SerialPort -ErrorAction SilentlyContinue |
            Select-Object DeviceID, Name, Description, PNPDeviceID)
        [PSCustomObject]@{
            pnp_ports = $pnpPorts
            serial_ports = $serialPorts
        }
    } catch {
        [PSCustomObject]@{ error = $_.Exception.Message }
    }
}

function Save-DesktopScreenshot {
    param(
        [Parameter(Mandatory = $true)][int]$Index,
        [Parameter(Mandatory = $true)][string]$SafeStep,
        [Parameter(Mandatory = $true)][string]$SafePhase
    )

    $screenshotPath = Join-Path $OutputDir ("step-{0:D2}-{1}-{2}-desktop.png" -f $Index, $SafeStep, $SafePhase)
    $errorPath = Join-Path $OutputDir ("step-{0:D2}-{1}-{2}-desktop-screenshot-error.txt" -f $Index, $SafeStep, $SafePhase)
    try {
        Add-Type -AssemblyName System.Windows.Forms
        Add-Type -AssemblyName System.Drawing
        $bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
        $bitmap = [System.Drawing.Bitmap]::new($bounds.Width, $bounds.Height)
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        try {
            $graphics.CopyFromScreen($bounds.Left, $bounds.Top, 0, 0, $bounds.Size)
            $bitmap.Save($screenshotPath, [System.Drawing.Imaging.ImageFormat]::Png)
            return $screenshotPath
        } finally {
            $graphics.Dispose()
            $bitmap.Dispose()
        }
    } catch {
        $_.Exception.Message | Set-Content -LiteralPath $errorPath -Encoding UTF8
        return $errorPath
    }
}

function Get-KnownTypeLogPaths {
    $paths = [System.Collections.Generic.List[string]]::new()
    if (-not [string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        $paths.Add((Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log")) | Out-Null
        $paths.Add((Join-Path $env:LOCALAPPDATA "Listener Type\Logs\capsule-timeline.log")) | Out-Null
        $paths.Add((Join-Path $env:LOCALAPPDATA "com.listener.type\EBWebView\chrome_debug.log")) | Out-Null
    }
    return @($paths | Select-Object -Unique)
}

function Save-TypeLogTail {
    param(
        [Parameter(Mandatory = $true)][int]$Index,
        [Parameter(Mandatory = $true)][string]$SafeStep,
        [Parameter(Mandatory = $true)][string]$SafePhase
    )

    $tailPath = Join-Path $OutputDir ("step-{0:D2}-{1}-{2}-type-log-tail.txt" -f $Index, $SafeStep, $SafePhase)
    $lines = [System.Collections.Generic.List[string]]::new()
    foreach ($logPath in Get-KnownTypeLogPaths) {
        $lines.Add("===== $logPath =====") | Out-Null
        if (Test-Path -LiteralPath $logPath) {
            try {
                Get-Content -LiteralPath $logPath -Tail 260 -ErrorAction Stop |
                    ForEach-Object { $lines.Add([string]$_) | Out-Null }
            } catch {
                $lines.Add("ERROR: $($_.Exception.Message)") | Out-Null
            }
        } else {
            $lines.Add("MISSING") | Out-Null
        }
        $lines.Add("") | Out-Null
    }
    $lines | Set-Content -LiteralPath $tailPath -Encoding UTF8
    return $tailPath
}

function Save-WindowsEventSnapshot {
    param(
        [Parameter(Mandatory = $true)][int]$Index,
        [Parameter(Mandatory = $true)][string]$SafeStep,
        [Parameter(Mandatory = $true)][string]$SafePhase,
        [datetime]$StartTime = (Get-Date).AddMinutes(-2),
        [datetime]$EndTime = (Get-Date)
    )

    $eventPath = Join-Path $OutputDir ("step-{0:D2}-{1}-{2}-windows-events.txt" -f $Index, $SafeStep, $SafePhase)
    $logs = @(
        "Microsoft-Windows-Bluetooth-Policy/Operational",
        "Microsoft-Windows-Bluetooth-BthLEPrepairing/Operational",
        "Microsoft-Windows-Bluetooth-Bthmini/Operational",
        "Microsoft-Windows-DeviceSetupManager/Admin",
        "Microsoft-Windows-Kernel-PnP/Configuration"
    )
    $lines = [System.Collections.Generic.List[string]]::new()
    $lines.Add("Window: $($StartTime.ToString('o')) .. $($EndTime.ToString('o'))") | Out-Null
    foreach ($logName in $logs) {
        $lines.Add("") | Out-Null
        $lines.Add("===== $logName =====") | Out-Null
        try {
            $events = @(Get-WinEvent -FilterHashtable @{
                    LogName = $logName
                    StartTime = $StartTime
                    EndTime = $EndTime
                } -MaxEvents 160 -ErrorAction Stop |
                Sort-Object TimeCreated |
                Select-Object TimeCreated, ProviderName, Id, LevelDisplayName,
                    @{ Name = "Message"; Expression = { (($_.Message -replace "\s+", " ").Trim()) } })
            if ($events.Count -eq 0) {
                $lines.Add("NO_EVENTS") | Out-Null
            } else {
                $events | Format-List | Out-String -Width 240 |
                    ForEach-Object { $lines.Add([string]$_) | Out-Null }
            }
        } catch {
            $lines.Add("ERROR: $($_.Exception.Message)") | Out-Null
        }
    }
    $lines | Set-Content -LiteralPath $eventPath -Encoding UTF8
    return $eventPath
}

function Save-Snapshot {
    param(
        [Parameter(Mandatory = $true)][int]$Index,
        [Parameter(Mandatory = $true)][string]$StepId,
        [Parameter(Mandatory = $true)][string]$Phase,
        [datetime]$StartTime = (Get-Date).AddMinutes(-2),
        [datetime]$EndTime = (Get-Date)
    )

    $safeStep = $StepId -replace "[^A-Za-z0-9_.-]", "_"
    $safePhase = $Phase -replace "[^A-Za-z0-9_.-]", "_"
    $devicePath = Join-Path $OutputDir ("step-{0:D2}-{1}-{2}-bluetooth.txt" -f $Index, $safeStep, $safePhase)
    $processPath = Join-Path $OutputDir ("step-{0:D2}-{1}-{2}-process.txt" -f $Index, $safeStep, $safePhase)
    $usbPath = Join-Path $OutputDir ("step-{0:D2}-{1}-{2}-usb-ports.txt" -f $Index, $safeStep, $safePhase)

    Get-BluetoothDeviceSnapshot | Format-Table -AutoSize | Out-String |
        Set-Content -LiteralPath $devicePath -Encoding UTF8
    Get-TypeProcessSnapshot | Format-Table -AutoSize | Out-String |
        Set-Content -LiteralPath $processPath -Encoding UTF8
    Get-UsbAndPortSnapshot | ConvertTo-Json -Depth 8 |
        Set-Content -LiteralPath $usbPath -Encoding UTF8
    $typeLogPath = Save-TypeLogTail -Index $Index -SafeStep $safeStep -SafePhase $safePhase
    $windowsEventPath = Save-WindowsEventSnapshot -Index $Index -SafeStep $safeStep -SafePhase $safePhase -StartTime $StartTime -EndTime $EndTime
    $desktopPath = Save-DesktopScreenshot -Index $Index -SafeStep $safeStep -SafePhase $safePhase

    [pscustomobject][ordered]@{
        bluetooth = $devicePath
        process = $processPath
        usb_ports = $usbPath
        type_log_tail = $typeLogPath
        windows_events = $windowsEventPath
        desktop_screenshot = $desktopPath
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
        $endedAt = Get-Date
        return [pscustomobject][ordered]@{
            index = $Index
            id = $Step.id
            title = $Step.title
            result = "SKIP"
            operator_note = ""
            operator_action = ""
            observation = ""
            dry_run_note = "NoPrompt dry run"
            started_at = $startedAt.ToString("o")
            ended_at = $endedAt.ToString("o")
            before = $before
            during = Save-Snapshot -Index $Index -StepId $Step.id -Phase "during" -StartTime $startedAt -EndTime $endedAt
            after = Save-Snapshot -Index $Index -StepId $Step.id -Phase "after"
        }
    }

    Ensure-FormsLoaded
    Invoke-NoticeSound

    $form = [System.Windows.Forms.Form]::new()
    $form.Text = "Listener 1.0.2 准量产验收"
    $form.StartPosition = [System.Windows.Forms.FormStartPosition]::Manual
    $form.ClientSize = [System.Drawing.Size]::new(760, 540)
    $form.MinimumSize = [System.Drawing.Size]::new(720, 500)
    $form.MaximizeBox = $false
    $form.TopMost = $true
    $form.Font = [System.Drawing.Font]::new("Microsoft YaHei UI", 10)
    $form.AutoScaleMode = [System.Windows.Forms.AutoScaleMode]::Dpi
    $form.AutoScroll = $true
    $workingArea = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
    $form.Location = [System.Drawing.Point]::new(
        $workingArea.Left + 12,
        [Math]::Max($workingArea.Top + 12, $workingArea.Bottom - $form.Height - 12)
    )

    $title = [System.Windows.Forms.Label]::new()
    $title.Text = "$Index/$Total  $($Step.title)"
    $title.Font = [System.Drawing.Font]::new("Microsoft YaHei UI", 12, [System.Drawing.FontStyle]::Bold)
    $title.AutoSize = $false
    $title.Location = [System.Drawing.Point]::new(16, 12)
    $title.Size = [System.Drawing.Size]::new(728, 28)
    $form.Controls.Add($title)

    $actionLabel = [System.Windows.Forms.Label]::new()
    $actionLabel.Text = "你现在做"
    $actionLabel.AutoSize = $false
    $actionLabel.Location = [System.Drawing.Point]::new(16, 52)
    $actionLabel.Size = [System.Drawing.Size]::new(728, 20)
    $form.Controls.Add($actionLabel)

    $actionBox = [System.Windows.Forms.TextBox]::new()
    $actionBox.Multiline = $true
    $actionBox.ReadOnly = $true
    $actionBox.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
    $actionBox.Location = [System.Drawing.Point]::new(16, 74)
    $actionBox.Size = [System.Drawing.Size]::new(728, 96)
    $actionBox.Text = Convert-ReviewText $Step.action
    $form.Controls.Add($actionBox)

    $expectedLabel = [System.Windows.Forms.Label]::new()
    $expectedLabel.Text = "通过标准"
    $expectedLabel.AutoSize = $false
    $expectedLabel.Location = [System.Drawing.Point]::new(16, 178)
    $expectedLabel.Size = [System.Drawing.Size]::new(728, 20)
    $form.Controls.Add($expectedLabel)

    $expectedBox = [System.Windows.Forms.TextBox]::new()
    $expectedBox.Multiline = $true
    $expectedBox.ReadOnly = $true
    $expectedBox.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
    $expectedBox.Location = [System.Drawing.Point]::new(16, 200)
    $expectedBox.Size = [System.Drawing.Size]::new(728, 86)
    $expectedBox.Text = Convert-ReviewText $Step.expected
    $form.Controls.Add($expectedBox)

    $operatorActionLabel = [System.Windows.Forms.Label]::new()
    $operatorActionLabel.Text = "备注（可留空；有异常、疑问或小瑕疵就写下来，我会当作需求处理）"
    $operatorActionLabel.AutoSize = $false
    $operatorActionLabel.Location = [System.Drawing.Point]::new(16, 296)
    $operatorActionLabel.Size = [System.Drawing.Size]::new(728, 20)
    $form.Controls.Add($operatorActionLabel)

    $operatorAction = [System.Windows.Forms.TextBox]::new()
    $operatorAction.Multiline = $true
    $operatorAction.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
    $operatorAction.Location = [System.Drawing.Point]::new(16, 318)
    $operatorAction.Size = [System.Drawing.Size]::new(728, 126)
    $operatorAction.Anchor = [System.Windows.Forms.AnchorStyles]::Left -bor [System.Windows.Forms.AnchorStyles]::Right -bor [System.Windows.Forms.AnchorStyles]::Top -bor [System.Windows.Forms.AnchorStyles]::Bottom
    $form.Controls.Add($operatorAction)

    $buttonPanel = [System.Windows.Forms.FlowLayoutPanel]::new()
    $buttonPanel.FlowDirection = [System.Windows.Forms.FlowDirection]::RightToLeft
    $buttonPanel.WrapContents = $false
    $buttonPanel.Location = [System.Drawing.Point]::new(16, 458)
    $buttonPanel.Size = [System.Drawing.Size]::new(728, 44)
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
        $button.Size = [System.Drawing.Size]::new(94, 32)
        $button.Add_Click({
            param($sender, $eventArgs)
            $script:preproductionReviewResult = [string]$sender.Tag
            $form.Close()
        })
        $buttonPanel.Controls.Add($button)
    }

    $form.Add_Shown({
        $form.Activate()
        $operatorAction.Focus()
    })
    [void]$form.ShowDialog()

    $endedAt = Get-Date
    $during = Save-Snapshot -Index $Index -StepId $Step.id -Phase "during" -StartTime $startedAt -EndTime $endedAt
    $after = Save-Snapshot -Index $Index -StepId $Step.id -Phase "after"

    [ordered]@{
        index = $Index
        id = $Step.id
        title = $Step.title
        result = $script:preproductionReviewResult
        operator_note = $operatorAction.Text
        operator_action = $operatorAction.Text
        observation = $operatorAction.Text
        action = $Step.action
        expected = $Step.expected
        evidence_hint = $Step.evidence_hint
        started_at = $startedAt.ToString("o")
        ended_at = $endedAt.ToString("o")
        before = $before
        during = $during
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
            "Windows 蓝牙和 Type 状态稳定，不在已连接/未连接之间循环。"))
    New-ReviewStep `
        -Id "same-name-write-no-repair" `
        -Title "同名写入不重配" `
        -Action (Join-Text @(
            "在 Type 蓝牙名称里填当前 Windows 正在显示的同一个名字。"
            "点击写入。这个步骤是同名写入，不是改名。")) `
        -Expected (Join-Text @(
            "不能触发重新配对。"
            "不能弹 Windows 添加设备通知。"
            "Type 保持连接，录音通道不被破坏。"))
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
            "可以提示用户手动恢复，但不能自动 PairAsync 抢回，这用于换电脑。"))
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
            "不能卡在已连接/未连接循环。"))
    New-ReviewStep `
        -Id "type-takeover-no-forced-repair" `
        -Title "Type 接管已配对设备" `
        -Action (Join-Text @(
            "在上一项原生配对成功后，重新打开最新 Type。"
            "不要改名，不要重新配对，等待 15 秒。")) `
        -Expected (Join-Text @(
            "Type 应该接管已配对设备并恢复 BLE 控制/录音通道。"
            "不应该强制重新配对。"
            "不应该重复弹 Windows 添加设备通知。"))
    New-ReviewStep `
        -Id "ec11-long-press-shutdown-led" `
        -Title "EC11 长按关机确认灯" `
        -Action (Join-Text @(
            "保持 Type 打开或关闭都可以，但不要点击 Windows 蓝牙弹窗。"
            "长按 EC11 旋钮约 1.2 到 1.5 秒，看到确认灯后松开；不要按到硬件关机边界。"
            "只做这一次长按，不要单击或双击。重点观察 PWR/状态灯、EC11 旋钮环、key1-key4 和 Type 是否像重启/重建。")) `
        -Expected (Join-Text @(
            "长按期间进入 1.0.1 已验收的关机确认灯效：warm amber PWR/状态灯亮起，EC11 旋钮环用同色按顺时针方向约 1.2 秒走满/亮满。"
            "松开后退出关机确认，不应该触发单击录音、双击重配或 Windows 连接通知。"
            "Type 不应该重启、白屏、反复重建后台录音，也不应该把长按识别成多个 EC11 单击。"
            "key1-key4 不能被带着红/绿乱闪。"))
    New-ReviewStep `
        -Id "ec11-rotate-ring-feedback" `
        -Title "EC11 旋转环形追光" `
        -Action (Join-Text @(
            "保持空闲，不要录音，不要 OTA。"
            "只转 EC11 外圈，不要向下按。"
            "顺时针慢慢转 EC11 旋钮一整圈，再逆时针慢慢转一整圈。"
            "重点看 EC11 旋钮环，不看 Windows 音量是否变化。")) `
        -Expected (Join-Text @(
            "旋转时 EC11 环出现白色方向性追光/转一圈反馈，保持约 2 到 3 秒。"
            "顺时针和逆时针方向不能反，不能只闪一下就没了。"
            "如果误按下 EC11，或者日志/实际行为只有 EC11 push、long press、shutdown_confirm、warm amber 关机确认灯，本步骤必须重来，不能算通过。"
            "PWR/BLE/REC/AI/key1-key4 不应该被带着乱闪；录音态下录音灯优先覆盖旋钮白色反馈是允许的。"))
    New-ReviewStep `
        -Id "ec11-single-not-double" `
        -Title "EC11 单击不误判双击" `
        -Action (Join-Text @(
            "单击一次 EC11 旋钮。"
            "不要快速双击。观察 Windows、Type 和蓝牙灯。")) `
        -Expected (Join-Text @(
            "只触发单击/本地反馈。"
            "不能打开重配流程，不能弹 Windows 连接通知。"
            "EC11 的按键行为要和 key1-key4 的单/双击窗口一致。"))
    New-ReviewStep `
        -Id "ec11-double-repair-with-type" `
        -Title "有 Type 的双击重配" `
        -Action (Join-Text @(
            "保持 Type 打开。快速双击 EC11 旋钮。"
            "等 Type 先清理这台电脑上的旧 Listener 配对。"
            "看到 Windows 右下角连接通知后，只点一次连接；如果没有通知，就打开 Windows 蓝牙页手动添加当前 Listener 名字。"
            "不要在 Type 里点别的恢复按钮，不要重复双击。")) `
        -Expected (Join-Text @(
            "Type 只负责清理旧配对和等待确认，不能自己 PairAsync 抢配。"
            "Windows 连接通知最多出现一次；点连接或手动添加后应成功。"
            "如果未清旧配对就直接点连接，出现连接失败不能算通过。"
            "不能长时间卡在已连接/未连接循环。"
            "灯效时机正确：重配提示和找 Type 提示不能互相错用。"
            "Type 必须在真实 BLE GATT 恢复后再显示已恢复。"))
    New-ReviewStep `
        -Id "computer-switch-product-flow" `
        -Title "换电脑产品流程" `
        -Action (Join-Text @(
            "模拟换电脑/没有 Type：先退出 Type。"
            "如果这台 Windows 里已经有旧 Listener 条目，先手动删除旧设备；不删除直接点连接失败是预期问题，不算通过。"
            "删除旧设备后，用 Windows 原生添加设备或右下角连接通知配对 Listener。"
            "再模拟有 Type 的电脑：打开 Type，确认它只接管已配对设备，不抢回旧电脑。")) `
        -Expected (Join-Text @(
            "没有 Type 的电脑也能作为普通蓝牙键盘配对，但旧缓存必须由用户自己删除。"
            "有 Type 的电脑可以帮本机清旧配对，但仍需要用户点 Windows 原生连接。"
            "旧电脑缓存不会被旧 Type 自动抢回。"
            "有 Type 场景能恢复 BLE 控制和录音。"))
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
            "key 灯不能出现之前类似未开 DMA 的红/绿乱闪。"))
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
            "不能出现 Windows 已连接但蓝牙灯一直是未连接/配对状态。"))
    New-ReviewStep `
        -Id "led-independent-contract" `
        -Title "灯效独立和防回退" `
        -Action (Join-Text @(
            "观察录音、蓝牙、EC11、key1-key4、OTA/idle 这些灯效。"
            "已验收的灯效不要重新调样式，只看是否被其它状态干扰。")) `
        -Expected (Join-Text @(
            "状态灯、旋钮灯、按键灯、边框灯相互独立。"
            "key 灯不能被 EC11/录音/蓝牙状态带着闪。"
            "已确认的蓝色双闪、找 Type、录音金色、shutdown/idle 灯效不能回退。"))
    New-ReviewStep `
        -Id "ota-wireless-smoke" `
        -Title "无线 OTA smoke" `
        -Action (Join-Text @(
            "在 Type 里选择最新 v1.0.2 OTA 包做一次 OTA preflight/探测，必要时做一次实际 OTA。"
            "如果不做完整传输，至少确认 OTA v2 GATT 可发现。")) `
        -Expected (Join-Text @(
            "OTA v2 服务和控制特征可发现。"
            "OTA 过程中设备有 OTA 灯效。"
            "界面状态、设备灯效和日志一致；不能报旧版本/低版本误判。"))
    New-ReviewStep `
        -Id "wired-flash-smoke" `
        -Title "有线刷机 smoke" `
        -Action (Join-Text @(
            "用 repo 的工具链做一次有线刷机或最短有线刷机 smoke。"
            "只能用 tools\\build.ps1 / tools\\idf.ps1 / release gate 包装脚本，不要裸跑 idf.py。")) `
        -Expected (Join-Text @(
            "有线刷机能完成，设备能重启到当前版本。"
            "不能再次出现 ESP-IDF shell 缺 esp_idf_monitor 的环境问题。"
            "刷机后蓝牙/录音/灯效仍然通过。"))
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
            "根目录没有旧包或 portable 包。"))
)
Assert-StepsMatchCanonicalScenarios -ReviewSteps $steps
$allSteps = @($steps)

function Find-CarryForwardHumanSummary {
    param(
        [Parameter(Mandatory = $true)][string[]]$RequiredStepIds,
        [Parameter(Mandatory = $true)][string[]]$FocusStepIds
    )

    $roots = @(
        (Join-Path $repoRoot ".cache\validation"),
        (Join-Path $repoRoot ".artifacts\v1.0.2-regression")
    ) | Where-Object { Test-Path -LiteralPath $_ }

    $candidates = foreach ($root in $roots) {
        Get-ChildItem -LiteralPath $root -Recurse -File -Filter "preproduction-human-review-summary.json" -ErrorAction SilentlyContinue
    }

    foreach ($candidate in @($candidates | Sort-Object LastWriteTime -Descending)) {
        try {
            $summary = Get-Content -Raw -LiteralPath $candidate.FullName | ConvertFrom-Json
        } catch {
            continue
        }
        if ($summary.status -ne "HUMAN_REVIEW_PASS") {
            continue
        }

        $records = @($summary.records)
        $recordById = @{}
        foreach ($record in $records) {
            $recordById[[string]$record.id] = $record
        }

        $usable = $true
        foreach ($stepId in $RequiredStepIds) {
            if ($FocusStepIds -contains $stepId) {
                continue
            }
            if (-not $recordById.ContainsKey($stepId)) {
                $usable = $false
                break
            }
            if ($recordById[$stepId].result -ne "PASS") {
                $usable = $false
                break
            }
        }
        if ($usable) {
            return [pscustomobject]@{
                path = $candidate.FullName
                summary = $summary
            }
        }
    }

    return $null
}

function Copy-ReviewRecordForSummary {
    param(
        [Parameter(Mandatory = $true)]$Record,
        [Parameter(Mandatory = $true)][int]$Index,
        [bool]$CarriedForward = $false,
        [string]$CarriedForwardFrom = ""
    )

    $copy = [ordered]@{}
    if ($Record -is [System.Collections.IDictionary]) {
        foreach ($key in $Record.Keys) {
            $copy[[string]$key] = $Record[$key]
        }
    } else {
        $keysProperty = $Record.PSObject.Properties["Keys"]
        $valuesProperty = $Record.PSObject.Properties["Values"]
        if ($keysProperty -and $valuesProperty) {
            $keyList = @($keysProperty.Value)
            $valueList = @($valuesProperty.Value)
        } else {
            $keyList = @()
            $valueList = @()
        }
        if (($keyList -contains "id") -and $keyList.Count -eq $valueList.Count) {
            for ($recordFieldIndex = 0; $recordFieldIndex -lt $keyList.Count; $recordFieldIndex++) {
                $copy[[string]$keyList[$recordFieldIndex]] = $valueList[$recordFieldIndex]
            }
        } else {
            foreach ($property in $Record.PSObject.Properties) {
                $copy[$property.Name] = $property.Value
            }
        }
    }
    $copy["index"] = $Index
    if ($CarriedForward) {
        $copy["carried_forward"] = $true
        $copy["carried_forward_from_summary"] = $CarriedForwardFrom
        $copy["carried_forward_at"] = (Get-Date).ToString("o")
        $copy["carried_forward_reason"] = "Previously PASS and not selected for this focused re-review."
    }
    return [pscustomobject]$copy
}

function Get-ReviewRecordField {
    param(
        [Parameter(Mandatory = $true)]$Record,
        [Parameter(Mandatory = $true)][string]$Name
    )

    if ($Record -is [System.Collections.IDictionary]) {
        if ($Record.Contains($Name)) {
            return $Record[$Name]
        }
    }

    $property = $Record.PSObject.Properties[$Name]
    if ($property) {
        return $property.Value
    }

    $keysProperty = $Record.PSObject.Properties["Keys"]
    $valuesProperty = $Record.PSObject.Properties["Values"]
    if ($keysProperty -and $valuesProperty) {
        $keyList = @($keysProperty.Value)
        $valueList = @($valuesProperty.Value)
        for ($recordFieldIndex = 0; $recordFieldIndex -lt $keyList.Count; $recordFieldIndex++) {
            if ([string]$keyList[$recordFieldIndex] -eq $Name) {
                return $valueList[$recordFieldIndex]
            }
        }
    }

    $syncRoot = $Record.PSObject.Properties["SyncRoot"]
    if ($syncRoot -and $null -ne $syncRoot.Value -and $syncRoot.Value -ne $Record) {
        return Get-ReviewRecordField -Record $syncRoot.Value -Name $Name
    }

    return $null
}

function Get-OperatorNoteText {
    param([Parameter(Mandatory = $true)]$Record)

    foreach ($fieldName in @("operator_note", "operator_action", "observation")) {
        $value = Get-ReviewRecordField -Record $Record -Name $fieldName
        $text = ([string]$value).Trim()
        if ($text -eq "NoPrompt dry run") {
            continue
        }
        if (-not [string]::IsNullOrWhiteSpace($text)) {
            return $text
        }
    }
    return ""
}

function Get-TextSha256 {
    param([string]$Text = "")

    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($Text)
        return ([BitConverter]::ToString($sha.ComputeHash($bytes)) -replace "-", "").ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
}

if ($ListSteps.IsPresent) {
    foreach ($step in $steps) {
        Write-Output ("{0}`t{1}" -f $step.id, $step.title)
    }
    exit 0
}

if ($requestedStepIds.Count -gt 0) {
    $knownStepIds = @{}
    foreach ($step in $steps) {
        $knownStepIds[[string]$step.id] = $true
    }
    $unknownStepIds = @(
        $requestedStepIds |
            Select-Object -Unique |
            Where-Object { -not $knownStepIds.ContainsKey([string]$_) }
    )
    if ($unknownStepIds.Count -gt 0) {
        throw "Unknown StepIds '$($unknownStepIds -join ', ')'. Use -ListSteps to see valid steps."
    }
    $selectedSteps = @($steps | Where-Object { $requestedStepIds -contains [string]$_.id })
    if ($selectedSteps.Count -eq 0) {
        throw "No steps selected. Use -ListSteps to see valid steps."
    }
    $steps = $selectedSteps
}

$carryForwardSummary = $null
if ($requestedStepIds.Count -gt 0) {
    $carryForwardSummary = Find-CarryForwardHumanSummary `
        -RequiredStepIds @($allSteps | ForEach-Object { [string]$_.id }) `
        -FocusStepIds @($requestedStepIds)
}

$existingRecords = @()
if ($resumeExistingFullReview) {
    $existingRecords = @(
        Get-Content -LiteralPath $sessionPath -ErrorAction SilentlyContinue |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
            ForEach-Object { $_ | ConvertFrom-Json }
    )

    if ($existingRecords.Count -gt $steps.Count) {
        throw "Existing review session has more records ($($existingRecords.Count)) than current steps ($($steps.Count)). Use a new OutputDir."
    }

    for ($j = 0; $j -lt $existingRecords.Count; $j++) {
        if ($existingRecords[$j].id -ne $steps[$j].id) {
            throw "Existing review session step $($j + 1) is '$($existingRecords[$j].id)', expected '$($steps[$j].id)'. Use a new OutputDir."
        }
    }
}

$startLines = @(
    "Listener 1.0.2 preproduction human review"
    "Generated: $((Get-Date).ToString('o'))"
    "Repo: $repoRoot"
    "Output: $OutputDir"
    "DeviceName: $DeviceName"
    "RandomName: $RandomName"
    "NoPrompt: $($NoPrompt.IsPresent)"
    "FocusStepIds: $(if ($requestedStepIds.Count -gt 0) { ($requestedStepIds -join ',') } else { 'FULL' })"
    "CarryForwardSummary: $(if ($carryForwardSummary) { $carryForwardSummary.path } else { 'NONE' })"
    "TypeHead: $($typeHeadInfo.head) $($typeHeadInfo.commit_time) $($typeHeadInfo.subject)"
    "FirmwareHead: $($firmwareHeadInfo.head) $($firmwareHeadInfo.commit_time) $($firmwareHeadInfo.subject)"
)
if ($resumeExistingFullReview) {
    $startLines += "ResumedExistingRecords: $($existingRecords.Count)"
    $startLines | Add-Content -LiteralPath $startInfoPath -Encoding UTF8
} else {
    $startLines | Set-Content -LiteralPath $startInfoPath -Encoding UTF8
}

$records = [System.Collections.Generic.List[object]]::new()
try {
    foreach ($existingRecord in $existingRecords) {
        $records.Add($existingRecord) | Out-Null
    }

    for ($i = $existingRecords.Count; $i -lt $steps.Count; $i++) {
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

$summaryRecords = [System.Collections.Generic.List[object]]::new()
$carriedForwardCount = 0
if ($requestedStepIds.Count -gt 0) {
    $newRecordById = @{}
    foreach ($record in $records) {
        $newRecordById[[string]$record.id] = $record
    }
    $carryRecordById = @{}
    if ($carryForwardSummary) {
        foreach ($record in @($carryForwardSummary.summary.records)) {
            $carryRecordById[[string]$record.id] = $record
        }
    }

    for ($summaryIndex = 0; $summaryIndex -lt $allSteps.Count; $summaryIndex++) {
        $stepId = [string]$allSteps[$summaryIndex].id
        if ($newRecordById.ContainsKey($stepId)) {
            $summaryRecords.Add((Copy-ReviewRecordForSummary -Record $newRecordById[$stepId] -Index ($summaryIndex + 1))) | Out-Null
        } elseif ($carryRecordById.ContainsKey($stepId)) {
            $summaryRecords.Add((Copy-ReviewRecordForSummary -Record $carryRecordById[$stepId] -Index ($summaryIndex + 1) -CarriedForward $true -CarriedForwardFrom $carryForwardSummary.path)) | Out-Null
            $carriedForwardCount += 1
        }
    }
} else {
    foreach ($record in $records) {
        $summaryRecords.Add($record) | Out-Null
    }
}

$summaryRecordArray = @(
    for ($summaryRecordIndex = 0; $summaryRecordIndex -lt $summaryRecords.Count; $summaryRecordIndex++) {
        $item = $summaryRecords[$summaryRecordIndex]
        if ($item -is [System.Array]) {
            foreach ($innerItem in $item) {
                $innerItem
            }
        } else {
            $item
        }
    }
)
$expectedRecordCount = if ($requestedStepIds.Count -gt 0) { $allSteps.Count } else { $steps.Count }
$recordsWithResult = @($summaryRecordArray | Where-Object { $null -ne $_.PSObject.Properties["result"] })
$missingResultCount = $summaryRecordArray.Count - $recordsWithResult.Count
$failCount = @($recordsWithResult | Where-Object { $_.result -eq "FAIL" }).Count
$incompleteCount = @($recordsWithResult | Where-Object { $_.result -in @("SKIP", "ABORT") }).Count + $missingResultCount
$status = if ($failCount -gt 0) {
    "HUMAN_REVIEW_FAIL"
} elseif ($incompleteCount -gt 0 -or $summaryRecordArray.Count -ne $expectedRecordCount) {
    "HUMAN_REVIEW_INCOMPLETE"
} else {
    "HUMAN_REVIEW_PASS"
}

$operatorNotes = [System.Collections.Generic.List[object]]::new()
foreach ($record in $summaryRecordArray) {
    $noteText = (Get-OperatorNoteText -Record $record).Trim()
    if ([string]::IsNullOrWhiteSpace($noteText)) {
        continue
    }
    $operatorNotes.Add([pscustomobject][ordered]@{
        id = [string](Get-ReviewRecordField -Record $record -Name "id")
        title = [string](Get-ReviewRecordField -Record $record -Name "title")
        result = [string](Get-ReviewRecordField -Record $record -Name "result")
        carried_forward = [bool](Get-ReviewRecordField -Record $record -Name "carried_forward")
        operator_note = $noteText
        operator_note_sha256 = Get-TextSha256 $noteText
    }) | Out-Null
}
$operatorNoteArray = @(
    for ($operatorNoteIndex = 0; $operatorNoteIndex -lt $operatorNotes.Count; $operatorNoteIndex++) {
        $operatorNotes[$operatorNoteIndex]
    }
)
$triageTemplatePath = Join-Path $OutputDir "preproduction-operator-note-triage.template.json"
$triageTemplate = [ordered]@{
    schema_version = 1
    status = "PENDING"
    source_summary = $summaryJsonPath
    generated_at = (Get-Date).ToString("o")
    rule = "Blank operator_note means the step had no extra operator remarks. Every non-empty operator note is treated as a human prompt, not a keyword hint. Read the whole note, split every requested action, uncertainty, or observation into work items, then decide whether each item needs a fix, can be accepted_benign with evidence, or must be deferred_by_human."
    operator_notes = @(
        foreach ($noteRecord in $operatorNoteArray) {
            [ordered]@{
                id = $noteRecord.id
                title = $noteRecord.title
                result = $noteRecord.result
                carried_forward = $noteRecord.carried_forward
                operator_note_sha256 = $noteRecord.operator_note_sha256
                operator_note = $noteRecord.operator_note
                operator_note_acknowledged = $false
                disposition = "open"
                evidence = @()
                parsed_requests = @()
                notes = "Treat operator_note as prompt text. Do not rely on keyword matching; preserve and address the full meaning."
            }
        }
    )
}
$triageTemplate | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $triageTemplatePath -Encoding UTF8

[ordered]@{
    schema_version = 1
    status = $status
    generated_at = (Get-Date).ToString("o")
    repo_root = $repoRoot
    output_dir = $OutputDir
    session_jsonl = $sessionPath
    device_name = $DeviceName
    random_name = $RandomName
    focus_step_ids = @($requestedStepIds)
    carried_forward_summary = $(if ($carryForwardSummary) { $carryForwardSummary.path } else { "" })
    carried_forward_count = $carriedForwardCount
    operator_note_review_status = $(if ($operatorNoteArray.Count -gt 0) { "PENDING" } else { "PASS" })
    operator_note_review_required_count = $operatorNoteArray.Count
    operator_note_recorded_count = $operatorNoteArray.Count
    operator_note_triage_template = $triageTemplatePath
    operator_notes = @($operatorNoteArray)
    type_git_head = $typeHeadInfo
    firmware_git_head = $firmwareHeadInfo
    records = @($summaryRecordArray)
} | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $summaryJsonPath -Encoding UTF8

$lines = [System.Collections.Generic.List[string]]::new()
$lines.Add("# Listener 1.0.2 Preproduction Human Review") | Out-Null
$lines.Add("") | Out-Null
$lines.Add("- Status: $status") | Out-Null
$lines.Add("- Output: $OutputDir") | Out-Null
$lines.Add("- Session: $sessionPath") | Out-Null
$lines.Add("- Random BLE name: $RandomName") | Out-Null
$lines.Add("- Focus steps: $(if ($requestedStepIds.Count -gt 0) { $requestedStepIds -join ', ' } else { 'FULL' })") | Out-Null
$lines.Add("- Carried forward: $carriedForwardCount") | Out-Null
$lines.Add("- Operator notes requiring triage: $($operatorNoteArray.Count)") | Out-Null
$lines.Add("- Blank operator notes mean normal pass with no extra remarks.") | Out-Null
$lines.Add("- Operator note triage template: $triageTemplatePath") | Out-Null
$lines.Add("- Type HEAD: $($typeHeadInfo.head) $($typeHeadInfo.commit_time)") | Out-Null
$lines.Add("- Firmware HEAD: $($firmwareHeadInfo.head) $($firmwareHeadInfo.commit_time)") | Out-Null
$lines.Add("") | Out-Null
$lines.Add("| # | StepId | Step | Result | Operator note | Evidence |") | Out-Null
$lines.Add("|---:|---|---|---|---|---|") | Out-Null
foreach ($record in $summaryRecordArray) {
    $operatorNoteValue = Get-OperatorNoteText -Record $record
    $operatorNote = Format-MarkdownCell $operatorNoteValue
    $isCarriedForward = [bool](Get-ReviewRecordField -Record $record -Name "carried_forward")
    $evidence = if ($isCarriedForward) { "carried forward from previous PASS summary" } else { "before/during/after logs in output dir" }
    $recordIndex = Get-ReviewRecordField -Record $record -Name "index"
    $recordId = Get-ReviewRecordField -Record $record -Name "id"
    $recordTitle = Get-ReviewRecordField -Record $record -Name "title"
    $recordResult = Get-ReviewRecordField -Record $record -Name "result"
    $lines.Add("| $recordIndex | $recordId | $recordTitle | $recordResult | $operatorNote | $evidence |") | Out-Null
}
$lines.Add("") | Out-Null
$lines.Add("Each step JSON record contains before/during/after snapshots with Bluetooth PnP, Type process, USB/serial, desktop screenshot, Listener Type log tail, capsule timeline tail, and Windows Bluetooth/device event logs.") | Out-Null
$lines.Add("") | Out-Null
$lines.Add("Release rule: HUMAN_REVIEW_PASS alone is not publishable when any non-empty operator note exists. Every non-empty note must be triaged as full prompt text in preproduction-operator-note-triage.json with evidence before release; blank notes mean normal pass with no extra remarks.") | Out-Null
$lines | Set-Content -LiteralPath $summaryPath -Encoding UTF8

Write-Host "preproduction_human_review_status=$status"
Write-Host "summary=$summaryPath"
Write-Host "session=$sessionPath"
if ($status -eq "HUMAN_REVIEW_PASS") {
    exit 0
}
exit 2
