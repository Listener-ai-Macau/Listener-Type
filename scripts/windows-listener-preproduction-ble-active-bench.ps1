[CmdletBinding(PositionalBinding = $false)]
param(
  [string]$OutputDir = "",
  [string]$TypeExe = "",
  [string]$ExpectedName = "listener",
  [ValidateSet("DryRun", "CleanupOnly", "PairOnly", "CleanupThenPair")]
  [string]$Mode = "DryRun",
  [int]$PairTimeoutMs = 30000,
  [int]$GattTimeoutMs = 10000,
  [switch]$ClickWindowsNotification,
  [switch]$Execute
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
if ([string]::IsNullOrWhiteSpace($TypeExe)) {
  $TypeExe = "C:\Program Files\Listener Type\listener-type.exe"
}
if (-not [string]::IsNullOrWhiteSpace($TypeExe) -and (Test-Path -LiteralPath $TypeExe)) {
  $TypeExe = (Resolve-Path -LiteralPath $TypeExe).Path
}
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
  $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
  $OutputDir = Join-Path $repoRoot ".cache\validation\preproduction-ble-active-$stamp"
} elseif (-not [System.IO.Path]::IsPathRooted($OutputDir)) {
  $OutputDir = Join-Path $repoRoot $OutputDir
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$OutputDir = (Resolve-Path -LiteralPath $OutputDir).Path

$summaryPath = Join-Path $OutputDir "preproduction-ble-active-summary.json"

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
  if ([string]::IsNullOrWhiteSpace($Path) -or -not (Test-Path -LiteralPath $Path)) {
    return ""
  }
  return [System.IO.Path]::GetRelativePath($OutputDir, (Resolve-Path -LiteralPath $Path).Path)
}

function Get-WindowsBleState {
  $pnp = @(Get-PnpDevice -ErrorAction SilentlyContinue | Where-Object {
      ([string]$_.InstanceId) -match "BTH|BTHLE|Bluetooth|HID\\\\.*BTH" -or
      ([string]$_.FriendlyName) -match "(?i)listener|blistener|bluetooth|hid keyboard|gatt"
    } | Sort-Object Class, FriendlyName, InstanceId | Select-Object Status, Class, FriendlyName, InstanceId)
  return [ordered]@{
    generated_at = (Get-Date).ToString("o")
    expected_name = $ExpectedName
    pnp_count = $pnp.Count
    pnp = @($pnp)
  }
}

function Invoke-ProcessCapture {
  param(
    [Parameter(Mandatory = $true)][string]$File,
    [string[]]$Arguments = @(),
    [Parameter(Mandatory = $true)][string]$Name,
    [int]$TimeoutMs = 15000
  )

  $stdoutPath = Join-Path $OutputDir "$Name.stdout.txt"
  $stderrPath = Join-Path $OutputDir "$Name.stderr.txt"
  $summaryFile = Join-Path $OutputDir "$Name.summary.json"
  $psi = [System.Diagnostics.ProcessStartInfo]::new()
  $psi.FileName = $File
  foreach ($arg in $Arguments) {
    $null = $psi.ArgumentList.Add($arg)
  }
  if (Test-Path -LiteralPath $File) {
    $psi.WorkingDirectory = Split-Path -Parent (Resolve-Path -LiteralPath $File).Path
  }
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
    try { $process.Kill($true) } catch {}
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
    stdout_path = $stdoutPath
    stderr_path = $stderrPath
    stdout = $stdout
    stderr = $stderr
    timed_out = -not $finished
    exit_code = if ($finished) { $process.ExitCode } else { $null }
    ok = ($finished -and $process.ExitCode -eq 0)
  }
}

function Convert-HeadlessJsonLine {
  param(
    [AllowNull()][string]$Text,
    [Parameter(Mandatory = $true)][string]$Prefix
  )
  if ([string]::IsNullOrWhiteSpace($Text)) {
    return $null
  }
  foreach ($line in ($Text -split "`r?`n")) {
    if ($line.StartsWith($Prefix, [System.StringComparison]::Ordinal)) {
      $json = $line.Substring($Prefix.Length)
      try {
        return $json | ConvertFrom-Json
      } catch {
        return [pscustomobject]@{ json_error = $_.Exception.Message; raw = $json }
      }
    }
  }
  return $null
}

