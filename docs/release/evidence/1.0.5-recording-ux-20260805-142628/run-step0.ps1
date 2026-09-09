$ErrorActionPreference = "Stop"
$log = Join-Path $PSScriptRoot "accept-runner.err.log"
try {
  "start $(Get-Date -Format o)" | Set-Content $log
  $op = Join-Path $env:AI_WORKFLOW_REPO "scripts\operator_prompt.ps1"
  if (-not (Test-Path $op)) { throw "missing $op" }
  $result = & pwsh -NoProfile -STA -File $op `
    -ReviewStyle -Input -Json `
    -Title "Listener 1.0.5 录音验收" `
    -ProgressText "0/4" `
    -ScopeText "Program Files 1.0.5；确认设备已连接" `
    -Message "准备开始验收。`r`n1) Type 托盘已运行`r`n2) 设备已连接`r`n3) 记事本可粘贴`r`n`r`n做完点通过；有问题点失败并写备注。" `
    -ExpectedText "设备已连接，可唤醒听写" `
    -Buttons "通过,失败,跳过,中止" `
    -OutputPath (Join-Path $PSScriptRoot "step-00-ready-note.txt") `
    -Width 760 -Height 520 -Position TopLeft
  $result | Set-Content (Join-Path $PSScriptRoot "step-00-ready.json") -Encoding utf8
  "done selected" | Add-Content $log
} catch {
  $_ | Out-String | Add-Content $log
  $Error | Out-String | Add-Content $log
}
