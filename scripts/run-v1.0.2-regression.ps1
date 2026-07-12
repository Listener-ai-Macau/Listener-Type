param(
  [string]$ReleaseExePath = "",
  [string]$OutputDir = "",
  [switch]$SkipBuild,
  [switch]$SkipPackage,
  [switch]$SkipHardware,
  [switch]$SkipNpmCi,
  [switch]$OnlyHardwareProbe
)

$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
$tauriRoot = Join-Path $repoRoot "src-tauri"
$installedTypeExePath = "C:\Program Files\Listener Type\listener-type.exe"
if ([string]::IsNullOrWhiteSpace($ReleaseExePath)) {
  $ReleaseExePath = $installedTypeExePath
}
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
  $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
  $OutputDir = Join-Path $repoRoot ".artifacts\v1.0.2-regression\$stamp"
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

$results = [System.Collections.Generic.List[object]]::new()

if ($OnlyHardwareProbe.IsPresent -and $SkipHardware.IsPresent) {
  throw "-OnlyHardwareProbe cannot be combined with -SkipHardware"
}

function ConvertTo-SafeName {
  param([Parameter(Mandatory = $true)][string]$Name)
  return ($Name -replace '[^A-Za-z0-9_.-]+', '_').Trim('_')
}

function Get-RequiredPreproductionStepIds {
  $manifestPath = Join-Path $PSScriptRoot "listener-preproduction-scenarios.json"
  if (-not (Test-Path -LiteralPath $manifestPath)) {
    throw "Canonical preproduction scenario manifest is missing: $manifestPath"
  }
  $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
  $ids = @($manifest.scenarios | ForEach-Object { [string]$_.id })
  if ($ids.Count -eq 0) {
    throw "Canonical preproduction scenario manifest has no scenarios: $manifestPath"
  }
  return $ids
}

function Invoke-External {
  param(
    [Parameter(Mandatory = $true)][string]$File,
    [string[]]$ProcessArgs = @(),
    [string]$WorkingDirectory = ""
  )

  if (-not [string]::IsNullOrWhiteSpace($WorkingDirectory)) {
    Push-Location $WorkingDirectory
  }
  try {
    Write-Host ("> {0} {1}" -f $File, ($ProcessArgs -join " "))
    & $File @ProcessArgs
    $exit = $LASTEXITCODE
    if ($null -ne $exit -and $exit -ne 0) {
      throw "$File exited with code $exit"
    }
  } finally {
    if (-not [string]::IsNullOrWhiteSpace($WorkingDirectory)) {
      Pop-Location
    }
  }
}

function Join-ProcessArguments {
  param([string[]]$ProcessArgs = @())

  $quoted = foreach ($arg in $ProcessArgs) {
    if ($arg -match '[\s"]') {
      '"' + ($arg -replace '"', '\"') + '"'
    } else {
      $arg
    }
  }
  return ($quoted -join " ")
}

function Read-TextFileFromOffset {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [long]$Offset = 0
  )

  if (-not (Test-Path -LiteralPath $Path)) {
    return ""
  }

  $stream = [System.IO.File]::Open(
    $Path,
    [System.IO.FileMode]::Open,
    [System.IO.FileAccess]::Read,
    [System.IO.FileShare]::ReadWrite
  )
  try {
    if ($Offset -gt $stream.Length) {
      $Offset = 0
    }
    $null = $stream.Seek($Offset, [System.IO.SeekOrigin]::Begin)
    $reader = [System.IO.StreamReader]::new(
      $stream,
      [System.Text.Encoding]::UTF8,
      $true,
      4096,
      $true
    )
    try {
      return $reader.ReadToEnd()
    } finally {
      $reader.Dispose()
    }
  } finally {
    $stream.Dispose()
  }
}

