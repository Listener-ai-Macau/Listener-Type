param(
    [Parameter(Mandatory = $true)]
    [string]$InterferenceWav,

    [Parameter(Mandatory = $true)]
    [string]$LogPath,

    [Parameter(Mandatory = $true)]
    [string]$OutputPath,

    [Parameter(Mandatory = $true)]
    [string]$CandidateExe,

    [Parameter(Mandatory = $true)]
    [string]$CandidateId,

    [switch]$SelfTest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System
[System.Windows.Forms.Application]::EnableVisualStyles()

$interference = [System.IO.Path]::GetFullPath($InterferenceWav)
$log = [System.IO.Path]::GetFullPath($LogPath)
$output = [System.IO.Path]::GetFullPath($OutputPath)
$candidateExe = [System.IO.Path]::GetFullPath($CandidateExe)
foreach ($required in @($interference, $log, $candidateExe)) {
    if (-not [System.IO.File]::Exists($required)) {
        throw "Development acceptance input is missing: $required"
    }
}

$spokenText = '开始录音。现在是主人第一句，我会在旁人说话时自然停顿一下。现在继续说主人第二句，最后这句话也不能丢。'
$player = [System.Media.SoundPlayer]::new($interference)
$player.Load()

function Get-FileSha256Hex {
    param([string]$Path)

    $sha = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        return [System.BitConverter]::ToString(
            $sha.ComputeHash($stream)).Replace('-', '')
    }
    finally {
        $stream.Dispose()
        $sha.Dispose()
    }
}

function Get-TextSha256Hex {
    param([string]$Text)

    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($Text)
        return [System.BitConverter]::ToString(
            $sha.ComputeHash($bytes)).Replace('-', '')
    }
    finally {
        $sha.Dispose()
    }
}

function Read-LogSuffix {
    param([long]$Offset)

    try {
        $stream = [System.IO.FileStream]::new(
            $log,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            ([System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete)
        )
        try {
            if ($stream.Length -le $Offset) {
                return ''
            }
            [void]$stream.Seek($Offset, [System.IO.SeekOrigin]::Begin)
            $buffer = [byte[]]::new([int]($stream.Length - $Offset))
            $read = $stream.Read($buffer, 0, $buffer.Length)
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
    param(
        [string]$Selection,
        [datetime]$StartedAt,
        [long]$LogOffset
    )

    $text = Read-LogSuffix -Offset $LogOffset
    $candidateMatches = [regex]::Matches(
        $text,
        'event=start embedded_session_id=(\d+) origin=VoiceActivation')
    $candidateIds = @(
        foreach ($match in $candidateMatches) {
            [int]$match.Groups[1].Value
        }
    )
    $acceptedMatches = [regex]::Matches(
        $text,
        'automatic streaming gate embedded_session_id=(\d+).*gate_decision=Accept')
    $acceptedIds = @(
        foreach ($match in $acceptedMatches) {
            [int]$match.Groups[1].Value
        }
    )
    $parent = [System.IO.Path]::GetDirectoryName($output)
    [System.IO.Directory]::CreateDirectory($parent) | Out-Null
    [ordered]@{
        schema = 'listener.cn-interference-development-acceptance.v1'
        candidateId = $CandidateId
        selection = $Selection
        startedAt = $StartedAt.ToUniversalTime().ToString('o')
        completedAt = [DateTime]::UtcNow.ToString('o')
        logByteOffset = $LogOffset
        embeddedCandidateIds = $candidateIds
        acceptedEmbeddedSessionIds = $acceptedIds
        sawRecording = $text -match 'source=frontend\.capsule event=event_received state=recording'
        sawDone = $text -match 'source=frontend\.capsule event=event_received state=done'
        interferenceSha256 = Get-FileSha256Hex -Path $interference
        executableSha256 = Get-FileSha256Hex -Path $candidateExe
        spokenPromptSha256 = Get-TextSha256Hex -Text $spokenText
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $output -Encoding UTF8
}

try {
    if ($SelfTest) {
        Save-Result `
            -Selection 'self_test' `
            -StartedAt ([DateTime]::Now) `
            -LogOffset ([System.IO.FileInfo]::new($log)).Length
        $selfTestPayload = Get-Content -LiteralPath $output -Raw | ConvertFrom-Json
        if ($selfTestPayload.selection -ne 'self_test' -or
            [string]::IsNullOrWhiteSpace($selfTestPayload.executableSha256)) {
            throw 'Development acceptance self-test did not persist its evidence.'
        }
        Write-Output "SELF_TEST_OK candidate=$CandidateId"
        exit 0
    }

    $ready = [System.Windows.Forms.MessageBox]::Show(
        "开发候选：$CandidateId`n`n点击【是】后才会开始循环播放中文干扰。`n干扰开始后，请在干扰中朗读下一窗口里的完整原文。",
        'Listener 干扰验收：准备',
        [System.Windows.Forms.MessageBoxButtons]::YesNo,
        [System.Windows.Forms.MessageBoxIcon]::Information,
        [System.Windows.Forms.MessageBoxDefaultButton]::Button1,
        [System.Windows.Forms.MessageBoxOptions]::DefaultDesktopOnly)
    if ($ready -ne [System.Windows.Forms.DialogResult]::Yes) {
        Save-Result -Selection 'aborted' -StartedAt ([DateTime]::Now) -LogOffset ([System.IO.FileInfo]::new($log)).Length
        exit 3
    }

    $startedAt = [DateTime]::Now
    $logOffset = ([System.IO.FileInfo]::new($log)).Length
    $player.PlayLooping()
    $choice = [System.Windows.Forms.MessageBox]::Show(
        "干扰正在持续播放。请自然朗读：`n`n$spokenText`n`n说完后等待胶囊自动结束。`n是 = 通过；否 = 失败，继续修；取消 = 中止。",
        'Listener 干扰验收：正在测试',
        [System.Windows.Forms.MessageBoxButtons]::YesNoCancel,
        [System.Windows.Forms.MessageBoxIcon]::None,
        [System.Windows.Forms.MessageBoxDefaultButton]::Button2,
        [System.Windows.Forms.MessageBoxOptions]::DefaultDesktopOnly)
    $player.Stop()

    if ($choice -eq [System.Windows.Forms.DialogResult]::Yes) {
        Save-Result -Selection 'passed' -StartedAt $startedAt -LogOffset $logOffset
        exit 0
    }
    if ($choice -eq [System.Windows.Forms.DialogResult]::No) {
        Save-Result -Selection 'failed' -StartedAt $startedAt -LogOffset $logOffset
        exit 2
    }
    Save-Result -Selection 'aborted' -StartedAt $startedAt -LogOffset $logOffset
    exit 3
}
catch {
    $player.Stop()
    $errorPath = "$output.error.txt"
    "$(Get-Date -Format o)`n$($_ | Out-String)" | Set-Content -LiteralPath $errorPath -Encoding UTF8
    [System.Windows.Forms.MessageBox]::Show(
        "验收工具发生错误，已记录到：`n$errorPath",
        'Listener 干扰验收工具错误',
        [System.Windows.Forms.MessageBoxButtons]::OK,
        [System.Windows.Forms.MessageBoxIcon]::Error,
        [System.Windows.Forms.MessageBoxDefaultButton]::Button1,
        [System.Windows.Forms.MessageBoxOptions]::DefaultDesktopOnly) | Out-Null
    exit 1
}
finally {
    $player.Stop()
    $player.Dispose()
}
