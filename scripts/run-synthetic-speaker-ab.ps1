[CmdletBinding()]
param(
    [string]$OutputRoot = ".artifacts/speaker-ab-synthetic",
    [string]$OwnerVoice = "Microsoft Kangkang",
    [string[]]$NonOwnerVoices = @(
        "Microsoft Huihui Desktop",
        "Microsoft Yaoyao",
        "Microsoft Zira Desktop"
    ),
    [string]$CampPlusModel = "$env:APPDATA/Listener Type/models/speaker-verification/3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx",
    [string]$ERes2NetModel = "$env:LOCALAPPDATA/Listener Type/models/speaker-verification-ab/3dspeaker_speech_eres2net_base_200k_sv_zh-cn_16k-common.onnx",
    [string]$RuntimeRoot = "$env:APPDATA/Listener Type/models/speaker-verification"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$output = if ([System.IO.Path]::IsPathRooted($OutputRoot)) {
    [System.IO.Path]::GetFullPath($OutputRoot)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $repoRoot $OutputRoot))
}
$rawRoot = Join-Path $output "raw"
$sampleRoot = Join-Path $output "samples"
$enrollmentRoot = Join-Path $output "enrollment"
$manifestPath = Join-Path $output "manifest.json"
$reportPath = Join-Path $output "report.json"
$metaPath = Join-Path $output "corpus-meta.json"
$verdictPath = Join-Path $output "verdict.json"

$ffmpeg = (Get-Command ffmpeg -ErrorAction Stop).Source
Add-Type -AssemblyName System.Speech

$installedVoices = @(
    [System.Speech.Synthesis.SpeechSynthesizer]::new().GetInstalledVoices() |
        ForEach-Object { $_.VoiceInfo.Name }
)
foreach ($voice in @($OwnerVoice) + $NonOwnerVoices) {
    if ($voice -notin $installedVoices) {
        throw "Synthetic speaker A/B voice is not installed: $voice"
    }
}
foreach ($required in @($CampPlusModel, $ERes2NetModel)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "Synthetic speaker A/B model is missing: $required"
    }
}

New-Item -ItemType Directory -Force -Path $rawRoot, $sampleRoot, $enrollmentRoot | Out-Null

function Write-TtsWav {
    param(
        [Parameter(Mandatory = $true)][string]$Voice,
        [Parameter(Mandatory = $true)][string]$Text,
        [Parameter(Mandatory = $true)][int]$Rate,
        [Parameter(Mandatory = $true)][string]$Path
    )

    $synth = [System.Speech.Synthesis.SpeechSynthesizer]::new()
    try {
        $synth.SelectVoice($Voice)
        $synth.Rate = $Rate
        $format = [System.Speech.AudioFormat.SpeechAudioFormatInfo]::new(
            16000,
            [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen,
            [System.Speech.AudioFormat.AudioChannel]::Mono
        )
        $synth.SetOutputToWaveFile($Path, $format)
        $synth.Speak($Text)
    } finally {
        $synth.Dispose()
    }
}

function Invoke-Ffmpeg {
    param([Parameter(Mandatory = $true)][string[]]$Arguments)

    & $ffmpeg -hide_banner -loglevel error -y @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "ffmpeg failed with exit code ${LASTEXITCODE}: $($Arguments -join ' ')"
    }
}

