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
    $OutputDir = Join-Path $repoRoot ".cache\validation\ble-user-scenarios-human-review-$stamp"
} elseif (-not [System.IO.Path]::IsPathRooted($OutputDir)) {
    $OutputDir = Join-Path $repoRoot $OutputDir
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$OutputDir = (Resolve-Path -LiteralPath $OutputDir).Path

if ([string]::IsNullOrWhiteSpace($LogPath)) {
    $LogPath = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
}

$sessionPath = Join-Path $OutputDir "ble-user-scenarios-session.jsonl"
$summaryPath = Join-Path $OutputDir "ble-user-scenarios-summary.md"
$summaryJsonPath = Join-Path $OutputDir "ble-user-scenarios-summary.json"

$mutexCreated = $false
$mutex = [System.Threading.Mutex]::new(
    $true,
    "Global\Denzic.Listener.BleUserScenariosHumanReview",
    [ref]$mutexCreated)
if (-not $mutexCreated) {
    Write-Warning "Another BLE user-scenarios review window is already running."
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

function Get-TypeProcessSnapshot {
    $processes = @(Get-Process -ErrorAction SilentlyContinue | Where-Object {
        $_.ProcessName -match "listener|type" -or $_.MainWindowTitle -match "Listener Type|com.listener.type"
    } | Select-Object Id, ProcessName, MainWindowTitle, Responding, StartTime)
    return $processes
}

function Get-BleDeviceSnapshot {
    $devices = @(Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue | Where-Object {
        $_.FriendlyName -match "listener|Billy|Blistener|Bluetooth|HID Keyboard|GATT" -or
        $_.InstanceId -match "BTHLE|HID\\.*BTH"
    } | Sort-Object Class, FriendlyName | Select-Object Status, Class, FriendlyName, InstanceId)
    return $devices
}

function Save-StepEvidence {
    param(
        [Parameter(Mandatory = $true)][int]$Index,
        [Parameter(Mandatory = $true)][string]$Phase,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][object[]]$Devices,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][object[]]$Processes
    )

    $safePhase = $Phase -replace "[^A-Za-z0-9_-]", "_"
    $devicePath = Join-Path $OutputDir ("step-{0:D2}-{1}-pnp.txt" -f $Index, $safePhase)
    $processPath = Join-Path $OutputDir ("step-{0:D2}-{1}-process.txt" -f $Index, $safePhase)
    $Devices | Format-Table -AutoSize | Out-String | Set-Content -LiteralPath $devicePath -Encoding UTF8
    $Processes | Format-Table -AutoSize | Out-String | Set-Content -LiteralPath $processPath -Encoding UTF8
    return [PSCustomObject]@{
        devices = $devicePath
        processes = $processPath
    }
}

function Show-Intro {
    if ($NoPrompt.IsPresent) {
        return $true
    }
    Ensure-FormsLoaded
    Invoke-ReviewSound
    $message = @"
这次是 Listener 蓝牙用户场景验收，会覆盖真实用户会遇到的路径。

会做这些场景：
1. 当前已配对连接状态基线。
2. Type 开着时，用户手动删除 Windows 配对，Type 不能自动抢回。
3. Type 开着时，用户手动添加回来，Type 可以重新接管。
4. 先退出 Type，避免它自动接管。
5. 没有 Type 时，只用 Windows 原生蓝牙做首次/干净配对。
6. 没有 Type 时，确认蓝牙键盘连接能保持。
7. Type 重新接管已经配好的设备。
8. 写入同名蓝牙名，不应该重新配对。
9. 写入随机蓝牙名，名字要真实显示为新名字。
10. 恢复默认名字 listener。
11. 双击旋钮重新配对。
12. Type 重启、断电或 idle 后恢复。
13. 最后再做一次录音，确认 BLE 音频链路没被配对流程破坏。

有些步骤会断开当前蓝牙，Windows 可能出现系统连接弹窗。每一步按窗口提示做；看到成功就点通过，失败就填现象点失败，不方便测的场景点跳过。
"@
    $result = [System.Windows.Forms.MessageBox]::Show(
        (Convert-PromptText $message),
        "Listener 蓝牙验收",
        [System.Windows.Forms.MessageBoxButtons]::OKCancel,
        [System.Windows.Forms.MessageBoxIcon]::Information)
    return $result -eq [System.Windows.Forms.DialogResult]::OK
}

