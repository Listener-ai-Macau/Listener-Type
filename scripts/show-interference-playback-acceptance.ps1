# Loop a "other speaker" voice from a small window so owner can test
# dictation under interference without changing isolation policy.
param(
    [string]$OutputPath = "",
    [string]$SpokenText = "今天下午三点开会，然后我们把方案再过一遍。如果没问题就按这个执行。"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Speech
[System.Windows.Forms.Application]::EnableVisualStyles()

$artifactDir = Join-Path (Split-Path $PSScriptRoot -Parent) ".artifacts\windows-msvc"
[System.IO.Directory]::CreateDirectory($artifactDir) | Out-Null
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $artifactDir "owner-result.txt"
}
$wavPath = Join-Path $artifactDir "interference-other-speaker.wav"

function Write-InterferenceWav {
    param([string]$Path)
    $synth = [System.Speech.Synthesis.SpeechSynthesizer]::new()
    try {
        $other = $synth.GetInstalledVoices() |
            Where-Object { $_.Enabled -and $_.VoiceInfo.Name -notmatch 'Huihui|Xiaoxiao|Yaoyao' } |
            Select-Object -First 1
        if ($null -ne $other) {
            $synth.SelectVoice($other.VoiceInfo.Name)
        }
        $synth.Rate = 0
        $synth.Volume = 80
        $synth.SetOutputToWaveFile($Path)
        $synth.Speak(
            "我是旁边的人在说话。今天天气不错，我们待会去吃饭。这个文件先放这里，你先忙你的。旁边电视还开着新闻，不要把我的话写进正文。"
        )
        $synth.Speak(
            "再重复一遍干扰：旁边有人聊天，键盘在敲，新闻在播。请继续说你自己的话。"
        )
    }
    finally {
        $synth.Dispose()
    }
}

if (-not (Test-Path -LiteralPath $wavPath) -or (Get-Item -LiteralPath $wavPath).Length -lt 8000) {
    Write-InterferenceWav -Path $wavPath
}

$player = [System.Media.SoundPlayer]::new($wavPath)
$player.Load()
$script:selection = "中止"

$form = New-Object System.Windows.Forms.Form
$form.Text = "Listener 干扰测试"
$form.Width = 720
$form.Height = 520
$form.StartPosition = "Manual"
$form.Location = New-Object System.Drawing.Point(40, 40)
$form.TopMost = $true
$form.Font = New-Object System.Drawing.Font("Microsoft YaHei UI", 11)

$label = New-Object System.Windows.Forms.Label
$label.Left = 20
$label.Top = 16
$label.Width = 660
$label.Height = 220
$label.Text = @"
旁人声会循环播放（电脑喇叭）。请对着板子测，不要改隔离策略。

1. 点「开始干扰」
2. 说「开始录音」
3. 朗读下面原文（中间可以停一下）

通过：能唤醒；胶囊出你的字；上屏有你的话，不要整段空白。旁人那句可以没有。
失败：唤不醒，或你的字被吃光。
"@
$form.Controls.Add($label)

$spoken = New-Object System.Windows.Forms.TextBox
$spoken.Left = 20
$spoken.Top = 250
$spoken.Width = 660
$spoken.Height = 70
$spoken.Multiline = $true
$spoken.ReadOnly = $true
$spoken.Text = $SpokenText
$form.Controls.Add($spoken)

$notes = New-Object System.Windows.Forms.TextBox
$notes.Left = 20
$notes.Top = 330
$notes.Width = 660
$notes.Height = 50
$notes.Multiline = $true
$form.Controls.Add($notes)

function Save-Result([string]$Selection) {
    $script:selection = $Selection
    try { $player.Stop() } catch { }
    $text = $notes.Text.Trim()
    [System.IO.File]::WriteAllText($OutputPath, $(if ($text) { $text } else { $Selection }), [System.Text.UTF8Encoding]::new($false))
    $form.Close()
}

$play = New-Object System.Windows.Forms.Button
$play.Text = "开始干扰"
$play.Left = 20
$play.Top = 400
$play.Width = 120
$play.Add_Click({ $player.PlayLooping(); $play.Text = "干扰播放中" })
$form.Controls.Add($play)

$stop = New-Object System.Windows.Forms.Button
$stop.Text = "停止干扰"
$stop.Left = 150
$stop.Top = 400
$stop.Width = 120
$stop.Add_Click({ $player.Stop(); $play.Text = "开始干扰" })
$form.Controls.Add($stop)

$pass = New-Object System.Windows.Forms.Button
$pass.Text = "通过"
$pass.Left = 360
$pass.Top = 400
$pass.Width = 90
$pass.Add_Click({ Save-Result "通过" })
$form.Controls.Add($pass)

$fail = New-Object System.Windows.Forms.Button
$fail.Text = "失败"
$fail.Left = 460
$fail.Top = 400
$fail.Width = 90
$fail.Add_Click({ Save-Result "失败" })
$form.Controls.Add($fail)

$skip = New-Object System.Windows.Forms.Button
$skip.Text = "跳过"
$skip.Left = 560
$skip.Top = 400
$width = 90
$skip.Width = $width
$skip.Add_Click({ Save-Result "跳过" })
$form.Controls.Add($skip)

$form.Add_FormClosed({ try { $player.Stop(); $player.Dispose() } catch { } })
[void]$form.ShowDialog()
Write-Output (@{ selected = $script:selection; output_path = $OutputPath; wav = $wavPath } | ConvertTo-Json -Compress)
