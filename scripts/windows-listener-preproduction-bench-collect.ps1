[CmdletBinding(PositionalBinding = $false)]
param(
  [string]$OutputDir = "",
  [string]$RepoBaseRoot = "",
  [string]$FirmwareRoot = "",
  [string]$TypeExe = "",
  [string]$ExpectedName = "listener",
  [string]$ActiveBleSummaryPath = "",
  [string]$SerialPort = "",
  [int]$LiveBleTimeoutMs = 20000,
  [switch]$SkipLiveBle,
  [switch]$SkipSerialSnapshot,
  [switch]$SkipScreenshot,
  [switch]$SkipNotificationScan,
  [switch]$NoReview
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
if ([string]::IsNullOrWhiteSpace($RepoBaseRoot)) {
  $RepoBaseRoot = (Resolve-Path -LiteralPath (Join-Path $repoRoot "..\..")).Path
} else {
  $RepoBaseRoot = (Resolve-Path -LiteralPath $RepoBaseRoot).Path
}
if ([string]::IsNullOrWhiteSpace($FirmwareRoot)) {
  $candidateFirmwareRoot = Join-Path (Split-Path -Parent $repoRoot) "Listener-Firmware"
  if (Test-Path -LiteralPath $candidateFirmwareRoot) {
    $FirmwareRoot = (Resolve-Path -LiteralPath $candidateFirmwareRoot).Path
  }
}
if ([string]::IsNullOrWhiteSpace($TypeExe)) {
  $TypeExe = "C:\Program Files\Listener Type\listener-type.exe"
}
if (-not [string]::IsNullOrWhiteSpace($TypeExe) -and (Test-Path -LiteralPath $TypeExe)) {
  $TypeExe = (Resolve-Path -LiteralPath $TypeExe).Path
}
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
  $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
  $OutputDir = Join-Path $repoRoot ".cache\validation\preproduction-bench-collect-$stamp"
} elseif (-not [System.IO.Path]::IsPathRooted($OutputDir)) {
  $OutputDir = Join-Path $repoRoot $OutputDir
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$OutputDir = (Resolve-Path -LiteralPath $OutputDir).Path

$manifestPath = Join-Path $OutputDir "preproduction-bench-capabilities.json"
$summaryPath = Join-Path $OutputDir "preproduction-bench-collect-summary.json"
$reviewSummaryPath = Join-Path $OutputDir "preproduction-bench-review-summary.json"

function Write-JsonFile {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)]$Value,
    [int]$Depth = 10
  )
  $encoding = New-Object System.Text.UTF8Encoding -ArgumentList $false
  [System.IO.File]::WriteAllText($Path, ($Value | ConvertTo-Json -Depth $Depth), $encoding)
}

function Write-TextFile {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [AllowNull()][string]$Text
  )
  $encoding = New-Object System.Text.UTF8Encoding -ArgumentList $false
  [System.IO.File]::WriteAllText($Path, [string]$Text, $encoding)
}

function Get-RelativeEvidencePath {
  param([AllowNull()][string]$Path)
  if ([string]::IsNullOrWhiteSpace($Path)) {
    return ""
  }
  return [System.IO.Path]::GetRelativePath($OutputDir, (Resolve-Path -LiteralPath $Path).Path)
}

function Add-Evidence {
  param(
    [Parameter(Mandatory = $true)]$Manifest,
    [Parameter(Mandatory = $true)][string]$StepId,
    [Parameter(Mandatory = $true)][string]$Key,
    [AllowNull()][string]$Path
  )
  if ([string]::IsNullOrWhiteSpace($Path) -or -not (Test-Path -LiteralPath $Path)) {
    return
  }
  if (-not $Manifest.evidence.Contains($StepId)) {
    $Manifest.evidence[$StepId] = [ordered]@{}
  }
  $Manifest.evidence[$StepId][$Key] = Get-RelativeEvidencePath $Path
}

