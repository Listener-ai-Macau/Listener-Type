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
    [ValidateRange(1, 60000)]
    [int]$MaxTriggerToTypeReadyMs = 10000,
    [ValidateRange(1, 60000)]
    [int]$MaxTypeRecoveryAckMs = 80,
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
$results = @()
$runFailed = $false
for ($iteration = 1; $iteration -le $Iterations; $iteration += 1) {
    $phaseDelayMs = Get-Random -Minimum $MinPhaseDelayMs -Maximum ($MaxPhaseDelayMs + 1)
    Start-Sleep -Milliseconds $phaseDelayMs

    $samplePath = Join-Path (Split-Path -Parent $OutputJson) ("ec11-recovery-sample-{0:D2}.json" -f $iteration)
    $mutex = [System.Threading.Mutex]::new($false, "Global\Listener_$Port")
    $lockAcquired = $false
    try {
        $lockAcquired = $mutex.WaitOne([TimeSpan]::FromSeconds(120))
        if (-not $lockAcquired) {
            throw "Timed out waiting for Global\Listener_$Port"
        }
        $lockOutput = & pwsh -NoProfile -File $singleSampleScript -Port $Port -CaptureSeconds $CaptureSeconds -MaxDoubleClickToAdvertisingAcceptedMs $MaxDoubleClickToAdvertisingAcceptedMs -MaxConnectionToEncryptionMs $MaxConnectionToEncryptionMs -MaxFreshPairingToTypeReadyMs $MaxFreshPairingToTypeReadyMs -MaxTriggerToTypeReadyMs $MaxTriggerToTypeReadyMs -MaxTypeRecoveryAckMs $MaxTypeRecoveryAckMs -OutputJson $samplePath 2>&1
        $lockExitCode = $LASTEXITCODE
    } finally {
        if ($lockAcquired) {
            $mutex.ReleaseMutex() | Out-Null
        }
        $mutex.Dispose()
    }

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
        trigger_to_type_ready_ms = $sample.trigger_to_type_ready_ms
        trigger_to_type_ready_ms_informational = $sample.trigger_to_type_ready_ms_informational
        firmware_pre_reset_notice_sent = $sample.firmware_pre_reset_notice_sent
        firmware_pre_reset_ack_received = $sample.firmware_pre_reset_ack_received
        type_pre_reset_notice_observed = $sample.type_pre_reset_notice_observed
        type_pre_reset_acknowledgement_queued = $sample.type_pre_reset_acknowledgement_queued
        type_recovery_notice_to_ack_ms = $sample.type_recovery_notice_to_ack_ms
        pre_authorization_ack_before_double_click = $sample.pre_authorization_ack_before_double_click
        pre_authorization_consumed_before_pairing_reset = $sample.pre_authorization_consumed_before_pairing_reset
        failure_reasons = @($sample.failure_reasons)
    }

    if ($lockExitCode -ne 0 -or $sample.status -ne "PASS" -or
        $sample.pre_authorization_ack_before_double_click -ne $true -or
        $sample.pre_authorization_consumed_before_pairing_reset -ne $true) {
        $runFailed = $true
        break
    }
}

$advertisingDurations = @($results | Where-Object { $null -ne $_.advertising_command_accepted_ms } | ForEach-Object { [int]$_.advertising_command_accepted_ms })
$encryptionDurations = @($results | Where-Object { $null -ne $_.connection_to_encryption_ms } | ForEach-Object { [int]$_.connection_to_encryption_ms })
$typeReadyDurations = @($results | Where-Object { $null -ne $_.fresh_pairing_to_type_ready_ms } | ForEach-Object { [int]$_.fresh_pairing_to_type_ready_ms })
$totalDurations = @($results | Where-Object { $null -ne $_.trigger_to_type_ready_ms } | ForEach-Object { [int]$_.trigger_to_type_ready_ms })
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
    max_trigger_to_type_ready_ms = $MaxTriggerToTypeReadyMs
    max_type_recovery_ack_ms = $MaxTypeRecoveryAckMs
    observed_max_advertising_command_accepted_ms = if ($advertisingDurations.Count -gt 0) { ($advertisingDurations | Measure-Object -Maximum).Maximum } else { $null }
    observed_max_connection_to_encryption_ms = if ($encryptionDurations.Count -gt 0) { ($encryptionDurations | Measure-Object -Maximum).Maximum } else { $null }
    observed_max_fresh_pairing_to_type_ready_ms = if ($typeReadyDurations.Count -gt 0) { ($typeReadyDurations | Measure-Object -Maximum).Maximum } else { $null }
    observed_max_trigger_to_type_ready_ms = if ($totalDurations.Count -gt 0) { ($totalDurations | Measure-Object -Maximum).Maximum } else { $null }
    type_controlled_samples_require_pre_reset_notice = $true
    type_controlled_samples_require_pre_authorization_before_double_click =
        $results.Count -eq $Iterations -and
        @($results | Where-Object { $_.pre_authorization_ack_before_double_click -ne $true }).Count -eq 0
    type_controlled_samples_require_pre_authorization_consumed_before_pairing_reset =
        $results.Count -eq $Iterations -and
        @($results | Where-Object { $_.pre_authorization_consumed_before_pairing_reset -ne $true }).Count -eq 0
    samples = @($results)
}
Write-RunSummary $summary
if ($summary.status -ne "PASS") {
    throw "Randomized EC11 Type recovery speed gate failed. See $OutputJson"
}
