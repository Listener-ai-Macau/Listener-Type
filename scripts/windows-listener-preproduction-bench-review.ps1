[CmdletBinding(PositionalBinding = $false)]
param(
  [string]$OutputDir = "",
  [string]$CapabilityManifest = "",
  [string]$StepId = "",
  [switch]$ListSteps,
  [switch]$WriteTemplate
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
  $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
  $OutputDir = Join-Path $repoRoot ".cache\validation\preproduction-bench-review-$stamp"
} elseif (-not [System.IO.Path]::IsPathRooted($OutputDir)) {
  $OutputDir = Join-Path $repoRoot $OutputDir
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$OutputDir = (Resolve-Path -LiteralPath $OutputDir).Path

$summaryPath = Join-Path $OutputDir "preproduction-bench-review-summary.json"
$templatePath = Join-Path $OutputDir "preproduction-bench-capabilities.template.json"

function New-BenchStep {
  param(
    [Parameter(Mandatory = $true)][string]$Id,
    [Parameter(Mandatory = $true)][string]$Title,
    [Parameter(Mandatory = $true)][string[]]$Capabilities,
    [Parameter(Mandatory = $true)][string[]]$Evidence
  )
  [pscustomobject]@{
    id = $Id
    title = $Title
    capabilities = $Capabilities
    evidence = $Evidence
  }
}

$steps = @(
  New-BenchStep "baseline-type-tray-ui" "Type tray/UI baseline" @("type_runtime", "desktop_visual_capture") @("type_process", "window_screenshot", "type_log")
  New-BenchStep "same-name-write-no-repair" "Same-name write does not repair" @("type_runtime", "windows_ble_automation") @("before_ble_state", "after_ble_state", "type_log")
  New-BenchStep "random-name-exact-cache-refresh" "Random BLE name exact cache refresh" @("type_runtime", "windows_ble_automation") @("name_write_log", "windows_ble_state", "gatt_probe")
  New-BenchStep "restore-default-listener" "Restore default listener name" @("type_runtime", "windows_ble_automation") @("name_write_log", "windows_ble_state", "gatt_probe")
  New-BenchStep "manual-windows-delete-no-type-autopair" "Manual Windows delete must not autopair with Type" @("type_runtime", "windows_ble_automation") @("delete_log", "twenty_second_state", "type_log")
  New-BenchStep "no-type-native-pairing" "No-Type native Windows pairing" @("windows_ble_automation") @("native_pair_log", "windows_ble_state", "hid_presence")
  New-BenchStep "type-takeover-no-forced-repair" "Type takeover without forced repair" @("type_runtime", "windows_ble_automation") @("takeover_log", "gatt_probe", "notification_count")
  New-BenchStep "ec11-single-not-double" "EC11 single is not double" @("physical_input_fixture", "windows_ble_automation", "led_optical_capture") @("physical_input_trace", "windows_ble_events", "led_capture")
  New-BenchStep "ec11-double-repair-with-type" "EC11 double-click repair with Type" @("physical_input_fixture", "type_runtime", "windows_ble_automation", "led_optical_capture") @("physical_input_trace", "pairing_flow_log", "led_capture", "gatt_probe")
  New-BenchStep "computer-switch-product-flow" "Computer switch flow" @("windows_ble_automation", "second_ble_host") @("old_host_state", "new_host_pair_log", "takeover_log")
  New-BenchStep "recording-response-and-led-priority" "Recording response and LED priority" @("physical_input_fixture", "audio_fixture", "type_runtime", "led_optical_capture") @("input_trace", "capsule_timeline", "audio_wav", "led_capture")
  New-BenchStep "ble-audio-type-link" "BLE audio Type link" @("audio_fixture", "type_runtime", "windows_ble_automation") @("ble_audio_probe", "type_log", "windows_ble_state")
  New-BenchStep "led-independent-contract" "LED independence contract" @("physical_input_fixture", "led_optical_capture", "power_relay") @("led_matrix_capture", "serial_led_status", "power_cycle_log")
  New-BenchStep "ota-wireless-smoke" "Wireless OTA smoke" @("type_runtime", "windows_ble_automation", "led_optical_capture", "ota_package") @("ota_probe", "ota_log", "led_capture")
  New-BenchStep "wired-flash-smoke" "Wired flash smoke" @("usb_power_relay", "wired_flash_port") @("flash_log", "post_flash_serial_status")
  New-BenchStep "release-package-final-check" "Release package final check" @("release_artifacts") @("msi_hash", "firmware_zip_hash", "root_package_listing")
)

if ($ListSteps.IsPresent) {
  foreach ($step in $steps) {
    Write-Output ("{0}`t{1}`tcapabilities={2}`tevidence={3}" -f $step.id, $step.title, ($step.capabilities -join ","), ($step.evidence -join ","))
  }
  exit 0
}

if (-not [string]::IsNullOrWhiteSpace($StepId)) {
  $selected = @($steps | Where-Object { $_.id -eq $StepId })
  if ($selected.Count -eq 0) {
    throw "Unknown StepId '$StepId'. Use -ListSteps to see valid steps."
  }
  $steps = $selected
}

$template = [ordered]@{
  schema_version = 1
  purpose = "Listener 1.0.2 unattended preproduction bench capabilities and evidence"
  capabilities = [ordered]@{
    type_runtime = $false
    desktop_visual_capture = $false
    windows_ble_automation = $false
    second_ble_host = $false
    usb_power_relay = $false
    power_relay = $false
    physical_input_fixture = $false
    led_optical_capture = $false
    audio_fixture = $false
    wired_flash_port = $false
    ota_package = $false
    release_artifacts = $false
  }
  evidence = [ordered]@{}
}
foreach ($step in $steps) {
  $template.evidence[$step.id] = [ordered]@{}
  foreach ($key in $step.evidence) {
    $template.evidence[$step.id][$key] = ""
  }
}

if ($WriteTemplate.IsPresent -or [string]::IsNullOrWhiteSpace($CapabilityManifest)) {
  $template | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $templatePath -Encoding UTF8
  if ($WriteTemplate.IsPresent) {
    Write-Host "bench_template=$templatePath"
    exit 0
  }
}

$manifest = if ([string]::IsNullOrWhiteSpace($CapabilityManifest)) {
  $template | ConvertTo-Json -Depth 10 | ConvertFrom-Json
} else {
  Get-Content -Raw -LiteralPath (Resolve-Path -LiteralPath $CapabilityManifest).Path | ConvertFrom-Json
}

function Get-ManifestBool {
  param([Parameter(Mandatory = $true)][string]$Name)
  if ($null -eq $manifest.capabilities) { return $false }
  $property = $manifest.capabilities.PSObject.Properties[$Name]
  return ($null -ne $property -and [bool]$property.Value)
}

function Resolve-EvidencePath {
  param([AllowNull()][string]$Path)
  if ([string]::IsNullOrWhiteSpace($Path)) { return "" }
  if ([System.IO.Path]::IsPathRooted($Path)) { return $Path }
  if (-not [string]::IsNullOrWhiteSpace($CapabilityManifest)) {
    $base = Split-Path -Parent (Resolve-Path -LiteralPath $CapabilityManifest).Path
    return (Join-Path $base $Path)
  }
  return (Join-Path $repoRoot $Path)
}

$records = [System.Collections.Generic.List[object]]::new()
foreach ($step in $steps) {
  $missingCapabilities = @($step.capabilities | Where-Object { -not (Get-ManifestBool $_) })
  $missingEvidence = [System.Collections.Generic.List[string]]::new()
  $evidenceOut = [ordered]@{}
  foreach ($key in $step.evidence) {
    $value = ""
    if ($null -ne $manifest.evidence -and $null -ne $manifest.evidence.PSObject.Properties[$step.id]) {
      $stepEvidence = $manifest.evidence.PSObject.Properties[$step.id].Value
      if ($null -ne $stepEvidence.PSObject.Properties[$key]) {
        $value = [string]$stepEvidence.PSObject.Properties[$key].Value
      }
    }
    $resolved = Resolve-EvidencePath $value
    $evidenceOut[$key] = $resolved
    if ([string]::IsNullOrWhiteSpace($resolved) -or -not (Test-Path -LiteralPath $resolved)) {
      $missingEvidence.Add($key) | Out-Null
    }
  }

  $status = if ($missingCapabilities.Count -eq 0 -and $missingEvidence.Count -eq 0) { "PASS" } else { "NO_GO" }
  $records.Add([ordered]@{
      id = $step.id
      title = $step.title
      status = $status
      required_capabilities = $step.capabilities
      missing_capabilities = $missingCapabilities
      required_evidence = $step.evidence
      missing_evidence = @($missingEvidence)
      evidence = $evidenceOut
    }) | Out-Null
}

$noGo = @($records | Where-Object { $_.status -ne "PASS" })
$status = if ($noGo.Count -eq 0) { "BENCH_REVIEW_PASS" } else { "BENCH_REVIEW_NO_GO" }

[ordered]@{
  schema_version = 1
  status = $status
  generated_at = (Get-Date).ToString("o")
  repo_root = $repoRoot
  output_dir = $OutputDir
  capability_manifest = $CapabilityManifest
  template = $templatePath
  records = @($records)
} | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $summaryPath -Encoding UTF8

Write-Host "preproduction_bench_review_status=$status"
Write-Host "summary=$summaryPath"
if ($status -eq "BENCH_REVIEW_PASS") {
  exit 0
}
exit 2