function Get-ActiveBleRecordEvidence {
  param(
    [AllowNull()]$Summary,
    [Parameter(Mandatory = $true)][string]$Stage
  )
  if ($null -eq $Summary) {
    return ""
  }
  $record = @($Summary.records | Where-Object { $_.stage -eq $Stage } | Select-Object -First 1)
  if ($record.Count -eq 0 -or $null -eq $record[0].PSObject.Properties["evidence"]) {
    return ""
  }
  $path = [string]$record[0].evidence
  if ([string]::IsNullOrWhiteSpace($path)) {
    return ""
  }
  if ([System.IO.Path]::IsPathRooted($path)) {
    return $path
  }
  if (-not [string]::IsNullOrWhiteSpace($ActiveBleSummaryPath)) {
    return Join-Path (Split-Path -Parent (Resolve-Path -LiteralPath $ActiveBleSummaryPath).Path) $path
  }
  return ""
}

function Get-ActiveBleEvidencePath {
  param(
    [AllowNull()]$Summary,
    [Parameter(Mandatory = $true)][string]$Key
  )
  if ($null -eq $Summary -or $null -eq $Summary.PSObject.Properties["evidence"]) {
    return ""
  }
  if ($null -eq $Summary.evidence.PSObject.Properties[$Key]) {
    return ""
  }
  $path = [string]$Summary.evidence.PSObject.Properties[$Key].Value
  if ([string]::IsNullOrWhiteSpace($path)) {
    return ""
  }
  if ([System.IO.Path]::IsPathRooted($path)) {
    return $path
  }
  if (-not [string]::IsNullOrWhiteSpace($ActiveBleSummaryPath)) {
    return Join-Path (Split-Path -Parent (Resolve-Path -LiteralPath $ActiveBleSummaryPath).Path) $path
  }
  return ""
}

function Get-TypeProcesses {
  $exeFilter = ""
  if (-not [string]::IsNullOrWhiteSpace($TypeExe)) {
    $exeFilter = [System.IO.Path]::GetFileName($TypeExe)
  }
  $rows = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
      $_.Name -eq "listener-type.exe" -or
      $_.Name -eq $exeFilter -or
      ([string]$_.CommandLine) -match "listener-type"
    })
  return @(
    foreach ($row in $rows) {
      $fileVersion = ""
      $productVersion = ""
      $lastWriteTime = ""
      $sha256 = ""
      $exePath = [string]$row.ExecutablePath
      if (-not [string]::IsNullOrWhiteSpace($exePath) -and (Test-Path -LiteralPath $exePath)) {
        try {
          $item = Get-Item -LiteralPath $exePath -ErrorAction Stop
          $fileVersion = [string]$item.VersionInfo.FileVersion
          $productVersion = [string]$item.VersionInfo.ProductVersion
          $lastWriteTime = $item.LastWriteTime.ToString("o")
          $sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $exePath).Hash
        } catch {
        }
      }
      [pscustomobject][ordered]@{
        ProcessId = $row.ProcessId
        Name = $row.Name
        ExecutablePath = $exePath
        FileVersion = $fileVersion
        ProductVersion = $productVersion
        LastWriteTime = $lastWriteTime
        Sha256 = $sha256
        CommandLine = $row.CommandLine
        CreationDate = $row.CreationDate
      }
    }
  )
}

function Get-TypeLogTail {
  param([int]$MaxBytes = 131072)
  $appLog = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
  if (-not (Test-Path -LiteralPath $appLog)) {
    return [pscustomobject]@{ path = $appLog; exists = $false; tail = "" }
  }
  $item = Get-Item -LiteralPath $appLog
  $stream = [System.IO.File]::Open($item.FullName, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
  try {
    $offset = [Math]::Max(0, $stream.Length - $MaxBytes)
    $null = $stream.Seek($offset, [System.IO.SeekOrigin]::Begin)
    $reader = [System.IO.StreamReader]::new($stream, [System.Text.Encoding]::UTF8, $true, 4096, $true)
    try {
      return [pscustomobject]@{ path = $item.FullName; exists = $true; tail = $reader.ReadToEnd() }
    } finally {
      $reader.Dispose()
    }
  } finally {
    $stream.Dispose()
  }
}

function Resolve-SerialPortName {
  if (-not [string]::IsNullOrWhiteSpace($SerialPort)) {
    return $SerialPort
  }
  try {
    $ports = @(Get-CimInstance Win32_SerialPort -ErrorAction SilentlyContinue |
      Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_.DeviceID) } |
      Select-Object -ExpandProperty DeviceID)
    if ($ports.Count -eq 1) {
      return [string]$ports[0]
    }
  } catch {
  }
  return ""
}

