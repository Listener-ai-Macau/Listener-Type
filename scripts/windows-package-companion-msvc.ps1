param(
  [string]$ArtifactsRoot = "",
  [switch]$SkipNpmCi,
  [switch]$NoBundle
)

$ErrorActionPreference = "Stop"

$appRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$configPath = Join-Path $appRoot "src-tauri\tauri.companion.conf.json"
if ([string]::IsNullOrWhiteSpace($ArtifactsRoot)) {
  $ArtifactsRoot = Join-Path $appRoot ".artifacts\companion-type-msvc"
}

function Add-PathEntry($PathEntry) {
  if ([string]::IsNullOrWhiteSpace($PathEntry) -or -not (Test-Path -LiteralPath $PathEntry)) {
    return
  }
  $entries = $env:PATH -split ";"
  if ($entries -notcontains $PathEntry) {
    $env:PATH = "$PathEntry;$env:PATH"
  }
}

function Get-PackageVersion {
  $packageJson = Get-Content -LiteralPath (Join-Path $appRoot "package.json") -Raw | ConvertFrom-Json
  return $packageJson.version
}

Push-Location $appRoot
try {
  if (-not (Test-Path -LiteralPath $configPath -PathType Leaf)) {
    throw "Companion Tauri config not found: $configPath"
  }

  if (-not $SkipNpmCi) {
    npm.cmd ci
    if ($LASTEXITCODE -ne 0) {
      throw "npm ci failed with code $LASTEXITCODE."
    }
  }

  $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
  Add-PathEntry $cargoBin

  $previousProfile = $env:LISTENER_TYPE_APP_PROFILE
  $env:LISTENER_TYPE_APP_PROFILE = "companion"
  $buildArgs = @("tauri", "build", "--config", $configPath, "--target", "x86_64-pc-windows-msvc")
  if ($NoBundle) {
    $buildArgs += "--no-bundle"
  } else {
    $buildArgs += @("--bundles", "nsis")
  }

  & npx.cmd @buildArgs
  if ($LASTEXITCODE -ne 0) {
    throw "Companion Type MSVC build failed with code $LASTEXITCODE."
  }

  $version = Get-PackageVersion
  New-Item -ItemType Directory -Force -Path $ArtifactsRoot | Out-Null
  $releaseRoot = Join-Path $appRoot "src-tauri\target\x86_64-pc-windows-msvc\release"
  $exePath = Join-Path $releaseRoot "listener-type.exe"
  if (Test-Path -LiteralPath $exePath -PathType Leaf) {
    $portableRoot = Join-Path $ArtifactsRoot "CompanionType_$($version)_x64_portable"
    New-Item -ItemType Directory -Force -Path $portableRoot | Out-Null
    Copy-Item -LiteralPath $exePath -Destination (Join-Path $portableRoot "companion-type.exe") -Force
    $launcher = @"
@echo off
set LISTENER_TYPE_APP_PROFILE=companion
set LISTENER_TYPE_BLE_TARGET_NAME=companion
start "" "%~dp0companion-type.exe" %*
"@
    Set-Content -LiteralPath (Join-Path $portableRoot "companion-type.cmd") -Value $launcher -Encoding ASCII
    Write-Host "Portable Companion Type app: $portableRoot"
  }
} finally {
  if ($null -eq $previousProfile) { Remove-Item Env:LISTENER_TYPE_APP_PROFILE -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_APP_PROFILE = $previousProfile }
  Pop-Location
}
