#requires -Version 7.0
<#
.SYNOPSIS
  Listener 1.0.4 录音体验人工验收：分步弹窗 + 写结果 JSON。
.DESCRIPTION
  使用 AIW operator-prompt 采集通过/失败/备注。日志由 agent 并行盯。
#>
param(
  [string]$EvidenceDir = ""
)

$ErrorActionPreference = "Stop"
$workflow = if ($env:AI_WORKFLOW_REPO -and (Test-Path (Join-Path $env:AI_WORKFLOW_REPO "scripts\operator_prompt.ps1"))) {
  $env:AI_WORKFLOW_REPO
} else {
  "C:\Users\Billy\Desktop\Denzic\ai-collaboration-workflow"
}
$operatorPrompt = Join-Path $workflow "scripts\operator_prompt.ps1"
if (-not (Test-Path $operatorPrompt)) { throw "missing operator_prompt.ps1: $operatorPrompt" }

if (-not $EvidenceDir) {
  $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
  $EvidenceDir = Join-Path $PSScriptRoot "..\docs\release\evidence\1.0.4-recording-ux-$stamp"
}
New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null

$results = [System.Collections.Generic.List[object]]::new()
$overall = "PASS"

function Invoke-Step {
  param(
    [string]$Id,
    [string]$Progress,
    [string]$Title,
    [string]$Scope,
    [string]$Message,
    [string]$Expected,
    [string]$Spoken = ""
  )
  $outJson = Join-Path $EvidenceDir ("step-{0}.json" -f $Id)
  $outNote = Join-Path $EvidenceDir ("step-{0}-note.txt" -f $Id)
  $argList = @(
    "-NoProfile", "-STA", "-File", $operatorPrompt,
    "-ReviewStyle", "-Input", "-Json",
    "-Title", $Title,
    "-ProgressText", $Progress,
    "-ScopeText", $Scope,
    "-Message", $Message,
    "-ExpectedText", $Expected,
    "-Buttons", "通过,失败,跳过,中止",
    "-OutputPath", $outNote,
    "-Width", "760",
    "-Height", "520",
    "-Position", "TopLeft"
  )
  if ($Spoken) {
    $argList += @("-SpokenText", $Spoken)
  }
  $raw = & pwsh @argList 2>&1 | Out-String
  $raw | Set-Content (Join-Path $EvidenceDir ("step-{0}-raw.txt" -f $Id)) -Encoding utf8
  $parsed = $null
  try {
    $jsonLines = @($raw -split "`r?`n" | Where-Object { $_.Trim().StartsWith("{") })
    if ($jsonLines.Count -gt 0) {
      $parsed = ($jsonLines[-1] | ConvertFrom-Json)
    }
  } catch {}
  if (-not $parsed) {
    $parsed = [pscustomobject]@{
      selected = "未知"
      text = $raw.Trim()
    }
  }
  $noteText = if (Test-Path $outNote) { Get-Content $outNote -Raw -ErrorAction SilentlyContinue } else { "" }
  $row = [ordered]@{
    id = $Id
    progress = $Progress
    title = $Title
    selected = [string]$parsed.selected
    note = if ($noteText) { $noteText.Trim() } elseif ($parsed.PSObject.Properties.Name -contains "text") { [string]$parsed.text } else { "" }
    observed_at = (Get-Date).ToString("o")
  }
  ($row | ConvertTo-Json -Depth 5) | Set-Content $outJson -Encoding utf8
  $script:results.Add([pscustomobject]$row)
  $sel = [string]$row.selected
  if ($sel -match "失败|FAIL") { $script:overall = "FAIL" }
  if ($sel -match "中止|ABORT") { $script:overall = "ABORT"; return $false }
  return $true
}

# Steps (abort stops early)
$steps = @(
  @{
    Id = "00-ready"; Progress = "0/3"; Title = "Listener 1.0.4 录音验收"
    Scope = "安装身份已是 Program Files 1.0.4；请确认板子已连上 Type、可唤醒。"
    Message = "准备开始验收。`n`n1) 托盘 Listener Type 已运行`n2) 设备已连接（灯正常）`n3) 焦点放在记事本或任意输入框`n`n点「通过」开始第 1 步；未连上可「跳过」说明原因。"
    Expected = "设备已连接，可以开始说唤醒词。"
    Spoken = ""
  },
  @{
    Id = "01-short-snappy"; Progress = "1/3"; Title = "短句跟手（默认 1s）"
    Scope = "标准听写"
    Message = "操作：`n1. 说唤醒词，开始录音`n2. 说一句短话（不要故意拖句号）`n3. 停说，等自动结束`n`n体感：`n- 胶囊尽快出现、有「正在听」感`n- 预览跟手`n- 约 1 秒收尾`n- 收尾立刻变处理中，不长时间悬空"
    Expected = "预览顺；约 1s 结束；立刻处理中；能上屏或明确剪贴板提示。"
    Spoken = "今天天气不错，我们继续测试。"
  },
  @{
    Id = "02-no-mid-cut"; Progress = "2/3"; Title = "中途不掐 + 句末耐停"
    Scope = "半句别断；句号后可稍耐停（约 2s）"
    Message = "操作 A（中途）：连续说较长一句，中间自然换气但不说完，看会不会半截掐断。`n`n操作 B（句末）：说完整句并带句号语气，停住，体感约 2 秒再收（比半句 1s 稍慢是预期）。`n`n若中途被掐 → 点失败并写现象。"
    Expected = "半句不误掐；句末可耐停约 2s；正文不丢。"
    Spoken = "我想先说明一下，这个功能主要是为了让长句在中间换气时不要被系统掐断。"
  },
  @{
    Id = "03-other-speaker"; Progress = "3/3"; Title = "别人说话不拖（可选）"
    Scope = "办公室场景：他人说话不进本人预览、不拖结束"
    Message = "若有第二人或可播另一段语音：`n1. 你说一句开始听写`n2. 另一人插话`n3. 看是否拖长结束、是否出别人字`n`n无条件请「跳过」。"
    Expected = "他人不拖本人结束时钟；不把别人字当本人正文。"
    Spoken = ""
  }
)

foreach ($s in $steps) {
  $cont = Invoke-Step -Id $s.Id -Progress $s.Progress -Title $s.Title -Scope $s.Scope -Message $s.Message -Expected $s.Expected -Spoken $s.Spoken
  if (-not $cont) { break }
}

# overall
if ($overall -eq "PASS") {
  $failed = @($results | Where-Object { $_.selected -match "失败|FAIL" })
  $aborted = @($results | Where-Object { $_.selected -match "中止|ABORT" })
  if ($aborted.Count -gt 0) { $overall = "ABORT" }
  elseif ($failed.Count -gt 0) { $overall = "FAIL" }
  else {
    $passed = @($results | Where-Object { $_.selected -match "通过|PASS" })
    if ($passed.Count -eq 0) { $overall = "INCOMPLETE" }
  }
}

$summary = [ordered]@{
  schema = "listener.1.0.4.recording_ux.owner_acceptance"
  result = $overall
  evidence_dir = (Resolve-Path $EvidenceDir).Path
  steps = @($results)
  finished_at = (Get-Date).ToString("o")
}
$summaryPath = Join-Path $EvidenceDir "operator-result.json"
($summary | ConvertTo-Json -Depth 6) | Set-Content $summaryPath -Encoding utf8
Write-Output ("ACCEPTANCE_RESULT={0}" -f $overall)
Write-Output ("EVIDENCE_DIR={0}" -f $EvidenceDir)
Write-Output ("SUMMARY={0}" -f $summaryPath)
exit 0
