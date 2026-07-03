param(
  [string]$ReleaseExePath = "",
  [string]$OutputDir = "",
  [switch]$SkipBuild,
  [switch]$SkipPackage,
  [switch]$SkipHardware,
  [switch]$SkipNpmCi
)

$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
$tauriRoot = Join-Path $repoRoot "src-tauri"
if ([string]::IsNullOrWhiteSpace($ReleaseExePath)) {
  $ReleaseExePath = Join-Path $tauriRoot "target\x86_64-pc-windows-msvc\release\listener-type.exe"
}
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
  $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
  $OutputDir = Join-Path $repoRoot ".artifacts\v1.0.2-regression\$stamp"
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

$results = [System.Collections.Generic.List[object]]::new()

function ConvertTo-SafeName {
  param([Parameter(Mandatory = $true)][string]$Name)
  return ($Name -replace '[^A-Za-z0-9_.-]+', '_').Trim('_')
}

function Invoke-External {
  param(
    [Parameter(Mandatory = $true)][string]$File,
    [string[]]$Args = @(),
    [string]$WorkingDirectory = ""
  )

  if (-not [string]::IsNullOrWhiteSpace($WorkingDirectory)) {
    Push-Location $WorkingDirectory
  }
  try {
    Write-Host ("> {0} {1}" -f $File, ($Args -join " "))
    & $File @Args
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
  }

  Invoke-Gate "rust format and BLE/OTA tests" {
    Invoke-External "cargo" @("fmt", "--", "--check") $tauriRoot
    Invoke-External "cargo" @("test", "embedded_ble_", "--lib") $tauriRoot
    Invoke-External "cargo" @("test", "device_ble_name", "--lib") $tauriRoot
    Invoke-External "cargo" @("test", "firmware_ota", "--lib") $tauriRoot
  }

  if (-not $SkipBuild.IsPresent) {
    Invoke-Gate "MSVC release build" {
      Invoke-External "cargo" @("build", "--release", "--target", "x86_64-pc-windows-msvc") $tauriRoot
    }
  }

  if (-not $SkipHardware.IsPresent) {
    Invoke-Gate "embedded audio BLE live probe" {
      if (-not (Test-Path -LiteralPath $ReleaseExePath)) {
        throw "Release exe not found: $ReleaseExePath"
      }
      Invoke-External $ReleaseExePath @("--probe-embedded-audio-ble-subscription", "12000") $repoRoot
      Invoke-External $ReleaseExePath @("--read-embedded-audio-ble-status", "12000") $repoRoot
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

  if (-not $SkipPackage.IsPresent) {
    Invoke-Gate "MSVC MSI package without portable zip" {
      Invoke-External "powershell" @(
        "-NoProfile",
        "-ExecutionPolicy", "Bypass",
        "-File", (Join-Path $PSScriptRoot "windows-package-msvc.ps1"),
        "-SkipRustInstall",
        "-SkipNpmCi",
        "-CleanArtifacts"
      ) $repoRoot
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
