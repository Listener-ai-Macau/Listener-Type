param(
    [Parameter(Mandatory = $true)]
    [string]$InterferenceWav,

    [Parameter(Mandatory = $true)]
    [string]$LogPath,

    [Parameter(Mandatory = $true)]
    [string]$OutputPath,

    [Parameter(Mandatory = $true)]
    [string]$InstalledExe,

    [Parameter(Mandatory = $true)]
    [string]$MsiPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System
[System.Windows.Forms.Application]::EnableVisualStyles()

$interference = [System.IO.Path]::GetFullPath($InterferenceWav)
$log = [System.IO.Path]::GetFullPath($LogPath)
$output = [System.IO.Path]::GetFullPath($OutputPath)
$installed = [System.IO.Path]::GetFullPath($InstalledExe)
$msi = [System.IO.Path]::GetFullPath($MsiPath)
foreach ($required in @($interference, $log, $installed, $msi)) {
    if (-not [System.IO.File]::Exists($required)) {
        throw "Acceptance input is missing: $required"
    }
}

$spokenText = '开始录音。现在是主人第一句，我会在旁人说话时自然停顿一下。现在继续说主人第二句，最后这句话也不能丢。'
$script:selection = 'aborted'
$script:startedAt = $null
$script:logOffset = 0L
$script:sawRecording = $false
$script:sawDone = $false
$script:playbackStopped = $false
$script:player = [System.Media.SoundPlayer]::new($interference)
$script:player.Load()

function Read-AppendedLogText {
    if ($null -eq $script:startedAt) { return '' }
    try {
        $stream = [System.IO.FileStream]::new(
            $log,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            ([System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete)
        )
        try {
            if ($stream.Length -le $script:logOffset) { return '' }
            [void]$stream.Seek($script:logOffset, [System.IO.SeekOrigin]::Begin)
            $remaining = [int]($stream.Length - $script:logOffset)
            $buffer = [byte[]]::new($remaining)
            $read = $stream.Read($buffer, 0, $remaining)
            return [System.Text.Encoding]::UTF8.GetString($buffer, 0, $read)
        }
        finally {
            $stream.Dispose()
        }
    }
    catch [System.IO.IOException] {
        return ''
    }
}

function Save-Result {
    param([string]$Selection)
    $script:selection = $Selection
    $script:player.Stop()
    $parent = [System.IO.Path]::GetDirectoryName($output)
    [System.IO.Directory]::CreateDirectory($parent) | Out-Null
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $promptBytes = [Text.Encoding]::UTF8.GetBytes($spokenText)
        $promptHash = [System.BitConverter]::ToString($sha.ComputeHash($promptBytes)).Replace('-', '')
    }
    finally {
        $sha.Dispose()
    }
    $payload = [ordered]@{
        schema = 'listener.cn-interference-human-acceptance.v1'
        selection = $Selection
        startedAt = if ($null -eq $script:startedAt) { $null } else { $script:startedAt.ToString('o') }
        completedAt = [DateTime]::UtcNow.ToString('o')
        sawRecording = $script:sawRecording
        sawDone = $script:sawDone
        interferenceSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $interference).Hash
        spokenPromptSha256 = $promptHash
        installedExeSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $installed).Hash
        msiSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $msi).Hash
        note = $note.Text.Trim()
    }
    $payload | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $output -Encoding UTF8
}

$form = [System.Windows.Forms.Form]::new()
$form.Text = 'Listener 中文多人干扰最终验收'
$form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
$form.Size = [System.Drawing.Size]::new(760, 590)
$form.MinimumSize = [System.Drawing.Size]::new(760, 590)
$form.TopMost = $true
$form.MaximizeBox = $false
$form.Font = [System.Drawing.Font]::new('Microsoft YaHei UI', 10)

$title = [System.Windows.Forms.Label]::new()
$title.Text = '中文多人干扰最终验收'
$title.Font = [System.Drawing.Font]::new('Microsoft YaHei UI', 19, [System.Drawing.FontStyle]::Bold)
$title.AutoSize = $true
$title.Location = [System.Drawing.Point]::new(28, 22)
$form.Controls.Add($title)

$instruction = [System.Windows.Forms.Label]::new()
$instruction.Text = '点击“播放干扰并开始”，听到旁人中文后立即自然朗读下面整段。中间停顿约 0.7 秒，等胶囊自动结束，再按结果选择。'
$instruction.AutoSize = $false
$instruction.Size = [System.Drawing.Size]::new(690, 48)
$instruction.Location = [System.Drawing.Point]::new(31, 68)
$form.Controls.Add($instruction)

$promptLabel = [System.Windows.Forms.Label]::new()
$promptLabel.Text = '只读朗读原文：'
$promptLabel.AutoSize = $true
$promptLabel.Location = [System.Drawing.Point]::new(31, 124)
$form.Controls.Add($promptLabel)

$prompt = [System.Windows.Forms.TextBox]::new()
$prompt.Text = $spokenText
$prompt.ReadOnly = $true
$prompt.Multiline = $true
$prompt.WordWrap = $true
$prompt.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
$prompt.BackColor = [System.Drawing.Color]::FromArgb(245, 248, 250)
$prompt.Font = [System.Drawing.Font]::new('Microsoft YaHei UI', 13)
$prompt.Size = [System.Drawing.Size]::new(690, 112)
$prompt.Location = [System.Drawing.Point]::new(31, 151)
$form.Controls.Add($prompt)

