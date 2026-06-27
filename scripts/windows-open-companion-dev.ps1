param(
  [switch]$Release
)

$ErrorActionPreference = "Stop"

$appRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$configPath = Join-Path $appRoot "src-tauri\tauri.companion.conf.json"
if (-not (Test-Path -LiteralPath $configPath -PathType Leaf)) {
  throw "Companion Tauri config not found: $configPath"
}

Push-Location $appRoot
try {
  $previousProfile = $env:LISTENER_TYPE_APP_PROFILE
  $previousTargetName = $env:LISTENER_TYPE_BLE_TARGET_NAME
  $previousHideMain = $env:LISTENER_TYPE_HIDE_MAIN_ON_START
  $previousShowMain = $env:LISTENER_TYPE_SHOW_MAIN_ON_START

  $env:LISTENER_TYPE_APP_PROFILE = "companion"
  $env:LISTENER_TYPE_BLE_TARGET_NAME = "companion"
  $env:LISTENER_TYPE_HIDE_MAIN_ON_START = "0"
  $env:LISTENER_TYPE_SHOW_MAIN_ON_START = "1"

  $args = @("tauri", "dev", "--config", $configPath)
  if ($Release) {
    $args += "--release"
  }

  Write-Host "Starting Companion Type dev app on Vite port 1421."
  Write-Host "Profile: Listener Type Companion"
  Write-Host "BLE target: companion"
  & npx.cmd @args
  if ($LASTEXITCODE -ne 0) {
    throw "Companion Type dev app exited with code $LASTEXITCODE."
  }
} finally {
  if ($null -eq $previousProfile) { Remove-Item Env:LISTENER_TYPE_APP_PROFILE -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_APP_PROFILE = $previousProfile }
  if ($null -eq $previousTargetName) { Remove-Item Env:LISTENER_TYPE_BLE_TARGET_NAME -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_BLE_TARGET_NAME = $previousTargetName }
  if ($null -eq $previousHideMain) { Remove-Item Env:LISTENER_TYPE_HIDE_MAIN_ON_START -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_HIDE_MAIN_ON_START = $previousHideMain }
  if ($null -eq $previousShowMain) { Remove-Item Env:LISTENER_TYPE_SHOW_MAIN_ON_START -ErrorAction SilentlyContinue } else { $env:LISTENER_TYPE_SHOW_MAIN_ON_START = $previousShowMain }
  Pop-Location
}
