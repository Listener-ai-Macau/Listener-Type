#requires -Version 7.0
$ErrorActionPreference = "Stop"
$EvidenceDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$op = Join-Path $env:AI_WORKFLOW_REPO "scripts\operator_prompt.ps1"
$results = [System.Collections.Generic.List[object]]::new()
$overall = "PASS"

function Show-Step {
  param(
    [string]$Id,
    [string]$Progress,
    [string]$Title,
    [string]$Scope,
    [string]$Message,
    [string]$Expected,
    [string]$Spoken = ""
  )
  $note = Join-Path $EvidenceDir "step-$Id-note.txt"
  $jsonOut = Join-Path $EvidenceDir "step-$Id.json"
  $rawOut = Join-Path $EvidenceDir "step-$Id-raw.txt"
  $arg = @(
    "-NoProfile", "-STA", "-File", "`"$op`"",
    "-ReviewStyle", "-Input", "-Json",
    "-Title", "`"$Title`"",
    "-ProgressText", "`"$Progress`"",
    "-ScopeText", "`"$Scope`"",
    "-Message", "`"$($Message -replace '"','`"')`"",
    "-ExpectedText", "`"$Expected`"",
    "-Buttons", "`"通过,失败,跳过,中止`"",
    "-OutputPath", "`"$note`"",
    "-Width", "760",
    "-Height", "520",
    "-Position", "TopLeft"
  )
  if ($Spoken) {
    $arg += @("-SpokenText", "`"$Spoken`"")
  }
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = "pwsh.exe"
  $psi.Arguments = ($arg -join " ")
  $psi.UseShellExecute = $false
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $psi.CreateNoWindow = $false
  $psi.WorkingDirectory = $EvidenceDir
  $p = New-Object System.Diagnostics.Process
  $p.StartInfo = $psi
  [void]$p.Start()
  $stdout = $p.StandardOutput.ReadToEnd()
  $stderr = $p.StandardError.ReadToEnd()
  $p.WaitForExit()
  ($stdout + "`n" + $stderr) | Set-Content $rawOut -Encoding utf8
  $selected = "未知"
  $text = ""
  try {
    $line = ($stdout -split "`r?`n" | Where-Object { $_.Trim().StartsWith("{") } | Select-Object -Last 1)
    if ($line) {
      $obj = $line | ConvertFrom-Json
      $selected = [string]$obj.selected
      $text = [string]$obj.text
    }
  } catch {}
  if (Test-Path $note) {
    $text = (Get-Content $note -Raw).Trim()
  }
  $row = [ordered]@{
    id = $Id
    progress = $Progress
    title = $Title
    selected = $selected
    note = $text
    observed_at = (Get-Date).ToString("o")
  }
  ($row | ConvertTo-Json -Depth 5) | Set-Content $jsonOut -Encoding utf8
  $script:results.Add([pscustomobject]$row)
  if ($selected -match "失败") { $script:overall = "FAIL" }
  if ($selected -match "中止") { $script:overall = "ABORT"; return $false }
  return $true
}

# If step 0 already done by standalone dialog, skip
$skip0 = $false
if (Test-Path (Join-Path $EvidenceDir "step-00-ready.json")) {
  $skip0 = $true
} elseif (Test-Path (Join-Path $EvidenceDir "step-00-ready-note.txt")) {
  # only note from standalone - still show step0 if no json selected
  $skip0 = $false
}

if (-not $skip0) {
  $c = Show-Step -Id "00-ready" -Progress "0/4" -Title "Listener 1.0.5 录音验收" `
    -Scope "Program Files 1.0.5；确认设备已连接" `
    -Message "准备验收。`n1) Type 托盘已运行`n2) 设备已连接`n3) 记事本可粘贴`n`n点通过开始第1步。" `
    -Expected "设备已连接，可唤醒听写"
  if (-not $c) { goto DONE }
}

$c = Show-Step -Id "01-short-snappy" -Progress "1/4" -Title "短句跟手（默认1s）" `
  -Scope "默认长文关；标准听写" `
  -Message "操作：唤醒 → 说短句 → 停说等自动结束。`n看：胶囊快、预览顺、约1s收、立刻处理中、能上屏或剪贴板提示。" `
  -Expected "预览顺；约1s结束；立刻处理中；上屏或明确剪贴板提示" `
  -Spoken "今天天气不错，我们继续测试。"
if (-not $c) { goto DONE }

$c = Show-Step -Id "02-no-mid-cut" -Progress "2/4" -Title "中途不掐+句末耐停" `
  -Scope "半句别断；句号后约2s" `
  -Message "A 中途：长句换气不说完，不应半截掐。`nB 句末：带句号语气停住，约2s再收（比1s稍慢是预期）。`n被掐请点失败。" `
  -Expected "半句不误掐；句末可耐停约2s；正文不丢" `
  -Spoken "我想先说明一下，这个功能主要是为了让长句在中间换气时不要被系统掐断。"
if (-not $c) { goto DONE }

$c = Show-Step -Id "03-long-form" -Progress "3/4" -Title "长文模式（可选）" `
  -Scope "设置里打开「长文听写模式」" `
  -Message "可选：开长文 → 中间短停口述 → 是否更耐停 → 测完关掉恢复默认。不便就跳过。" `
  -Expected "长文开更耐停；关回后仍1s跟手"
if (-not $c) { goto DONE }

$c = Show-Step -Id "04-other-speaker" -Progress "4/4" -Title "别人说话不拖（可选）" `
  -Scope "他人插话" `
  -Message "可选：你说一句后他人插话，看是否拖结束、是否出别人字。无条件跳过。" `
  -Expected "他人不拖本人结束；别人字不进本人正文"

:DONE
if ($overall -eq "PASS") {
  if (@($results | Where-Object { $_.selected -match "失败" }).Count -gt 0) { $overall = "FAIL" }
  elseif (@($results | Where-Object { $_.selected -match "中止" }).Count -gt 0) { $overall = "ABORT" }
  elseif (@($results | Where-Object { $_.selected -match "通过" }).Count -eq 0 -and -not $skip0) { $overall = "INCOMPLETE" }
}
$summary = [ordered]@{
  schema = "listener.1.0.5.recording_ux.owner_acceptance"
  result = $overall
  evidence_dir = $EvidenceDir
  steps = @($results)
  finished_at = (Get-Date).ToString("o")
}
($summary | ConvertTo-Json -Depth 6) | Set-Content (Join-Path $EvidenceDir "operator-result.json") -Encoding utf8
"ACCEPTANCE_RESULT=$overall" | Set-Content (Join-Path $EvidenceDir "accept-runner.log") -Encoding utf8
