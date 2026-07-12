#requires -Version 7.0
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [Parameter(Mandatory = $true)][string]$StatePath,
    [Parameter(Mandatory = $true)][string]$SummaryPath,
    [string]$TriagePath = ""
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

function Test-SamePath {
    param([Parameter(Mandatory = $true)][string]$Left, [Parameter(Mandatory = $true)][string]$Right)

    return [string]::Equals(
        [System.IO.Path]::GetFullPath($Left),
        [System.IO.Path]::GetFullPath($Right),
        [System.StringComparison]::OrdinalIgnoreCase
    )
}

function Get-OperatorNoteText {
    param([Parameter(Mandatory = $true)]$Record)

    foreach ($fieldName in @("operator_note", "operator_action", "observation")) {
        $property = $Record.PSObject.Properties[$fieldName]
        if ($null -eq $property) {
            continue
        }
        $value = ([string]$property.Value).Trim()
        if (-not [string]::IsNullOrWhiteSpace($value)) {
            return $value
        }
    }
    return ""
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$manifestPath = Resolve-ExistingFile -Path (Join-Path $PSScriptRoot "listener-preproduction-scenarios.json") -Label "Scenario manifest"
$checkerPath = Resolve-ExistingFile -Path (Join-Path $PSScriptRoot "check-preproduction-operator-note-triage.mjs") -Label "Operator-note triage checker"
$statePath = Resolve-ExistingFile -Path $StatePath -Label "Total review state"
$summaryPath = Resolve-ExistingFile -Path $SummaryPath -Label "Focused review summary"
$triagePath = if ([string]::IsNullOrWhiteSpace($TriagePath)) {
    ""
} else {
    Resolve-ExistingFile -Path $TriagePath -Label "Focused review triage"
}

$manifest = Read-JsonFile -Path $manifestPath -Label "Scenario manifest"
$scenarioIds = @($manifest.scenarios | ForEach-Object { ([string]$_.id).Trim() })
if ($scenarioIds.Count -eq 0 -or $scenarioIds.Count -ne @($scenarioIds | Select-Object -Unique).Count) {
    throw "Scenario manifest must declare unique non-empty scenario ids: $manifestPath"
}

$state = Read-JsonFile -Path $statePath -Label "Total review state"
if ([string]$state.status -ne "ACTIVE" -or [string]::IsNullOrWhiteSpace([string]$state.next_step_id) -or $null -eq $state.completed_records) {
    throw "Total review state must be ACTIVE with next_step_id and completed_records: $statePath"
}
$currentId = ([string]$state.next_step_id).Trim()
$currentIndex = [Array]::IndexOf([string[]]$scenarioIds, $currentId)
if ($currentIndex -lt 0) {
    throw "Total review state next_step_id is not in the canonical scenario manifest: $currentId"
}

$summary = Read-JsonFile -Path $summaryPath -Label "Focused review summary"
if (-not (Test-SamePath -Left ([string]$summary.total_review_state) -Right $statePath)) {
    throw "Focused review summary belongs to a different total review state: $summaryPath"
}
$focusedIds = @($summary.focus_step_ids | ForEach-Object { ([string]$_).Trim() })
if ($focusedIds.Count -ne 1 -or $focusedIds[0] -ne $currentId) {
    throw "Focused review summary must contain exactly the current total-review step '$currentId': $summaryPath"
}
$matchingRecords = @($summary.records | Where-Object { ([string]$_.id).Trim() -eq $currentId })
$matchingRecordCarriedForward = $false
if ($matchingRecords.Count -eq 1) {
    $carriedForwardProperty = $matchingRecords[0].PSObject.Properties["carried_forward"]
    if ($null -ne $carriedForwardProperty) {
        $matchingRecordCarriedForward = [bool]$carriedForwardProperty.Value
    }
}
if ($matchingRecords.Count -ne 1 -or [string]$matchingRecords[0].result -ne "PASS" -or $matchingRecordCarriedForward) {
    throw "Focused review summary does not prove an original human PASS for '$currentId': $summaryPath"
}

$summaryHasOperatorNote = @($summary.records | Where-Object {
    -not [string]::IsNullOrWhiteSpace((Get-OperatorNoteText -Record $_))
}).Count -gt 0
if ($summaryHasOperatorNote) {
    if ([string]::IsNullOrWhiteSpace($triagePath)) {
        throw "Focused review triage is required because the summary contains an operator note: $summaryPath"
    }
    $triage = Read-JsonFile -Path $triagePath -Label "Focused review triage"
    if ([string]$triage.status -ne "PASS" -or -not (Test-SamePath -Left ([string]$triage.source_summary) -Right $summaryPath)) {
        throw "Focused review triage must be PASS and belong to the same summary: $triagePath"
    }

    $node = Get-Command node -CommandType Application -ErrorAction SilentlyContinue
    if ($null -eq $node) {
        throw "Node.js is required to validate focused operator-note triage."
    }
    & $node.Source $checkerPath --summary $summaryPath --triage $triagePath
    if ($LASTEXITCODE -ne 0) {
        throw "Focused operator-note triage checker failed for: $summaryPath"
    }
}

$completedIds = @()
foreach ($record in @($state.completed_records)) {
    $recordId = ([string]$record.id).Trim()
    if ([string]::IsNullOrWhiteSpace($recordId) -or $scenarioIds -notcontains $recordId -or $completedIds -contains $recordId) {
        throw "Total review state has an invalid or duplicate completed record id: $statePath"
    }
    $completedIds += $recordId
}
if ($completedIds -contains $currentId) {
    throw "Total review state already marks current step complete: $currentId"
}

$nextState = [ordered]@{}
foreach ($property in $state.PSObject.Properties) {
    $nextState[$property.Name] = $property.Value
}
$nextState.completed_records = @($state.completed_records) + @([ordered]@{
    id = $currentId
    source_summary = $summaryPath
})
$nextState.last_advanced_at = (Get-Date).ToString("o")
$nextState.last_advanced_by = "advance-preproduction-total-review.ps1"
$nextState.last_advanced_summary = $summaryPath
if ($currentIndex -eq ($scenarioIds.Count - 1)) {
    $nextState.status = "COMPLETE"
    $nextState.next_step_id = ""
} else {
    $nextState.next_step_id = $scenarioIds[$currentIndex + 1]
}

$advance = [pscustomobject]@{
    state_path = $statePath
    completed_step_id = $currentId
    next_step_id = [string]$nextState.next_step_id
    state_status = [string]$nextState.status
    summary_path = $summaryPath
    triage_path = $triagePath
}

if ($WhatIfPreference) {
    $advance | ConvertTo-Json -Compress
    exit 0
}

$tempPath = Join-Path (Split-Path -Parent $statePath) (".$([System.IO.Path]::GetFileName($statePath)).$([guid]::NewGuid().ToString('N')).tmp")
try {
    $nextState | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $tempPath -Encoding utf8NoBOM
    Move-Item -LiteralPath $tempPath -Destination $statePath -Force
} finally {
    if (Test-Path -LiteralPath $tempPath) {
        Remove-Item -LiteralPath $tempPath -Force
    }
}

$advance | ConvertTo-Json -Compress