function Show-Step {
    param(
        [Parameter(Mandatory = $true)][int]$Index,
        [Parameter(Mandatory = $true)][int]$Total,
        [Parameter(Mandatory = $true)][string]$Title,
        [Parameter(Mandatory = $true)][string]$Action,
        [Parameter(Mandatory = $true)][string]$Expected,
        [string]$DefaultObservation = "",
        [string]$ClipboardText = "",
        [switch]$EnableNotificationHelper,
        [string]$NotificationTargetName = "listener"
    )

    $startAt = Get-Date
    $logOffset = Get-LogLength
    $beforeDevices = @(Get-BleDeviceSnapshot)
    $beforeProcesses = @(Get-TypeProcessSnapshot)
    $beforeEvidence = Save-StepEvidence -Index $Index -Phase "before" -Devices $beforeDevices -Processes $beforeProcesses

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
            before_evidence = $beforeEvidence
            after_evidence = $null
            notification_helper = @()
        }
    }

    Ensure-FormsLoaded
    Invoke-ReviewSound

    if (-not [string]::IsNullOrWhiteSpace($ClipboardText)) {
        try {
            [System.Windows.Forms.Clipboard]::SetText($ClipboardText)
        } catch {
        }
    }

    $notificationRuns = [System.Collections.Generic.List[object]]::new()
    $form = [System.Windows.Forms.Form]::new()
    try {
        $form.Text = "蓝牙验收 $Index/$Total"
        $form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
        $form.TopMost = $true
        $form.ShowInTaskbar = $true
        $form.AutoScaleMode = [System.Windows.Forms.AutoScaleMode]::Dpi
        $form.ClientSize = [System.Drawing.Size]::new(720, 520)
        $form.MinimumSize = [System.Drawing.Size]::new(640, 460)
        $form.Font = [System.Drawing.Font]::new("Microsoft YaHei UI", 9)

        $titleLabel = [System.Windows.Forms.Label]::new()
        $titleLabel.Text = "$Index/$Total  $Title"
        $titleLabel.Left = 14
        $titleLabel.Top = 12
        $titleLabel.Width = $form.ClientSize.Width - 28
        $titleLabel.Height = 28
        $titleLabel.Font = [System.Drawing.Font]::new("Microsoft YaHei UI", 11, [System.Drawing.FontStyle]::Bold)
        $titleLabel.Anchor = "Top,Left,Right"
        $form.Controls.Add($titleLabel)

        $actionLabel = [System.Windows.Forms.Label]::new()
        $actionLabel.Text = "现在做："
        $actionLabel.Left = 14
        $actionLabel.Top = 48
        $actionLabel.Width = 120
        $actionLabel.Height = 20
        $form.Controls.Add($actionLabel)

        $actionBox = [System.Windows.Forms.TextBox]::new()
        $actionBox.Left = 14
        $actionBox.Top = 70
        $actionBox.Width = $form.ClientSize.Width - 28
        $actionBox.Height = 110
        $actionBox.Multiline = $true
        $actionBox.ReadOnly = $true
        $actionBox.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
        $actionBox.Text = Convert-PromptText $Action
        $actionBox.Anchor = "Top,Left,Right"
        $form.Controls.Add($actionBox)

        $expectedLabel = [System.Windows.Forms.Label]::new()
        $expectedLabel.Text = "通过标准："
        $expectedLabel.Left = 14
        $expectedLabel.Top = 190
        $expectedLabel.Width = 120
        $expectedLabel.Height = 20
        $form.Controls.Add($expectedLabel)

        $expectedBox = [System.Windows.Forms.TextBox]::new()
        $expectedBox.Left = 14
        $expectedBox.Top = 212
        $expectedBox.Width = $form.ClientSize.Width - 28
        $expectedBox.Height = 82
        $expectedBox.Multiline = $true
        $expectedBox.ReadOnly = $true
        $expectedBox.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
        $expectedBox.Text = Convert-PromptText $Expected
        $expectedBox.Anchor = "Top,Left,Right"
        $form.Controls.Add($expectedBox)

        $label = [System.Windows.Forms.Label]::new()
        $label.Text = "你看到的现象："
        $label.Left = 14
        $label.Top = 306
        $label.Width = 180
        $label.Height = 22
        $form.Controls.Add($label)

        $observation = [System.Windows.Forms.TextBox]::new()
        $observation.Left = 14
        $observation.Top = 330
        $observation.Width = $form.ClientSize.Width - 28
        $observation.Height = 118
        $observation.Multiline = $true
        $observation.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
        $observation.Text = Convert-PromptText $DefaultObservation
        $observation.Anchor = "Top,Bottom,Left,Right"
        $form.Controls.Add($observation)

        $holder = @{ result = "ABORT" }

        $abortButton = [System.Windows.Forms.Button]::new()
        $abortButton.Text = "停止"
        $abortButton.Width = 82
        $abortButton.Height = 32
        $abortButton.Left = 14
        $abortButton.Top = $form.ClientSize.Height - 48
        $abortButton.Anchor = "Left,Bottom"
        $abortButton.Add_Click({ $holder.result = "ABORT"; $form.Close() })
        $form.Controls.Add($abortButton)

        $notifyButton = [System.Windows.Forms.Button]::new()
        $notifyButton.Text = "扫/点 Win 通知"
        $notifyButton.Width = 136
        $notifyButton.Height = 32
        $notifyButton.Left = 106
        $notifyButton.Top = $form.ClientSize.Height - 48
        $notifyButton.Anchor = "Left,Bottom"
        $notifyButton.Enabled = $EnableNotificationHelper.IsPresent
        $notifyButton.Add_Click({
            $notifyButton.Enabled = $false
            try {
                Invoke-ReviewSound
                $helper = Join-Path $repoRoot "scripts\windows-ble-notification-helper.ps1"
                $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
                $helperOut = Join-Path $OutputDir ("step-{0:D2}-windows-notification-{1}.json" -f $Index, $stamp)
                $helperOutput = & pwsh -NoProfile -File $helper `
                    -Action ClickConnect `
                    -TargetName $NotificationTargetName `
                    -TimeoutSeconds 3 `
                    -MaxElements 1800 `
                    -OutputPath $helperOut 2>&1
                $exitCode = $LASTEXITCODE
                $run = [PSCustomObject]@{
                    at = (Get-Date).ToString("o")
                    target = $NotificationTargetName
                    exit_code = $exitCode
                    output_path = $helperOut
                    console = (($helperOutput | ForEach-Object { [string]$_ }) -join "`n")
                }
                $notificationRuns.Add($run) | Out-Null
                $observation.AppendText((Convert-PromptText ("`r`n[AI通知助手] target={0} exit={1} output={2}`r`n{3}`r`n" -f $NotificationTargetName, $exitCode, $helperOut, $run.console)))
            } catch {
                $notificationRuns.Add([PSCustomObject]@{
                    at = (Get-Date).ToString("o")
                    target = $NotificationTargetName
                    exit_code = -1
                    output_path = ""
                    console = $_.Exception.Message
                }) | Out-Null
                $observation.AppendText((Convert-PromptText ("`r`n[AI通知助手] 失败：{0}`r`n" -f $_.Exception.Message)))
            } finally {
                $notifyButton.Enabled = $EnableNotificationHelper.IsPresent
                $form.Activate()
                $form.BringToFront()
            }
        })
        $form.Controls.Add($notifyButton)

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
        $afterDevices = @(Get-BleDeviceSnapshot)
        $afterProcesses = @(Get-TypeProcessSnapshot)
        $afterEvidence = Save-StepEvidence -Index $Index -Phase "after" -Devices $afterDevices -Processes $afterProcesses
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
            before_evidence = $beforeEvidence
            after_evidence = $afterEvidence
            notification_helper = @($notificationRuns)
        }
    } finally {
        $form.Dispose()
    }
}

