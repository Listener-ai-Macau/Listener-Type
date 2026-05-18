param(
    [int]$Seed = 20260518,
    [int]$Count = 6,
    [string]$OutDir = "artifacts\embedded_audio_accuracy",
    [string]$VoiceName = "",
    [int]$PayloadBytes = 480,
    [int]$AsrChunkBytes = 3200,
    [string]$ProbeExe = "",
    [int]$TimeoutSeconds = 90,
    [double]$MaxMeanCer = 0.25,
    [double]$MaxCaseCer = 0.45,
    [switch]$SkipReplay,
    [switch]$NoPaceAudio
)

$ErrorActionPreference = "Stop"

function Get-RepoRoot {
    $scriptPath = Split-Path -Parent $PSCommandPath
    return (Resolve-Path (Join-Path $scriptPath "..\..")).Path
}

function Set-RustMsvcEnvironment {
    $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
    if (Test-Path -LiteralPath $cargoBin) {
        $env:PATH = "$cargoBin;$env:PATH"
    }

    $msvcRoot = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC"
    $msvcVersion = Get-ChildItem -LiteralPath $msvcRoot -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending |
        Select-Object -First 1
    if ($null -eq $msvcVersion) {
        return
    }

    $msvcBin = Join-Path $msvcVersion.FullName "bin\Hostx64\x64"
    $msvcLib = Join-Path $msvcVersion.FullName "lib\x64"
    $windowsKitRoot = "C:\Program Files (x86)\Windows Kits\10\Lib"
    $windowsKitVersion = Get-ChildItem -LiteralPath $windowsKitRoot -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending |
        Select-Object -First 1
    if ($null -eq $windowsKitVersion) {
        return
    }

    $env:PATH = "$msvcBin;$env:PATH"
    $env:LIB = "$msvcLib;$(Join-Path $windowsKitVersion.FullName 'ucrt\x64');$(Join-Path $windowsKitVersion.FullName 'um\x64')"
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = Join-Path $msvcBin "link.exe"
}

function Resolve-ProbeExecutable {
    param(
        [string]$RepoRoot,
        [string]$RequestedProbeExe
    )

    if (-not [string]::IsNullOrWhiteSpace($RequestedProbeExe)) {
        $requested = if ([System.IO.Path]::IsPathRooted($RequestedProbeExe)) {
            $RequestedProbeExe
        } else {
            Join-Path $RepoRoot $RequestedProbeExe
        }
        if (-not (Test-Path -LiteralPath $requested)) {
            throw "Volcengine probe executable not found: $requested"
        }
        return (Resolve-Path -LiteralPath $requested).Path
    }

    $probePath = Join-Path $RepoRoot "tools\volcengine_asr_probe\target\debug\listener-volcengine-asr-probe.exe"
    if (Test-Path -LiteralPath $probePath) {
        return (Resolve-Path -LiteralPath $probePath).Path
    }

    Set-RustMsvcEnvironment
    $manifestPath = Join-Path $RepoRoot "tools\volcengine_asr_probe\Cargo.toml"
    $buildOutput = & cargo build --quiet --manifest-path $manifestPath 2>&1
    if ($LASTEXITCODE -ne 0) {
        $buildOutput | Write-Output
        throw "failed to build Volcengine ASR probe"
    }
    if (-not (Test-Path -LiteralPath $probePath)) {
        throw "Volcengine probe build finished, but executable is missing: $probePath"
    }
    return (Resolve-Path -LiteralPath $probePath).Path
}

function Invoke-TtsFixtureGeneration {
    param(
        [string]$RepoRoot,
        [int]$Seed,
        [int]$Count,
        [string]$OutDir,
        [string]$VoiceName,
        [int]$PayloadBytes,
        [bool]$SkipReplay
    )

    $scriptPath = Join-Path $RepoRoot "tools\embedded_audio_replay\generate_tts_fixtures.ps1"
    $args = @(
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        $scriptPath,
        "-Seed",
        [string]$Seed,
        "-Count",
        [string]$Count,
        "-OutDir",
        $OutDir,
        "-PayloadBytes",
        [string]$PayloadBytes
    )
    if (-not [string]::IsNullOrWhiteSpace($VoiceName)) {
        $args += @("-VoiceName", $VoiceName)
    }
    if ($SkipReplay) {
        $args += "-SkipReplay"
    }

    $output = & powershell.exe @args 2>&1
    if ($LASTEXITCODE -ne 0) {
        $output | Write-Output
        throw "TTS fixture generation failed"
    }

    $jsonLine = $output |
        Where-Object { [string]$_ -like "tts_fixture_result_json=*" } |
        Select-Object -Last 1
    if ([string]::IsNullOrWhiteSpace($jsonLine)) {
        $output | Write-Output
        throw "tts_fixture_result_json line missing"
    }
    return ([string]$jsonLine).Substring("tts_fixture_result_json=".Length) | ConvertFrom-Json
}

