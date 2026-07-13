param(
  [string]$ListenerExe = "C:\Program Files\Listener Type\listener-type.exe",
  [int]$ReadyTimeoutSeconds = 25,
  [int]$MaxNotifyReadyMs = 3000,
  [string]$OutputJson = "",
  [switch]$RequirePersistedPath
)

$ErrorActionPreference = "Stop"

function Convert-LogTimestamp {
  param([string]$Value)
  $parsed = [datetimeoffset]::MinValue
  if ([datetimeoffset]::TryParse($Value, [ref]$parsed)) {
    return $parsed
  }
  return $null
}

function Read-StartupSessions {
  param(
    [string]$LogPath,
    [datetimeoffset]$Marker
  )

  if (-not (Test-Path -LiteralPath $LogPath)) {
    return @()
  }

  $sessions = @()
  $current = $null
  foreach ($line in Get-Content -LiteralPath $LogPath) {
    # A process handoff can append a new startup record directly after the old
    # logger's final partial line. Parse every complete record on that line.
    $records = [regex]::Matches(
      $line,
      "(?<ts>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})) \[(?<lvl>[^\]]+)\] (?<msg>.*?)(?=(?:\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})) \[|$)"
    )
    foreach ($record in $records) {
      $timestamp = Convert-LogTimestamp $record.Groups["ts"].Value
      if ($null -eq $timestamp) {
        continue
      }
      $message = $record.Groups["msg"].Value
      if ($message -like "*=== Listener Type 启动 ===*") {
        if ($current) {
          $sessions += $current
        }
        $current = [ordered]@{
          start  = $timestamp
          events = @()
        }
      }
      if (-not $current -or $current.start -lt $Marker) {
        continue
      }
      $interesting =
        $message -like "*startup BLE*" -or
        $message -like "*persisted*" -or
        $message -like "*GATT session ready*" -or
        $message -like "*selected*" -or
        $message -like "*notify CCCD*" -or
        $message -like "*Type heartbeat ready*" -or
        $message -like "*background listener notify ready*" -or
        $message -like "*PairAsync*"
      if ($interesting) {
        $current.events += [pscustomobject]@{
          t   = $timestamp.ToString("o")
          ms  = [int](($timestamp - $current.start).TotalMilliseconds)
          msg = $message
        }
      }
    }
  }
  if ($current) {
    $sessions += $current
  }
  @($sessions | Where-Object { $_.start -ge $Marker })
}

if (-not (Test-Path -LiteralPath $ListenerExe)) {
  throw "Listener Type exe not found: $ListenerExe"
}

$repoRoot = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($OutputJson)) {
  $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
  $OutputJson = Join-Path $repoRoot ".cache\validation\type-startup-reconnect-$stamp\summary.json"
}
$outputDir = Split-Path -Parent $OutputJson
New-Item -ItemType Directory -Force -Path $outputDir | Out-Null

$logPath = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
$statePath = Join-Path $env:APPDATA "Listener Type\ble_device_state.json"
$running = @(Get-Process -Name "listener-type" -ErrorAction SilentlyContinue)
$gracefulShutdown = $true
if ($running.Count -gt 0) {
  Start-Process -FilePath $ListenerExe -ArgumentList "--quit" -WindowStyle Hidden | Out-Null
  $shutdownDeadline = (Get-Date).AddSeconds(6)
  while ((Get-Date) -lt $shutdownDeadline -and (Get-Process -Name "listener-type" -ErrorAction SilentlyContinue)) {
    Start-Sleep -Milliseconds 150
  }
  $remaining = @(Get-Process -Name "listener-type" -ErrorAction SilentlyContinue)
  if ($remaining.Count -gt 0) {
    $gracefulShutdown = $false
    $remaining | Stop-Process -Force
  }
}

$marker = [datetimeoffset]::UtcNow
$process = Start-Process -FilePath $ListenerExe -PassThru

$deadline = (Get-Date).AddSeconds($ReadyTimeoutSeconds)
$last = $null
while ((Get-Date) -lt $deadline) {
  $sessions = Read-StartupSessions -LogPath $logPath -Marker $marker
  $last = $sessions | Select-Object -Last 1
  if ($last -and ($last.events | Where-Object { $_.msg -like "*background listener notify ready*" })) {
    break
  }
  Start-Sleep -Milliseconds 250
}

if (-not $last) {
  throw "No fresh Listener Type startup was found after $($marker.ToString("o"))"
}

$ready = $last.events | Where-Object { $_.msg -like "*background listener notify ready*" } | Select-Object -Last 1
$heartbeat = $last.events | Where-Object { $_.msg -like "*Type heartbeat ready sent*" } | Select-Object -Last 1
$persistedSelected = $last.events | Where-Object { $_.msg -like "*selected persisted startup audio notify*" } | Select-Object -Last 1
$nativeHidSelected = $last.events | Where-Object { $_.msg -like "*selected native Windows HID startup audio notify*" } | Select-Object -Last 1
$startupFastPathSelected = @(
  $persistedSelected
  $nativeHidSelected
) | Where-Object { $null -ne $_ } | Select-Object -Last 1
$pairAsync = @($last.events | Where-Object {
  $_.msg -like "*background Type recovery PairAsync result status=*" -or
  $_.msg -like "*device BLE name change Windows PairAsync status=*" -or
  $_.msg -like "*trying custom PairAsync fallback*"
})
$errors = @()
if (-not $ready) {
  $errors += "background listener notify ready not observed"
} elseif ($ready.ms -gt $MaxNotifyReadyMs) {
  $errors += "notify ready took $($ready.ms) ms, over $MaxNotifyReadyMs ms"
}
if ($RequirePersistedPath -and -not $startupFastPathSelected) {
  $errors += "persisted/native-HID startup audio notify path was not used"
}
if ($pairAsync.Count -ne 0) {
  $errors += "PairAsync appeared during Type restart validation"
}

$stateText = if (Test-Path -LiteralPath $statePath) {
  Get-Content -Raw -LiteralPath $statePath
} else {
  $null
}

$summary = [ordered]@{
  status                           = if ($errors.Count -eq 0) { "PASS" } else { "FAIL" }
  errors                           = $errors
  started_at                       = $last.start.ToString("o")
  process_id                       = $process.Id
  persisted_path_used              = [bool]$persistedSelected
  native_windows_hid_path_used     = [bool]$nativeHidSelected
  startup_fast_path_used           = [bool]$startupFastPathSelected
  no_pair_async                    = ($pairAsync.Count -eq 0)
  graceful_shutdown                = $gracefulShutdown
  start_to_notify_ready_ms         = if ($ready) { $ready.ms } else { $null }
  start_to_type_heartbeat_ready_ms = if ($heartbeat) { $heartbeat.ms } else { $null }
  max_start_to_notify_ready_ms     = $MaxNotifyReadyMs
  events                           = $last.events
  ble_device_state_json            = $stateText
}

$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $OutputJson -Encoding UTF8
$summary | ConvertTo-Json -Depth 8

if ($errors.Count -gt 0) {
  throw ($errors -join "; ")
}
