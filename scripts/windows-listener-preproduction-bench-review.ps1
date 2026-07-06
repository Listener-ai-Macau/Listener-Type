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

function Get-ScenarioManifest {
  $path = Join-Path $PSScriptRoot "listener-preproduction-scenarios.json"
  if (-not (Test-Path -LiteralPath $path)) {
    throw "Canonical preproduction scenario manifest is missing: $path"
  }
  $manifest = Get-Content -Raw -LiteralPath $path | ConvertFrom-Json
  $scenarios = @($manifest.scenarios)
  if ($scenarios.Count -eq 0) {
    throw "Canonical preproduction scenario manifest has no scenarios: $path"
  }
  return $manifest
}

$scenarioManifest = Get-ScenarioManifest
$steps = @($scenarioManifest.scenarios | ForEach-Object {
    New-BenchStep `
      -Id ([string]$_.id) `
      -Title ([string]$_.bench_title) `
      -Capabilities @($_.bench_capabilities) `
      -Evidence @($_.bench_evidence)
  })

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
  scenario_manifest = (Join-Path $PSScriptRoot "listener-preproduction-scenarios.json")
  capabilities = [ordered]@{}
  evidence = [ordered]@{}
}
$capabilityNames = @($steps | ForEach-Object { $_.capabilities } | Sort-Object -Unique)
foreach ($capability in $capabilityNames) {
  $template.capabilities[$capability] = $false
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