function New-QualitySample {
    param(
        [Parameter(Mandatory = $true)][string]$SourcePath,
        [Parameter(Mandatory = $true)][ValidateSet("clean", "noisy", "far_field")][string]$Quality,
        [Parameter(Mandatory = $true)][string]$DestinationPath,
        [string]$Interferer
    )

    if ([string]::IsNullOrWhiteSpace($SourcePath)) {
        throw "Synthetic speaker A/B sample input is empty for quality=$Quality output=$DestinationPath"
    }
    if ([string]::IsNullOrWhiteSpace($DestinationPath)) {
        throw "Synthetic speaker A/B sample output is empty for quality=$Quality input=$SourcePath"
    }

    if ($Quality -eq "clean") {
        Invoke-Ffmpeg -Arguments @(
            "-i", $SourcePath,
            "-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le",
            $DestinationPath
        )
        return
    }
    if ($Quality -eq "far_field") {
        Invoke-Ffmpeg -Arguments @(
            "-i", $SourcePath,
            "-af", "volume=0.38,highpass=f=120,lowpass=f=3600,aecho=0.8:0.7:35:0.16,alimiter=limit=0.95",
            "-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le",
            $DestinationPath
        )
        return
    }
    if (-not [string]::IsNullOrWhiteSpace($Interferer)) {
        Invoke-Ffmpeg -Arguments @(
            "-i", $SourcePath,
            "-i", $Interferer,
            "-filter_complex", "[1:a]volume=0.72,adelay=220|220[i];[0:a][i]amix=inputs=2:duration=first:normalize=0,alimiter=limit=0.95[a]",
            "-map", "[a]",
            "-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le",
            $DestinationPath
        )
        return
    }
    Invoke-Ffmpeg -Arguments @(
        "-i", $SourcePath,
        "-filter_complex", "anoisesrc=color=pink:amplitude=0.025:r=16000:d=30[n];[0:a][n]amix=inputs=2:duration=first:normalize=0,alimiter=limit=0.95[a]",
        "-map", "[a]",
        "-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le",
        $DestinationPath
    )
}

$enrollmentTexts = @(
    "开始录音，现在登记第一段测试声纹。",
    "开始录音，现在登记第二段测试声纹。",
    "开始录音，现在登记第三段测试声纹。"
)
$enrollmentWavs = @()
for ($index = 0; $index -lt $enrollmentTexts.Count; $index++) {
    $path = Join-Path $enrollmentRoot ("owner-enrollment-{0:d2}.wav" -f ($index + 1))
    Write-TtsWav -Voice $OwnerVoice -Text $enrollmentTexts[$index] -Rate ($index - 1) -Path $path
    $enrollmentWavs += $path
}

$ownerTexts = [ordered]@{
    short = @(
        "今天开始测试录音。",
        "请记录这一小句话。",
        "主人现在正在说话。"
    )
    medium = @(
        "现在检查自动结束是否正常。",
        "主人说完以后应该结束录音。",
        "房间有声音也只追踪主人。"
    )
    long = @(
        "这是一段更长的主人测试语音，用来确认模型在连续说话的时候不会中途丢失身份，也不会把最后几个字吞掉。",
        "现在进行长句测试，主人会自然地说完完整内容，然后保持安静，系统需要根据主人真正停止的时间结束录音。",
        "我们希望声纹识别能够在噪声和其他人讲话的时候保持稳定，同时也要在主人停止以后及时释放自动结束计时器。"
    )
}
$nonOwnerTextsZh = [ordered]@{
    short = @(
        "这是另一个人的声音。",
        "旁边的人正在讲话。",
        "不要把这句话当主人。"
    )
    medium = @(
        "另一个说话人正在讲话。",
        "系统不要追踪房间所有声音。",
        "这段声音不是注册的主人。"
    )
    long = @(
        "这是一段来自另外一个说话人的长句，它会持续一段时间，用来检查系统是否错误地把旁人的声音判断成已经注册的主人。",
        "在主人已经停止说话以后，旁边的人可能继续交谈，但是自动结束应该只追踪注册声纹而不是追踪房间里的全部声音。",
        "如果模型无法区分主人和其他人，录音就会被错误地延长，所以这一段专门用来测试这种持续干扰条件。"
    )
}
$nonOwnerTextsEn = [ordered]@{
    short = @(
        "This is another speaker.",
        "Do not accept this voice.",
        "The owner is not speaking."
    )
    medium = @(
        "Another person is speaking.",
        "Do not follow every room voice.",
        "This voice is not the owner."
    )
    long = @(
        "This is a longer sentence from a different speaker and it must not extend the registered owner's recording session.",
        "After the registered owner becomes quiet another person may continue talking in the room without holding the endpoint forever.",
        "Speaker identity should remain independent from the language and the words that are spoken during this evaluation."
    )
}

