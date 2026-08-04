param(
  [string]$OutputDir = "",
  [string]$OutputJson = "",
  [int]$HoldSeconds = 1
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$scriptDir = Split-Path -Parent $PSCommandPath
$typeRoot = (Resolve-Path -LiteralPath (Join-Path $scriptDir "..")).Path
$productionProfile = Join-Path $env:APPDATA "Listener Type"
$productionLocal = Join-Path $env:LOCALAPPDATA "Listener Type"

function Get-TreeSnapshot {
  param([string]$Root)
  if (-not (Test-Path -LiteralPath $Root)) {
    return [ordered]@{
      exists = $false
      files  = @()
      count  = 0
      bytes  = 0
      mtimes = @{}
    }
  }
  $files = Get-ChildItem -LiteralPath $Root -Recurse -File -Force -ErrorAction SilentlyContinue
  $mtimes = @{}
  $bytes = 0L
  foreach ($f in $files) {
    $rel = $f.FullName.Substring($Root.Length).TrimStart('\', '/')
    $mtimes[$rel] = $f.LastWriteTimeUtc.ToString("o")
    $bytes += $f.Length
  }
  return [ordered]@{
    exists = $true
    files  = @($mtimes.Keys | Sort-Object)
    count  = $mtimes.Count
    bytes  = $bytes
    mtimes = $mtimes
  }
}

function Compare-Snapshots {
  param($Before, $After, [string]$Label)
  $added = @()
  $changed = @()
  $removed = @()
  if (-not $Before.exists -and -not $After.exists) {
    return [pscustomobject]@{ label = $Label; status = "PASS"; added = @(); changed = @(); removed = @() }
  }
  $beforeKeys = @($Before.mtimes.Keys)
  $afterKeys = @($After.mtimes.Keys)
  foreach ($k in $afterKeys) {
    if (-not $Before.mtimes.ContainsKey($k)) {
      $added += $k
    } elseif ($Before.mtimes[$k] -ne $After.mtimes[$k]) {
      $changed += $k
    }
  }
  foreach ($k in $beforeKeys) {
    if (-not $After.mtimes.ContainsKey($k)) {
      $removed += $k
    }
  }
  $status = if (($added.Count + $changed.Count + $removed.Count) -eq 0) { "PASS" } else { "FAIL" }
  return [pscustomobject]@{
    label   = $Label
    status  = $status
    added   = $added
    changed = $changed
    removed = $removed
  }
}

if ([string]::IsNullOrWhiteSpace($OutputDir)) {
  $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
  $OutputDir = Join-Path $typeRoot ".artifacts\listener-1.0.4-clean-env\$stamp"
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
if ([string]::IsNullOrWhiteSpace($OutputJson)) {
  $OutputJson = Join-Path $OutputDir "clean-env-summary.json"
}

$beforeApp = Get-TreeSnapshot $productionProfile
$beforeLocal = Get-TreeSnapshot $productionLocal

# Isolated synthetic data dir — never APPDATA production profile.
$isolated = Join-Path $env:TEMP ("listener-type-clean-env-{0}-{1}" -f $PID, [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $isolated | Out-Null
$marker = Join-Path $isolated "marker.txt"
"clean-env probe" | Set-Content -LiteralPath $marker -Encoding utf8

# Run protected contracts under isolated env (no Type process).
$prevData = [Environment]::GetEnvironmentVariable("LISTENER_TYPE_DATA_DIR", "Process")
[Environment]::SetEnvironmentVariable("LISTENER_TYPE_DATA_DIR", $isolated, "Process")
try {
  $contractLog = Join-Path $OutputDir "protected-contracts.log"
  & node (Join-Path $scriptDir "verify-listener-1.0.4-protected-contracts.mjs") *>&1 |
    Tee-Object -FilePath $contractLog | Out-Null
  $contractExit = $LASTEXITCODE
} finally {
  if ($null -eq $prevData) {
    [Environment]::SetEnvironmentVariable("LISTENER_TYPE_DATA_DIR", $null, "Process")
  } else {
    [Environment]::SetEnvironmentVariable("LISTENER_TYPE_DATA_DIR", $prevData, "Process")
  }
}

Start-Sleep -Seconds $HoldSeconds

$afterApp = Get-TreeSnapshot $productionProfile
$afterLocal = Get-TreeSnapshot $productionLocal
$appDiff = Compare-Snapshots $beforeApp $afterApp "APPDATA\\Listener Type"
$localDiff = Compare-Snapshots $beforeLocal $afterLocal "LOCALAPPDATA\\Listener Type"

# Artifact output must stay under repo .artifacts or TEMP, never production profile.
$artifactIsSafe = $OutputDir.StartsWith((Join-Path $typeRoot ".artifacts"), [System.StringComparison]::OrdinalIgnoreCase) -or
  $OutputDir.StartsWith($env:TEMP, [System.StringComparison]::OrdinalIgnoreCase)

$checks = @(
  [pscustomobject]@{ id = "protected_contracts"; status = $(if ($contractExit -eq 0) { "PASS" } else { "FAIL" }); detail = "exit=$contractExit" }
  [pscustomobject]@{ id = "no_appdata_write"; status = $appDiff.status; detail = ($appDiff | ConvertTo-Json -Compress) }
  [pscustomobject]@{ id = "no_localappdata_write"; status = $localDiff.status; detail = ($localDiff | ConvertTo-Json -Compress) }
  [pscustomobject]@{ id = "artifact_path_safe"; status = $(if ($artifactIsSafe) { "PASS" } else { "FAIL" }); detail = $OutputDir }
  [pscustomobject]@{ id = "isolated_data_dir"; status = $(if (Test-Path -LiteralPath $marker) { "PASS" } else { "FAIL" }); detail = $isolated }
)

$failed = @($checks | Where-Object { $_.status -eq "FAIL" })
$summary = [ordered]@{
  schema         = "listener.1.0.4.clean_env"
  schema_version = 1
  result         = $(if ($failed.Count -eq 0) { "PASS" } else { "FAIL" })
  production_appdata = $productionProfile
  production_localappdata = $productionLocal
  isolated_data_dir = $isolated
  output_dir     = $OutputDir
  checks         = $checks
  appdata_diff   = $appDiff
  localappdata_diff = $localDiff
  privacy_note   = "This probe does not read user transcripts, credentials, or recordings. It only compares file mtimes under production profiles."
}

$json = $summary | ConvertTo-Json -Depth 10
Set-Content -LiteralPath $OutputJson -Value $json -Encoding utf8
Write-Output $json

# Cleanup isolated probe dir.
try { Remove-Item -LiteralPath $isolated -Recurse -Force -ErrorAction SilentlyContinue } catch {}

if ($summary.result -ne "PASS") { exit 1 }
exit 0
