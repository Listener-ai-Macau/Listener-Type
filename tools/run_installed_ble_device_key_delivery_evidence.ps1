param(
  [string]$ListenerExe = 'C:\Program Files\Listener Type\listener-type.exe',
  [string]$OutputDir = '',
  [int]$TimeoutSeconds = 3
)

$ErrorActionPreference = 'Stop'

function Get-TextSha256 {
  param([string]$Value)

  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes($Value)
    ([System.BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '')
  } finally {
    $sha.Dispose()
  }
}

function Read-FileBytesAfterOffset {
  param(
    [string]$Path,
    [long]$Offset
  )

  if (-not (Test-Path -LiteralPath $Path)) {
    return ''
  }
  $stream = [System.IO.File]::Open(
    $Path,
    [System.IO.FileMode]::Open,
    [System.IO.FileAccess]::Read,
    [System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete
  )
  try {
    if ($stream.Length -le $Offset) {
      return ''
    }
    $null = $stream.Seek($Offset, [System.IO.SeekOrigin]::Begin)
    $remaining = [int]($stream.Length - $Offset)
    $bytes = [byte[]]::new($remaining)
    $read = 0
    while ($read -lt $remaining) {
      $count = $stream.Read($bytes, $read, $remaining - $read)
      if ($count -le 0) { break }
      $read += $count
    }
    [System.Text.Encoding]::UTF8.GetString($bytes, 0, $read)
  } finally {
    $stream.Dispose()
  }
}

if (-not (Test-Path -LiteralPath $ListenerExe)) {
  throw "Installed Type executable not found: $ListenerExe"
}
$resolvedExe = (Resolve-Path -LiteralPath $ListenerExe).Path
if ($resolvedExe -ne 'C:\Program Files\Listener Type\listener-type.exe') {
  throw "Physical validation must use only C:\Program Files\Listener Type\listener-type.exe; got $resolvedExe"
}

$repoRoot = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
  $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
  $OutputDir = Join-Path $repoRoot ".cache\validation\ble-device-key-delivery-$stamp"
}
$allowedCacheRoot = (Join-Path $repoRoot '.cache\validation').ToLowerInvariant()
$resolvedOutputDir = [System.IO.Path]::GetFullPath($OutputDir)
$normalizedOutputDir = $resolvedOutputDir.ToLowerInvariant()
$allowedCachePrefix = $allowedCacheRoot + [System.IO.Path]::DirectorySeparatorChar
if ($normalizedOutputDir -ne $allowedCacheRoot -and -not $normalizedOutputDir.StartsWith($allowedCachePrefix)) {
  throw "OutputDir must stay under $allowedCacheRoot"
}
New-Item -ItemType Directory -Force -Path $resolvedOutputDir | Out-Null

$summaryPath = Join-Path $resolvedOutputDir 'machine-summary.json'
$errors = @()
$preferencesPath = Join-Path $env:APPDATA 'Listener Type\preferences.json'
$mappingSafe = $false
try {
  $preferences = Get-Content -Raw -LiteralPath $preferencesPath | ConvertFrom-Json
  $key1 = $preferences.deviceCustomKeys.key1
  $modifiers = @($key1.shortcut.modifiers)
  $mappingSafe =
    ([string]$key1.action -eq 'sendShortcut') -and
    ([string]$key1.shortcut.primary -ieq 'RightControl') -and
    ($modifiers.Count -eq 0)
} catch {
  $errors += 'could not read the current KEY1 safety mapping.'
}
if (-not $mappingSafe) {
  $errors += 'KEY1 is not the safe unmodified RightControl SendShortcut mapping; diagnostic command was not sent.'
}

$installed = @(
  Get-CimInstance Win32_Process -Filter "Name='listener-type.exe'" |
    Where-Object { $_.ExecutablePath -eq $resolvedExe }
)
if ($installed.Count -ne 1) {
  $errors += 'exactly one installed Program Files Listener Type process must already be running.'
}

