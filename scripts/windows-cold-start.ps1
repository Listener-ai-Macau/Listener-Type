param(
  [string]$ExePath = "",
  [switch]$FreshBuild,
  [switch]$PreferDebug,
  [switch]$ShowMain,
  [switch]$KeepLogs,
  [switch]$ForceImmediateShow
)

$ErrorActionPreference = "Stop"

$appRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$artifactExe = Join-Path $appRoot ".artifacts\windows-gnu\dev\listener-type.exe"
$debugExe = Join-Path $appRoot "src-tauri\target\debug\listener-type.exe"

function Resolve-DefaultExePath {
  param(
    [string]$ArtifactExe,
    [string]$DebugExe,
    [switch]$PreferDebug
  )

  $artifactItem = if (Test-Path $ArtifactExe) { Get-Item $ArtifactExe } else { $null }
  $debugItem = if (Test-Path $DebugExe) { Get-Item $DebugExe } else { $null }

  if ($PreferDebug -and $debugItem) {
    return $debugItem.FullName
  }
  if ($debugItem -and (-not $artifactItem -or $debugItem.LastWriteTime -gt $artifactItem.LastWriteTime)) {
    return $debugItem.FullName
  }
  if ($artifactItem) {
    return $artifactItem.FullName
  }
  if ($debugItem) {
    return $debugItem.FullName
  }
  return $ArtifactExe
}

if ($FreshBuild) {
  # Stop running instance before build — Windows locks the exe and cargo can't overwrite it
  $running = Get-Process listener-type -ErrorAction SilentlyContinue
  if ($running) {
    Write-Host "Stopping running Listener Type (would lock debug exe during build)..."
    $running | Stop-Process -Force
    Start-Sleep -Milliseconds 600
  }
  Push-Location $appRoot
  try {
    Write-Host "Building frontend dist..."
    npm run build
    Write-Host "Building backend debug exe..."
    cargo build --manifest-path src-tauri/Cargo.toml
  } finally {
    Pop-Location
  }
}

if ([string]::IsNullOrWhiteSpace($ExePath)) {
  $ExePath = Resolve-DefaultExePath -ArtifactExe $artifactExe -DebugExe $debugExe -PreferDebug:$PreferDebug
}

if (-not (Test-Path $ExePath)) {
  throw "Listener Type executable not found: $ExePath"
}

if (-not $env:SystemDrive) {
  $env:SystemDrive = "C:"
}
if (-not $env:ProgramData) {
  $env:ProgramData = Join-Path $env:SystemDrive "ProgramData"
}

$logPath = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
$workingDirectory = Split-Path $ExePath -Parent

Add-Type @"
using System;
using System.Runtime.InteropServices;

public static class ListenerTypeWindow {
  [DllImport("user32.dll")]
  public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

  [DllImport("user32.dll")]
  public static extern bool SetForegroundWindow(IntPtr hWnd);
}
"@

function Show-ListenerTypeWindow($Process) {
  if ($null -eq $Process -or $Process.MainWindowHandle -eq 0) {
    return $false
  }

  [ListenerTypeWindow]::ShowWindow($Process.MainWindowHandle, 9) | Out-Null
  [ListenerTypeWindow]::SetForegroundWindow($Process.MainWindowHandle) | Out-Null
  return $true
}

Write-Host "== Windows cold start =="
Write-Host "ExePath: $ExePath"

$running = Get-Process listener-type -ErrorAction SilentlyContinue
if ($running) {
  Write-Host "Stopping existing Listener Type processes..."
  $running | Stop-Process -Force
  Start-Sleep -Milliseconds 600
}

if (-not $KeepLogs -and (Test-Path $logPath)) {
  Remove-Item -LiteralPath $logPath -Force -ErrorAction SilentlyContinue
  Write-Host "Cleared log: $logPath"
}

$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:USERPROFILE\scoop\persist\rustup\.cargo\bin;$env:USERPROFILE\scoop\apps\rustup\current\.cargo\bin;$env:USERPROFILE\scoop\apps\mingw\current\bin;$env:PATH"
$useImmediateShow = $ShowMain -and $ForceImmediateShow
if ($useImmediateShow) {
  $env:LISTENER_TYPE_SHOW_MAIN_ON_START = "1"
}

try {
  $process = Start-Process -FilePath $ExePath -WorkingDirectory $workingDirectory -PassThru
} finally {
  if ($useImmediateShow) {
    Remove-Item Env:LISTENER_TYPE_SHOW_MAIN_ON_START -ErrorAction SilentlyContinue
  }
}

Write-Host "Started Listener Type cold. pid=$($process.Id)"
Write-Host "Log path: $logPath"
if ($ShowMain) {
  if ($ForceImmediateShow) {
    Write-Host "Mode: backend immediate show (debug-only, may expose startup shell)"
  } else {
    Write-Host "Mode: frontend-managed first show (recommended for startup contract testing)"
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-Date) -lt $deadline) {
      Start-Sleep -Milliseconds 250
      $current = Get-Process -Id $process.Id -ErrorAction SilentlyContinue
      if (Show-ListenerTypeWindow $current) {
        Write-Host "Listener Type main window became visible and was brought to foreground. pid=$($current.Id)"
        break
      }
    }
  }
} else {
  Write-Host "Mode: startup-default visibility"
}
