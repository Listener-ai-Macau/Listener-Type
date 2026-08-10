[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$Manifest,

  [string]$Output = ".artifacts/speaker-evaluation/report.json",

  [string]$RuntimeRoot = "$env:APPDATA/Listener Type/models/speaker-verification"
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$manifestPath = (Resolve-Path -LiteralPath $Manifest).Path
$runtimeRootPath = (Resolve-Path -LiteralPath $RuntimeRoot).Path
foreach ($runtimeFile in @("onnxruntime.dll", "onnxruntime_providers_shared.dll", "sherpa-onnx-c-api.dll")) {
  if (-not (Test-Path -LiteralPath (Join-Path $runtimeRootPath $runtimeFile))) {
    throw "Speaker evaluation runtime is missing $runtimeFile under $runtimeRootPath."
  }
}
$outputPath = if ([System.IO.Path]::IsPathRooted($Output)) {
  $Output
} else {
  Join-Path $repoRoot $Output
}

$manifestData = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
if (@($manifestData.models).Count -lt 1) {
  throw "Speaker evaluation manifest must contain at least one model."
}
if (@($manifestData.enrollment_session_wavs).Count -lt 3) {
  throw "Speaker evaluation manifest needs at least three owner enrollment references."
}
$ownerCount = @($manifestData.samples | Where-Object { $_.label -eq "owner" }).Count
$nonOwnerCount = @($manifestData.samples | Where-Object { $_.label -eq "non_owner" }).Count
if ($ownerCount -lt 20 -or $nonOwnerCount -lt 20) {
  throw "Speaker evaluation needs at least 20 owner and 20 non-owner samples; owner=$ownerCount non_owner=$nonOwnerCount."
}

$outputDir = Split-Path -Parent $outputPath
if ($outputDir) {
  New-Item -ItemType Directory -Force -Path $outputDir | Out-Null
}

$env:LISTENER_SPEAKER_EVAL_MANIFEST = $manifestPath
$env:LISTENER_SPEAKER_EVAL_OUTPUT = $outputPath
$env:LISTENER_SPEAKER_EVAL_RUNTIME_ROOT = $runtimeRootPath
try {
  Push-Location $repoRoot
  try {
    cargo test --manifest-path src-tauri/Cargo.toml --lib `
      speaker_verification::platform::tests::runtime_evaluates_listener_labeled_speaker_corpus `
      -- --ignored --exact --nocapture
    if ($LASTEXITCODE -ne 0) {
      throw "Speaker verification evaluation failed with exit code $LASTEXITCODE."
    }
  } finally {
    Pop-Location
  }
} finally {
  Remove-Item Env:LISTENER_SPEAKER_EVAL_MANIFEST -ErrorAction SilentlyContinue
  Remove-Item Env:LISTENER_SPEAKER_EVAL_OUTPUT -ErrorAction SilentlyContinue
  Remove-Item Env:LISTENER_SPEAKER_EVAL_RUNTIME_ROOT -ErrorAction SilentlyContinue
}

$reports = Get-Content -Raw -LiteralPath $outputPath | ConvertFrom-Json
$reports | Select-Object model, model_sha256, threshold, owner_recall, non_owner_suppression, inference_p95_ms, pass | Format-Table -AutoSize
if (-not (@($reports | Where-Object { $_.pass }).Count)) {
  throw "No evaluated model met all speaker verification gates."
}
Write-Host "Speaker verification evaluation PASS: $outputPath"