function Convert-ToSimplifiedChinese {
    param([string]$Text)

    if ([string]::IsNullOrEmpty($Text)) {
        return ""
    }

    try {
        Add-Type -AssemblyName Microsoft.VisualBasic -ErrorAction SilentlyContinue
        return [Microsoft.VisualBasic.Strings]::StrConv(
            $Text,
            [Microsoft.VisualBasic.VbStrConv]::SimplifiedChinese,
            2052
        )
    } catch {
        $result = $Text
        $pairs = @(
            @("藍", "蓝"), @("牙", "牙"), @("音", "音"), @("頻", "频"),
            @("軟", "软"), @("體", "体"), @("測", "测"), @("試", "试"),
            @("會", "会"), @("議", "议"), @("錄", "录"), @("檢", "检"),
            @("查", "查"), @("狀", "状"), @("態", "态"), @("團", "团"),
            @("隊", "队"), @("語", "语"), @("碼", "码"), @("據", "据"),
            @("輸", "输"), @("聯", "联"), @("調", "调"), @("點", "点"),
            @("後", "后"), @("續", "续"), @("開", "开"), @("關", "关"),
            @("啟", "启"), @("閉", "闭"), @("發", "发"), @("現", "现"),
            @("個", "个"), @("與", "与"), @("裡", "里"), @("為", "为")
        )
        foreach ($pair in $pairs) {
            $result = $result.Replace($pair[0], $pair[1])
        }
        return $result
    }
}

function Normalize-AccuracyText {
    param([string]$Text)

    if ([string]::IsNullOrWhiteSpace($Text)) {
        return ""
    }

    $normalized = $Text.Normalize([System.Text.NormalizationForm]::FormKC)
    $normalized = Convert-ToSimplifiedChinese -Text $normalized
    $normalized = $normalized.ToLowerInvariant()
    return ($normalized -replace "[\p{P}\p{S}\s]+", "")
}

function Get-TextElements {
    param([string]$Text)

    if ([string]::IsNullOrEmpty($Text)) {
        return @()
    }

    $indexes = [System.Globalization.StringInfo]::ParseCombiningCharacters($Text)
    $elements = [System.Collections.Generic.List[string]]::new()
    for ($i = 0; $i -lt $indexes.Length; $i++) {
        $start = $indexes[$i]
        $end = if ($i + 1 -lt $indexes.Length) { $indexes[$i + 1] } else { $Text.Length }
        $elements.Add($Text.Substring($start, $end - $start))
    }
    return @($elements.ToArray())
}

function Get-EditDistance {
    param(
        [string[]]$Expected,
        [string[]]$Actual
    )

    if ($Expected.Count -eq 0) {
        return $Actual.Count
    }
    if ($Actual.Count -eq 0) {
        return $Expected.Count
    }

    $previous = New-Object int[] ($Actual.Count + 1)
    $current = New-Object int[] ($Actual.Count + 1)
    for ($j = 0; $j -le $Actual.Count; $j++) {
        $previous[$j] = $j
    }

    for ($i = 1; $i -le $Expected.Count; $i++) {
        $current[0] = $i
        for ($j = 1; $j -le $Actual.Count; $j++) {
            $cost = if ($Expected[$i - 1] -eq $Actual[$j - 1]) { 0 } else { 1 }
            $deleteCost = $previous[$j] + 1
            $insertCost = $current[$j - 1] + 1
            $replaceCost = $previous[$j - 1] + $cost
            $current[$j] = [Math]::Min($deleteCost, [Math]::Min($insertCost, $replaceCost))
        }
        $swap = $previous
        $previous = $current
        $current = $swap
    }

    return $previous[$Actual.Count]
}

function Invoke-VolcengineTranscribe {
    param(
        [string]$ProbePath,
        [string]$WavPath,
        [string]$JsonOutPath,
        [int]$TimeoutSeconds,
        [int]$AsrChunkBytes,
        [bool]$PaceAudio
    )

    $args = @(
        "transcribe",
        "--audio",
        $WavPath,
        "--format",
        "wav",
        "--timeout-seconds",
        [string]$TimeoutSeconds,
        "--chunk-bytes",
        [string]$AsrChunkBytes,
        "--json-out",
        $JsonOutPath
    )
    if ($PaceAudio) {
        $args += "--pace-audio"
    }

    $output = & $ProbePath @args 2>&1
    $exitCode = $LASTEXITCODE
    $report = $null
    if (Test-Path -LiteralPath $JsonOutPath) {
        try {
            $report = Get-Content -Raw -LiteralPath $JsonOutPath | ConvertFrom-Json
        } catch {
            $report = $null
        }
    }

    return [ordered]@{
        exitCode = $exitCode
        report = $report
        output = @($output | ForEach-Object { [string]$_ })
    }
}

$repoRoot = Get-RepoRoot
$resolvedOutDir = if ([System.IO.Path]::IsPathRooted($OutDir)) {
    $OutDir
} else {
    Join-Path $repoRoot $OutDir
}
New-Item -ItemType Directory -Path $resolvedOutDir -Force | Out-Null

