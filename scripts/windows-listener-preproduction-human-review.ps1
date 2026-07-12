[CmdletBinding(PositionalBinding = $false)]
param(
    [string]$OutputDir = "",
    [string]$StepId = "",
    [string[]]$StepIds = @(),
    [string]$DeviceName = "listener",
    [string]$RandomName = "",
    [string]$TotalReviewStatePath = "",
    [switch]$ListSteps,
    [switch]$NoPrompt,
    [switch]$NoSound,
    [switch]$StatusSelfTest
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
if (-not [string]::IsNullOrWhiteSpace($TotalReviewStatePath)) {
    if (-not [System.IO.Path]::IsPathRooted($TotalReviewStatePath)) {
        $TotalReviewStatePath = Join-Path $repoRoot $TotalReviewStatePath
    }
    $TotalReviewStatePath = [System.IO.Path]::GetFullPath($TotalReviewStatePath)
}

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

function New-ReviewRandomBleName {
    $alphabet = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789"
    do {
        $chars = for ($i = 0; $i -lt 12; $i++) {
            $alphabet[[System.Security.Cryptography.RandomNumberGenerator]::GetInt32($alphabet.Length)]
        }
        $candidate = -join $chars
    } while ($candidate -match '^(?i:listener|listner|lt|type)')
    return $candidate
}

if ([string]::IsNullOrWhiteSpace($RandomName)) {
    $RandomName = New-ReviewRandomBleName
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

function Get-GitWorktreeInfo {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        return [PSCustomObject]@{
            path = $Path
            exists = $false
            dirty = $false
            dirty_count = 0
            status_porcelain = @()
            diff_stat = @()
            error = "path not found"
        }
    }

    try {
        $status = @(& git -C $Path status --porcelain=v1 --untracked-files=all 2>$null)
        $diffStat = @(& git -C $Path diff --stat 2>$null)
        return [PSCustomObject]@{
            path = $Path
            exists = $true
            dirty = $status.Count -gt 0
            dirty_count = $status.Count
            status_porcelain = @($status)
            diff_stat = @($diffStat)
            error = ""
        }
    } catch {
        return [PSCustomObject]@{
            path = $Path
            exists = $true
            dirty = $false
            dirty_count = 0
            status_porcelain = @()
            diff_stat = @()
            error = $_.Exception.Message
        }
    }
}

$listenerRoot = (Split-Path -Parent $repoRoot)
$firmwareRoot = Join-Path $listenerRoot "Listener-Firmware"
$typeHeadInfo = Get-GitHeadInfo -Path $repoRoot
$firmwareHeadInfo = Get-GitHeadInfo -Path $firmwareRoot
$typeWorktreeInfo = Get-GitWorktreeInfo -Path $repoRoot
$firmwareWorktreeInfo = Get-GitWorktreeInfo -Path $firmwareRoot

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
        $processes = @(Get-Process | Where-Object {
                $_.ProcessName -match "listener|type" -or $_.MainWindowTitle -match "Listener Type|com.listener.type"
            })
        @(
            foreach ($process in $processes) {
                $cim = $null
                try {
                    $cim = Get-CimInstance Win32_Process -Filter ("ProcessId={0}" -f $process.Id) -ErrorAction Stop
                } catch {
                }
                $exePath = ""
                try {
                    $exePath = [string]$process.Path
                } catch {
                }
                if ([string]::IsNullOrWhiteSpace($exePath) -and $null -ne $cim) {
                    $exePath = [string]$cim.ExecutablePath
                }
                $fileVersion = ""
                $productVersion = ""
                $lastWriteTime = ""
                $sha256 = ""
                if (-not [string]::IsNullOrWhiteSpace($exePath) -and (Test-Path -LiteralPath $exePath)) {
                    try {
                        $item = Get-Item -LiteralPath $exePath -ErrorAction Stop
                        $fileVersion = [string]$item.VersionInfo.FileVersion
                        $productVersion = [string]$item.VersionInfo.ProductVersion
                        $lastWriteTime = $item.LastWriteTime.ToString("o")
                        $sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $exePath).Hash
                    } catch {
                    }
                }
                [pscustomobject][ordered]@{
                    Id = $process.Id
                    ProcessName = $process.ProcessName
                    MainWindowTitle = $process.MainWindowTitle
                    Responding = $process.Responding
                    StartTime = $process.StartTime
                    ExecutablePath = $exePath
                    FileVersion = $fileVersion
                    ProductVersion = $productVersion
                    LastWriteTime = $lastWriteTime
                    Sha256 = $sha256
                    CommandLine = $(if ($null -ne $cim) { [string]$cim.CommandLine } else { "" })
                }
            }
        )
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
        [Parameter(Mandatory = $true)][int]$OverallIndex,
        [Parameter(Mandatory = $true)][int]$OverallTotal,
        [Parameter(Mandatory = $true)][string]$ReviewScope,
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

    $overallIndexSafe = if ($OverallIndex -gt 0) { $OverallIndex } else { $Index }
    $progressText = if ($OverallTotal -gt $Total) {
        "$overallIndexSafe/$OverallTotal（本次 $Index/$Total）"
    } else {
        "$Index/$Total"
    }

    $form = [System.Windows.Forms.Form]::new()
    $form.Text = "Listener 1.0.2 总验收 $progressText"
    $form.StartPosition = [System.Windows.Forms.FormStartPosition]::Manual
    $form.ClientSize = [System.Drawing.Size]::new(680, 360)
    $form.MinimumSize = [System.Drawing.Size]::new(640, 340)
    $form.MaximizeBox = $false
    $form.TopMost = $true
    $form.Font = [System.Drawing.Font]::new("Microsoft YaHei UI", 10)
    $form.AutoScaleMode = [System.Windows.Forms.AutoScaleMode]::Dpi
    $form.AutoScroll = $false
    $workingArea = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
    $form.Location = [System.Drawing.Point]::new(
        $workingArea.Left + 12,
        $workingArea.Top + 72
    )

    $scopeText = if ($OverallTotal -gt $Total) {
        "总体验收范围 $OverallTotal 项；当前总进度 $overallIndexSafe/$OverallTotal；本次$($ReviewScope)第 $Index/$Total 项；有备注会停下修备注"
    } else {
        "总体验收范围 $OverallTotal 项；第 $Index/$Total 项；有备注会停下修备注"
    }

    $title = [System.Windows.Forms.Label]::new()
    $title.Text = "总验收 $progressText  $($Step.title)"
    $title.Font = [System.Drawing.Font]::new("Microsoft YaHei UI", 12, [System.Drawing.FontStyle]::Bold)
    $title.AutoSize = $false
    $title.Location = [System.Drawing.Point]::new(16, 10)
    $title.Size = [System.Drawing.Size]::new(648, 26)
    $form.Controls.Add($title)

    $actionLabel = [System.Windows.Forms.Label]::new()
    $actionLabel.Text = "你现在做 - $scopeText"
    $actionLabel.AutoSize = $false
    $actionLabel.Location = [System.Drawing.Point]::new(16, 42)
    $actionLabel.Size = [System.Drawing.Size]::new(648, 20)
    $form.Controls.Add($actionLabel)

    $actionBox = [System.Windows.Forms.TextBox]::new()
    $actionBox.Multiline = $true
    $actionBox.ReadOnly = $true
    $actionBox.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
    $actionBox.Location = [System.Drawing.Point]::new(16, 64)
    $actionBox.Size = [System.Drawing.Size]::new(648, 72)
    $actionBox.Text = Convert-ReviewText $Step.action
    $form.Controls.Add($actionBox)

    $expectedLabel = [System.Windows.Forms.Label]::new()
    $expectedLabel.Text = "通过标准"
    $expectedLabel.AutoSize = $false
    $expectedLabel.Location = [System.Drawing.Point]::new(16, 144)
    $expectedLabel.Size = [System.Drawing.Size]::new(648, 20)
    $form.Controls.Add($expectedLabel)

    $expectedBox = [System.Windows.Forms.TextBox]::new()
    $expectedBox.Multiline = $true
    $expectedBox.ReadOnly = $true
    $expectedBox.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
    $expectedBox.Location = [System.Drawing.Point]::new(16, 166)
    $expectedBox.Size = [System.Drawing.Size]::new(648, 62)
    $expectedBox.Text = Convert-ReviewText $Step.expected
    $form.Controls.Add($expectedBox)

    $operatorActionLabel = [System.Windows.Forms.Label]::new()
    $operatorActionLabel.Text = "备注（可留空；有异常、疑问或小瑕疵就写下来，我会当作需求处理）"
    $operatorActionLabel.AutoSize = $false
    $operatorActionLabel.Location = [System.Drawing.Point]::new(16, 236)
    $operatorActionLabel.Size = [System.Drawing.Size]::new(648, 20)
    $form.Controls.Add($operatorActionLabel)

    $operatorAction = [System.Windows.Forms.TextBox]::new()
    $operatorAction.Multiline = $true
    $operatorAction.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
    $operatorAction.Location = [System.Drawing.Point]::new(16, 258)
    $operatorAction.Size = [System.Drawing.Size]::new(648, 52)
    $operatorAction.Anchor = [System.Windows.Forms.AnchorStyles]::Left -bor [System.Windows.Forms.AnchorStyles]::Right -bor [System.Windows.Forms.AnchorStyles]::Top -bor [System.Windows.Forms.AnchorStyles]::Bottom
    $form.Controls.Add($operatorAction)

    $buttonPanel = [System.Windows.Forms.FlowLayoutPanel]::new()
    $buttonPanel.FlowDirection = [System.Windows.Forms.FlowDirection]::RightToLeft
    $buttonPanel.WrapContents = $false
    $buttonPanel.Location = [System.Drawing.Point]::new(16, 318)
    $buttonPanel.Size = [System.Drawing.Size]::new(648, 34)
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
        $button.Size = [System.Drawing.Size]::new(86, 28)
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
            "先用最新 MSI 更新 Listener Type；像普通用户一样从 C:\Program Files\Listener Type\listener-type.exe、桌面 Listener Type.lnk 或托盘启动，不要从 repo target 目录启动。"
            "确认托盘里是最新版本；从托盘打开主窗口。不要继续下一步，直到窗口不是空白 WebView。"
            "本步骤的进程证据必须显示 ExecutablePath 是 Program Files 安装路径，并带 FileVersion/ProductVersion/Sha256。")) `
        -Expected (Join-Text @(
            "主窗口能打开，界面不空白；桌面快捷方式和托盘启动都指向已安装的最新 MSI exe，而不是 repo build output。"
            "Windows 蓝牙和 Type 状态稳定，不在已连接/未连接之间循环。"))
    New-ReviewStep `
        -Id "pwr-boot-shutdown-led" `
        -Title "PWR 开机/关机确认灯" `
        -Action (Join-Text @(
            "让 Listener 断电或重启到刚刷入的固件；开机时只观察 PWR/LED1 第一帧，不要按 EC11 或 key1-key4。"
            "继续观察到 BLE/LED2 稳定可见；把 PWR 亮起到 BLE 稳定可见这段当作真实启动时间，确认 PWR 从 warm amber 启动态切回正常电源状态的时机是在 BLE 启动可见之后。"
            "开机稳定后，垂直按下 EC11 旋钮几次但不要旋转；确认按下不会被识别成旋转、不会改音量/亮度，也不会触发 EC11 环形旋转追光。"
            "开机稳定后，长按 EC11 约 1.2 到 1.5 秒；观察关机确认 PWR 是否仍是已验收 warm amber，同时看 EC11 环形待关机灯效是否保持连续、不要黑一下再恢复；看到后松开，不要按到硬件关机边界。"
            "本项只验 PWR 开机第一帧、BLE ready 后 PWR 启动完成切换、EC11 直按不误触发旋转、以及关机确认 PWR/EC11 是否各自符合合同；如果看到 key/其它灯异常只写备注，不在本项调整。")) `
        -Expected (Join-Text @(
            "开机 PWR 第一帧应直接显示与关机确认同色同亮的 warm amber；不能先黑一下再亮，并且仍受 Type 状态灯四区亮度 cap 约束。"
            "BLE/LED2 稳定可见后，PWR 才从启动 warm amber 交给正常电源状态；这个正常色表示启动完成，不是首帧白，也不能为了显得启动更快而提前显示。"
            "EC11 直按只应进入按键单击/双击/长按判定，不应产生旋转、音量/亮度变化或 EC11 环形旋转灯效。"
            "关机确认 PWR 保持已验收 warm amber；EC11 环形待关机灯效保持原有连续填充效果，不应间歇性全黑再恢复；开机琥珀灯不能影响后续 PWR/BLE/EC11/key 灯效。"
            "松开 EC11 后退出 pending 关机确认是受保护行为；不应触发单击、双击重配或 Windows 连接通知。"))
    New-ReviewStep `
        -Id "type-brightness-low-power-sync" `
        -Title "Type 亮度和低功耗精确同步" `
        -Action (Join-Text @(
            "从 Windows 托盘图标打开最新 Type 的设备设置页；不要从旧窗口、旧桌面快捷方式或旧 exe 猜测。"
            "本项只验 Type 亮度/低功耗设置，不验其它灯效样式。"
            "确认状态灯亮度和按键灯亮度默认/迁移后是 80；如果要临时改随机值，请记下输入的数字。"
            "清空任意一个亮度数字框后直接输入 50，输入框应显示 50，不能残留前导 0 变成 050。"
            "在 Type 里写入任意状态灯/按键灯亮度，以及任意外接/电池低功耗分钟值；不要只测 12 分钟。"
            "保存后刷新或重新打开设备设置页，观察 Type 显示值和设备读回值是否仍等于刚输入的值。")) `
        -Expected (Join-Text @(
            "Type 默认状态灯/按键灯亮度是 80；EC11/边框仍按各自设置，不被状态灯或按键灯拖动。"
            "亮度数字框删除时允许临时为空，重新输入后不能把旧的 0 拼进新值。"
            "Type 写入多少亮度，固件最大亮度 cap 就是多少；不能绕过 Type 设置，也不能偷偷变回 100。"
            "Type 写入多少低功耗分钟，读回就必须是多少；不能 12 变 13，也不能任何其它数字被四舍五入或夹带改写。"
            "低功耗开关只能决定是否进入低功耗语义状态，不能另加隐藏亮度层。"
            "如果有任何异常、疑问或观感备注，请写在备注里；脚本会停在本项等待 triage。"))
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
            "不要手动打开 Windows 添加设备；等待 Type 自动清理本机旧配对并恢复。")) `
        -Expected (Join-Text @(
            "任意合法 1-29 个可见 ASCII 名字都能写入；不能包含空格、引号、分号、等号或反斜杠。"
            "本步骤生成的随机名必须是无产品前缀的 12 位随机 ASCII 串，用来证明任意随机名都可写。"
            "不同名改名由 Type 自动清理本机旧配对并恢复。"
            "BLE 灯效和双击恢复一样：蓝色双闪/重连恢复，完成前不能显示已连接蓝底。"
            "Windows 最终显示精确新名字：$RandomName。"
            "改名恢复走 silent Type 路径：本机 Type 不能打开 Windows 添加设备/蓝牙设置或本机用户配对提示，其它电脑也不应因为改名收到 Swift Pair 弹窗。"
            "不能继续显示旧缓存名，Type 最终能恢复。")) `
        -ClipboardText $RandomName
    New-ReviewStep `
        -Id "restore-default-listener" `
        -Title "恢复默认名字 listener" `
        -Action (Join-Text @(
            "在 Type 蓝牙名称里填 listener 并写入。"
            "不要手动打开 Windows 添加设备；等待 Type 自动清理本机旧配对并恢复。")) `
        -Expected (Join-Text @(
            "默认名字 listener 能恢复。"
            "BLE 灯效和双击恢复一样：蓝色双闪/重连恢复，完成前不能显示已连接蓝底。"
            "改名恢复走 silent Type 路径：本机 Type 不能打开 Windows 添加设备/蓝牙设置或本机用户配对提示，其它电脑也不应因为改名收到 Swift Pair 弹窗。"
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
            "不要改名，不要重新配对，等待 3 秒；超过 3 秒才恢复就按速度回退记录备注。")) `
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
        -Title "EC11 单击/双击边界" `
        -Action (Join-Text @(
            "保持 Type 打开且当前不要录音。"
            "先单击一次 EC11 旋钮，等待约 1 秒。确认这是单击行为。"
            "如果录音被单击启动，请用 EC11 单击或 Type 取消让它回到空闲后再继续。"
            "然后快速双击一次 EC11 旋钮，只看它是否被识别成双击；本步骤不要求完整自动重连通过。")) `
        -Expected (Join-Text @(
            "单击必须只触发单击/本地反馈或单击录音动作，不能打开重配流程，不能弹 Windows 连接通知。"
            "快速双击必须取消第一下单击，不应该先进入录音或显示单击录音胶囊。"
            "快速双击应进入双击重配提示/蓝色重配灯效；如果后续自动重连失败，只在备注里写自动重连现象，边界本身按是否误判来判定。"
            "EC11 的按键行为要和 key1-key4 的单/双击窗口一致。"))
    New-ReviewStep `
        -Id "ec11-double-repair-with-type" `
        -Title "有 Type 的双击重配" `
        -Action (Join-Text @(
            "保持 Type 打开。快速双击 EC11 旋钮。"
            "等 Type 先清理这台电脑上的旧 Listener 配对，再寻找 Listener 恢复广播并自动恢复本机连接。"
            "不要点击 Windows 蓝牙弹窗，不要在 Type 里点别的恢复按钮，不要重复双击。"
            "如果另一台电脑已经通过 Windows 弹窗连上，本机 Type 应该停止等待，不要再抢回。")) `
        -Expected (Join-Text @(
            "Type 会先清理本机旧配对，再寻找恢复广播并走本机自动 PairAsync/GATT 恢复。"
            "同一台电脑恢复时不需要用户点击 Windows 连接通知。"
            "如果另一台电脑先用 Windows 弹窗连上，本机 Type 不能循环清理或抢回。"
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
            "有 Type 的电脑可以帮本机清旧配对并自动恢复。"
            "如果另一台电脑先通过 Windows 弹窗连上，旧电脑 Type 不能循环清理或抢回。"
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
            "如果不做完整传输，至少确认 OTA v1 GATT 可发现。")) `
        -Expected (Join-Text @(
            "OTA v1 服务和控制特征可发现。"
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
            "不要保留 Type portable zip。"
            "核对根目录 Type MSI/Firmware OTA zip 的 SHA256 必须分别等于本次最终打包源产物。")) `
        -Expected (Join-Text @(
            "Type MSI 是最新 v1.0.2。"
            "Firmware OTA zip 是最新 v1.0.2。"
            "根目录没有旧包或 portable 包。"
            "root package hash listing 里 type_source_hash_matches_root 和 firmware_source_hash_matches_root 都必须为 true。"))
)
Assert-StepsMatchCanonicalScenarios -ReviewSteps $steps
$allSteps = @($steps)