function Invoke-ListenerTypeGuiLogProbe {
  param(
    [Parameter(Mandatory = $true)][string]$File,
    [string[]]$CliArgs = @(),
    [string]$WorkingDirectory = "",
    [Parameter(Mandatory = $true)][string]$ExpectedPattern,
    [Parameter(Mandatory = $true)][string]$FailurePattern,
    [Parameter(Mandatory = $true)][string]$LogStem,
    [int]$TimeoutSeconds = 45
  )

  $appLog = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
  $beforeLength = 0
  if (Test-Path -LiteralPath $appLog) {
    $beforeLength = (Get-Item -LiteralPath $appLog).Length
  }

  $argLine = Join-ProcessArguments $CliArgs
  Write-Host ("> {0} {1}" -f $File, $argLine)
  $startArgs = @{
    FilePath = $File
    ArgumentList = $argLine
    PassThru = $true
    WindowStyle = "Hidden"
  }
  if (-not [string]::IsNullOrWhiteSpace($WorkingDirectory)) {
    $startArgs.WorkingDirectory = $WorkingDirectory
  }
  $process = Start-Process @startArgs

  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  $delta = ""
  do {
    Start-Sleep -Milliseconds 500
    $delta = Read-TextFileFromOffset -Path $appLog -Offset $beforeLength
    if ($delta -match $ExpectedPattern) {
      $deltaPath = Join-Path $OutputDir "$LogStem.app-log.txt"
      $delta | Set-Content -LiteralPath $deltaPath -Encoding utf8
      Write-Host "[probe] PASS marker found in Listener Type app log: $deltaPath"
      return
    }
    if ($delta -match $FailurePattern) {
      $deltaPath = Join-Path $OutputDir "$LogStem.app-log.txt"
      $delta | Set-Content -LiteralPath $deltaPath -Encoding utf8
      throw "Listener Type GUI probe failed; see app log delta: $deltaPath"
    }
  } while ((Get-Date) -lt $deadline)

  $timeoutLog = Join-Path $OutputDir "$LogStem.app-log.txt"
  $delta | Set-Content -LiteralPath $timeoutLog -Encoding utf8
  $processStatus = if ($process.HasExited) {
    "exited code=$($process.ExitCode)"
  } else {
    "still running pid=$($process.Id)"
  }
  throw "Timed out waiting for Listener Type GUI probe PASS marker after $TimeoutSeconds s ($processStatus); see app log delta: $timeoutLog"
}

function Restart-ListenerTypeForBleProbe {
  param(
    [Parameter(Mandatory = $true)][string]$File,
    [string]$LogStem = "embedded_audio_ble_live_probe_startup",
    [int]$TimeoutSeconds = 100
  )

  $resolvedFile = (Resolve-Path -LiteralPath $File).Path
  $running = @(Get-CimInstance Win32_Process -Filter "Name = 'listener-type.exe'" -ErrorAction SilentlyContinue |
    Where-Object {
      $commandLine = [string]$_.CommandLine
      $commandLine -like "*$resolvedFile*"
    })
  foreach ($process in $running) {
    Write-Host "[probe] stopping existing Listener Type before BLE live probe pid=$($process.ProcessId)"
    Stop-Process -Id $process.ProcessId -Force
  }
  Start-Sleep -Milliseconds 700

  $appLog = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
  $beforeLength = 0
  if (Test-Path -LiteralPath $appLog) {
    $beforeLength = (Get-Item -LiteralPath $appLog).Length
  }

  $previousHide = [Environment]::GetEnvironmentVariable("LISTENER_TYPE_HIDE_MAIN_ON_START", "Process")
  [Environment]::SetEnvironmentVariable("LISTENER_TYPE_HIDE_MAIN_ON_START", "1", "Process")
  try {
    $process = Start-Process `
      -FilePath $resolvedFile `
      -WorkingDirectory (Split-Path $resolvedFile -Parent) `
      -WindowStyle Hidden `
      -PassThru
  } finally {
    if ($null -eq $previousHide) {
      [Environment]::SetEnvironmentVariable("LISTENER_TYPE_HIDE_MAIN_ON_START", $null, "Process")
    } else {
      [Environment]::SetEnvironmentVariable("LISTENER_TYPE_HIDE_MAIN_ON_START", $previousHide, "Process")
    }
  }
  Write-Host "[probe] started Listener Type for BLE live probe pid=$($process.Id)"

  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  $delta = ""
  do {
    Start-Sleep -Milliseconds 1000
    $delta = Read-TextFileFromOffset -Path $appLog -Offset $beforeLength
    if ($delta -match "background listener notify ready") {
      $deltaPath = Join-Path $OutputDir "$LogStem.app-log.txt"
      $delta | Set-Content -LiteralPath $deltaPath -Encoding utf8
      Write-Host "[probe] fresh background listener ready: $deltaPath"
      return
    }
  } while ((Get-Date) -lt $deadline)

  $timeoutLog = Join-Path $OutputDir "$LogStem.app-log.txt"
  $delta | Set-Content -LiteralPath $timeoutLog -Encoding utf8
  throw "Timed out waiting for fresh Listener Type background notify ready after restart; see app log delta: $timeoutLog"
}