function Start-NotificationClickHelper {
  if (-not $ClickWindowsNotification.IsPresent) {
    return $null
  }
  $helper = Join-Path $PSScriptRoot "windows-ble-notification-helper.ps1"
  if (-not (Test-Path -LiteralPath $helper)) {
    throw "Windows BLE notification helper not found: $helper"
  }
  $outputPath = Join-Path $OutputDir "pair-notification-click.json"
  $stdoutPath = Join-Path $OutputDir "pair-notification-click.stdout.txt"
  $stderrPath = Join-Path $OutputDir "pair-notification-click.stderr.txt"
  $args = @(
    "-NoProfile",
    "-File", $helper,
    "-Action", "ClickConnect",
    "-TargetName", $ExpectedName,
    "-TimeoutSeconds", ([string]([Math]::Max(8, [int]($PairTimeoutMs / 1000)))),
    "-OutputPath", $outputPath
  )
  $process = Start-Process -FilePath "pwsh" -ArgumentList $args -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
  return [pscustomobject]@{
    process = $process
    output = $outputPath
    stdout = $stdoutPath
    stderr = $stderrPath
  }
}

function Stop-NotificationClickHelper {
  param([AllowNull()]$Helper)
  if ($null -eq $Helper) {
    return $null
  }
  $process = $Helper.process
  if (-not $process.WaitForExit(2000)) {
    try { $process.Kill($true) } catch {}
  }
  return [ordered]@{
    output = Get-RelativeEvidencePath $Helper.output
    stdout = Get-RelativeEvidencePath $Helper.stdout
    stderr = Get-RelativeEvidencePath $Helper.stderr
    exit_code = if ($process.HasExited) { $process.ExitCode } else { $null }
  }
}

$records = [System.Collections.Generic.List[object]]::new()
$notes = [System.Collections.Generic.List[string]]::new()

$beforeStatePath = Join-Path $OutputDir "before-windows-ble-state.json"
Write-JsonFile -Path $beforeStatePath -Value (Get-WindowsBleState) -Depth 10

