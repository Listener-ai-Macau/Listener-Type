param(
  [string]$ListenerRoot = "",
  [string]$TypeRepo = "",
  [string]$FirmwareRepo = "",
  [string]$OutputJson = "",
  [switch]$AllowDirtyWorktree,
  [switch]$AllowDifferentHead
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$scriptDir = Split-Path -Parent $PSCommandPath
if ([string]::IsNullOrWhiteSpace($TypeRepo)) {
  $TypeRepo = (Resolve-Path -LiteralPath (Join-Path $scriptDir "..")).Path
}
if ([string]::IsNullOrWhiteSpace($ListenerRoot)) {
  $ListenerRoot = (Resolve-Path -LiteralPath (Join-Path $TypeRepo "..")).Path
}
if ([string]::IsNullOrWhiteSpace($FirmwareRepo)) {
  $FirmwareRepo = Join-Path $ListenerRoot "Listener-Firmware"
}

$baselinePath = Join-Path $scriptDir "listener-1.0.4-requirement-test-map.json"
if (-not (Test-Path -LiteralPath $baselinePath)) {
  throw "release baseline missing: $baselinePath"
}
$baseline = (Get-Content -Raw -LiteralPath $baselinePath | ConvertFrom-Json).frozen_baseline
$expected = [ordered]@{
  type_tag        = [string]$baseline.type_tag
  firmware_commit = [string]$baseline.firmware_commit
  firmware_tag    = [string]$baseline.firmware_tag
  msi_sha256      = ([string]$baseline.msi_sha256).ToUpperInvariant()
  ota_sha256      = ([string]$baseline.ota_sha256).ToUpperInvariant()
  msi_name        = "ListenerType_1.0.4_x64_en-US.msi"
  ota_name        = "ListenerFirmware_1.0.4_ota.zip"
}

function Get-Sha256Upper {
  param([Parameter(Mandatory = $true)][string]$Path)
  return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToUpperInvariant()
}

function Get-GitHead {
  param([Parameter(Mandatory = $true)][string]$Repo)
  return (& git -C $Repo rev-parse HEAD).Trim()
}

function Get-GitTagCommit {
  param(
    [Parameter(Mandatory = $true)][string]$Repo,
    [Parameter(Mandatory = $true)][string]$Tag
  )
  return (& git -C $Repo rev-list -n 1 $Tag).Trim()
}

function Get-GitDescribe {
  param([Parameter(Mandatory = $true)][string]$Repo)
  return (& git -C $Repo describe --tags --always).Trim()
}

function Get-GitDirtyCount {
  param([Parameter(Mandatory = $true)][string]$Repo)
  $lines = @(& git -C $Repo status --porcelain=v1 --untracked-files=all)
  return @($lines | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }).Count
}

$checks = [System.Collections.Generic.List[object]]::new()
function Add-Check {
  param(
    [string]$Id,
    [string]$Status,
    [string]$Detail = ""
  )
  $checks.Add([pscustomobject]@{
      id     = $Id
      status = $Status
      detail = $Detail
    }) | Out-Null
  $prefix = if ($Status -eq "PASS") { "[PASS]" } elseif ($Status -eq "WARN") { "[WARN]" } else { "[FAIL]" }
  Write-Host ("{0} {1}: {2}" -f $prefix, $Id, $Detail)
}

$msiPath = Join-Path $ListenerRoot $expected.msi_name
$otaPath = Join-Path $ListenerRoot $expected.ota_name
$sumsPath = Join-Path $ListenerRoot "SHA256SUMS.txt"

if (-not (Test-Path -LiteralPath $msiPath)) {
  Add-Check "msi_present" "FAIL" "missing $msiPath"
} else {
  $msiHash = Get-Sha256Upper $msiPath
  if ($msiHash -eq $expected.msi_sha256) {
    Add-Check "msi_sha256" "PASS" $msiHash
  } else {
    Add-Check "msi_sha256" "FAIL" "got $msiHash expected $($expected.msi_sha256)"
  }
}

if (-not (Test-Path -LiteralPath $otaPath)) {
  Add-Check "ota_present" "FAIL" "missing $otaPath"
} else {
  $otaHash = Get-Sha256Upper $otaPath
  if ($otaHash -eq $expected.ota_sha256) {
    Add-Check "ota_sha256" "PASS" $otaHash
  } else {
    Add-Check "ota_sha256" "FAIL" "got $otaHash expected $($expected.ota_sha256)"
  }
}

