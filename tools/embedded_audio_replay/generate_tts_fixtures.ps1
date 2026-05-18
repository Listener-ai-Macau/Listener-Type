param(
    [int]$Seed = 20260518,
    [int]$Count = 6,
    [string]$OutDir = "artifacts\embedded_audio_tts",
    [string]$VoiceName = "",
    [int]$PayloadBytes = 480,
    [switch]$SkipReplay
)

$ErrorActionPreference = "Stop"

function Get-RepoRoot {
    $scriptPath = Split-Path -Parent $PSCommandPath
    return (Resolve-Path (Join-Path $scriptPath "..\..")).Path
}

function Set-RustMsvcEnvironment {
    $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
    $msvcRoot = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC"
    $msvcVersion = Get-ChildItem -LiteralPath $msvcRoot -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending |
        Select-Object -First 1
    if ($null -eq $msvcVersion) {
        throw "MSVC Build Tools not found under $msvcRoot"
    }

    $msvcBin = Join-Path $msvcVersion.FullName "bin\Hostx64\x64"
    $msvcLib = Join-Path $msvcVersion.FullName "lib\x64"
    $windowsKitRoot = "C:\Program Files (x86)\Windows Kits\10\Lib"
    $windowsKitVersion = Get-ChildItem -LiteralPath $windowsKitRoot -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending |
        Select-Object -First 1
    if ($null -eq $windowsKitVersion) {
        throw "Windows 10 SDK libs not found under $windowsKitRoot"
    }

    $env:PATH = "$msvcBin;$cargoBin;$env:PATH"
    $env:LIB = "$msvcLib;$(Join-Path $windowsKitVersion.FullName 'ucrt\x64');$(Join-Path $windowsKitVersion.FullName 'um\x64')"
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = Join-Path $msvcBin "link.exe"
}

function New-RandomSentence {
    param(
        [System.Random]$Random
    )

    $categories = @(
        @{
            Name = "普通听写"
            Templates = @(
                "今天下午{time}提醒我检查{thing}。",
                "帮我记录一下，{thing}已经完成第一轮验证。",
                "下次会议前，把{thing}的状态同步给团队。"
            )
        },
        @{
            Name = "数字日期"
            Templates = @(
                "明天{time}提醒我检查蓝牙音频丢包率。",
                "把第{number}轮测试结果保存到{folder}目录。",
                "五月十八日晚上{time}复查P十三的通过率。"
            )
        },
        @{
            Name = "中英混合"
            Templates = @(
                "请把{term}的payload大小设置为{number}字节。",
                "Listener-Type收到{term}以后继续走ASR流程。",
                "如果{term}失败，就输出replay result json。"
            )
        },
        @{
            Name = "Agent指令"
            Templates = @(
                "帮我总结这次蓝牙音频联调的风险点。",
                "把刚才的测试结果整理成一条任务。",
                "根据P十三结果，判断下一步是否需要真实设备。"
            )
        },
        @{
            Name = "领域词表"
            Templates = @(
                "这次需要确认{term}和{term2}没有被重新解释。",
                "固件只负责采集PCM，软件负责ASR和插入。",
                "会话结束时要记录session id和expected packet count。"
            )
        }
    )

    $times = @("三点半", "十点十五分", "下午两点", "晚上八点")
    $things = @("固件合同", "随机语音样本", "蓝牙回放工具", "软件接入方案")
    $terms = @("BLE", "PCM", "VKA1", "session id", "audio data", "Listener-Type")
    $folders = @("artifacts", "p13", "logs", "reports")
    $numbers = @(3, 8, 16, 32, 480, 1024)

    $category = $categories[$Random.Next($categories.Count)]
    $template = $category.Templates[$Random.Next($category.Templates.Count)]
    $text = $template
    $text = $text.Replace("{time}", $times[$Random.Next($times.Count)])
    $text = $text.Replace("{thing}", $things[$Random.Next($things.Count)])
    $text = $text.Replace("{term}", $terms[$Random.Next($terms.Count)])
    $text = $text.Replace("{term2}", $terms[$Random.Next($terms.Count)])
    $text = $text.Replace("{folder}", $folders[$Random.Next($folders.Count)])
    $text = $text.Replace("{number}", [string]$numbers[$Random.Next($numbers.Count)])

    return [ordered]@{
        Category = $category.Name
        Text = $text
    }
}

function Normalize-ExpectedText {
    param([string]$Text)
    return (($Text.Trim() -replace "[\s，。！？、,.!?:：;；]+", "").ToLowerInvariant())
}