$typeLogPath = Join-Path $env:LOCALAPPDATA 'Listener Type\Logs\listener-type.log'
$typeLogOffset = if (Test-Path -LiteralPath $typeLogPath) {
  (Get-Item -LiteralPath $typeLogPath).Length
} else {
  0
}
$serialText = ''
$serialError = $null
$commandSent = $false
$mutex = $null
$port = $null
$acquired = $false
if ($errors.Count -eq 0) {
  $created = $false
  $mutex = [System.Threading.Mutex]::new($false, 'Global\Listener_COM3', [ref]$created)
  try {
    $acquired = $mutex.WaitOne(1500)
    if (-not $acquired) { throw 'Global\\Listener_COM3 mutex timed out before KEY1 delivery capture.' }
    $port = [System.IO.Ports.SerialPort]::new('COM3', 115200, [System.IO.Ports.Parity]::None, 8, [System.IO.Ports.StopBits]::One)
    $port.ReadTimeout = 80
    $port.WriteTimeout = 500
    $port.Open()
    $port.DiscardInBuffer()
    $port.Write("~KEY:KEY1:SINGLE`r`n")
    $commandSent = $true
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
      $serialText += $port.ReadExisting()
      Start-Sleep -Milliseconds 40
    }
    $serialText += $port.ReadExisting()
  } catch {
    $serialError = $_.Exception.Message
    $errors += 'serial KEY1 delivery capture failed.'
  } finally {
    if ($port) { $port.Dispose() }
    if ($acquired) { $mutex.ReleaseMutex() }
    if ($mutex) { $mutex.Dispose() }
  }
}

$typeLogText = ''
$logDeadline = (Get-Date).AddSeconds($TimeoutSeconds)
do {
  $typeLogText = Read-FileBytesAfterOffset -Path $typeLogPath -Offset $typeLogOffset
  $pressedCount = [regex]::Matches($typeLogText, '\[device-key\] KEY1 singleClick pressed action=SendShortcut', 'IgnoreCase').Count
  $sentCount = [regex]::Matches($typeLogText, '\[device-key\] KEY1 sent shortcut (RightControl|Right Ctrl|Right Control)', 'IgnoreCase').Count
  if ($pressedCount -gt 0 -and $sentCount -gt 0) { break }
  Start-Sleep -Milliseconds 75
} while ((Get-Date) -lt $logDeadline)

$firmwareGeneratedCount = [regex]::Matches($serialText, '~KEY:GENERATED logical=KEY1 gesture=single result=ESP_OK', 'IgnoreCase').Count
$firmwareFallbackCount = [regex]::Matches($serialText, 'custom key fallback queued: logical=KEY1.*usage=F13.*gesture=single', 'IgnoreCase').Count
if ($commandSent -and $firmwareGeneratedCount -lt 1) {
  $errors += 'firmware did not accept the generated KEY1 command.'
}
if ($commandSent -and $firmwareFallbackCount -lt 1) {
  $errors += 'firmware did not queue the KEY1 F13 fallback HID event.'
}
if ($commandSent -and $pressedCount -lt 1) {
  $errors += 'installed Type did not observe the KEY1 single-click device-key event.'
}
if ($commandSent -and $sentCount -lt 1) {
  $errors += 'installed Type did not complete the safe RightControl action for KEY1.'
}

$summary = [ordered]@{
  schema = 'listener.ble.device-key-delivery-evidence.v1'
  captured_at = (Get-Date).ToString('o')
  installed_type = [ordered]@{
    executable = $resolvedExe
    exe_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $resolvedExe).Hash
    process_id = if ($installed.Count -eq 1) { $installed[0].ProcessId } else { $null }
  }
  safety_precondition = [ordered]@{
    key = 'KEY1'
    expected_action = 'SendShortcut'
    expected_shortcut = 'RightControl'
    mapping_verified = $mappingSafe
  }
  serial_mutex = 'Global\\Listener_COM3'
  firmware = [ordered]@{
    generated_command_accepted_count = $firmwareGeneratedCount
    fallback_hid_queued_count = $firmwareFallbackCount
    serial_sha256 = Get-TextSha256 $serialText
    serial_error = $serialError
  }
  installed_type_delivery = [ordered]@{
    key_pressed_count = $pressedCount
    safe_shortcut_sent_count = $sentCount
    new_log_sha256 = Get-TextSha256 $typeLogText
  }
  privacy = [ordered]@{
    audio_retained = $false
    stimulus_retained = $false
    transcript_retained = $false
    raw_serial_or_log_retained = $false
  }
  verdict = if ($errors.Count -eq 0) { 'PASS' } else { 'NO_GO' }
  errors = $errors
}
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding utf8NoBOM
$summary | ConvertTo-Json -Depth 8
if ($errors.Count -gt 0) { exit 1 }