function Invoke-SerialLedStatusSnapshot {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$PortName
  )
  if ([string]::IsNullOrWhiteSpace($FirmwareRoot)) {
    throw "FirmwareRoot is not set."
  }
  $sendSerial = Join-Path $FirmwareRoot "tools\send_serial_and_capture.ps1"
  if (-not (Test-Path -LiteralPath $sendSerial)) {
    throw "Missing firmware serial helper: $sendSerial"
  }
  & pwsh -NoProfile -File $sendSerial `
    -Port $PortName `
    -Command "~LED:STATUS detail=summary" `
    -InitialReadMs 200 `
    -CommandReadMs 1200 `
    -CommandDelayMs 0 `
    -OutputPath $Path | Out-Null
  if ($LASTEXITCODE -ne 0) {
    throw "serial LED status helper exited with code $LASTEXITCODE"
  }
}

function Invoke-SerialDeviceSettingsSnapshot {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$PortName
  )
  if ([string]::IsNullOrWhiteSpace($FirmwareRoot)) {
    throw "FirmwareRoot is not set."
  }
  $sendSerial = Join-Path $FirmwareRoot "tools\send_serial_and_capture.ps1"
  if (-not (Test-Path -LiteralPath $sendSerial)) {
    throw "Missing firmware serial helper: $sendSerial"
  }
  & pwsh -NoProfile -File $sendSerial `
    -Port $PortName `
    -Command "~DEVICE:SETTINGS" `
    -InitialReadMs 200 `
    -CommandReadMs 1200 `
    -CommandDelayMs 0 `
    -OutputPath $Path | Out-Null
  if ($LASTEXITCODE -ne 0) {
    throw "serial device settings helper exited with code $LASTEXITCODE"
  }
}

function Save-DesktopScreenshot {
  param([Parameter(Mandatory = $true)][string]$Path)
  Add-Type -AssemblyName System.Windows.Forms
  Add-Type -AssemblyName System.Drawing
  $bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
  $bitmap = [System.Drawing.Bitmap]::new($bounds.Width, $bounds.Height)
  $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
  try {
    $graphics.CopyFromScreen($bounds.Left, $bounds.Top, 0, 0, $bounds.Size)
    $bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
  } finally {
    $graphics.Dispose()
    $bitmap.Dispose()
  }
}

function Get-WindowsBleState {
  $pnp = @(Get-PnpDevice -ErrorAction SilentlyContinue | Where-Object {
      ([string]$_.InstanceId) -match "BTH|BTHLE|Bluetooth|HID\\\\.*BTH" -or
      ([string]$_.FriendlyName) -match "(?i)listener|blistener|bluetooth|hid keyboard|gatt"
    } | Sort-Object Class, FriendlyName, InstanceId | Select-Object Status, Class, FriendlyName, InstanceId)
  $services = @(Get-Service -ErrorAction SilentlyContinue | Where-Object {
      $_.Name -in @("bthserv", "DeviceAssociationService", "DeviceInstall", "hidserv")
    } | Select-Object Name, Status, StartType, DisplayName)
  return [ordered]@{
    generated_at = (Get-Date).ToString("o")
    expected_name = $ExpectedName
    pnp_count = $pnp.Count
    pnp = @($pnp)
    services = @($services)
  }
}

function Get-WindowsBleEvents {
  $logNames = @(
    "Microsoft-Windows-Bluetooth-BthLEPrepairing/Operational",
    "Microsoft-Windows-Bluetooth-User/Operational",
    "Microsoft-Windows-DeviceSetupManager/Admin"
  )
  $events = [System.Collections.Generic.List[object]]::new()
  foreach ($logName in $logNames) {
    try {
      $rows = @(Get-WinEvent -FilterHashtable @{ LogName = $logName; StartTime = (Get-Date).AddHours(-8) } -MaxEvents 50 -ErrorAction Stop)
      foreach ($row in $rows) {
        $events.Add([ordered]@{
            log_name = $logName
            time_created = $row.TimeCreated.ToString("o")
            id = $row.Id
            provider = $row.ProviderName
            level = $row.LevelDisplayName
            message = $row.Message
          }) | Out-Null
      }
    } catch {
      $events.Add([ordered]@{
          log_name = $logName
          unavailable = $true
          error = $_.Exception.Message
        }) | Out-Null
    }
  }
  return [ordered]@{
    generated_at = (Get-Date).ToString("o")
    events = @($events)
  }
}