function Convert-VoiceToFile {
    param(
        [System.Speech.Synthesis.SpeechSynthesizer]$Synth,
        [string]$Text,
        [string]$Path
    )

    $format = [System.Speech.AudioFormat.SpeechAudioFormatInfo]::new(
        16000,
        [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen,
        [System.Speech.AudioFormat.AudioChannel]::Mono
    )
    $Synth.SetOutputToWaveFile($Path, $format)
    $Synth.Speak($Text) | Out-Null
    $Synth.SetOutputToNull()
}

function Invoke-ReplayTool {
    param(
        [string]$RepoRoot,
        [string]$WavPath,
        [int]$SessionId,
        [int]$PayloadBytes
    )

    Set-RustMsvcEnvironment
    $manifestPath = Join-Path $RepoRoot "tools\embedded_audio_replay\Cargo.toml"
    $output = & cargo run --quiet --manifest-path $manifestPath -- --input $WavPath --format wav --session-id $SessionId --payload-bytes $PayloadBytes
    if ($LASTEXITCODE -ne 0) {
        throw "embedded audio replay tool failed for $WavPath"
    }

    $jsonLine = $output | Where-Object { $_ -like "replay_result_json=*" } | Select-Object -Last 1
    if ([string]::IsNullOrWhiteSpace($jsonLine)) {
        throw "replay_result_json line missing for $WavPath"
    }
    return ($jsonLine.Substring("replay_result_json=".Length) | ConvertFrom-Json)
}

$repoRoot = Get-RepoRoot
$resolvedOutDir = if ([System.IO.Path]::IsPathRooted($OutDir)) {
    $OutDir
} else {
    Join-Path $repoRoot $OutDir
}
New-Item -ItemType Directory -Path $resolvedOutDir -Force | Out-Null

Add-Type -AssemblyName System.Speech
$synth = [System.Speech.Synthesis.SpeechSynthesizer]::new()
try {
    if (-not [string]::IsNullOrWhiteSpace($VoiceName)) {
        $synth.SelectVoice($VoiceName)
    } else {
        $zhVoice = $synth.GetInstalledVoices() |
            Where-Object { $_.VoiceInfo.Culture.Name -like "zh-*" } |
            Select-Object -First 1
        if ($null -ne $zhVoice) {
            $synth.SelectVoice($zhVoice.VoiceInfo.Name)
        }
    }

    $random = [System.Random]::new($Seed)
    $cases = @()
    $replayPass = 0
    $replayWarn = 0
    $replayFail = 0

    for ($index = 0; $index -lt $Count; $index++) {
        $sentence = New-RandomSentence -Random $random
        $caseId = "p13-random-{0:d4}" -f ($index + 1)
        $wavPath = Join-Path $resolvedOutDir "$caseId.wav"
        Convert-VoiceToFile -Synth $synth -Text $sentence.Text -Path $wavPath
        $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $wavPath).Hash.ToLowerInvariant()

        $replay = $null
        if (-not $SkipReplay) {
            $replay = Invoke-ReplayTool -RepoRoot $repoRoot -WavPath $wavPath -SessionId (1000 + $index) -PayloadBytes $PayloadBytes
            switch ($replay.status) {
                "PASS" { $replayPass++ }
                "WARNING" { $replayWarn++ }
                default { $replayFail++ }
            }
        }

        $cases += [ordered]@{
            caseId = $caseId
            category = $sentence.Category
            text = $sentence.Text
            expectedNormalized = Normalize-ExpectedText -Text $sentence.Text
            wavPath = $wavPath
            wavSha256 = $hash
            replay = $replay
        }
    }

    $manifestPath = Join-Path $resolvedOutDir ("manifest.seed{0}.json" -f $Seed)
    $summaryStatus = if ($SkipReplay) {
        "GENERATED"
    } elseif ($replayFail -eq 0 -and $replayWarn -eq 0) {
        "PASS"
    } elseif ($replayFail -eq 0) {
        "WARNING"
    } else {
        "FAIL"
    }

    $manifest = [ordered]@{
        status = $summaryStatus
        seed = $Seed
        count = $Count
        voice = $synth.Voice.Name
        payloadBytes = $PayloadBytes
        outDir = $resolvedOutDir
        replayPass = $replayPass
        replayWarning = $replayWarn
        replayFail = $replayFail
        cases = $cases
    }

    $manifest | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
    $summary = [ordered]@{
        status = $summaryStatus
        seed = $Seed
        count = $Count
        voice = $synth.Voice.Name
        manifestPath = $manifestPath
        replayPass = $replayPass
        replayWarning = $replayWarn
        replayFail = $replayFail
    }
    $summaryJson = $summary | ConvertTo-Json -Compress
    Write-Output "tts_fixture_result_json=$summaryJson"
} finally {
    $synth.Dispose()
}