function Invoke-EmbeddedAudioBleLiveProbe {
  if (-not (Test-Path -LiteralPath $ReleaseExePath)) {
    throw "Installed Listener Type exe not found: $ReleaseExePath. Run the MSI package gate with -InstallMsi before hardware probes."
  }
  Restart-ListenerTypeForBleProbe `
    -File $ReleaseExePath `
    -LogStem "embedded_audio_ble_live_probe_startup" `
    -TimeoutSeconds 100
  Invoke-ListenerTypeGuiLogProbe `
    -File $ReleaseExePath `
    -CliArgs @("--probe-embedded-audio-ble-subscription", "12000") `
    -WorkingDirectory (Split-Path (Resolve-Path -LiteralPath $ReleaseExePath).Path -Parent) `
    -ExpectedPattern "(\[cli\] probe-embedded-audio-ble-subscription PASS|embedded_ble_probe_result=PASS)" `
    -FailurePattern "(\[cli\] probe-embedded-audio-ble-subscription failed|embedded_ble_probe_result=FAIL)" `
    -LogStem "embedded_audio_ble_live_probe" `
    -TimeoutSeconds 45
  Write-Host "[probe] BLE status characteristic reads are not used here while the tray app owns the background notify session."
}

function Invoke-Gate {
  param(
    [Parameter(Mandatory = $true)][string]$Name,
    [Parameter(Mandatory = $true)][scriptblock]$Block
  )

  $safe = ConvertTo-SafeName $Name
  $logPath = Join-Path $OutputDir "$safe.log"
  Write-Host ""
  Write-Host "=== $Name ==="
  $started = Get-Date
  Start-Transcript -Path $logPath -Force | Out-Null
  try {
    & $Block
    $elapsed = [int]((Get-Date) - $started).TotalSeconds
    $results.Add([pscustomobject]@{
      name = $Name
      status = "PASS"
      seconds = $elapsed
      log = $logPath
    }) | Out-Null
    Write-Host "[PASS] $Name ($elapsed s)"
  } catch {
    $elapsed = [int]((Get-Date) - $started).TotalSeconds
    $results.Add([pscustomobject]@{
      name = $Name
      status = "FAIL"
      seconds = $elapsed
      log = $logPath
      error = $_.Exception.Message
    }) | Out-Null
    Write-Host "[FAIL] $Name ($elapsed s): $($_.Exception.Message)"
    throw
  } finally {
    Stop-Transcript | Out-Null
  }
}