try {
    if (-not (Show-Intro)) {
        Write-Host "ble_user_scenarios_status=ABORTED output_dir=$OutputDir"
        exit 2
    }

    $randomSuffix = -join ((48..57 + 65..90) | Get-Random -Count 4 | ForEach-Object { [char]$_ })
    $randomName = "listener-$randomSuffix"

    $steps = @(
        [PSCustomObject]@{
            title = "当前连接基线"
            action = "确认 Type 已经打开在托盘，Windows 蓝牙里当前 Listener 设备显示已连接。`r`n`r`n不要操作设备，先观察 10 秒：Windows 不要在已连接/未连接之间跳；Listener 蓝牙灯也要和连接状态一致。"
            expected = "Windows 设备状态稳定；Type 不弹反复重连；蓝牙灯不是配对乱闪；这个步骤给后面所有破坏性测试做基线。"
            default = "Windows显示：；灯效：；Type状态："
        },
        [PSCustomObject]@{
            title = "有 Type 手动删除不抢回"
            action = "保持 Type 打开。`r`n`r`n在 Windows 设置 > 蓝牙和其他设备里，删除当前 Listener/Billy/Blistener/listener 这一类设备。删除后等 20 秒，不要点击添加设备。"
            expected = "Type 不能自动把它配回来；Windows 里不应该自己重新出现已连接；可以出现 Type 提示等待手动添加，但不能自动 PairAsync/自动弹连接。这个场景用于换电脑。"
            default = "删除结果：；20秒后Windows状态：；有没有自动弹连接/自动恢复：；Type提示："
        },
        [PSCustomObject]@{
            title = "有 Type 手动添加回来"
            action = "保持 Type 打开。`r`n`r`n现在用 Windows 添加设备 > 蓝牙，手动选择 Listener 当前显示的名字并连接。"
            expected = "用户手动配回这台电脑后，Windows 键盘连接成功；Type 可以检测到并恢复 BLE 控制/录音通道；不能卡在已连接/未连接循环。"
            default = "添加设备看到的名字：；连接结果：；Type恢复耗时：约  秒；现象："
            notification = $true
            notificationTarget = "listener"
        },
        [PSCustomObject]@{
            title = "退出 Type 准备干净配对"
            action = "先退出 Type 托盘程序。`r`n`r`n确认任务栏/托盘里没有 Listener Type；如果还有窗口或托盘图标，把它关掉。"
            expected = "Type 退出后，Windows 蓝牙设备可以还显示已连接；但不能再由 Type 自动清配对或自动重连。"
            default = "Type是否已退出：；Windows当前状态："
        },
        [PSCustomObject]@{
            title = "无 Type 首次或干净配对"
            action = "保持 Type 关闭。`r`n`r`n在 Windows 设置 > 蓝牙和其他设备里，删除当前 Listener/Billy/Blistener/listener 这一类旧设备。然后让 Listener 进入配对状态，再用 Windows 添加设备 > 蓝牙，选择它完成连接。"
            expected = "没有 Type 的电脑也应该能正常把 Listener 当蓝牙键盘配上；不能一直显示请尝试重新连接设备；不能卡在已连接/未连接循环。"
            default = "删除旧设备：成功/失败；添加设备看到的名字：；最终状态："
            notification = $true
            notificationTarget = "listener"
        },
        [PSCustomObject]@{
            title = "无 Type 连接保持"
            action = "保持 Type 关闭，再观察 15 秒。`r`n`r`n如果你方便，可以按一下几个普通按键，看 Windows 是否还能收到键盘输入。不要打开 Type。"
            expected = "无 Type 状态下，Windows 蓝牙键盘连接应该能保持；不能刚配上就断；不能必须开 Type 才能连上键盘。"
            default = "15秒后状态：；按键是否可用：；现象："
        },
        [PSCustomObject]@{
            title = "Type 接管已配对设备"
            action = "重新打开最新 Type。`r`n`r`n等待 15 秒，观察 Type 是否自动找到刚才已配好的 Listener，并恢复 BLE 控制通道。"
            expected = "Type 应该接管已经配好的设备；不应该强制重新配对；不应该不停弹 Windows 添加设备通知。"
            default = "Type打开后多久恢复：约  秒；有没有弹窗：；现象："
        },
        [PSCustomObject]@{
            title = "同名写入不重配"
            action = "在 Type 的蓝牙名称输入框里填入当前 Windows 正在显示的同一个名字，然后点击写入。`r`n`r`n注意：这是同名写入。"
            expected = "同名写入不能触发重新配对；不能弹 Windows 添加设备；不能断开后让用户重新连接。"
            default = "当前名字：；写入后有没有重新配对/弹窗/断连："
        },
        [PSCustomObject]@{
            title = "随机名改名"
            action = "在 Type 蓝牙名称里填这个随机名字并写入：$randomName`r`n`r`n这个名字已经复制到剪贴板。如果 Windows 弹添加设备，只点一次连接，然后等它稳定。"
            expected = "任意合法 ASCII 名字都应该可用；Windows 最终显示新名字 $randomName；不能继续显示旧缓存名；不能无限弹添加设备或连接失败。"
            default = "写入名字：$randomName；Windows最终显示：；弹窗次数：；连接结果："
            clipboard = $randomName
            notification = $true
            notificationTarget = $randomName
        },
        [PSCustomObject]@{
            title = "恢复默认名字 listener"
            action = "在 Type 蓝牙名称里填默认名字 listener 并写入。`r`n`r`n如果 Windows 需要你点连接，只点一次，然后等它稳定。"
            expected = "默认名字 listener 可以恢复；Windows 最终显示 listener；Type 恢复连接；不能把名字截断、缓存成旧名、或者同名误判。"
            default = "Windows最终显示：；Type状态：；现象："
            clipboard = "listener"
            notification = $true
            notificationTarget = "listener"
        },
        [PSCustomObject]@{
            title = "双击旋钮重新配对"
            action = "保持 Type 打开。双击旋钮触发重新配对。`r`n`r`n如果 Windows 右下角出现连接通知，只点一次连接；如果没有通知，就打开 Windows 蓝牙页观察。"
            expected = "双击应进入明确的重新配对流程；不能单击误触发；不能一直在已连接/未连接之间跳；蓝牙灯要和配对/连接状态一致。"
            default = "双击后弹窗：有/无；点连接结果：；Windows状态：；灯效："
            notification = $true
            notificationTarget = "listener"
        },
        [PSCustomObject]@{
            title = "Type 重启恢复"
            action = "退出 Type，然后重新打开最新 Type。`r`n`r`n不要重新配对，等 15 秒看它自己恢复。"
            expected = "Type 重启后应该自动恢复已配对设备；不应该弹重复连接通知；不应该只剩键盘连接但 Type BLE 控制通道连不上。"
            default = "重启后恢复：成功/失败；耗时：约  秒；现象："
        },
        [PSCustomObject]@{
            title = "断电或 idle 恢复"
            action = "让 Listener 断电再上电，或者等待/触发一次 idle 后唤醒。`r`n`r`n不要改名，不要重新配对，观察 Windows 和 Type 是否恢复。"
            expected = "断电/idle 后应自动恢复；不应该需要删除设备；灯效不要留在配对状态；Type 日志不应该持续 transport_not_ready。"
            default = "恢复方式：断电/idle；恢复耗时：约  秒；Windows状态：；灯效："
        },
        [PSCustomObject]@{
            title = "最终录音链路"
            action = "在前面蓝牙流程都做完后，按一次录音键开始，再按一次停止。`r`n`r`n这一步确认配对/改名/重连没有破坏 BLE 录音通道。"
            expected = "录音胶囊快速出现；设备录音灯稳定；Type 收到音频并转写；日志里不应该出现连接中断或 missing_packets 明显异常。"
            default = "开始响应：约  秒；停止/转写：约  秒；现象："
        }
    )

    $records = [System.Collections.Generic.List[object]]::new()
    for ($i = 0; $i -lt $steps.Count; $i++) {
        $step = $steps[$i]
        $clipboardText = ""
        if ($step.PSObject.Properties.Name -contains "clipboard") {
            $clipboardText = $step.clipboard
        }
        $enableNotificationHelper = $false
        if ($step.PSObject.Properties.Name -contains "notification") {
            $enableNotificationHelper = [bool]$step.notification
        }
        $notificationTargetName = "listener"
        if ($step.PSObject.Properties.Name -contains "notificationTarget") {
            $notificationTargetName = [string]$step.notificationTarget
        }
        $record = Show-Step `
            -Index ($i + 1) `
            -Total $steps.Count `
            -Title $step.title `
            -Action $step.action `
            -Expected $step.expected `
            -DefaultObservation $step.default `
            -ClipboardText $clipboardText `
            -EnableNotificationHelper:$enableNotificationHelper `
            -NotificationTargetName $notificationTargetName
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
        random_name = $randomName
        records = @($records)
    } | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $summaryJsonPath -Encoding UTF8

    $lines = [System.Collections.Generic.List[string]]::new()
    $lines.Add("# BLE User Scenarios Human Review") | Out-Null
    $lines.Add("") | Out-Null
    $lines.Add("- Status: $status") | Out-Null
    $lines.Add("- Random name: $randomName") | Out-Null
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
            "蓝牙用户场景验收已记录。`r`n`r`n结果：$status`r`n目录：$OutputDir",
            "Listener 蓝牙验收完成",
            [System.Windows.Forms.MessageBoxButtons]::OK,
            [System.Windows.Forms.MessageBoxIcon]::Information)
    }

    Write-Host "ble_user_scenarios_status=$status"
    Write-Host "summary=$summaryPath"
    Write-Host "session=$sessionPath"
    if ($status -eq "HUMAN_REVIEW_FAIL") {
        exit 1
    }
} catch {
    $errorText = ($_ | Out-String).Trim()
    $errorPath = Join-Path $OutputDir "ble-user-scenarios-error.txt"
    $errorText | Set-Content -LiteralPath $errorPath -Encoding UTF8

    $recordSnapshot = @()
    $recordsVar = Get-Variable -Name records -ErrorAction SilentlyContinue
    if ($recordsVar -and $null -ne $recordsVar.Value) {
        $recordSnapshot = @($recordsVar.Value)
    }
    $randomNameValue = ""
    $randomNameVar = Get-Variable -Name randomName -ErrorAction SilentlyContinue
    if ($randomNameVar -and $null -ne $randomNameVar.Value) {
        $randomNameValue = [string]$randomNameVar.Value
    }

    [ordered]@{
        status = "SCRIPT_ERROR"
        generated_at = (Get-Date).ToString("o")
        output_dir = $OutputDir
        session_jsonl = $sessionPath
        log_path = $LogPath
        random_name = $randomNameValue
        error = $errorText
        records = $recordSnapshot
    } | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $summaryJsonPath -Encoding UTF8

    $lines = [System.Collections.Generic.List[string]]::new()
    $lines.Add("# BLE User Scenarios Human Review") | Out-Null
    $lines.Add("") | Out-Null
    $lines.Add("- Status: SCRIPT_ERROR") | Out-Null
    $lines.Add("- Error: $errorPath") | Out-Null
    $lines.Add("- Log: $LogPath") | Out-Null
    $lines.Add("- Session: $sessionPath") | Out-Null
    $lines.Add("") | Out-Null
    $lines.Add('```text') | Out-Null
    $lines.Add($errorText) | Out-Null
    $lines.Add('```') | Out-Null
    $lines | Set-Content -LiteralPath $summaryPath -Encoding UTF8

    Write-Host "ble_user_scenarios_status=SCRIPT_ERROR"
    Write-Host "summary=$summaryPath"
    Write-Host "error=$errorPath"
    if (-not $NoPrompt.IsPresent) {
        try {
            Ensure-FormsLoaded
            [void][System.Windows.Forms.MessageBox]::Show(
                "蓝牙验收脚本异常，已记录错误。`r`n`r`n目录：$OutputDir",
                "Listener 蓝牙验收异常",
                [System.Windows.Forms.MessageBoxButtons]::OK,
                [System.Windows.Forms.MessageBoxIcon]::Error)
        } catch {
        }
    }
    exit 4
} finally {
    if ($mutexCreated) {
        $mutex.ReleaseMutex() | Out-Null
    }
    $mutex.Dispose()
}