function Invoke-ProcessCapture {
  param(
    [Parameter(Mandatory = $true)][string]$File,
    [string[]]$Arguments = @(),
    [Parameter(Mandatory = $true)][string]$Name,
    [int]$TimeoutMs = 8000
  )
  $stdoutPath = Join-Path $OutputDir "$Name.stdout.txt"
  $stderrPath = Join-Path $OutputDir "$Name.stderr.txt"
  $summaryFile = Join-Path $OutputDir "$Name.summary.json"
  $psi = [System.Diagnostics.ProcessStartInfo]::new()
  $psi.FileName = $File
  foreach ($arg in $Arguments) {
    $null = $psi.ArgumentList.Add($arg)
  }
  $psi.WorkingDirectory = Split-Path -Parent $File
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $psi.UseShellExecute = $false
  $psi.CreateNoWindow = $true
  $process = [System.Diagnostics.Process]::new()
  $process.StartInfo = $psi
  $startedAt = Get-Date
  $null = $process.Start()
  $stdoutTask = $process.StandardOutput.ReadToEndAsync()
  $stderrTask = $process.StandardError.ReadToEndAsync()
  $finished = $process.WaitForExit($TimeoutMs)
  if (-not $finished) {
    try {
      $process.Kill($true)
    } catch {
    }
  }
  $stdout = $stdoutTask.GetAwaiter().GetResult()
  $stderr = $stderrTask.GetAwaiter().GetResult()
  Write-TextFile -Path $stdoutPath -Text $stdout
  Write-TextFile -Path $stderrPath -Text $stderr
  $result = [ordered]@{
    generated_at = (Get-Date).ToString("o")
    started_at = $startedAt.ToString("o")
    file = $File
    arguments = $Arguments
    timeout_ms = $TimeoutMs
    timed_out = -not $finished
    exit_code = if ($finished) { $process.ExitCode } else { $null }
    stdout = Get-RelativeEvidencePath $stdoutPath
    stderr = Get-RelativeEvidencePath $stderrPath
  }
  Write-JsonFile -Path $summaryFile -Value $result -Depth 6
  return [pscustomobject]@{
    path = $summaryFile
    ok = ($finished -and $process.ExitCode -eq 0)
    result = $result
  }
}