try {
  if ($OnlyHardwareProbe.IsPresent) {
    Invoke-Gate "embedded audio BLE live probe" {
      Invoke-EmbeddedAudioBleLiveProbe
    }
  } else {
    Invoke-Gate "npm dependencies" {
      if ((Test-Path -LiteralPath (Join-Path $repoRoot "node_modules")) -or $SkipNpmCi.IsPresent) {
        Write-Host "[skip] node_modules present or -SkipNpmCi set"
        return
      }
      Invoke-External "npm.cmd" @("ci") $repoRoot
    }

    Invoke-Gate "frontend unit and policy tests" {
      Invoke-External "npm.cmd" @("run", "test") $repoRoot
      Invoke-External "npm.cmd" @("run", "verify") $repoRoot
      Invoke-External "npm.cmd" @("run", "release:check") $repoRoot
      Invoke-External "npm.cmd" @("run", "check:embedded-ble-processing-led") $repoRoot
      Invoke-External "npm.cmd" @("run", "check:preproduction-bench-contract") $repoRoot
    }

    Invoke-Gate "rust format and BLE/OTA tests" {
      Invoke-External "cargo" @("fmt", "--", "--check") $tauriRoot
      Invoke-External "cargo" @("test", "embedded_ble_", "--lib") $tauriRoot
      Invoke-External "cargo" @("test", "device_ble_name", "--lib") $tauriRoot
      Invoke-External "cargo" @("test", "firmware_ota", "--lib") $tauriRoot
    }

    Invoke-Gate "preproduction human review script smoke" {
      $humanGate = Join-Path $PSScriptRoot "windows-listener-preproduction-human-review.ps1"
      $requiredStepIds = @(Get-RequiredPreproductionStepIds)
      if (-not (Test-Path -LiteralPath $humanGate)) {
        throw "Final preproduction human review script not found: $humanGate"
      }

      $null = [scriptblock]::Create((Get-Content -LiteralPath $humanGate -Raw))
      & pwsh -NoProfile -File $humanGate -StatusSelfTest -NoSound -OutputDir (Join-Path $OutputDir "preproduction-human-review-status-selftest")
      $exit = $LASTEXITCODE
      if ($null -ne $exit -and $exit -ne 0) {
        throw "Preproduction human review focused-status self-test exited with code $exit"
      }
      $steps = @(& pwsh -NoProfile -File $humanGate -ListSteps)
      $exit = $LASTEXITCODE
      if ($null -ne $exit -and $exit -ne 0) {
        throw "Preproduction human review -ListSteps exited with code $exit"
      }
      if ($steps.Count -lt $requiredStepIds.Count) {
        throw "Preproduction human review must expose all final user gates; expected at least $($requiredStepIds.Count) steps, got $($steps.Count)"
      }

      $stepText = $steps -join "`n"
      foreach ($stepId in $requiredStepIds) {
        if ($stepText -notmatch [regex]::Escape($stepId)) {
          throw "Preproduction human review missing required step '$stepId'"
        }
      }

      $dryRunDir = Join-Path $OutputDir "preproduction-human-review-dryrun"
      & pwsh -NoProfile -File $humanGate -NoPrompt -NoSound -OutputDir $dryRunDir
      $exit = $LASTEXITCODE
      if ($exit -ne 2) {
        throw "Preproduction human review dry-run should return 2/HUMAN_REVIEW_INCOMPLETE, got $exit"
      }
      $dryRunSummary = Join-Path $dryRunDir "preproduction-human-review-summary.json"
      if (-not (Test-Path -LiteralPath $dryRunSummary)) {
        throw "Preproduction human review dry-run summary not written: $dryRunSummary"
      }
      $dryRun = Get-Content -LiteralPath $dryRunSummary -Raw | ConvertFrom-Json
      if ($dryRun.status -ne "HUMAN_REVIEW_INCOMPLETE") {
        throw "Preproduction human review dry-run status should be HUMAN_REVIEW_INCOMPLETE, got $($dryRun.status)"
      }
      $dryRunRecords = @($dryRun.records)
      if ($dryRunRecords.Count -ne $requiredStepIds.Count) {
        throw "Preproduction human review dry-run must record all $($requiredStepIds.Count) final gates, got $($dryRunRecords.Count)"
      }
      foreach ($stepId in $requiredStepIds) {
        $record = @($dryRunRecords | Where-Object { $_.id -eq $stepId })
        if ($record.Count -ne 1) {
          throw "Preproduction human review dry-run did not record exactly one '$stepId' step"
        }
        if ($record[0].result -ne "SKIP") {
          throw "Preproduction human review dry-run step '$stepId' should be SKIP, got $($record[0].result)"
        }
      }
      if (-not (Test-Path -LiteralPath $dryRun.session_jsonl)) {
        throw "Preproduction human review dry-run session not written: $($dryRun.session_jsonl)"
      }
    }

    Invoke-Gate "preproduction bench review contract smoke" {
      $benchGate = Join-Path $PSScriptRoot "windows-listener-preproduction-bench-review.ps1"
      $requiredStepIds = @(Get-RequiredPreproductionStepIds)
      if (-not (Test-Path -LiteralPath $benchGate)) {
        throw "Final preproduction bench review script not found: $benchGate"
      }

      $steps = @(& pwsh -NoProfile -File $benchGate -ListSteps)
      $exit = $LASTEXITCODE
      if ($null -ne $exit -and $exit -ne 0) {
        throw "Preproduction bench review -ListSteps exited with code $exit"
      }
      if ($steps.Count -lt $requiredStepIds.Count) {
        throw "Preproduction bench review must expose all final user gates; expected at least $($requiredStepIds.Count) steps, got $($steps.Count)"
      }
      $stepText = $steps -join "`n"
      foreach ($stepId in $requiredStepIds) {
        if ($stepText -notmatch [regex]::Escape($stepId)) {
          throw "Preproduction bench review missing required step '$stepId'"
        }
      }

      $templateDir = Join-Path $OutputDir "preproduction-bench-review-template"
      & pwsh -NoProfile -File $benchGate -WriteTemplate -OutputDir $templateDir
      $exit = $LASTEXITCODE
      if ($exit -ne 0) {
        throw "Preproduction bench review template generation failed with code $exit"
      }

      $dryRunDir = Join-Path $OutputDir "preproduction-bench-review-no-go"
      & pwsh -NoProfile -File $benchGate -OutputDir $dryRunDir
      $exit = $LASTEXITCODE
      if ($exit -ne 2) {
        throw "Preproduction bench review without capability manifest should return 2/BENCH_REVIEW_NO_GO, got $exit"
      }
      $summary = Join-Path $dryRunDir "preproduction-bench-review-summary.json"
      if (-not (Test-Path -LiteralPath $summary)) {
        throw "Preproduction bench review summary not written: $summary"
      }
      $bench = Get-Content -LiteralPath $summary -Raw | ConvertFrom-Json
      if ($bench.status -ne "BENCH_REVIEW_NO_GO") {
        throw "Preproduction bench review dry-run should be BENCH_REVIEW_NO_GO, got $($bench.status)"
      }
      $records = @($bench.records)
      if ($records.Count -lt $requiredStepIds.Count) {
        throw "Preproduction bench review dry-run must record all release scenarios, got $($records.Count)"
      }
      if (@($records | Where-Object { $_.missing_capabilities.Count -gt 0 -or $_.missing_evidence.Count -gt 0 }).Count -eq 0) {
        throw "Preproduction bench review dry-run must report missing capabilities/evidence when no bench manifest is provided"
      }
    }

    Invoke-Gate "preproduction BLE active automation dry-run smoke" {
      $activeBleGate = Join-Path $PSScriptRoot "windows-listener-preproduction-ble-active-bench.ps1"
      if (-not (Test-Path -LiteralPath $activeBleGate)) {
        throw "Active Windows BLE bench script not found: $activeBleGate"
      }

      $null = [scriptblock]::Create((Get-Content -LiteralPath $activeBleGate -Raw))
      $dryRunDir = Join-Path $OutputDir "preproduction-ble-active-dryrun"
      & pwsh -NoProfile -File $activeBleGate -OutputDir $dryRunDir -Mode DryRun
      $exit = $LASTEXITCODE
      if ($exit -ne 0) {
        throw "Active Windows BLE bench dry-run failed with code $exit"
      }

      $summaryPath = Join-Path $dryRunDir "preproduction-ble-active-summary.json"
      $beforeState = Join-Path $dryRunDir "before-windows-ble-state.json"
      $afterState = Join-Path $dryRunDir "after-windows-ble-state.json"
      $plan = Join-Path $dryRunDir "dry-run-plan.json"
      foreach ($path in @($summaryPath, $beforeState, $afterState, $plan)) {
        if (-not (Test-Path -LiteralPath $path)) {
          throw "Active Windows BLE bench dry-run did not write required artifact: $path"
        }
      }

      $summary = Get-Content -LiteralPath $summaryPath -Raw | ConvertFrom-Json
      if ($summary.status -ne "WINDOWS_BLE_AUTOMATION_DRY_RUN") {
        throw "Active Windows BLE bench dry-run should report WINDOWS_BLE_AUTOMATION_DRY_RUN, got $($summary.status)"
      }
      if ($summary.execute -ne $false) {
        throw "Active Windows BLE bench dry-run must not execute pairing mutations"
      }
      if ($summary.capabilities.windows_ble_automation -ne $false) {
        throw "Active Windows BLE bench dry-run must not claim windows_ble_automation capability"
      }

      $dryRunPlan = Get-Content -LiteralPath $plan -Raw | ConvertFrom-Json
      $planText = $dryRunPlan.planned_actions | ConvertTo-Json -Depth 5
      foreach ($token in @("--cleanup-embedded-ble-pairing", "--prompt-embedded-ble-pairing-only", "--read-embedded-audio-ble-status")) {
        if ($planText -notmatch [regex]::Escape($token)) {
          throw "Active Windows BLE bench dry-run plan is missing $token"
        }
      }
    }

    Invoke-Gate "preproduction bench evidence collect smoke" {
      $benchCollect = Join-Path $PSScriptRoot "windows-listener-preproduction-bench-collect.ps1"
      if (-not (Test-Path -LiteralPath $benchCollect)) {
        throw "Preproduction bench evidence collector not found: $benchCollect"
      }

      $activeBleSummary = Join-Path $OutputDir "preproduction-ble-active-dryrun\preproduction-ble-active-summary.json"
      $collectDir = Join-Path $OutputDir "preproduction-bench-collect-smoke"
      & pwsh -NoProfile -File $benchCollect -OutputDir $collectDir -ActiveBleSummaryPath $activeBleSummary -LiveBleTimeoutMs 20000 -SkipLiveBle -SkipScreenshot -SkipNotificationScan
      $exit = $LASTEXITCODE
      if ($exit -ne 0) {
        throw "Preproduction bench evidence collector smoke failed with code $exit"
      }
      $collectSummary = Join-Path $collectDir "preproduction-bench-collect-summary.json"
      $manifest = Join-Path $collectDir "preproduction-bench-capabilities.json"
      $reviewSummary = Join-Path $collectDir "preproduction-bench-review-summary.json"
      foreach ($path in @($collectSummary, $manifest, $reviewSummary)) {
        if (-not (Test-Path -LiteralPath $path)) {
          throw "Preproduction bench evidence collector did not write required artifact: $path"
        }
      }

      $collect = Get-Content -LiteralPath $collectSummary -Raw | ConvertFrom-Json
      if ($collect.status -ne "BENCH_COLLECT_COMPLETE") {
        throw "Bench collector status should be BENCH_COLLECT_COMPLETE, got $($collect.status)"
      }
      if ($collect.review_status -ne "BENCH_REVIEW_NO_GO") {
        throw "Offline bench collector smoke should still leave bench review NO_GO, got $($collect.review_status)"
      }
      if ($collect.capabilities.windows_ble_automation -ne $false) {
        throw "Offline bench collector must not accept a dry-run active BLE summary as windows_ble_automation PASS"
      }
      $collectNoteText = @($collect.notes) -join "`n"
      if ($collectNoteText -notmatch "Active BLE summary was provided but not PASS") {
        throw "Bench collector should record why dry-run active BLE evidence did not satisfy windows_ble_automation"
      }
      $bench = Get-Content -LiteralPath $reviewSummary -Raw | ConvertFrom-Json
      $releaseRecord = @($bench.records | Where-Object { $_.id -eq "release-package-final-check" })
      if ($releaseRecord.Count -ne 1) {
        throw "Bench collector review should include exactly one release-package-final-check record"
      }
      $releaseArtifactsPath = Join-Path $collectDir "release-artifacts.json"
      if (-not (Test-Path -LiteralPath $releaseArtifactsPath)) {
        throw "Bench collector should write release-artifacts.json for the final root package hash gate"
      }
      $releaseArtifacts = Get-Content -LiteralPath $releaseArtifactsPath -Raw | ConvertFrom-Json
      if ($releaseArtifacts.PSObject.Properties["type_source_hash_matches_root"] -eq $null -or
          $releaseArtifacts.PSObject.Properties["firmware_source_hash_matches_root"] -eq $null) {
        throw "release-artifacts.json must include source-to-root hash comparison fields"
      }
      if ($releaseArtifacts.all_latest_sources_staged) {
        if ($releaseRecord[0].status -ne "PASS") {
          throw "Bench collector should mark release-package-final-check PASS when root packages match latest source hashes, got $($releaseRecord[0].status)"
        }
      } elseif ($releaseRecord[0].status -ne "NO_GO") {
        throw "Bench collector should keep release-package-final-check NO_GO until root packages match latest source hashes, got $($releaseRecord[0].status)"
      }
      $remainingNoGo = @($bench.records | Where-Object { $_.status -eq "NO_GO" })
      if ($remainingNoGo.Count -eq 0) {
        throw "Offline bench collector must not make the full physical bench pass without hardware capabilities"
      }
    }

    if (-not $SkipPackage.IsPresent) {
      Invoke-Gate "MSVC MSI package without portable zip (single release build)" {
        Invoke-External "pwsh" @(
          "-NoProfile",
          "-File", (Join-Path $PSScriptRoot "windows-package-msvc.ps1"),
          "-SkipRustInstall",
          "-SkipNpmCi",
          "-IncrementalReleaseBuild",
          "-CleanArtifacts",
          "-InstallMsi",
          "-LaunchInstalledApp"
        ) $repoRoot
      }
    } elseif (-not $SkipBuild.IsPresent) {
      Invoke-Gate "MSVC release check without exe overwrite" {
        Invoke-External "cargo" @("check", "--release", "--target", "x86_64-pc-windows-msvc", "-j", "1") $tauriRoot
      }
    }

    if (-not $SkipHardware.IsPresent) {
      Invoke-Gate "embedded audio BLE live probe" {
        Invoke-EmbeddedAudioBleLiveProbe
      }
    } else {
      $results.Add([pscustomobject]@{
        name = "embedded audio BLE live probe"
        status = "SKIP"
        seconds = 0
        log = ""
        error = "Skipped by -SkipHardware"
      }) | Out-Null
    }
  }
} finally {
  $summaryPath = Join-Path $OutputDir "summary.json"
  $results | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $summaryPath -Encoding utf8
  Write-Host ""
  Write-Host "Regression summary: $summaryPath"
  $results | Format-Table -AutoSize
}

$failed = @($results | Where-Object { $_.status -eq "FAIL" })
if ($failed.Count -gt 0) {
  exit 1
}

Write-Host "PASS: Listener Type v1.0.2 regression gate completed."