$ttsSummary = Invoke-TtsFixtureGeneration `
    -RepoRoot $repoRoot `
    -Seed $Seed `
    -Count $Count `
    -OutDir $resolvedOutDir `
    -VoiceName $VoiceName `
    -PayloadBytes $PayloadBytes `
    -SkipReplay ([bool]$SkipReplay)

$manifestPath = [string]$ttsSummary.manifestPath
if (-not (Test-Path -LiteralPath $manifestPath)) {
    throw "TTS manifest missing: $manifestPath"
}
$manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
$probePath = Resolve-ProbeExecutable -RepoRoot $repoRoot -RequestedProbeExe $ProbeExe

$caseResults = @()
$asrFailCount = 0
$warningCount = 0

foreach ($case in $manifest.cases) {
    $caseId = [string]$case.caseId
    $wavPath = [string]$case.wavPath
    $jsonOutPath = Join-Path $resolvedOutDir "$caseId.volcengine.json"
    $probeResult = Invoke-VolcengineTranscribe `
        -ProbePath $probePath `
        -WavPath $wavPath `
        -JsonOutPath $jsonOutPath `
        -TimeoutSeconds $TimeoutSeconds `
        -AsrChunkBytes $AsrChunkBytes `
        -PaceAudio (-not [bool]$NoPaceAudio)

    $probeReport = $probeResult.report
    $asrStatus = if ($null -ne $probeReport -and $probeReport.status) {
        [string]$probeReport.status
    } else {
        "FAIL"
    }
    $transcript = if ($null -ne $probeReport -and $null -ne $probeReport.transcript) {
        [string]$probeReport.transcript
    } else {
        ""
    }
    $expected = [string]$case.text
    $expectedNormalized = Normalize-AccuracyText -Text $expected
    $transcriptNormalized = Normalize-AccuracyText -Text $transcript
    $expectedElements = @(Get-TextElements -Text $expectedNormalized)
    $transcriptElements = @(Get-TextElements -Text $transcriptNormalized)
    $distance = Get-EditDistance -Expected $expectedElements -Actual $transcriptElements
    $cer = if ($expectedElements.Count -eq 0) {
        if ($transcriptElements.Count -eq 0) { 0.0 } else { 1.0 }
    } else {
        [Math]::Round($distance / [double]$expectedElements.Count, 6)
    }

    $caseStatus = if ($asrStatus -ne "PASS" -or $probeResult.exitCode -ne 0) {
        $asrFailCount++
        "FAIL"
    } elseif ($cer -le $MaxCaseCer) {
        "PASS"
    } else {
        $warningCount++
        "WARNING"
    }

    $caseResults += [pscustomobject][ordered]@{
        caseId = $caseId
        category = [string]$case.category
        status = $caseStatus
        expected = $expected
        transcript = $transcript
        expectedNormalized = $expectedNormalized
        transcriptNormalized = $transcriptNormalized
        editDistance = $distance
        referenceLength = $expectedElements.Count
        cer = $cer
        wavPath = $wavPath
        wavSha256 = [string]$case.wavSha256
        asrStatus = $asrStatus
        asrReportPath = $jsonOutPath
        asrError = if ($null -ne $probeReport) { $probeReport.error } else { "probe report missing" }
    }
}

$meanCer = if ($caseResults.Count -eq 0) {
    0.0
} else {
    [Math]::Round((($caseResults | Measure-Object -Property cer -Average).Average), 6)
}
$maxCer = if ($caseResults.Count -eq 0) {
    0.0
} else {
    [Math]::Round((($caseResults | Measure-Object -Property cer -Maximum).Maximum), 6)
}

$summaryStatus = if ($asrFailCount -gt 0) {
    "FAIL"
} elseif ($meanCer -le $MaxMeanCer -and $maxCer -le $MaxCaseCer) {
    "PASS"
} else {
    "WARNING"
}

$reportPath = Join-Path $resolvedOutDir ("accuracy.seed{0}.json" -f $Seed)
$report = [ordered]@{
    status = $summaryStatus
    seed = $Seed
    count = $caseResults.Count
    voice = [string]$manifest.voice
    provider = "volcengine"
    thresholds = [ordered]@{
        maxMeanCer = $MaxMeanCer
        maxCaseCer = $MaxCaseCer
    }
    meanCer = $meanCer
    maxCer = $maxCer
    passCount = @($caseResults | Where-Object { $_.status -eq "PASS" }).Count
    warningCount = $warningCount
    failCount = $asrFailCount
    manifestPath = $manifestPath
    outDir = $resolvedOutDir
    probeExe = $probePath
    replayStatus = [string]$manifest.status
    cases = $caseResults
}

$report | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath $reportPath -Encoding UTF8
$summary = [ordered]@{
    status = $summaryStatus
    seed = $Seed
    count = $caseResults.Count
    meanCer = $meanCer
    maxCer = $maxCer
    reportPath = $reportPath
}
$summaryJson = $summary | ConvertTo-Json -Compress
Write-Output "asr_accuracy_result_json=$summaryJson"

if ($summaryStatus -eq "FAIL") {
    exit 1
}