$rawOwner = @{}
$rawNonOwner = @{}
$rates = @{ short = 2; medium = 0; long = -1 }
foreach ($duration in @("short", "medium", "long")) {
    for ($variant = 0; $variant -lt 3; $variant++) {
        $ownerRaw = Join-Path $rawRoot ("owner-{0}-{1:d2}.wav" -f $duration, ($variant + 1))
        Write-TtsWav -Voice $OwnerVoice -Text $ownerTexts[$duration][$variant] -Rate $rates[$duration] -Path $ownerRaw
        $sampleKey = [string]::Format("{0}-{1}", $duration, $variant)
        $rawOwner[$sampleKey] = $ownerRaw

        $voice = $NonOwnerVoices[$variant % $NonOwnerVoices.Count]
        $textSet = if ($voice -like "*Zira*") { $nonOwnerTextsEn } else { $nonOwnerTextsZh }
        $nonOwnerRaw = Join-Path $rawRoot ("non-owner-{0}-{1:d2}.wav" -f $duration, ($variant + 1))
        Write-TtsWav -Voice $voice -Text $textSet[$duration][$variant] -Rate $rates[$duration] -Path $nonOwnerRaw
        $rawNonOwner[$sampleKey] = $nonOwnerRaw
    }
}

$samples = [System.Collections.Generic.List[object]]::new()
$overlapCount = 0
if ($rawOwner.Count -ne 9 -or $rawNonOwner.Count -ne 9) {
    throw "Synthetic raw corpus index is incomplete: owner=$($rawOwner.Count) non_owner=$($rawNonOwner.Count) owner_keys=$($rawOwner.Keys -join ',')"
}
Write-Verbose "Synthetic owner corpus keys: $($rawOwner.Keys -join ',')"
foreach ($quality in @("clean", "noisy", "far_field")) {
    foreach ($duration in @("short", "medium", "long")) {
        for ($variant = 0; $variant -lt 3; $variant++) {
            $key = [string]::Format("{0}-{1}", $duration, $variant)
            $ownerOutput = Join-Path $sampleRoot ("owner-{0}-{1}-{2:d2}.wav" -f $quality, $duration, ($variant + 1))
            $interferer = if ($quality -eq "noisy" -and $variant -ge 1) { $rawNonOwner[$key] } else { $null }
            if ($interferer) { $overlapCount++ }
            New-QualitySample -SourcePath $rawOwner[$key] -Quality $quality -DestinationPath $ownerOutput -Interferer $interferer
            $samples.Add([ordered]@{
                id = "owner-$quality-$duration-$($variant + 1)"
                path = $ownerOutput
                label = "owner"
                quality = $quality
            })

            $nonOwnerOutput = Join-Path $sampleRoot ("non-owner-{0}-{1}-{2:d2}.wav" -f $quality, $duration, ($variant + 1))
            New-QualitySample -SourcePath $rawNonOwner[$key] -Quality $quality -DestinationPath $nonOwnerOutput
            $samples.Add([ordered]@{
                id = "non-owner-$quality-$duration-$($variant + 1)"
                path = $nonOwnerOutput
                label = "non_owner"
                quality = $quality
            })
        }
    }
}

$manifest = [ordered]@{
    models = @(
        [ordered]@{ name = "campplus-current"; path = [System.IO.Path]::GetFullPath($CampPlusModel) },
        [ordered]@{ name = "eres2net-candidate"; path = [System.IO.Path]::GetFullPath($ERes2NetModel) }
    )
    enrollment_session_wavs = $enrollmentWavs
    samples = $samples
}
$manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding UTF8

