<#
.SYNOPSIS
  Listener 1.0.4 one-click anti-regression entry (Type + Firmware + release identity).

.DESCRIPTION
  Reuses existing verify_* / release-check / OTA / long-session tools.
  Default mode is offline/static + unit contracts (no hardware, no install mutation).
  Use -IncludeHardware for installed BLE/OTA/long-session gates when a device is present.
  Writes machine-readable summary.json under .artifacts/listener-1.0.4-regression/<stamp>/.
#>
param(
  [string]$OutputDir = "",
  [switch]$IncludeHardware,
  [switch]$SkipTypeUnit,
  [switch]$SkipFirmwareStatic,
  [switch]$SkipNpm,
  [switch]$AllowDirtyWorktree,
  [switch]$AllowDifferentHead,
  [string]$ComPort = "",
  [int]$LongSessionMinutes = 20
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$scriptDir = Split-Path -Parent $PSCommandPath
$typeRoot = (Resolve-Path -LiteralPath (Join-Path $scriptDir "..")).Path
$listenerRoot = (Resolve-Path -LiteralPath (Join-Path $typeRoot "..")).Path
$firmwareRoot = Join-Path $listenerRoot "Listener-Firmware"
$mapPath = Join-Path $scriptDir "listener-1.0.4-requirement-test-map.json"

if ([string]::IsNullOrWhiteSpace($OutputDir)) {
  $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
  $OutputDir = Join-Path $typeRoot ".artifacts\listener-1.0.4-regression\$stamp"
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

$results = [System.Collections.Generic.List[object]]::new()
$startedAll = Get-Date

function ConvertTo-SafeName {
  param([string]$Name)
  return ($Name -replace '[^A-Za-z0-9_.-]+', '_').Trim('_')
}

function Invoke-Gate {
  param(
    [Parameter(Mandatory = $true)][string]$Name,
    [Parameter(Mandatory = $true)][string]$MapId,
    [Parameter(Mandatory = $true)][scriptblock]$Block,
    [string]$Level = "L0",
    [switch]$OptionalHardware
  )

  $safe = ConvertTo-SafeName $Name
  $logPath = Join-Path $OutputDir "$safe.log"
  Write-Host ""
  Write-Host "=== $Name ==="
  $started = Get-Date
  $status = "PASS"
  $errorText = $null
  try {
    $global:LASTEXITCODE = 0
    & $Block *>&1 | Tee-Object -FilePath $logPath | Out-Host
    $exitCode = 0
    if (Test-Path variable:global:LASTEXITCODE) {
      $exitCode = [int]$global:LASTEXITCODE
    }
    if ($exitCode -ne 0) {
      throw "command exited with code $exitCode"
    }
  } catch {
    if ($OptionalHardware.IsPresent -and -not $IncludeHardware.IsPresent) {
      $status = "SKIPPED_HARDWARE"
      $errorText = $_.Exception.Message
    } else {
      $status = "FAIL"
      $errorText = $_.Exception.Message
    }
  }
  $elapsed = [int]((Get-Date) - $started).TotalSeconds
  $entry = [ordered]@{
    name     = $Name
    map_id   = $MapId
    level    = $Level
    status   = $status
    seconds  = $elapsed
    log      = $logPath
    error    = $errorText
  }
  $results.Add([pscustomobject]$entry) | Out-Null
  Write-Host ("[{0}] {1} ({2} s)" -f $status, $Name, $elapsed)
  if ($status -eq "FAIL") {
    throw "Gate failed: $Name — $errorText"
  }
}

function Invoke-PwshFile {
  param(
    [Parameter(Mandatory = $true)][string]$File,
    [string[]]$Args = @(),
    [string]$WorkingDirectory = ""
  )
  $argList = @("-NoProfile", "-File", $File) + $Args
  Write-Host ("> pwsh {0}" -f ($argList -join " "))
  if ($WorkingDirectory) {
    Push-Location $WorkingDirectory
  }
  try {
    $global:LASTEXITCODE = 0
    & pwsh @argList
    if ((Test-Path variable:global:LASTEXITCODE) -and [int]$global:LASTEXITCODE -ne 0) {
      throw "pwsh $File exited $global:LASTEXITCODE"
    }
  } finally {
    if ($WorkingDirectory) { Pop-Location }
  }
}

function Invoke-Node {
  param(
    [Parameter(Mandatory = $true)][string]$File,
    [string[]]$Args = @(),
    [string]$WorkingDirectory = $typeRoot
  )
  Write-Host ("> node $File $($Args -join ' ')")
  Push-Location $WorkingDirectory
  try {
    $global:LASTEXITCODE = 0
    & node $File @Args
    if ((Test-Path variable:global:LASTEXITCODE) -and [int]$global:LASTEXITCODE -ne 0) {
      throw "node $File exited $global:LASTEXITCODE"
    }
  } finally {
    Pop-Location
  }
}

function Invoke-Python {
  param(
    [Parameter(Mandatory = $true)][string]$File,
    [string[]]$Args = @(),
    [string]$WorkingDirectory = ""
  )
  Write-Host ("> python $File $($Args -join ' ')")
  if ($WorkingDirectory) { Push-Location $WorkingDirectory }
  try {
    $global:LASTEXITCODE = 0
    & python $File @Args
    if ((Test-Path variable:global:LASTEXITCODE) -and [int]$global:LASTEXITCODE -ne 0) {
      throw "python $File exited $global:LASTEXITCODE"
    }
  } finally {
    if ($WorkingDirectory) { Pop-Location }
  }
}

# Snapshot production profiles before machine gates (for final clean-env proof).
$prodApp = Join-Path $env:APPDATA "Listener Type"
$prodLocal = Join-Path $env:LOCALAPPDATA "Listener Type"
function Get-ProfileFingerprint {
  param(
    [string]$Root,
    [string[]]$ExcludePathSubstrings = @()
  )
  if (-not (Test-Path -LiteralPath $Root)) { return "missing" }
  $files = @(Get-ChildItem -LiteralPath $Root -Recurse -File -Force -ErrorAction SilentlyContinue)
  if ($ExcludePathSubstrings.Count -gt 0) {
    $files = @($files | Where-Object {
        $full = $_.FullName
        -not ($ExcludePathSubstrings | Where-Object { $full -like "*$_*" })
      })
  }
  $hash = [System.Security.Cryptography.SHA256]::Create()
  $joined = ($files | Sort-Object FullName | ForEach-Object {
      "{0}|{1}|{2}" -f $_.FullName.Substring($Root.Length), $_.Length, $_.LastWriteTimeUtc.Ticks
    }) -join "`n"
  $bytes = [System.Text.Encoding]::UTF8.GetBytes($joined)
  return ([System.BitConverter]::ToString($hash.ComputeHash($bytes))).Replace("-", "")
}
# Ignore live app log churn under LOCALAPPDATA\Logs when the product is already running.
$localExclude = @("\Logs\", "/Logs/", "\wake-diag-live\", "/wake-diag-live/")
$beforeAppFp = Get-ProfileFingerprint $prodApp
$beforeLocalFp = Get-ProfileFingerprint $prodLocal $localExclude

try {
  Invoke-Gate -Name "requirement map present" -MapId "REG-MAP" -Level "L0" -Block {
    if (-not (Test-Path -LiteralPath $mapPath)) {
      throw "missing requirement map: $mapPath"
    }
    $map = Get-Content -Raw -LiteralPath $mapPath | ConvertFrom-Json
    if (-not $map.items -or $map.items.Count -lt 8) {
      throw "requirement map has too few items"
    }
    Copy-Item -LiteralPath $mapPath -Destination (Join-Path $OutputDir "requirement-test-map.json") -Force
    Write-Host ("mapped items: {0}" -f $map.items.Count)
  }

  Invoke-Gate -Name "frozen identity" -MapId "REG-ID-001" -Level "L0" -Block {
    $idJson = Join-Path $OutputDir "identity.json"
    $args = @(
      "-ListenerRoot", $listenerRoot,
      "-TypeRepo", $typeRoot,
      "-FirmwareRepo", $firmwareRoot,
      "-OutputJson", $idJson
    )
    if ($AllowDirtyWorktree.IsPresent) { $args += "-AllowDirtyWorktree" }
    if ($AllowDifferentHead.IsPresent) { $args += "-AllowDifferentHead" }
    Invoke-PwshFile (Join-Path $scriptDir "verify-listener-1.0.4-identity.ps1") $args
  }

  Invoke-Gate -Name "protected source contracts" -MapId "REG-WAKE-002" -Level "L0" -Block {
    $contractOut = Join-Path $OutputDir "protected-contracts.json"
    Push-Location $typeRoot
    try {
      $jsonText = & node (Join-Path $scriptDir "verify-listener-1.0.4-protected-contracts.mjs") | Out-String
      if ($LASTEXITCODE -ne 0) {
        $jsonText | Set-Content -LiteralPath $contractOut -Encoding utf8
        throw "protected contracts failed"
      }
      $jsonText | Set-Content -LiteralPath $contractOut -Encoding utf8
      Write-Host $jsonText
    } finally {
      Pop-Location
    }
  }

  Invoke-Gate -Name "clean env probe" -MapId "REG-ENV-001" -Level "L0" -Block {
    $cleanDir = Join-Path $OutputDir "clean-env"
    Invoke-PwshFile (Join-Path $scriptDir "verify-listener-1.0.4-clean-env.ps1") @(
      "-OutputDir", $cleanDir
    )
  }

  Invoke-Gate -Name "release root artifacts" -MapId "REG-ID-001" -Level "L0" -Block {
    # Pure Node check; does not require node_modules.
    Invoke-Node (Join-Path $scriptDir "check-release-root-artifacts.mjs")
  }

  Invoke-Gate -Name "firmware OTA static regression (1.0.3/1.0.4)" -MapId "REG-OTA-001" -Level "L0" -Block {
    $py = Join-Path $firmwareRoot "tools\verify_listener_103_ota_regression.py"
    if (-not (Test-Path -LiteralPath $py)) {
      throw "missing $py"
    }
    Invoke-Python $py -WorkingDirectory $firmwareRoot
  }

  Invoke-Gate -Name "firmware voice recording FSM" -MapId "REG-STAB-001" -Level "L0" -Block {
    Invoke-Python (Join-Path $firmwareRoot "tools\verify_voice_recording_control_fsm.py") -WorkingDirectory $firmwareRoot
  }

  if (-not $SkipFirmwareStatic.IsPresent) {
    Invoke-Gate -Name "firmware power manager static" -MapId "REG-WAKE-001" -Level "L0" -Block {
      Invoke-Python (Join-Path $firmwareRoot "tools\verify_power_manager_static.py") -WorkingDirectory $firmwareRoot
    }
    Invoke-Gate -Name "firmware status LED static" -MapId "REG-FEAT-001" -Level "L0" -Block {
      Invoke-Python (Join-Path $firmwareRoot "tools\verify_status_led_static.py") -WorkingDirectory $firmwareRoot
    }
    Invoke-Gate -Name "firmware audio leveling static" -MapId "REG-FEAT-001" -Level "L0" -Block {
      Invoke-Python (Join-Path $firmwareRoot "tools\verify_audio_leveling_platform_static.py") -WorkingDirectory $firmwareRoot
    }
    Invoke-Gate -Name "firmware BLE audio transport model" -MapId "REG-BLE-001" -Level "L0" -Block {
      Invoke-Python (Join-Path $firmwareRoot "tools\verify_ble_audio_transport_model.py") -WorkingDirectory $firmwareRoot
    }
  }

  # Type npm/cargo suite — reuses release-check when dependencies exist.
  if (-not $SkipTypeUnit.IsPresent) {
    $hasNodeModules = Test-Path -LiteralPath (Join-Path $typeRoot "node_modules")
    if ($hasNodeModules -and -not $SkipNpm.IsPresent) {
      Invoke-Gate -Name "Type OTA speed log contract" -MapId "REG-OTA-001" -Level "L0" -Block {
        Push-Location $typeRoot
        try {
          $global:LASTEXITCODE = 0
          & npm run check:ota-speed-log-contract
          if ((Test-Path variable:global:LASTEXITCODE) -and [int]$global:LASTEXITCODE -ne 0) { throw "npm check:ota-speed-log-contract failed" }
        } finally { Pop-Location }
      }
      Invoke-Gate -Name "Type recording latency contract" -MapId "REG-END-001" -Level "L0" -Block {
        Push-Location $typeRoot
        try {
          $global:LASTEXITCODE = 0
          & npm run check:recording-latency
          if ((Test-Path variable:global:LASTEXITCODE) -and [int]$global:LASTEXITCODE -ne 0) { throw "npm check:recording-latency failed" }
        } finally { Pop-Location }
      }
      Invoke-Gate -Name "Type frontend firmware OTA tests" -MapId "REG-OTA-001" -Level "L0" -Block {
        Push-Location $typeRoot
        try {
          $global:LASTEXITCODE = 0
          & npm run test:firmware-ota
          if ((Test-Path variable:global:LASTEXITCODE) -and [int]$global:LASTEXITCODE -ne 0) { throw "npm test:firmware-ota failed" }
        } finally { Pop-Location }
      }
    } else {
      Write-Host "Type npm suite skipped (node_modules missing or -SkipNpm). Source contracts still ran."
      $results.Add([pscustomobject]@{
          name    = "Type npm suite"
          map_id  = "REG-TYPE-NPM"
          level   = "L0"
          status  = "SKIPPED_DEPS"
          seconds = 0
          log     = $null
          error   = "node_modules missing or SkipNpm"
        }) | Out-Null
    }

    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if ($cargo) {
      Invoke-Gate -Name "Type focused dictation/endpoint unit tests" -MapId "REG-END-001" -Level "L0" -Block {
        # After cache wipe, frontendDist ../dist may be absent; create a compile-only stub.
        $distDir = Join-Path $typeRoot "dist"
        $stubCreated = $false
        if (-not (Test-Path -LiteralPath (Join-Path $distDir "index.html"))) {
          New-Item -ItemType Directory -Force -Path $distDir | Out-Null
          "<!doctype html><title>listener-type-test-stub</title>" |
            Set-Content -LiteralPath (Join-Path $distDir "index.html") -Encoding utf8
          $stubCreated = $true
          Write-Host "created compile-only dist stub for cargo tests"
        }
        $prev = [Environment]::GetEnvironmentVariable("LISTENER_TYPE_DISABLE_BACKGROUND_BLE", "Process")
        $prevData = [Environment]::GetEnvironmentVariable("LISTENER_TYPE_DATA_DIR", "Process")
        $iso = Join-Path $env:TEMP ("listener-type-reg-{0}" -f [guid]::NewGuid().ToString("N"))
        New-Item -ItemType Directory -Force -Path $iso | Out-Null
        [Environment]::SetEnvironmentVariable("LISTENER_TYPE_DISABLE_BACKGROUND_BLE", "1", "Process")
        [Environment]::SetEnvironmentVariable("LISTENER_TYPE_DATA_DIR", $iso, "Process")
        try {
          Push-Location (Join-Path $typeRoot "src-tauri")
          try {
            $global:LASTEXITCODE = 0
            # Module path is coordinator::dictation::tests (not dictation_tests).
            & cargo test --lib coordinator::dictation::tests -- --test-threads=1
            if ((Test-Path variable:global:LASTEXITCODE) -and [int]$global:LASTEXITCODE -ne 0) {
              throw "cargo coordinator::dictation::tests failed"
            }
          } finally { Pop-Location }
        } finally {
          [Environment]::SetEnvironmentVariable("LISTENER_TYPE_DISABLE_BACKGROUND_BLE", $prev, "Process")
          [Environment]::SetEnvironmentVariable("LISTENER_TYPE_DATA_DIR", $prevData, "Process")
          Remove-Item -LiteralPath $iso -Recurse -Force -ErrorAction SilentlyContinue
          if ($stubCreated) {
            # Leave dist stub in place for subsequent cargo runs; it is gitignored build output.
            Write-Host "left compile-only dist stub at $distDir (gitignored)"
          }
        }
      }
    } else {
      $results.Add([pscustomobject]@{
          name    = "Type cargo unit tests"
          map_id  = "REG-TYPE-CARGO"
          level   = "L0"
          status  = "SKIPPED_DEPS"
          seconds = 0
          log     = $null
          error   = "cargo not on PATH"
        }) | Out-Null
    }
  }

  if ($IncludeHardware.IsPresent) {
    $otaPackage = Join-Path $listenerRoot "ListenerFirmware_1.0.4_ota.zip"
    if ([string]::IsNullOrWhiteSpace($ComPort)) {
      # Prefer the non-STLink USB serial device when present (product ESP32-S3 CDC).
      $ports = @([System.IO.Ports.SerialPort]::GetPortNames())
      if ($ports -contains "COM3") { $ComPort = "COM3" }
      elseif ($ports.Count -gt 0) { $ComPort = $ports[0] }
    }

    Invoke-Gate -Name "Type warm start notify-ready" -MapId "REG-BLE-001" -Level "L2" -Block {
      $json = Join-Path $OutputDir "notify-ready.json"
      Invoke-PwshFile (Join-Path $scriptDir "check-type-startup-reconnect-speed.ps1") @(
        "-ListenerExe", "C:\Program Files\Listener Type\listener-type.exe",
        "-MaxNotifyReadyMs", "3000",
        "-ReadyTimeoutSeconds", "25",
        "-OutputJson", $json,
        "-AllowForcedTermination"
      )
      $summary = Get-Content -Raw -LiteralPath $json | ConvertFrom-Json
      if ($summary.status -ne "PASS") {
        throw ("notify-ready gate FAIL: {0}" -f (($summary.errors | ForEach-Object { $_ }) -join "; "))
      }
      if ($null -eq $summary.start_to_notify_ready_ms -or [int]$summary.start_to_notify_ready_ms -ge 3000) {
        throw ("notify-ready ms={0} not < 3000" -f $summary.start_to_notify_ready_ms)
      }
      Write-Host ("notify-ready ms={0} (limit 3000)" -f $summary.start_to_notify_ready_ms)
    }

    Invoke-Gate -Name "installed OTA triple gate" -MapId "REG-OTA-001" -Level "L3" -Block {
      $otaScript = Join-Path $scriptDir "run-installed-ota-hidden.ps1"
      if (-not (Test-Path -LiteralPath $otaScript)) { throw "missing $otaScript" }
      if (-not (Test-Path -LiteralPath $otaPackage)) { throw "missing OTA package $otaPackage" }

      $otaDir = Join-Path $OutputDir "ota-triple"
      New-Item -ItemType Directory -Force -Path $otaDir | Out-Null
      $runs = @()
      for ($i = 1; $i -le 3; $i++) {
        $outJson = Join-Path $otaDir ("ota-run-{0}.json" -f $i)
        Write-Host ("--- OTA run {0}/3 ---" -f $i)
        Invoke-PwshFile $otaScript @(
          "-Package", $otaPackage,
          "-OutputJson", $outJson,
          "-ExePath", "C:\Program Files\Listener Type\listener-type.exe"
        )
        if (-not (Test-Path -LiteralPath $outJson)) {
          throw "OTA run $i did not write $outJson"
        }
        $run = Get-Content -Raw -LiteralPath $outJson | ConvertFrom-Json
        $runs += $run
        $bps = $null
        if ($null -ne $run.PSObject.Properties["machineGate"] -and
            $null -ne $run.machineGate -and
            $null -ne $run.machineGate.PSObject.Properties["protocolTransferBytesPerSecond"]) {
          $bps = [double]$run.machineGate.protocolTransferBytesPerSecond
        }
        $recoveries = 0
        if ($null -ne $run.PSObject.Properties["result"] -and $null -ne $run.result) {
          if ($null -ne $run.result.PSObject.Properties["offsetRecoveries"]) {
            $recoveries = [int]$run.result.offsetRecoveries
          }
        }
        # Installed OTA CDP result does not always surface offsetRecoveries; scan Type log delta if needed.
        if ($recoveries -eq 0) {
          $appLog = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
          if (Test-Path -LiteralPath $appLog) {
            $tail = Get-Content -LiteralPath $appLog -Tail 200 -ErrorAction SilentlyContinue
            $recLine = $tail | Where-Object { $_ -match "offset recoveries=(?<n>\d+)" } | Select-Object -Last 1
            if ($recLine -match "offset recoveries=(?<n>\d+)") {
              $recoveries = [int]$Matches["n"]
            }
          }
        }
        $pass = ([string]$run.status -eq "PASS")
        if (-not $pass) {
          throw ("OTA run {0} reported fail: {1}" -f $i, ($run | ConvertTo-Json -Compress -Depth 6))
        }
        if ($null -eq $bps -or $bps -le (60 * 1024)) {
          throw ("OTA run {0} speed {1} B/s not strictly > 60 KiB/s" -f $i, $bps)
        }
        if ($recoveries -ne 0) {
          throw ("OTA run {0} offset recoveries={1}, expected 0" -f $i, $recoveries)
        }
        Write-Host ("OTA run {0} PASS speed={1:N1} B/s ({2:N1} KiB/s) recoveries={3}" -f $i, $bps, ($bps / 1024.0), $recoveries)
        Start-Sleep -Seconds 3
      }
      $tripleSummary = [ordered]@{
        result = "PASS"
        runs   = $runs
      }
      $tripleSummary | ConvertTo-Json -Depth 10 |
        Set-Content -LiteralPath (Join-Path $otaDir "triple-summary.json") -Encoding utf8
    }

    Invoke-Gate -Name "Type-connected long soak >=20min" -MapId "REG-STAB-001" -Level "L3" -Block {
      if ([string]::IsNullOrWhiteSpace($ComPort)) {
        throw "No serial COM port available for long soak (pass -ComPort COMx)"
      }
      # Product path: Type keeps BLE notify; serial briefly toggles VREC for wall-clock soak.
      # (Raw firmware long_session auto-stops on silence without close-mic speech.)
      $soakScript = Join-Path $scriptDir "run-listener-1.0.4-type-connected-soak.ps1"
      if (-not (Test-Path -LiteralPath $soakScript)) { throw "missing $soakScript" }
      $soakDir = Join-Path $OutputDir "type-connected-soak"
      Invoke-PwshFile $soakScript @(
        "-ComPort", $ComPort,
        "-SoakMinutes", "$LongSessionMinutes",
        "-OutputDir", $soakDir,
        "-RequireNotifyUnderMs", "3000"
      )
      $soakSummary = Join-Path $soakDir "long-soak-summary.json"
      if (-not (Test-Path -LiteralPath $soakSummary)) {
        throw "soak summary missing: $soakSummary"
      }
      $soak = Get-Content -Raw -LiteralPath $soakSummary | ConvertFrom-Json
      if ([string]$soak.result -ne "PASS") {
        throw ("soak FAIL: {0}" -f (($soak.failures | ForEach-Object { $_ }) -join "; "))
      }
      Write-Host ("soak PASS elapsed={0}s cycles={1}" -f $soak.elapsed_seconds, $soak.session_cycles)
    }
  } else {
    foreach ($name in @(
        "Type warm start notify-ready",
        "installed OTA triple gate",
        "firmware long session >=20min"
      )) {
      $results.Add([pscustomobject]@{
          name    = $name
          map_id  = "REG-HARDWARE"
          level   = "L2"
          status  = "SKIPPED_HARDWARE"
          seconds = 0
          log     = $null
          error   = "pass -IncludeHardware when device + installed Type are ready"
        }) | Out-Null
    }
  }
} catch {
  Write-Host "Regression aborted: $($_.Exception.Message)"
}

$afterAppFp = Get-ProfileFingerprint $prodApp
$afterLocalFp = Get-ProfileFingerprint $prodLocal $localExclude
$appOk = ($beforeAppFp -eq $afterAppFp)
$localOk = ($beforeLocalFp -eq $afterLocalFp)
# Production preference profile must not change. Local non-log state also must not change.
$envStatus = if ($appOk -and $localOk) { "PASS" } else { "FAIL" }
$results.Add([pscustomobject]@{
    name    = "production profile unchanged after regression"
    map_id  = "REG-ENV-001"
    level   = "L0"
    status  = $envStatus
    seconds = 0
    log     = $null
    error   = $(if ($envStatus -eq "FAIL") {
        "profile fingerprint changed appdata_match=$appOk local_non_log_match=$localOk"
      } else { $null })
  }) | Out-Null

$failed = @($results | Where-Object { $_.status -eq "FAIL" })
$passed = @($results | Where-Object { $_.status -eq "PASS" })
$skipped = @($results | Where-Object { $_.status -like "SKIPPED*" })
$overall = if ($failed.Count -eq 0) { "PASS" } else { "FAIL" }
$elapsedAll = [int]((Get-Date) - $startedAll).TotalSeconds

$summary = [ordered]@{
  schema         = "listener.1.0.4.regression_summary"
  schema_version = 1
  result         = $overall
  started_at     = $startedAll.ToString("o")
  elapsed_seconds = $elapsedAll
  output_dir     = $OutputDir
  include_hardware = [bool]$IncludeHardware
  type_root      = $typeRoot
  firmware_root  = $firmwareRoot
  listener_root  = $listenerRoot
  requirement_map = "listener-1.0.4-requirement-test-map.json"
  counts         = [ordered]@{
    pass    = $passed.Count
    fail    = $failed.Count
    skipped = $skipped.Count
    total   = $results.Count
  }
  production_profile_guard = [ordered]@{
    status              = $envStatus
    appdata_path        = $prodApp
    localappdata_path   = $prodLocal
    appdata_fingerprint_match = ($beforeAppFp -eq $afterAppFp)
    localappdata_fingerprint_match = ($beforeLocalFp -eq $afterLocalFp)
  }
  gates          = @($results)
  human_residual = @(
    "唤醒手感",
    "双人实说",
    "灯效视觉",
    "Windows 配对弹窗"
  )
  non_goals = @(
    "不继续主观微调唤醒或自动结束",
    "不延长 1000 ms 自动结束",
    "不推送/不发布",
    "不改已发布 v1.0.4 tag / 根目录 MSI/OTA"
  )
}

$summaryPath = Join-Path $OutputDir "summary.json"
$summary | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $summaryPath -Encoding utf8
Write-Host ""
Write-Host ("RESULT={0}  PASS={1} FAIL={2} SKIPPED={3}  summary={4}" -f `
    $overall, $passed.Count, $failed.Count, $skipped.Count, $summaryPath)

if ($overall -ne "PASS") { exit 1 }
exit 0
