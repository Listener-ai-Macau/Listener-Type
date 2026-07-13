#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$StatePath,
    [Parameter(Mandatory = $true)][string]$StepId,
    [Parameter(Mandatory = $true)][string]$OutputDir,
    [Parameter(Mandatory = $true)][string[]]$EvidencePath,
    [string]$DelegationNote = "Operator delegated this focused acceptance to machine validation.",
    [switch]$Execute
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Resolve-ExistingFile {
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][string]$Label)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Label is missing: $Path"
    }
    return (Resolve-Path -LiteralPath $Path).Path
}

function Read-JsonFile {
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][string]$Label)

    try {
        return Get-Content -Raw -LiteralPath $Path | ConvertFrom-Json
    } catch {
        throw "$Label is unreadable JSON: $Path ($($_.Exception.Message))"
    }
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$manifestPath = Resolve-ExistingFile -Path (Join-Path $PSScriptRoot "listener-preproduction-scenarios.json") -Label "Scenario manifest"
$statePath = Resolve-ExistingFile -Path $StatePath -Label "Total review state"
$state = Read-JsonFile -Path $statePath -Label "Total review state"
if ([string]$state.status -ne "ACTIVE" -or [string]$state.next_step_id -ne $StepId) {
    throw "Machine acceptance must match the active total-review step '$([string]$state.next_step_id)': $statePath"
}

$manifest = Read-JsonFile -Path $manifestPath -Label "Scenario manifest"
$scenario = @($manifest.scenarios | Where-Object { [string]$_.id -eq $StepId })
if ($scenario.Count -ne 1) {
    throw "Unknown or duplicate scenario id '$StepId' in $manifestPath"
}

$evidence = @()
foreach ($rawValue in $EvidencePath) {
    foreach ($rawPath in ([string]$rawValue -split ",")) {
        $trimmedPath = $rawPath.Trim()
        if ([string]::IsNullOrWhiteSpace($trimmedPath)) {
            continue
        }
        $path = Resolve-ExistingFile -Path $trimmedPath -Label "Machine evidence"
        $document = Read-JsonFile -Path $path -Label "Machine evidence"
        if ([string]$document.status -ne "PASS") {
            throw "Machine evidence must report status=PASS: $path"
        }
        $evidence += [ordered]@{
            path = $path
            sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash
            status = [string]$document.status
        }
    }
}
if ($evidence.Count -eq 0) {
    throw "Machine acceptance requires at least one non-empty evidence path."
}

if (-not [System.IO.Path]::IsPathRooted($OutputDir)) {
    $OutputDir = Join-Path $repoRoot $OutputDir
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$OutputDir = (Resolve-Path -LiteralPath $OutputDir).Path
$summaryPath = Join-Path $OutputDir "preproduction-machine-acceptance-summary.json"
$sessionPath = Join-Path $OutputDir "preproduction-machine-acceptance-session.jsonl"
$now = (Get-Date).ToString("o")
$scenarioTitle = if ($scenario[0].PSObject.Properties["bench_title"]) {
    [string]$scenario[0].bench_title
} else {
    $StepId
}
$record = [ordered]@{
    id = $StepId
    title = $scenarioTitle
    result = "PASS"
    acceptance_kind = "machine"
    operator_note = ""
    operator_action = ""
    observation = ""
    machine_evidence = @($evidence)
    started_at = $now
    ended_at = $now
}
$summary = [ordered]@{
    schema_version = 1
    status = "MACHINE_VALIDATION_PASS"
    review_mode = "machine"
    acceptance_authority = "operator-delegated"
    delegation_note = $DelegationNote
    total_review_state = $statePath
    focus_step_ids = @($StepId)
    output_dir = $OutputDir
    session_jsonl = $sessionPath
    records = @($record)
}

if (-not $Execute.IsPresent) {
    [pscustomobject]@{
        status = "READY"
        step_id = $StepId
        summary_path = $summaryPath
        evidence = $evidence
    } | ConvertTo-Json -Depth 8
    exit 0
}

($record | ConvertTo-Json -Depth 8 -Compress) | Set-Content -LiteralPath $sessionPath -Encoding utf8NoBOM
$summary | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $summaryPath -Encoding utf8NoBOM

$advanceScript = Join-Path $PSScriptRoot "advance-preproduction-total-review.ps1"
& pwsh -NoProfile -File $advanceScript -StatePath $statePath -SummaryPath $summaryPath
if ($LASTEXITCODE -ne 0) {
    throw "Could not advance total review from machine acceptance: $summaryPath"
}

[pscustomobject]@{
    status = "PASS"
    step_id = $StepId
    summary_path = $summaryPath
    evidence = $evidence
} | ConvertTo-Json -Depth 8