[ordered]@{
    schema = "listener.synthetic-speaker-ab.v1"
    generated_at = [DateTime]::UtcNow.ToString("o")
    owner_voice = $OwnerVoice
    non_owner_voices = $NonOwnerVoices
    owner_sample_count = @($samples | Where-Object { $_.label -eq "owner" }).Count
    non_owner_sample_count = @($samples | Where-Object { $_.label -eq "non_owner" }).Count
    simultaneous_two_voice_sample_count = $overlapCount
    limitations = @(
        "Synthetic TTS is a deterministic engineering smoke test, not a replacement for real airborne human acceptance.",
        "Each model registers its own enrollment embeddings; embeddings are never compared across models."
    )
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $metaPath -Encoding UTF8

$evaluationError = $null
try {
    & (Join-Path $PSScriptRoot "run-speaker-verification-evaluation.ps1") `
        -Manifest $manifestPath `
        -Output $reportPath `
        -RuntimeRoot $RuntimeRoot
} catch {
    $evaluationError = $_.Exception.Message
}
if (-not (Test-Path -LiteralPath $reportPath -PathType Leaf)) {
    throw "Synthetic speaker A/B did not produce a report: $evaluationError"
}

$reports = @(Get-Content -Raw -LiteralPath $reportPath | ConvertFrom-Json)
$sessionTargetThreshold = 0.55
$modelVerdicts = @(
    foreach ($modelReport in $reports) {
        $ownerScores = @($modelReport.scores | Where-Object { $_.label -eq "owner" })
        $nonOwnerScores = @($modelReport.scores | Where-Object { $_.label -eq "non_owner" })
        $overlapOwnerScores = @($ownerScores | Where-Object {
            $_.id -match '^owner-noisy-' -and $_.id -match '-(2|3)$'
        })
        $ownerTargetRate = @($ownerScores | Where-Object {
            [double]$_.score -ge $sessionTargetThreshold
        }).Count / [double]$ownerScores.Count
        $nonOwnerFalseTargetRate = @($nonOwnerScores | Where-Object {
            [double]$_.score -ge $sessionTargetThreshold
        }).Count / [double]$nonOwnerScores.Count
        $ownerConsensusTargetRate = @($ownerScores | Where-Object {
            [double]$_.consensus_score -ge $sessionTargetThreshold
        }).Count / [double]$ownerScores.Count
        $nonOwnerConsensusFalseTargetRate = @($nonOwnerScores | Where-Object {
            [double]$_.consensus_score -ge $sessionTargetThreshold
        }).Count / [double]$nonOwnerScores.Count
        $overlapOwnerTargetRate = @($overlapOwnerScores | Where-Object {
            [double]$_.score -ge $sessionTargetThreshold
        }).Count / [double]$overlapOwnerScores.Count
        $overlapOwnerConsensusTargetRate = @($overlapOwnerScores | Where-Object {
            [double]$_.consensus_score -ge $sessionTargetThreshold
        }).Count / [double]$overlapOwnerScores.Count
        $endpointProxyPass = $ownerTargetRate -ge 0.90 -and
            $overlapOwnerTargetRate -ge 0.80 -and
            $nonOwnerFalseTargetRate -le 0.10 -and
            [double]$modelReport.inference_p95_ms -le 300
        [ordered]@{
            model = $modelReport.model
            calibrated_threshold = $modelReport.threshold
            owner_target_rate_at_0_55 = $ownerTargetRate
            overlap_owner_target_rate_at_0_55 = $overlapOwnerTargetRate
            non_owner_false_target_rate_at_0_55 = $nonOwnerFalseTargetRate
            owner_consensus_target_rate_at_0_55 = $ownerConsensusTargetRate
            overlap_owner_consensus_target_rate_at_0_55 = $overlapOwnerConsensusTargetRate
            non_owner_consensus_false_target_rate_at_0_55 = $nonOwnerConsensusFalseTargetRate
            inference_p95_ms = $modelReport.inference_p95_ms
            verification_gate_pass = [bool]$modelReport.pass
            endpoint_proxy_pass = $endpointProxyPass
        }
    }
)
$passingModels = @($modelVerdicts | Where-Object {
    $_.verification_gate_pass -and $_.endpoint_proxy_pass
})
[ordered]@{
    schema = "listener.synthetic-speaker-ab-verdict.v1"
    generated_at = [DateTime]::UtcNow.ToString("o")
    result = if ($passingModels.Count -gt 0) { "PASS" } else { "NO_GO" }
    session_target_threshold = $sessionTargetThreshold
    evaluation_error = $evaluationError
    models = $modelVerdicts
    interpretation = "A non-owner score at or above the session Target threshold is an endpoint-extension proxy: that window could renew the owner watermark."
} | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $verdictPath -Encoding UTF8

Write-Output $(if ($passingModels.Count -gt 0) { "SYNTHETIC_SPEAKER_AB_PASS" } else { "SYNTHETIC_SPEAKER_AB_NO_GO" })
Write-Output "manifest=$manifestPath"
Write-Output "report=$reportPath"
Write-Output "meta=$metaPath"
Write-Output "verdict=$verdictPath"
if ($passingModels.Count -eq 0) {
    exit 2
}