function Get-NormalizedExistingPath {
    param([string]$Path)

    if ([string]::IsNullOrWhiteSpace($Path)) {
        return ""
    }
    try {
        return ([System.IO.Path]::GetFullPath((Resolve-Path -LiteralPath $Path).Path)).TrimEnd('\')
    } catch {
        return ""
    }
}

function Test-CarryForwardHumanRecordSummaryCandidate {
    param(
        [Parameter(Mandatory = $true)]$Candidate,
        [Parameter(Mandatory = $true)]$Summary
    )

    $candidateDir = Get-NormalizedExistingPath -Path $Candidate.Directory.FullName
    if ([string]::IsNullOrWhiteSpace($candidateDir)) {
        return $false
    }

    # preproduction-human-review* is a conventional directory name; validated output/session files are authoritative.

    $candidatePathText = $Candidate.FullName.ToLowerInvariant()
    foreach ($fixtureMarker in @("operator-note", "dryrun", "dry-run", "smoke", "fixture", "script-gate", "format-check", "no-hardware")) {
        if ($candidatePathText.Contains($fixtureMarker)) {
            return $false
        }
    }

    $summaryOutputDir = Get-NormalizedExistingPath -Path ([string]$Summary.output_dir)
    if ($summaryOutputDir -ne $candidateDir) {
        return $false
    }

    $sessionPath = Get-NormalizedExistingPath -Path ([string]$Summary.session_jsonl)
    if ([string]::IsNullOrWhiteSpace($sessionPath) -or (Split-Path -Parent $sessionPath) -ne $candidateDir) {
        return $false
    }

    if ($Summary.status -notin @("HUMAN_REVIEW_PASS", "HUMAN_REVIEW_INCOMPLETE", "HUMAN_REVIEW_FAIL")) {
        return $false
    }

    $records = @($Summary.records)
    if ($records.Count -eq 0) {
        return $false
    }

    foreach ($record in $records) {
        if ($null -eq $record -or $null -eq $record.PSObject.Properties["id"]) {
            return $false
        }
        $recordId = [string]$record.id
        if ([string]::IsNullOrWhiteSpace($recordId)) {
            return $false
        }
        $recordJson = $record | ConvertTo-Json -Depth 10 -Compress
        if ($recordJson -match "NoPrompt dry run" -or $recordJson -match "dryrun" -or $recordJson -match "operator-note") {
            return $false
        }
    }

    $operatorNotes = @()
    foreach ($record in $records) {
        $carriedForwardProperty = $record.PSObject.Properties["carried_forward"]
        if ($carriedForwardProperty -and $carriedForwardProperty.Value -eq $true) {
            continue
        }
        $operatorNoteProperty = $record.PSObject.Properties["operator_note"]
        if ($operatorNoteProperty -and -not [string]::IsNullOrWhiteSpace([string]$operatorNoteProperty.Value)) {
            $operatorNotes += $operatorNoteProperty.Value
        }
    }
    if ($operatorNotes.Count -gt 0) {
        $triagePath = Join-Path $candidateDir "preproduction-operator-note-triage.json"
        if (-not (Test-Path -LiteralPath $triagePath)) {
            return $false
        }
        try {
            $triage = Get-Content -Raw -LiteralPath $triagePath | ConvertFrom-Json
        } catch {
            return $false
        }
        $triageStatusProperty = $triage.PSObject.Properties["status"]
        $triageSourceProperty = $triage.PSObject.Properties["source_summary"]
        # triage.source_summary must match the candidate summary before its record can be carried forward.
        if ($null -eq $triageStatusProperty -or $null -eq $triageSourceProperty) {
            return $false
        }
        if ([string]$triageStatusProperty.Value -ne "PASS") {
            return $false
        }
        $triageSource = Get-NormalizedExistingPath -Path ([string]$triageSourceProperty.Value)
        if ($triageSource -ne (Get-NormalizedExistingPath -Path $Candidate.FullName)) {
            return $false
        }
    }

    return $true
}

function Get-ExplicitTotalReviewRecordSet {
    param(
        [string]$StatePath = "",
        [Parameter(Mandatory = $true)][string[]]$RequiredStepIds
    )

    $empty = [pscustomobject]@{
        records = @{}
        sources_by_id = @{}
        blocked_items = @()
        state_path = ""
        next_step_id = ""
    }
    if ([string]::IsNullOrWhiteSpace($StatePath)) {
        return $empty
    }
    if (-not (Test-Path -LiteralPath $StatePath)) {
        throw "Current total review state is missing: $StatePath"
    }

    try {
        $state = Get-Content -Raw -LiteralPath $StatePath | ConvertFrom-Json
    } catch {
        throw "Current total review state is unreadable: $StatePath ($($_.Exception.Message))"
    }
    $statusProperty = $state.PSObject.Properties["status"]
    $nextStepProperty = $state.PSObject.Properties["next_step_id"]
    $completedProperty = $state.PSObject.Properties["completed_records"]
    if ($null -eq $statusProperty -or [string]$statusProperty.Value -ne "ACTIVE" -or
        $null -eq $nextStepProperty -or $null -eq $completedProperty) {
        throw "Current total review state must declare ACTIVE status, next_step_id, and completed_records: $StatePath"
    }

    $nextStepId = ([string]$nextStepProperty.Value).Trim()
    if ([string]::IsNullOrWhiteSpace($nextStepId) -or $RequiredStepIds -notcontains $nextStepId) {
        throw "Current total review state has an unknown next_step_id '$nextStepId': $StatePath"
    }

    $passRecords = @{}
    $sourceById = @{}
    foreach ($entry in @($completedProperty.Value)) {
        $idProperty = $entry.PSObject.Properties["id"]
        $sourceProperty = $entry.PSObject.Properties["source_summary"]
        if ($null -eq $idProperty -or $null -eq $sourceProperty) {
            throw "Current total review state has a completed record without id/source_summary: $StatePath"
        }
        $recordId = ([string]$idProperty.Value).Trim()
        if ([string]::IsNullOrWhiteSpace($recordId) -or $RequiredStepIds -notcontains $recordId -or $passRecords.ContainsKey($recordId)) {
            throw "Current total review state has an invalid or duplicate completed id '$recordId': $StatePath"
        }
        $sourceSummaryPath = Get-NormalizedExistingPath -Path ([string]$sourceProperty.Value)
        if ([string]::IsNullOrWhiteSpace($sourceSummaryPath)) {
            throw "Current total review state source summary is missing for '$recordId': $($sourceProperty.Value)"
        }
        try {
            $summary = Get-Content -Raw -LiteralPath $sourceSummaryPath | ConvertFrom-Json
        } catch {
            throw "Current total review state source summary is unreadable for '$recordId': $sourceSummaryPath"
        }
        $candidate = Get-Item -LiteralPath $sourceSummaryPath
        if (-not (Test-CarryForwardHumanRecordSummaryCandidate -Candidate $candidate -Summary $summary)) {
            throw "Current total review state source summary is not a triaged real human record for '$recordId': $sourceSummaryPath"
        }
        $matchingRecords = @($summary.records | Where-Object { [string]$_.id -eq $recordId })
        if ($matchingRecords.Count -ne 1 -or [string]$matchingRecords[0].result -ne "PASS") {
            throw "Current total review state source does not prove PASS for '$recordId': $sourceSummaryPath"
        }
        $carriedForwardProperty = $matchingRecords[0].PSObject.Properties["carried_forward"]
        if ($carriedForwardProperty -and $carriedForwardProperty.Value -eq $true) {
            throw "Current total review state must reference an original human record, not a carried record: $sourceSummaryPath"
        }
        $passRecords[$recordId] = $matchingRecords[0]
        $sourceById[$recordId] = $sourceSummaryPath
    }
    if ($passRecords.ContainsKey($nextStepId)) {
        throw "Current total review next_step_id '$nextStepId' is already marked complete: $StatePath"
    }

    return [pscustomobject]@{
        records = $passRecords
        sources_by_id = $sourceById
        blocked_items = @()
        state_path = $StatePath
        next_step_id = $nextStepId
    }
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
        $copy["operator_note"] = ""
        $copy["operator_action"] = ""
        $copy["observation"] = ""
        $copy["carried_forward_operator_note_status"] = "closed_in_source"
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

function Get-FocusedReviewStatus {
    param(
        [Parameter(Mandatory = $true)][object[]]$Records,
        [Parameter(Mandatory = $true)][int]$ExpectedRecordCount,
        [Parameter(Mandatory = $true)][bool]$StoppedAfterOperatorNote
    )

    $recordsWithResult = @($Records | Where-Object { $null -ne (Get-ReviewRecordField -Record $_ -Name "result") })
    $missingResultCount = $Records.Count - $recordsWithResult.Count
    $failCount = @($recordsWithResult | Where-Object { [string](Get-ReviewRecordField -Record $_ -Name "result") -eq "FAIL" }).Count
    $incompleteCount = @($recordsWithResult | Where-Object { [string](Get-ReviewRecordField -Record $_ -Name "result") -in @("SKIP", "ABORT") }).Count + $missingResultCount

    if ($failCount -gt 0) {
        return "FOCUSED_HUMAN_REVIEW_FAIL"
    }
    if ($incompleteCount -gt 0 -or $Records.Count -ne $ExpectedRecordCount -or $StoppedAfterOperatorNote) {
        return "FOCUSED_HUMAN_REVIEW_INCOMPLETE"
    }
    return "FOCUSED_HUMAN_REVIEW_PASS"
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

if ($StatusSelfTest.IsPresent) {
    try {
        $syntheticRecords = @([ordered]@{ result = "PASS" })
        $syntheticStatus = Get-FocusedReviewStatus -Records $syntheticRecords -ExpectedRecordCount 1 -StoppedAfterOperatorNote $false
        if ($syntheticStatus -ne "FOCUSED_HUMAN_REVIEW_PASS") {
            throw "Focused PASS self-test expected FOCUSED_HUMAN_REVIEW_PASS, got $syntheticStatus"
        }
        Write-Host "PASS: focused human-review status recognizes an OrderedDictionary PASS record"
        exit 0
    } finally {
        if ($null -ne $singleInstanceMutex) {
            try { $singleInstanceMutex.ReleaseMutex() | Out-Null } catch {}
            $singleInstanceMutex.Dispose()
        }
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
$carryForwardRecordSet = [pscustomobject]@{
    records = @{}
    sources_by_id = @{}
    blocked_items = @()
    state_path = ""
    next_step_id = ""
}
if ($requestedStepIds.Count -gt 0) {
    $carryForwardRecordSet = Get-ExplicitTotalReviewRecordSet `
        -StatePath $TotalReviewStatePath `
        -RequiredStepIds @($allSteps | ForEach-Object { [string]$_.id })
    if (-not [string]::IsNullOrWhiteSpace($carryForwardRecordSet.next_step_id) -and
        $requestedStepIds -notcontains $carryForwardRecordSet.next_step_id) {
        throw "Current total review state expects '$($carryForwardRecordSet.next_step_id)', but requested '$($requestedStepIds -join ', ')'."
    }
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
    "CarryForwardRecordCount: $($carryForwardRecordSet.records.Count)"
    "TotalReviewState: $(if ([string]::IsNullOrWhiteSpace($carryForwardRecordSet.state_path)) { 'NONE' } else { $carryForwardRecordSet.state_path })"
    "TotalReviewNextStep: $(if ([string]::IsNullOrWhiteSpace($carryForwardRecordSet.next_step_id)) { 'NONE' } else { $carryForwardRecordSet.next_step_id })"
    "TypeHead: $($typeHeadInfo.head) $($typeHeadInfo.commit_time) $($typeHeadInfo.subject)"
    "TypeDirty: $($typeWorktreeInfo.dirty) dirty_count=$($typeWorktreeInfo.dirty_count)"
    "FirmwareHead: $($firmwareHeadInfo.head) $($firmwareHeadInfo.commit_time) $($firmwareHeadInfo.subject)"
    "FirmwareDirty: $($firmwareWorktreeInfo.dirty) dirty_count=$($firmwareWorktreeInfo.dirty_count)"
)
if ($resumeExistingFullReview) {
    $startLines += "ResumedExistingRecords: $($existingRecords.Count)"
    $startLines | Add-Content -LiteralPath $startInfoPath -Encoding UTF8
} else {
    $startLines | Set-Content -LiteralPath $startInfoPath -Encoding UTF8
}

$records = [System.Collections.Generic.List[object]]::new()
$stoppedAfterOperatorNote = $false
$reviewScopeLabel = if ($requestedStepIds.Count -gt 0) { "聚焦验收 " } else { "" }
$overallIndexById = @{}
for ($overallStepIndex = 0; $overallStepIndex -lt $allSteps.Count; $overallStepIndex++) {
    $overallIndexById[[string]$allSteps[$overallStepIndex].id] = $overallStepIndex + 1
}
try {
    foreach ($existingRecord in $existingRecords) {
        $records.Add($existingRecord) | Out-Null
    }

    for ($i = $existingRecords.Count; $i -lt $steps.Count; $i++) {
        $overallIndex = if ($overallIndexById.ContainsKey([string]$steps[$i].id)) { [int]$overallIndexById[[string]$steps[$i].id] } else { $i + 1 }
        $record = Show-ReviewStep -Index ($i + 1) -Total $steps.Count -OverallIndex $overallIndex -OverallTotal $allSteps.Count -ReviewScope $reviewScopeLabel -Step $steps[$i]
        $records.Add($record) | Out-Null
        ($record | ConvertTo-Json -Depth 10 -Compress) | Add-Content -LiteralPath $sessionPath -Encoding UTF8
        $operatorNote = Get-OperatorNoteText -Record $record
        if (-not [string]::IsNullOrWhiteSpace($operatorNote)) {
            $stoppedAfterOperatorNote = $true
            Write-Host "operator_note_requires_triage=1 step=$($record.id)"
            Write-Host "human_review_stopped_after_operator_note=1"
            break
        }
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
    $carryRecordById = $carryForwardRecordSet.records

    for ($summaryIndex = 0; $summaryIndex -lt $allSteps.Count; $summaryIndex++) {
        $stepId = [string]$allSteps[$summaryIndex].id
        if ($newRecordById.ContainsKey($stepId)) {
            $summaryRecords.Add((Copy-ReviewRecordForSummary -Record $newRecordById[$stepId] -Index ($summaryIndex + 1))) | Out-Null
        } elseif ($carryRecordById.ContainsKey($stepId)) {
            $summaryRecords.Add((Copy-ReviewRecordForSummary -Record $carryRecordById[$stepId] -Index ($summaryIndex + 1) -CarriedForward $true -CarriedForwardFrom ([string]$carryForwardRecordSet.sources_by_id[$stepId])) ) | Out-Null
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
} elseif ($incompleteCount -gt 0 -or $summaryRecordArray.Count -ne $expectedRecordCount -or $stoppedAfterOperatorNote) {
    "HUMAN_REVIEW_INCOMPLETE"
} else {
    "HUMAN_REVIEW_PASS"
}

$focusedReviewStatus = ""
if ($requestedStepIds.Count -gt 0) {
    $focusedReviewStatus = Get-FocusedReviewStatus `
        -Records @($records | ForEach-Object { $_ }) `
        -ExpectedRecordCount $steps.Count `
        -StoppedAfterOperatorNote $stoppedAfterOperatorNote
}

$passCount = @($recordsWithResult | Where-Object { $_.result -eq "PASS" }).Count
$skipCount = @($recordsWithResult | Where-Object { $_.result -eq "SKIP" }).Count
$abortCount = @($recordsWithResult | Where-Object { $_.result -eq "ABORT" }).Count
$progress = [ordered]@{
    rule = "One overall acceptance session shows total scope and progress, then advances one focused item at a time; any operator note stops progress for triage."
    total_items = $expectedRecordCount
    current_session_items = $steps.Count
    completed_items = $recordsWithResult.Count
    passed_items = $passCount
    failed_items = $failCount
    skipped_items = $skipCount
    aborted_items = $abortCount
    missing_result_items = $missingResultCount
    incomplete_items = $incompleteCount
    carried_forward_items = $carriedForwardCount
    carried_forward_blocked_items = @($carryForwardRecordSet.blocked_items)
    total_review_state_records = $carryForwardRecordSet.records.Count
    stopped_after_operator_note = $stoppedAfterOperatorNote
    focus_mode = $requestedStepIds.Count -gt 0
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
$totalReviewAdvanceScript = if (-not [string]::IsNullOrWhiteSpace($carryForwardRecordSet.state_path)) {
    Join-Path $PSScriptRoot "advance-preproduction-total-review.ps1"
} else {
    ""
}
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
    focused_review_status = $focusedReviewStatus
    generated_at = (Get-Date).ToString("o")
    repo_root = $repoRoot
    output_dir = $OutputDir
    session_jsonl = $sessionPath
    device_name = $DeviceName
    random_name = $RandomName
    focus_step_ids = @($requestedStepIds)
    progress = $progress
    carried_forward_summary = $(if ($carryForwardSummary) { $carryForwardSummary.path } else { "" })
    carried_forward_count = $carriedForwardCount
    total_review_state = $carryForwardRecordSet.state_path
    total_review_next_step_id = $carryForwardRecordSet.next_step_id
    total_review_advance_script = $totalReviewAdvanceScript
    carried_forward_sources = @(
        foreach ($step in $allSteps) {
            $stepId = [string]$step.id
            if ($carryForwardRecordSet.sources_by_id.ContainsKey($stepId)) {
                [ordered]@{
                    id = $stepId
                    summary_path = [string]$carryForwardRecordSet.sources_by_id[$stepId]
                }
            }
        }
    )
    carried_forward_blocked_items = @($carryForwardRecordSet.blocked_items)
    stopped_after_operator_note = $stoppedAfterOperatorNote
    operator_note_review_status = $(if ($operatorNoteArray.Count -gt 0) { "PENDING" } else { "PASS" })
    operator_note_review_required_count = $operatorNoteArray.Count
    operator_note_recorded_count = $operatorNoteArray.Count
    operator_note_triage_template = $triageTemplatePath
    operator_notes = @($operatorNoteArray)
    type_git_head = $typeHeadInfo
    type_git_worktree = $typeWorktreeInfo
    firmware_git_head = $firmwareHeadInfo
    firmware_git_worktree = $firmwareWorktreeInfo
    records = @($summaryRecordArray)
} | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $summaryJsonPath -Encoding UTF8

$lines = [System.Collections.Generic.List[string]]::new()
$lines.Add("# Listener 1.0.2 Preproduction Human Review") | Out-Null
$lines.Add("") | Out-Null
$lines.Add("- Status: $status") | Out-Null
if (-not [string]::IsNullOrWhiteSpace($focusedReviewStatus)) {
    $lines.Add("- Focused review status: $focusedReviewStatus") | Out-Null
}
$lines.Add("- Output: $OutputDir") | Out-Null
$lines.Add("- Session: $sessionPath") | Out-Null
$lines.Add("- Random BLE name: $RandomName") | Out-Null
$lines.Add("- Focus steps: $(if ($requestedStepIds.Count -gt 0) { $requestedStepIds -join ', ' } else { 'FULL' })") | Out-Null
$lines.Add("- Progress: passed $passCount/$expectedRecordCount, failed $failCount, incomplete $incompleteCount, carried forward $carriedForwardCount, current session $($steps.Count) item(s).") | Out-Null
$lines.Add("- Review rule: one overall acceptance session shows total scope and progress, then advances one focused item at a time; any operator note stops progress for triage.") | Out-Null
$lines.Add("- Total-review advance script: $(if ([string]::IsNullOrWhiteSpace($totalReviewAdvanceScript)) { 'not applicable' } else { $totalReviewAdvanceScript })") | Out-Null
$lines.Add("- Carried forward: $carriedForwardCount") | Out-Null
$lines.Add("- Stopped after operator note: $stoppedAfterOperatorNote") | Out-Null
$lines.Add("- Operator notes requiring triage: $($operatorNoteArray.Count)") | Out-Null
$lines.Add("- Blank operator notes mean normal pass with no extra remarks.") | Out-Null
$lines.Add("- Operator note triage template: $triageTemplatePath") | Out-Null
$lines.Add("- Type HEAD: $($typeHeadInfo.head) $($typeHeadInfo.commit_time)") | Out-Null
$lines.Add("- Type worktree dirty: $($typeWorktreeInfo.dirty) ($($typeWorktreeInfo.dirty_count) paths)") | Out-Null
$lines.Add("- Firmware HEAD: $($firmwareHeadInfo.head) $($firmwareHeadInfo.commit_time)") | Out-Null
$lines.Add("- Firmware worktree dirty: $($firmwareWorktreeInfo.dirty) ($($firmwareWorktreeInfo.dirty_count) paths)") | Out-Null
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
if (-not [string]::IsNullOrWhiteSpace($focusedReviewStatus)) {
    Write-Host "focused_human_review_status=$focusedReviewStatus"
}
Write-Host "summary=$summaryPath"
Write-Host "session=$sessionPath"
if ($status -eq "HUMAN_REVIEW_PASS" -or $focusedReviewStatus -eq "FOCUSED_HUMAN_REVIEW_PASS") {
    exit 0
}
exit 2
