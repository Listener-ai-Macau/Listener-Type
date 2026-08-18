param(
    [Parameter(Mandatory = $true)]
    [string]$LogPath,

    [Parameter(Mandatory = $true)]
    [string]$OutputPath,

    [ValidateRange(1, 100)]
    [int]$Attempts = 5,

    [ValidateRange(1000, 30000)]
    [int]$AssociationTimeoutMs = 6000,

    [ValidateRange(6000, 30000)]
    [int]$RoundSettleMs = 7000
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()

$resolvedLogPath = [System.IO.Path]::GetFullPath($LogPath)
$resolvedOutputPath = [System.IO.Path]::GetFullPath($OutputPath)
if (-not [System.IO.File]::Exists($resolvedLogPath)) {
    [System.Windows.Forms.MessageBox]::Show(
        "找不到 Listener 日志：`n$resolvedLogPath",
        'Listener 唤醒验收',
        [System.Windows.Forms.MessageBoxButtons]::OK,
        [System.Windows.Forms.MessageBoxIcon]::Error
    ) | Out-Null
    exit 1
}

function Read-AppendedLogText {
    param(
        [string]$Path,
        [long]$Offset
    )

    try {
        $stream = [System.IO.FileStream]::new(
            $Path,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            ([System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete)
        )
        try {
            if ($stream.Length -le $Offset) {
                return ''
            }
            [void]$stream.Seek($Offset, [System.IO.SeekOrigin]::Begin)
            $remaining = [int]($stream.Length - $Offset)
            $buffer = [byte[]]::new($remaining)
            $read = $stream.Read($buffer, 0, $remaining)
            return [System.Text.Encoding]::UTF8.GetString($buffer, 0, $read)
        }
        finally {
            $stream.Dispose()
        }
    }
    catch [System.IO.IOException] {
        # Listener can rotate or reopen its log between timer ticks. Treat that
        # as a transient empty read; the 100 ms timer will retry without
        # surfacing a WinForms unhandled-exception dialog to the operator.
        return ''
    }
}

function Get-LatestFrontendCapsuleState {
    try {
        $logLength = ([System.IO.FileInfo]::new($resolvedLogPath)).Length
    }
    catch [System.IO.IOException] {
        return 'unknown'
    }
    $tailBytes = 256KB
    $tailOffset = [Math]::Max(0L, $logLength - $tailBytes)
    $text = Read-AppendedLogText -Path $resolvedLogPath -Offset $tailOffset
    $statePattern =
        'source=frontend\.capsule event=event_received state=(recording|transcribing|polishing|done|error|idle)'
    $matches = [regex]::Matches(
        $text,
        $statePattern,
        [System.Text.RegularExpressions.RegexOptions]::IgnoreCase
    )
    if ($matches.Count -gt 0) {
        $script:lastKnownCapsuleState =
            $matches[$matches.Count - 1].Groups[1].Value.ToLowerInvariant()
        return $script:lastKnownCapsuleState
    }

    # A noisy room can append megabytes of rejected wake candidates after the
    # last capsule transition. On the first lookup only, fall back to the whole
    # log so the operator still starts from the real latest state. Later timer
    # ticks reuse that state and only inspect the bounded tail above.
    if ($script:lastKnownCapsuleState -eq 'unknown' -and $tailOffset -gt 0) {
        $fullText = Read-AppendedLogText -Path $resolvedLogPath -Offset 0
        $matches = [regex]::Matches(
            $fullText,
            $statePattern,
            [System.Text.RegularExpressions.RegexOptions]::IgnoreCase
        )
        if ($matches.Count -gt 0) {
            $script:lastKnownCapsuleState =
                $matches[$matches.Count - 1].Groups[1].Value.ToLowerInvariant()
        }
    }
    return $script:lastKnownCapsuleState
}

function Write-MarkerArtifact {
    param([bool]$Completed)

    $parent = [System.IO.Path]::GetDirectoryName($resolvedOutputPath)
    [System.IO.Directory]::CreateDirectory($parent) | Out-Null
    $payload = [ordered]@{
        schema = 'listener.live-wake-attempt-markers.v1'
        scenario = 'real-device-natural-wake-only'
        log = $resolvedLogPath
        startedAt = $script:startedAt
        completedAt = if ($Completed) { [DateTime]::UtcNow.ToString('o') } else { $null }
        associationTimeoutMs = $AssociationTimeoutMs
        attempts = @($script:markers)
    }
    $payload | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $resolvedOutputPath -Encoding UTF8
}

$script:markers = [System.Collections.Generic.List[object]]::new()
$script:ordinal = 1
$script:roundStartedAt = $null
$script:roundDeadline = $null
$script:roundSettleAt = $null
$script:roundLogOffset = 0L
$script:roundCandidateId = $null
$script:startedAt = [DateTime]::UtcNow.ToString('o')
$script:lastKnownCapsuleState = 'unknown'

$form = [System.Windows.Forms.Form]::new()
$form.Text = 'Listener 唤醒验收'
$form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
$form.Size = [System.Drawing.Size]::new(560, 390)
$form.MinimumSize = [System.Drawing.Size]::new(560, 390)
$form.TopMost = $true
$form.MaximizeBox = $false
$form.Font = [System.Drawing.Font]::new('Microsoft YaHei UI', 10)

$title = [System.Windows.Forms.Label]::new()
$title.Text = '真人唤醒验收'
$title.Font = [System.Drawing.Font]::new('Microsoft YaHei UI', 18, [System.Drawing.FontStyle]::Bold)
$title.AutoSize = $true
$title.Location = [System.Drawing.Point]::new(28, 24)
$form.Controls.Add($title)

$privacy = [System.Windows.Forms.Label]::new()
$privacy.Text = '只记录轮次、时间和数字 session ID；不保存音频或转写内容。'
$privacy.ForeColor = [System.Drawing.Color]::DimGray
$privacy.AutoSize = $true
$privacy.Location = [System.Drawing.Point]::new(31, 68)
$form.Controls.Add($privacy)

$instruction = [System.Windows.Forms.Label]::new()
$instruction.Text = '每轮点击按钮后，立即自然说一次“开始录音”。' + [Environment]::NewLine + '弹窗会等待设备候选，并自动留出胶囊结束时间。'
$instruction.AutoSize = $true
$instruction.Location = [System.Drawing.Point]::new(31, 106)
$form.Controls.Add($instruction)

$progress = [System.Windows.Forms.ProgressBar]::new()
$progress.Minimum = 0
$progress.Maximum = $Attempts
$progress.Value = 0
$progress.Size = [System.Drawing.Size]::new(490, 18)
$progress.Location = [System.Drawing.Point]::new(31, 166)
$form.Controls.Add($progress)

$roundLabel = [System.Windows.Forms.Label]::new()
$roundLabel.Text = "准备第 1/$Attempts 轮"
$roundLabel.Font = [System.Drawing.Font]::new('Microsoft YaHei UI', 12, [System.Drawing.FontStyle]::Bold)
$roundLabel.AutoSize = $true
$roundLabel.Location = [System.Drawing.Point]::new(31, 202)
$form.Controls.Add($roundLabel)

$statusLabel = [System.Windows.Forms.Label]::new()
$statusLabel.Text = '准备好后点击下面的按钮。'
$statusLabel.AutoSize = $false
$statusLabel.Size = [System.Drawing.Size]::new(490, 48)
$statusLabel.Location = [System.Drawing.Point]::new(31, 236)
$form.Controls.Add($statusLabel)

$startButton = [System.Windows.Forms.Button]::new()
$startButton.Text = '开始第 1 次'
$startButton.Size = [System.Drawing.Size]::new(220, 48)
$startButton.Location = [System.Drawing.Point]::new(31, 292)
$startButton.BackColor = [System.Drawing.Color]::FromArgb(43, 108, 176)
$startButton.ForeColor = [System.Drawing.Color]::White
$startButton.FlatStyle = [System.Windows.Forms.FlatStyle]::Flat
$form.Controls.Add($startButton)

$closeButton = [System.Windows.Forms.Button]::new()
$closeButton.Text = '稍后再测'
$closeButton.Size = [System.Drawing.Size]::new(120, 48)
$closeButton.Location = [System.Drawing.Point]::new(401, 292)
$closeButton.Add_Click({ $form.Close() })
$form.Controls.Add($closeButton)

$timer = [System.Windows.Forms.Timer]::new()
$timer.Interval = 100

$startButton.Add_Click({
    if ($script:ordinal -gt $Attempts -or $null -ne $script:roundStartedAt) {
        return
    }
    $currentState = Get-LatestFrontendCapsuleState
    if ($currentState -ne 'idle') {
        $statusLabel.Text = "Listener 当前状态是 $currentState。请等胶囊完全消失后再点击。"
        $statusLabel.ForeColor = [System.Drawing.Color]::FromArgb(180, 83, 9)
        return
    }
    $script:roundStartedAt = [DateTime]::UtcNow
    $script:roundDeadline = $script:roundStartedAt.AddMilliseconds($AssociationTimeoutMs)
    $script:roundSettleAt = $script:roundStartedAt.AddMilliseconds($RoundSettleMs)
    $script:roundLogOffset = ([System.IO.FileInfo]::new($resolvedLogPath)).Length
    $script:roundCandidateId = $null
    $startButton.Enabled = $false
    $roundLabel.Text = "第 $($script:ordinal)/$Attempts 轮：现在说"
    $statusLabel.Text = '请现在自然说：开始录音'
    $statusLabel.ForeColor = [System.Drawing.Color]::FromArgb(43, 108, 176)
    $timer.Start()
})

$timer.Add_Tick({
    $now = [DateTime]::UtcNow
    if ($null -eq $script:roundCandidateId -and $now -le $script:roundDeadline) {
        $appended = Read-AppendedLogText -Path $resolvedLogPath -Offset $script:roundLogOffset
        $match = [regex]::Match($appended, 'event=start embedded_session_id=(\d+) origin=VoiceActivation')
        if ($match.Success) {
            $script:roundCandidateId = [int]$match.Groups[1].Value
            $statusLabel.Text = "已捕获设备候选 $($script:roundCandidateId)，请等待胶囊结束……"
            $statusLabel.ForeColor = [System.Drawing.Color]::FromArgb(39, 103, 73)
        }
    }

    if ($now -lt $script:roundSettleAt) {
        return
    }

    $currentState = Get-LatestFrontendCapsuleState
    if ($currentState -ne 'idle') {
        $statusLabel.Text = "已捕获本轮；正在等待 Listener 从 $currentState 回到 idle……"
        $statusLabel.ForeColor = [System.Drawing.Color]::FromArgb(180, 83, 9)
        return
    }

    $timer.Stop()
    $marker = [ordered]@{
        ordinal = $script:ordinal
        markedAt = $script:roundStartedAt.ToString('o')
        associationEndedAt = $now.ToString('o')
        logByteOffset = $script:roundLogOffset
        embeddedSessionId = $script:roundCandidateId
    }
    $script:markers.Add([pscustomobject]$marker)
    Write-MarkerArtifact -Completed:$false
    $progress.Value = $script:ordinal

    if ($script:ordinal -ge $Attempts) {
        Write-MarkerArtifact -Completed:$true
        $roundLabel.Text = "已完成 $Attempts/$Attempts 轮"
        $statusLabel.Text = "验收标注已保存。请回到 Codex 回复：5次测完"
        $statusLabel.ForeColor = [System.Drawing.Color]::FromArgb(39, 103, 73)
        $startButton.Text = '验收标注完成'
        $startButton.Enabled = $false
        $closeButton.Text = '关闭'
        [System.Windows.Forms.MessageBox]::Show(
            "5 次标注已完成。`n请回到 Codex 回复：5次测完",
            'Listener 唤醒验收',
            [System.Windows.Forms.MessageBoxButtons]::OK,
            [System.Windows.Forms.MessageBoxIcon]::Information
        ) | Out-Null
        return
    }

    $resultText = if ($null -eq $script:roundCandidateId) {
        '本轮未观察到设备候选，已明确记为失败。'
    }
    else {
        "本轮已关联候选 $($script:roundCandidateId)。"
    }
    $script:ordinal += 1
    $script:roundStartedAt = $null
    $script:roundDeadline = $null
    $script:roundSettleAt = $null
    $script:roundCandidateId = $null
    $roundLabel.Text = "准备第 $($script:ordinal)/$Attempts 轮"
    $statusLabel.Text = "$resultText 准备好后开始下一轮。"
    $statusLabel.ForeColor = [System.Drawing.Color]::DimGray
    $startButton.Text = "开始第 $($script:ordinal) 次"
    $startButton.Enabled = $true
})

$form.Add_FormClosing({
    $timer.Stop()
    if ($script:markers.Count -gt 0 -and $script:markers.Count -lt $Attempts) {
        Write-MarkerArtifact -Completed:$false
    }
})

[void]$form.ShowDialog()