function Get-ReleaseArtifacts {
  $typeMsi = Join-Path $repoRoot ".artifacts\windows-msvc\ListenerType_1.0.2_x64_en-US.msi"
  $rootMsi = Join-Path $RepoBaseRoot "ListenerType_1.0.2_x64_en-US.msi"
  $rootFirmwareZip = Join-Path $RepoBaseRoot "ListenerFirmware_1.0.2_ota.zip"
  $firmwareZip = $null
  if (-not [string]::IsNullOrWhiteSpace($FirmwareRoot) -and (Test-Path -LiteralPath $FirmwareRoot)) {
    $firmwareZip = @(Get-ChildItem -LiteralPath (Join-Path $FirmwareRoot ".cache\ota_firmware") -File -Filter "*.zip" -ErrorAction SilentlyContinue |
      Sort-Object LastWriteTime -Descending |
      Select-Object -First 1)
  }
  $expectedRootNames = @(
    [System.IO.Path]::GetFileName($rootMsi),
    [System.IO.Path]::GetFileName($rootFirmwareZip)
  )
  $rootPackageFiles = @(
    Get-ChildItem -LiteralPath $RepoBaseRoot -File -ErrorAction SilentlyContinue |
      Where-Object { $_.Name -match '^(ListenerType_|ListenerFirmware_).*\.(msi|zip)$' }
  )
  $forbiddenRootPackages = @(
    $rootPackageFiles |
      Where-Object {
        $_.Name -notin $expectedRootNames -or
        $_.Name -match '(?i)portable'
      } |
      Select-Object -ExpandProperty FullName
  )
  function Get-FileSha256OrEmpty {
    param([AllowNull()][string]$Path)
    if ([string]::IsNullOrWhiteSpace($Path) -or -not (Test-Path -LiteralPath $Path)) {
      return ""
    }
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash
  }
  $typeSourceHash = Get-FileSha256OrEmpty $typeMsi
  $rootMsiHash = Get-FileSha256OrEmpty $rootMsi
  $firmwareSourceHash = if ($firmwareZip) { Get-FileSha256OrEmpty $firmwareZip.FullName } else { "" }
  $rootFirmwareHash = Get-FileSha256OrEmpty $rootFirmwareZip
  $typeSourceHashMatches = -not [string]::IsNullOrWhiteSpace($typeSourceHash) -and $typeSourceHash -eq $rootMsiHash
  $firmwareSourceHashMatches = -not [string]::IsNullOrWhiteSpace($firmwareSourceHash) -and $firmwareSourceHash -eq $rootFirmwareHash
  $files = @($typeMsi, $rootMsi, $rootFirmwareZip)
  if ($firmwareZip) {
    $files += $firmwareZip.FullName
  }
  $entries = foreach ($file in $files) {
    if ([string]::IsNullOrWhiteSpace($file)) { continue }
    if (Test-Path -LiteralPath $file) {
      $item = Get-Item -LiteralPath $file
      [ordered]@{
        path = $item.FullName
        name = $item.Name
        length = $item.Length
        last_write_time = $item.LastWriteTime.ToString("o")
        sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $item.FullName).Hash
      }
    } else {
      [ordered]@{
        path = $file
        missing = $true
      }
    }
  }
  return [ordered]@{
    generated_at = (Get-Date).ToString("o")
    repo_base_root = $RepoBaseRoot
    firmware_root = $FirmwareRoot
    entries = @($entries)
    all_expected_present = @($entries | Where-Object {
        $property = $_.PSObject.Properties["missing"]
        $null -ne $property -and [bool]$property.Value
      }).Count -eq 0
    all_latest_sources_staged = @($entries | Where-Object {
        $property = $_.PSObject.Properties["missing"]
        $null -ne $property -and [bool]$property.Value
      }).Count -eq 0 -and
      $forbiddenRootPackages.Count -eq 0 -and
      $typeSourceHashMatches -and
      $firmwareSourceHashMatches
    forbidden_root_packages = @($forbiddenRootPackages)
    type_source_hash_matches_root = $typeSourceHashMatches
    firmware_source_hash_matches_root = $firmwareSourceHashMatches
    type_source_hash = $typeSourceHash
    root_msi_hash = $rootMsiHash
    firmware_source_hash = $firmwareSourceHash
    root_firmware_hash = $rootFirmwareHash
    type_msi = $typeMsi
    firmware_zip = if ($firmwareZip) { $firmwareZip.FullName } else { "" }
    root_msi = $rootMsi
    root_firmware_zip = $rootFirmwareZip
  }
}

$manifest = [ordered]@{
  schema_version = 1
  purpose = "Listener 1.0.2 preproduction bench collected local evidence"
  generated_at = (Get-Date).ToString("o")
  capabilities = [ordered]@{
    type_runtime = $false
    desktop_visual_capture = $false
    windows_ble_automation = $false
    second_ble_host = $false
    usb_power_relay = $false
    power_relay = $false
    physical_input_fixture = $false
    led_optical_capture = $false
    audio_fixture = $false
    wired_flash_port = $false
    ota_package = $false
    release_artifacts = $false
  }
  evidence = [ordered]@{}
}

$notes = [System.Collections.Generic.List[string]]::new()

$typeProcessPath = Join-Path $OutputDir "type-process.json"
$typeProcesses = @(Get-TypeProcesses)
Write-JsonFile -Path $typeProcessPath -Value ([ordered]@{
    generated_at = (Get-Date).ToString("o")
    type_exe = $TypeExe
    process_count = $typeProcesses.Count
    processes = @($typeProcesses)
  }) -Depth 8
$manifest.capabilities.type_runtime = $typeProcesses.Count -gt 0
if (-not $manifest.capabilities.type_runtime) {
  $notes.Add("type_runtime capability is false because no running listener-type process was found.") | Out-Null
}

$typeLog = Get-TypeLogTail
$typeLogPath = Join-Path $OutputDir "type-log-tail.txt"
Write-TextFile -Path $typeLogPath -Text $typeLog.tail
if (-not $typeLog.exists) {
  $notes.Add("Type log was not found at $($typeLog.path).") | Out-Null
}