$status = [System.Windows.Forms.Label]::new()
$status.Text = '准备好后点击开始；音频不会在点击前播放。'
$status.AutoSize = $false
$status.Size = [System.Drawing.Size]::new(690, 44)
$status.Location = [System.Drawing.Point]::new(31, 279)
$status.ForeColor = [System.Drawing.Color]::DimGray
$form.Controls.Add($status)

$start = [System.Windows.Forms.Button]::new()
$start.Text = '播放干扰并开始'
$start.Size = [System.Drawing.Size]::new(220, 48)
$start.Location = [System.Drawing.Point]::new(31, 329)
$start.BackColor = [System.Drawing.Color]::FromArgb(43, 108, 176)
$start.ForeColor = [System.Drawing.Color]::White
$start.FlatStyle = [System.Windows.Forms.FlatStyle]::Flat
$form.Controls.Add($start)

$noteLabel = [System.Windows.Forms.Label]::new()
$noteLabel.Text = '失败时可简短说明（可不填）：'
$noteLabel.AutoSize = $true
$noteLabel.Location = [System.Drawing.Point]::new(31, 393)
$form.Controls.Add($noteLabel)

$note = [System.Windows.Forms.TextBox]::new()
$note.Size = [System.Drawing.Size]::new(690, 29)
$note.Location = [System.Drawing.Point]::new(31, 420)
$form.Controls.Add($note)

$pass = [System.Windows.Forms.Button]::new()
$pass.Text = '通过'
$pass.Size = [System.Drawing.Size]::new(160, 48)
$pass.Location = [System.Drawing.Point]::new(31, 470)
$pass.BackColor = [System.Drawing.Color]::FromArgb(39, 103, 73)
$pass.ForeColor = [System.Drawing.Color]::White
$pass.Enabled = $false
$form.Controls.Add($pass)

$fail = [System.Windows.Forms.Button]::new()
$fail.Text = '失败，继续修'
$fail.Size = [System.Drawing.Size]::new(180, 48)
$fail.Location = [System.Drawing.Point]::new(207, 470)
$fail.BackColor = [System.Drawing.Color]::FromArgb(180, 60, 60)
$fail.ForeColor = [System.Drawing.Color]::White
$fail.Enabled = $false
$form.Controls.Add($fail)

$abort = [System.Windows.Forms.Button]::new()
$abort.Text = '中止'
$abort.Size = [System.Drawing.Size]::new(120, 48)
$abort.Location = [System.Drawing.Point]::new(601, 470)
$form.Controls.Add($abort)

$timer = [System.Windows.Forms.Timer]::new()
$timer.Interval = 250
$timer.Add_Tick({
    if ($null -eq $script:startedAt) { return }
    $elapsed = ([DateTime]::UtcNow - $script:startedAt).TotalSeconds
    $text = Read-AppendedLogText
    if ($text -match 'source=frontend\.capsule event=event_received state=recording') {
        $script:sawRecording = $true
    }
    if ($text -match 'source=frontend\.capsule event=event_received state=done') {
        $script:sawDone = $true
    }
    if (-not $script:playbackStopped -and $elapsed -ge 18) {
        $script:player.Stop()
        $script:playbackStopped = $true
    }
    if ($script:sawDone) {
        $status.Text = '已看到胶囊完成。请检查：只保留你的内容、首尾完整、没有旁人文字，然后选择结果。'
        $status.ForeColor = [System.Drawing.Color]::FromArgb(39, 103, 73)
        $pass.Enabled = $true
    }
    elseif ($script:sawRecording) {
        $status.Text = '已成功唤醒并录音；请说完整段并等待自动结束。'
        $status.ForeColor = [System.Drawing.Color]::FromArgb(43, 108, 176)
    }
    elseif ($elapsed -ge 8) {
        $status.Text = '尚未看到录音胶囊。如果确实没有唤醒，请选择“失败，继续修”。'
        $status.ForeColor = [System.Drawing.Color]::FromArgb(180, 60, 60)
    }
})

$start.Add_Click({
    $script:startedAt = [DateTime]::UtcNow
    $script:logOffset = ([System.IO.FileInfo]::new($log)).Length
    $start.Enabled = $false
    $fail.Enabled = $true
    $status.Text = '干扰正在播放：现在立即从“开始录音”朗读完整段。'
    $status.ForeColor = [System.Drawing.Color]::FromArgb(43, 108, 176)
    $script:player.PlayLooping()
    $timer.Start()
})

$pass.Add_Click({ Save-Result -Selection 'passed'; $form.Close() })
$fail.Add_Click({ Save-Result -Selection 'failed'; $form.Close() })
$abort.Add_Click({ Save-Result -Selection 'aborted'; $form.Close() })
$form.Add_FormClosing({
    $script:player.Stop()
    if (-not [System.IO.File]::Exists($output)) {
        Save-Result -Selection 'aborted'
    }
})

[void]$form.ShowDialog()
switch ($script:selection) {
    'passed' { exit 0 }
    'failed' { exit 2 }
    default { exit 3 }
}