if (Test-Path -LiteralPath $sumsPath) {
  $sums = Get-Content -LiteralPath $sumsPath -Raw
  if ($sums -match [regex]::Escape($expected.msi_sha256.ToLowerInvariant()) -or
    $sums -match [regex]::Escape($expected.msi_sha256)) {
    Add-Check "sha256sums_msi" "PASS" "SHA256SUMS contains frozen MSI hash"
  } else {
    Add-Check "sha256sums_msi" "FAIL" "SHA256SUMS missing frozen MSI hash"
  }
  if ($sums -match [regex]::Escape($expected.ota_sha256.ToLowerInvariant()) -or
    $sums -match [regex]::Escape($expected.ota_sha256)) {
    Add-Check "sha256sums_ota" "PASS" "SHA256SUMS contains frozen OTA hash"
  } else {
    Add-Check "sha256sums_ota" "FAIL" "SHA256SUMS missing frozen OTA hash"
  }
} else {
  Add-Check "sha256sums" "WARN" "SHA256SUMS.txt missing"
}

$typeHead = Get-GitHead $TypeRepo
$fwHead = Get-GitHead $FirmwareRepo
$typeTagCommit = Get-GitTagCommit $TypeRepo $expected.type_tag
$fwTagCommit = Get-GitTagCommit $FirmwareRepo $expected.firmware_tag
$typeDesc = Get-GitDescribe $TypeRepo
$fwDesc = Get-GitDescribe $FirmwareRepo
$typeDirty = Get-GitDirtyCount $TypeRepo
$fwDirty = Get-GitDirtyCount $FirmwareRepo

if ($typeHead -eq $typeTagCommit) {
  Add-Check "type_commit" "PASS" "$typeHead (tag $($expected.type_tag))"
} elseif ($AllowDifferentHead.IsPresent) {
  Add-Check "type_commit" "WARN" "head=$typeHead tag_commit=$typeTagCommit (AllowDifferentHead)"
} else {
  Add-Check "type_commit" "FAIL" "head=$typeHead tag_commit=$typeTagCommit; release HEAD must match $($expected.type_tag)"
}

if ($fwTagCommit -eq $expected.firmware_commit -and $fwHead -eq $expected.firmware_commit) {
  Add-Check "firmware_commit" "PASS" "$fwHead (tag $($expected.firmware_tag))"
} else {
  Add-Check "firmware_commit" "FAIL" "head=$fwHead tag_commit=$fwTagCommit expected=$($expected.firmware_commit)"
}

if ($typeDesc -like "v1.0.4*") {
  Add-Check "type_tag" "PASS" $typeDesc
} else {
  Add-Check "type_tag" "WARN" "describe=$typeDesc"
}

if ($fwDesc -eq $expected.firmware_tag) {
  Add-Check "firmware_tag" "PASS" $fwDesc
} else {
  Add-Check "firmware_tag" "FAIL" "describe=$fwDesc expected=$($expected.firmware_tag)"
}

if ($typeDirty -eq 0) {
  Add-Check "type_clean" "PASS" "clean worktree"
} elseif ($AllowDirtyWorktree.IsPresent) {
  Add-Check "type_clean" "WARN" "dirty_paths=$typeDirty (AllowDirtyWorktree)"
} else {
  Add-Check "type_clean" "FAIL" "dirty_paths=$typeDirty; use -AllowDirtyWorktree while adding anti-regression tooling"
}

if ($fwDirty -eq 0) {
  Add-Check "firmware_clean" "PASS" "clean worktree"
} elseif ($AllowDirtyWorktree.IsPresent) {
  Add-Check "firmware_clean" "WARN" "dirty_paths=$fwDirty (AllowDirtyWorktree)"
} else {
  Add-Check "firmware_clean" "FAIL" "dirty_paths=$fwDirty; use -AllowDirtyWorktree while adding anti-regression tooling"
}

$failed = @($checks | Where-Object { $_.status -eq "FAIL" })
$result = if ($failed.Count -eq 0) { "PASS" } else { "FAIL" }

$summary = [ordered]@{
  schema          = "listener.1.0.4.identity"
  schema_version  = 1
  result          = $result
  expected        = $expected
  observed        = [ordered]@{
    type_commit     = $typeHead
    type_describe   = $typeDesc
    type_dirty      = $typeDirty
    firmware_commit = $fwHead
    firmware_describe = $fwDesc
    firmware_dirty  = $fwDirty
    msi_path        = $msiPath
    ota_path        = $otaPath
  }
  checks          = @($checks)
}

$json = $summary | ConvertTo-Json -Depth 8
if (-not [string]::IsNullOrWhiteSpace($OutputJson)) {
  $dir = Split-Path -Parent $OutputJson
  if (-not [string]::IsNullOrWhiteSpace($dir)) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
  }
  Set-Content -LiteralPath $OutputJson -Value $json -Encoding utf8
}
Write-Output $json
if ($result -ne "PASS") { exit 1 }
exit 0