$screenshotPath = Join-Path $OutputDir "desktop-screenshot.png"
if (-not $SkipScreenshot.IsPresent) {
  try {
    Save-DesktopScreenshot -Path $screenshotPath
    $manifest.capabilities.desktop_visual_capture = Test-Path -LiteralPath $screenshotPath
  } catch {
    $errorPath = Join-Path $OutputDir "desktop-screenshot-error.txt"
    Write-TextFile -Path $errorPath -Text $_.Exception.ToString()
    $notes.Add("desktop screenshot failed: $($_.Exception.Message)") | Out-Null
  }
} else {
  $notes.Add("desktop screenshot skipped by -SkipScreenshot.") | Out-Null
}

$bleStatePath = Join-Path $OutputDir "windows-ble-state.json"
Write-JsonFile -Path $bleStatePath -Value (Get-WindowsBleState) -Depth 10
$bleEventsPath = Join-Path $OutputDir "windows-ble-events.json"
Write-JsonFile -Path $bleEventsPath -Value (Get-WindowsBleEvents) -Depth 8

$notificationScanPath = ""
if (-not $SkipNotificationScan.IsPresent) {
  $notificationScanPath = Join-Path $OutputDir "windows-ble-notification-scan.json"
  try {
    & pwsh -NoProfile -File (Join-Path $PSScriptRoot "windows-ble-notification-helper.ps1") -Action List -TargetName $ExpectedName -TimeoutSeconds 3 -OutputPath $notificationScanPath | Out-Null
  } catch {
    $notificationError = Join-Path $OutputDir "windows-ble-notification-scan-error.txt"
    Write-TextFile -Path $notificationError -Text $_.Exception.ToString()
    $notes.Add("notification scan failed: $($_.Exception.Message)") | Out-Null
  }
} else {
  $notes.Add("Windows notification scan skipped by -SkipNotificationScan.") | Out-Null
}

$serialLedStatusPath = ""
$serialDeviceSettingsPath = ""
$resolvedSerialPort = Resolve-SerialPortName
if (-not $SkipSerialSnapshot.IsPresent -and -not [string]::IsNullOrWhiteSpace($resolvedSerialPort)) {
  $serialLedStatusPath = Join-Path $OutputDir "serial-led-status.txt"
  $serialDeviceSettingsPath = Join-Path $OutputDir "serial-device-settings.txt"
  try {
    Invoke-SerialLedStatusSnapshot -Path $serialLedStatusPath -PortName $resolvedSerialPort
    Invoke-SerialDeviceSettingsSnapshot -Path $serialDeviceSettingsPath -PortName $resolvedSerialPort
    $manifest.capabilities.wired_flash_port = $true
    $notes.Add("serial LED status and device settings captured from $resolvedSerialPort; wired_flash_port capability records serial port availability only, not a wired flash smoke PASS.") | Out-Null
  } catch {
    $serialErrorPath = Join-Path $OutputDir "serial-led-status-error.txt"
    Write-TextFile -Path $serialErrorPath -Text $_.Exception.ToString()
    $notes.Add("serial LED/device settings snapshot failed on $resolvedSerialPort`: $($_.Exception.Message)") | Out-Null
  }
} elseif ($SkipSerialSnapshot.IsPresent) {
  $notes.Add("serial LED/device settings snapshot skipped by -SkipSerialSnapshot.") | Out-Null
} else {
  $notes.Add("serial LED/device settings snapshot skipped because no unique serial port was resolved.") | Out-Null
}

$audioStatus = $null
$otaProbe = $null
if (-not $SkipLiveBle.IsPresent -and -not [string]::IsNullOrWhiteSpace($TypeExe) -and (Test-Path -LiteralPath $TypeExe)) {
  $audioStatus = Invoke-ProcessCapture -File $TypeExe -Arguments @("--read-embedded-audio-ble-status", [string]$LiveBleTimeoutMs) -Name "ble-audio-status" -TimeoutMs ([Math]::Max(4000, $LiveBleTimeoutMs + 4000))
  $otaProbe = Invoke-ProcessCapture -File $TypeExe -Arguments @("--probe-listener-ota-v2-gatt", [string]$LiveBleTimeoutMs) -Name "ota-v2-gatt-probe" -TimeoutMs ([Math]::Max(4000, $LiveBleTimeoutMs + 4000))
} else {
  $notes.Add("live BLE CLI probes skipped or release exe missing.") | Out-Null
}