if (-not $Execute.IsPresent -or $Mode -eq "DryRun") {
  $planned = @(
    [ordered]@{ stage = "before_state"; evidence = "before-windows-ble-state.json" },
    [ordered]@{ stage = "cleanup"; command = "$TypeExe --cleanup-embedded-ble-pairing $ExpectedName"; mutates_windows_pairing = $true },
    [ordered]@{ stage = "pair"; command = "$TypeExe --prompt-embedded-ble-pairing-only $ExpectedName"; mutates_windows_pairing = $true; can_click_windows_notification = $true },
    [ordered]@{ stage = "gatt_probe"; command = "$TypeExe --read-embedded-audio-ble-status $GattTimeoutMs"; mutates_windows_pairing = $false },
    [ordered]@{ stage = "after_state"; evidence = "after-windows-ble-state.json" }
  )
  $planPath = Join-Path $OutputDir "dry-run-plan.json"
  Write-JsonFile -Path $planPath -Value ([ordered]@{
      generated_at = (Get-Date).ToString("o")
      mode = $Mode
      execute = [bool]$Execute
      execute_switch = "-Execute"
      expected_name = $ExpectedName
      type_exe = $TypeExe
      planned_actions = $planned
    }) -Depth 8
  $notes.Add("Dry-run only. No Windows PairAsync/UnpairAsync path was executed; windows_ble_automation remains unproven.") | Out-Null
  $status = "WINDOWS_BLE_AUTOMATION_DRY_RUN"
} else {
  if ([string]::IsNullOrWhiteSpace($TypeExe) -or -not (Test-Path -LiteralPath $TypeExe)) {
    throw "Type release exe not found: $TypeExe"
  }
  $status = "WINDOWS_BLE_AUTOMATION_PASS"

  if ($Mode -in @("CleanupOnly", "CleanupThenPair")) {
    $cleanup = Invoke-ProcessCapture -File $TypeExe -Arguments @("--cleanup-embedded-ble-pairing", $ExpectedName) -Name "cleanup-pairing" -TimeoutMs 30000
    $cleanupJson = Convert-HeadlessJsonLine -Text $cleanup.stdout -Prefix "embedded_ble_cleanup_json="
    $cleanupStatus = if ($cleanup.ok) { "PASS" } else { "NO_GO" }
    if ($cleanupStatus -ne "PASS") { $status = "WINDOWS_BLE_AUTOMATION_NO_GO" }
    $records.Add([ordered]@{
        stage = "cleanup_pairing"
        status = $cleanupStatus
        command = "--cleanup-embedded-ble-pairing"
        evidence = $cleanup.path
        parsed = $cleanupJson
      }) | Out-Null
  }

  if ($Mode -in @("PairOnly", "CleanupThenPair")) {
    $helperRun = Start-NotificationClickHelper
    try {
      $pair = Invoke-ProcessCapture -File $TypeExe -Arguments @("--prompt-embedded-ble-pairing-only", $ExpectedName) -Name "pairing-prompt" -TimeoutMs ([Math]::Max(15000, $PairTimeoutMs + 5000))
    } finally {
      $notificationResult = Stop-NotificationClickHelper -Helper $helperRun
    }
    $pairJson = Convert-HeadlessJsonLine -Text $pair.stdout -Prefix "embedded_ble_pairing_prompt_json="
    $pairResultStatus = if ($null -ne $pairJson -and $null -ne $pairJson.PSObject.Properties["status"]) { [string]$pairJson.status } else { "" }
    $pairStatus = if ($pair.ok -and $pairResultStatus -in @("Paired", "AlreadyPaired")) { "PASS" } else { "NO_GO" }
    if ($pairStatus -ne "PASS") { $status = "WINDOWS_BLE_AUTOMATION_NO_GO" }
    $records.Add([ordered]@{
        stage = "pairing_prompt"
        status = $pairStatus
        command = "--prompt-embedded-ble-pairing-only"
        evidence = $pair.path
        notification = $notificationResult
        parsed = $pairJson
      }) | Out-Null
  }

  $gatt = Invoke-ProcessCapture -File $TypeExe -Arguments @("--read-embedded-audio-ble-status", [string]$GattTimeoutMs) -Name "gatt-audio-status" -TimeoutMs ([Math]::Max(6000, $GattTimeoutMs + 4000))
  $gattStatus = if ($gatt.ok) { "PASS" } else { "NO_GO" }
  if ($gattStatus -ne "PASS") { $status = "WINDOWS_BLE_AUTOMATION_NO_GO" }
  $records.Add([ordered]@{
      stage = "gatt_audio_status"
      status = $gattStatus
      command = "--read-embedded-audio-ble-status"
      evidence = $gatt.path
    }) | Out-Null
}

$afterStatePath = Join-Path $OutputDir "after-windows-ble-state.json"
Write-JsonFile -Path $afterStatePath -Value (Get-WindowsBleState) -Depth 10

$summary = [ordered]@{
  schema_version = 1
  status = $status
  generated_at = (Get-Date).ToString("o")
  repo_root = $repoRoot
  output_dir = $OutputDir
  mode = $Mode
  execute = [bool]$Execute
  expected_name = $ExpectedName
  type_exe = $TypeExe
  capabilities = [ordered]@{
    windows_ble_automation = ($status -eq "WINDOWS_BLE_AUTOMATION_PASS")
  }
  evidence = [ordered]@{
    before_ble_state = Get-RelativeEvidencePath $beforeStatePath
    after_ble_state = Get-RelativeEvidencePath $afterStatePath
  }
  records = @($records)
  notes = @($notes)
}
Write-JsonFile -Path $summaryPath -Value $summary -Depth 12

Write-Host "windows_ble_active_status=$status"
Write-Host "summary=$summaryPath"
if ($status -eq "WINDOWS_BLE_AUTOMATION_PASS" -or $status -eq "WINDOWS_BLE_AUTOMATION_DRY_RUN") {
  exit 0
}
exit 2
