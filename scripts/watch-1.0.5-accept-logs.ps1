#requires -Version 7.0
param(
  [Parameter(Mandatory = $true)][string]$EvidenceDir,
  [int]$BaselineOffset = 0,
  [int]$TimeoutSec = 1800
)

$ErrorActionPreference = "Continue"
$log = "C:\Users\Billy\AppData\Local\Listener Type\Logs\listener-type.log"
$capsule = "C:\Users\Billy\AppData\Local\Listener Type\Logs\capsule-timeline.log"
$out = Join-Path $EvidenceDir "live-log-hits.jsonl"
$doneMarker = Join-Path $EvidenceDir "operator-result.json"
$start = Get-Date
$offset = [Math]::Max(0, $BaselineOffset)

function Emit-Hit {
  param([string]$Source, [string]$Line)
  $patterns = @(
    "stop_to_transcribing_ms=",
    "stop_to_done_ms=",
    "target_speaker_inactive_",
    "sentence_pause=",
    "仍在剪贴板",
    "上屏失败",
    "已粘贴",
    "pending_unattributed",
    "provisional body growth",
    "wake_to_capsule",
    "state=Transcribing",
    "state=Done",
    "state=Recording",
    "two_pass_empty",
    "missing_packets"
  )
  $hit = $false
  foreach ($p in $patterns) {
    if ($Line -like "*$p*") { $hit = $true; break }
  }
  if (-not $hit) { return }
  $row = [ordered]@{
    ts = (Get-Date).ToString("o")
    source = $Source
    line = $Line.Trim()
  }
  ($row | ConvertTo-Json -Compress) | Add-Content -Path $out -Encoding utf8
  Write-Output ("[{0}] {1}" -f $Source, $Line.Trim())
}

Write-Output "WATCH_START evidence=$EvidenceDir offset=$offset"
while (((Get-Date) - $start).TotalSeconds -lt $TimeoutSec) {
  if (Test-Path $doneMarker) {
    # give a few seconds after operator finishes
    Start-Sleep -Seconds 3
  }
  if (Test-Path $log) {
    $fs = [System.IO.File]::Open($log, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    try {
      if ($offset -gt $fs.Length) { $offset = 0 }
      $fs.Seek($offset, [System.IO.SeekOrigin]::Begin) | Out-Null
      $sr = New-Object System.IO.StreamReader($fs)
      while ($null -ne ($line = $sr.ReadLine())) {
        Emit-Hit -Source "listener-type.log" -Line $line
      }
      $offset = $fs.Position
    } finally {
      $fs.Dispose()
    }
  }
  if (Test-Path $doneMarker) {
    Write-Output "WATCH_STOP operator-result present"
    break
  }
  Start-Sleep -Milliseconds 400
}
# final capsule tail snapshot
if (Test-Path $capsule) {
  Get-Content $capsule -Tail 80 | Set-Content (Join-Path $EvidenceDir "capsule-timeline-tail.log") -Encoding utf8
}
$offset | Set-Content (Join-Path $EvidenceDir "log-end-offset.txt")
Write-Output "WATCH_END"