$releaseArtifactsPath = Join-Path $OutputDir "release-artifacts.json"
$releaseArtifacts = Get-ReleaseArtifacts
Write-JsonFile -Path $releaseArtifactsPath -Value $releaseArtifacts -Depth 8
$manifest.capabilities.release_artifacts = [bool]$releaseArtifacts.all_latest_sources_staged
$manifest.capabilities.ota_package = -not [string]::IsNullOrWhiteSpace([string]$releaseArtifacts.firmware_zip)

$activeBle = $null
$resolvedActiveBleSummaryPath = ""
if (-not [string]::IsNullOrWhiteSpace($ActiveBleSummaryPath)) {
  try {
    $resolvedActiveBleSummaryPath = (Resolve-Path -LiteralPath $ActiveBleSummaryPath).Path
    $activeBle = Get-Content -LiteralPath $resolvedActiveBleSummaryPath -Raw | ConvertFrom-Json
    if ($activeBle.status -eq "WINDOWS_BLE_AUTOMATION_PASS") {
      $manifest.capabilities.windows_ble_automation = $true
      $notes.Add("windows_ble_automation capability accepted from active BLE summary: $resolvedActiveBleSummaryPath") | Out-Null
    } else {
      $notes.Add("Active BLE summary was provided but not PASS: status=$($activeBle.status)") | Out-Null
    }
  } catch {
    $notes.Add("Active BLE summary could not be read: $($_.Exception.Message)") | Out-Null
  }
}

Add-Evidence -Manifest $manifest -StepId "baseline-type-tray-ui" -Key "type_process" -Path $typeProcessPath
Add-Evidence -Manifest $manifest -StepId "baseline-type-tray-ui" -Key "window_screenshot" -Path $screenshotPath
Add-Evidence -Manifest $manifest -StepId "baseline-type-tray-ui" -Key "type_log" -Path $typeLogPath
Add-Evidence -Manifest $manifest -StepId "same-name-write-no-repair" -Key "before_ble_state" -Path $bleStatePath
Add-Evidence -Manifest $manifest -StepId "same-name-write-no-repair" -Key "after_ble_state" -Path (Get-ActiveBleEvidencePath -Summary $activeBle -Key "after_ble_state")
Add-Evidence -Manifest $manifest -StepId "same-name-write-no-repair" -Key "type_log" -Path $typeLogPath
Add-Evidence -Manifest $manifest -StepId "random-name-exact-cache-refresh" -Key "windows_ble_state" -Path $bleStatePath
Add-Evidence -Manifest $manifest -StepId "restore-default-listener" -Key "windows_ble_state" -Path $bleStatePath
Add-Evidence -Manifest $manifest -StepId "manual-windows-delete-no-type-autopair" -Key "twenty_second_state" -Path $bleStatePath
Add-Evidence -Manifest $manifest -StepId "manual-windows-delete-no-type-autopair" -Key "type_log" -Path $typeLogPath
Add-Evidence -Manifest $manifest -StepId "no-type-native-pairing" -Key "windows_ble_state" -Path $bleStatePath
Add-Evidence -Manifest $manifest -StepId "no-type-native-pairing" -Key "hid_presence" -Path $bleStatePath
Add-Evidence -Manifest $manifest -StepId "type-takeover-no-forced-repair" -Key "takeover_log" -Path $(if ($activeBle) { $resolvedActiveBleSummaryPath } else { "" })
Add-Evidence -Manifest $manifest -StepId "type-takeover-no-forced-repair" -Key "gatt_probe" -Path $(if ($audioStatus) { $audioStatus.path } else { "" })
Add-Evidence -Manifest $manifest -StepId "type-takeover-no-forced-repair" -Key "gatt_probe" -Path (Get-ActiveBleRecordEvidence -Summary $activeBle -Stage "gatt_audio_status")
Add-Evidence -Manifest $manifest -StepId "ec11-long-press-shutdown-led" -Key "type_log" -Path $typeLogPath
Add-Evidence -Manifest $manifest -StepId "ec11-rotate-ring-feedback" -Key "serial_led_status" -Path $serialLedStatusPath
Add-Evidence -Manifest $manifest -StepId "ec11-single-not-double" -Key "windows_ble_events" -Path $bleEventsPath
Add-Evidence -Manifest $manifest -StepId "ec11-double-repair-with-type" -Key "type_unpair_log" -Path $(if ($activeBle) { $resolvedActiveBleSummaryPath } else { "" })
Add-Evidence -Manifest $manifest -StepId "ec11-double-repair-with-type" -Key "windows_native_pair_log" -Path $(if ($activeBle) { $resolvedActiveBleSummaryPath } else { "" })
Add-Evidence -Manifest $manifest -StepId "ec11-double-repair-with-type" -Key "gatt_probe" -Path (Get-ActiveBleRecordEvidence -Summary $activeBle -Stage "gatt_audio_status")
Add-Evidence -Manifest $manifest -StepId "ble-audio-type-link" -Key "ble_audio_probe" -Path $(if ($audioStatus) { $audioStatus.path } else { "" })
Add-Evidence -Manifest $manifest -StepId "ble-audio-type-link" -Key "type_log" -Path $typeLogPath
Add-Evidence -Manifest $manifest -StepId "ble-audio-type-link" -Key "windows_ble_state" -Path $bleStatePath
Add-Evidence -Manifest $manifest -StepId "led-independent-contract" -Key "serial_led_status" -Path $serialLedStatusPath
Add-Evidence -Manifest $manifest -StepId "type-brightness-low-power-sync" -Key "type_log" -Path $typeLogPath
Add-Evidence -Manifest $manifest -StepId "type-brightness-low-power-sync" -Key "device_settings_readback" -Path $serialDeviceSettingsPath
Add-Evidence -Manifest $manifest -StepId "ota-wireless-smoke" -Key "ota_probe" -Path $(if ($otaProbe) { $otaProbe.path } else { "" })
Add-Evidence -Manifest $manifest -StepId "ota-wireless-smoke" -Key "ota_log" -Path $(if ($otaProbe) { $otaProbe.path } else { "" })
Add-Evidence -Manifest $manifest -StepId "release-package-final-check" -Key "msi_hash" -Path $releaseArtifactsPath
Add-Evidence -Manifest $manifest -StepId "release-package-final-check" -Key "firmware_zip_hash" -Path $releaseArtifactsPath
Add-Evidence -Manifest $manifest -StepId "release-package-final-check" -Key "root_package_listing" -Path $releaseArtifactsPath
Add-Evidence -Manifest $manifest -StepId "release-package-final-check" -Key "source_hash_comparison" -Path $releaseArtifactsPath

