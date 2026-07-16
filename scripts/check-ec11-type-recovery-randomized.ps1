[CmdletBinding(PositionalBinding = $false)]
param(
    [string]$Port = "COM3",
    [ValidateRange(3, 8)]
    [int]$Iterations = 5,
    [ValidateRange(10, 60)]
    [int]$CaptureSeconds = 25,
    [ValidateRange(1, 60000)]
    [int]$MaxDoubleClickToAdvertisingAcceptedMs = 250,
    [ValidateRange(1, 60000)]
    [int]$MaxConnectionToEncryptionMs = 1200,
    [ValidateRange(1, 60000)]
    [int]$MaxFreshPairingToTypeReadyMs = 6000,
    [ValidateRange(0, 15000)]
    [int]$MinPhaseDelayMs = 600,
    [ValidateRange(1, 20000)]
    [int]$MaxPhaseDelayMs = 7600,
    [Parameter(Mandatory = $true)]
    [string]$OutputJson
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if ($MaxPhaseDelayMs -le $MinPhaseDelayMs) {
    throw "-MaxPhaseDelayMs must be greater than -MinPhaseDelayMs"
}

function Write-RunSummary {
    param([System.Collections.IDictionary]$Summary)

    $directory = Split-Path -Parent $OutputJson
    if (-not [string]::IsNullOrWhiteSpace($directory)) {
        New-Item -ItemType Directory -Force -Path $directory | Out-Null
    }
    $json = $Summary | ConvertTo-Json -Depth 10
    Set-Content -LiteralPath $OutputJson -Value $json -Encoding utf8
    Write-Output $json
}

$typeRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
$singleSampleScript = Join-Path $PSScriptRoot "check-ec11-type-recovery-speed.ps1"
$workflowRoot = $env:AI_WORKFLOW_REPO
if ([string]::IsNullOrWhiteSpace($workflowRoot) -or -not (Test-Path -LiteralPath (Join-Path $workflowRoot "docs\\agent_quickstart.md"))) {
    $workflowRoot = "C:\Users\Billy\Desktop\Denzic\ai-collaboration-workflow"
}
$aiwScript = Join-Path $workflowRoot "scripts\\aiw.ps1"
if (-not (Test-Path -LiteralPath $aiwScript)) {
    throw "Missing workflow lock command: $aiwScript"
}

$results = @()
$runFailed = $false
for ($iteration = 1; $iteration -le $Iterations; $iteration += 1) {
    $phaseDelayMs = Get-Random -Minimum $MinPhaseDelayMs -Maximum ($MaxPhaseDelayMs + 1)
    Start-Sleep -Milliseconds $phaseDelayMs

    $samplePath = Join-Path (Split-Path -Parent $OutputJson) ("ec11-recovery-sample-{0:D2}.json" -f $iteration)
    $lockOutput = & pwsh -NoProfile -File $aiwScript with-lock -Resource $Port -Wait -WaitTimeoutSeconds 120 -Purpose "randomized EC11 Type recovery speed sample $iteration of $Iterations" -Run pwsh -NoProfile -File $singleSampleScript -Port $Port -CaptureSeconds $CaptureSeconds -MaxDoubleClickToAdvertisingAcceptedMs $MaxDoubleClickToAdvertisingAcceptedMs -MaxConnectionToEncryptionMs $MaxConnectionToEncryptionMs -MaxFreshPairingToTypeReadyMs $MaxFreshPairingToTypeReadyMs -OutputJson $samplePath 2>&1
    $lockExitCode = $LASTEXITCODE

    $sample = if (Test-Path -LiteralPath $samplePath) {
        Get-Content -LiteralPath $samplePath -Raw | ConvertFrom-Json
    } else {
        [pscustomobject]@{
            status = "FAIL"
            failure_reasons = @("sample did not produce a JSON artifact", ($lockOutput | Out-String).Trim())
        }
    }
    $results += [ordered]@{
        iteration = $iteration
        phase_delay_ms = $phaseDelayMs
        lock_exit_code = $lockExitCode
        sample_path = $samplePath
        status = $sample.status
        advertising_command_accepted_ms = $sample.advertising_command_accepted_ms
        connection_to_encryption_ms = $sample.connection_to_encryption_ms
        fresh_pairing_to_type_ready_ms = $sample.fresh_pairing_to_type_ready_ms
        trigger_to_type_ready_ms_informational = $sample.trigger_to_type_ready_ms_informational
        firmware_pre_reset_notice_sent = $sample.firmware_pre_reset_notice_sent
        type_pre_reset_notice_observed = $sample.type_pre_reset_notice_observed
        failure_reasons = @($sample.failure_reasons)
    }

    if ($lockExitCode -ne 0 -or $sample.status -ne "PASS") {
        $runFailed = $true
        break
    }
}

$advertisingDurations = @($results | Where-Object { $null -ne $_.advertising_command_accepted_ms } | ForEach-Object { [int]$_.advertising_command_accepted_ms })
$encryptionDurations = @($results | Where-Object { $null -ne $_.connection_to_encryption_ms } | ForEach-Object { [int]$_.connection_to_encryption_ms })
$typeReadyDurations = @($results | Where-Object { $null -ne $_.fresh_pairing_to_type_ready_ms } | ForEach-Object { [int]$_.fresh_pairing_to_type_ready_ms })
$summary = [ordered]@{
    status = if (-not $runFailed -and $results.Count -eq $Iterations) { "PASS" } else { "FAIL" }
    measurement_scope = "randomized-phase machine gate: generated EC11 double-click recovery with independent advertising, encryption, and TYPE:READY stages"
    physical_gpio_measurement = $false
    trigger_kind = "firmware_generated_ec11_double_after_debounce"
    iterations_requested = $Iterations
    iterations_completed = $results.Count
    max_double_click_to_advertising_accepted_ms = $MaxDoubleClickToAdvertisingAcceptedMs
    max_connection_to_encryption_ms = $MaxConnectionToEncryptionMs
    max_fresh_pairing_to_type_ready_ms = $MaxFreshPairingToTypeReadyMs
    observed_max_advertising_command_accepted_ms = if ($advertisingDurations.Count -gt 0) { ($advertisingDurations | Measure-Object -Maximum).Maximum } else { $null }
    observed_max_connection_to_encryption_ms = if ($encryptionDurations.Count -gt 0) { ($encryptionDurations | Measure-Object -Maximum).Maximum } else { $null }
    observed_max_fresh_pairing_to_type_ready_ms = if ($typeReadyDurations.Count -gt 0) { ($typeReadyDurations | Measure-Object -Maximum).Maximum } else { $null }
    type_controlled_samples_require_pre_reset_notice = $true
    samples = @($results)
}
Write-RunSummary $summary
if ($summary.status -ne "PASS") {
    throw "Randomized EC11 Type recovery speed gate failed. See $OutputJson"
}