Write-JsonFile -Path $manifestPath -Value $manifest -Depth 12

$reviewExit = $null
if (-not $NoReview.IsPresent) {
  & pwsh -NoProfile -File (Join-Path $PSScriptRoot "windows-listener-preproduction-bench-review.ps1") -CapabilityManifest $manifestPath -OutputDir $OutputDir | Out-Null
  $reviewExit = $LASTEXITCODE
  if ($reviewExit -ne 0 -and $reviewExit -ne 2) {
    throw "bench review exited with unexpected code $reviewExit"
  }
}

$reviewStatus = ""
if (Test-Path -LiteralPath $reviewSummaryPath) {
  try {
    $reviewStatus = [string](Get-Content -LiteralPath $reviewSummaryPath -Raw | ConvertFrom-Json).status
  } catch {
  }
}

$summary = [ordered]@{
  schema_version = 1
  status = "BENCH_COLLECT_COMPLETE"
  generated_at = (Get-Date).ToString("o")
  repo_root = $repoRoot
  repo_base_root = $RepoBaseRoot
  firmware_root = $FirmwareRoot
  type_exe = $TypeExe
  active_ble_summary = $resolvedActiveBleSummaryPath
  output_dir = $OutputDir
  manifest = $manifestPath
  review_summary = if (Test-Path -LiteralPath $reviewSummaryPath) { $reviewSummaryPath } else { "" }
  review_status = $reviewStatus
  capabilities = $manifest.capabilities
  notes = @($notes)
}
Write-JsonFile -Path $summaryPath -Value $summary -Depth 8

Write-Host "bench_collect_status=BENCH_COLLECT_COMPLETE"
Write-Host "manifest=$manifestPath"
Write-Host "summary=$summaryPath"
if ($reviewStatus) {
  Write-Host "bench_review_status=$reviewStatus"
  Write-Host "bench_review_summary=$reviewSummaryPath"
}
