[CmdletBinding(PositionalBinding = $false)]
param(
    [string]$Scenario = "manual",
    [datetime]$Since = (Get-Date).AddMinutes(-10),
    [string]$OutDir = ".artifacts\ble_pairing_evidence",
    [string]$Port = "COM3",
    [string]$FirmwareRepo = "..\Listener-Firmware",
    [switch]$SkipSerial,
    [string]$Observation = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

function Resolve-RepoPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return $Path
    }
    return [System.IO.Path]::GetFullPath((Join-Path $RepoRoot $Path))
}

function New-SafeName {
    param([Parameter(Mandatory = $true)][string]$Name)
    $safe = $Name -replace '[^A-Za-z0-9_.-]+', '_'
    $safe = $safe.Trim('_')
    if ([string]::IsNullOrWhiteSpace($safe)) {
        return "manual"
    }
    return $safe
}

function Add-Section {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Title
    )
    Add-Content -LiteralPath $Path -Encoding UTF8 -Value ""
    Add-Content -LiteralPath $Path -Encoding UTF8 -Value ("=== {0} ===" -f $Title)
}

function Export-CommandText {
    param(
        [Parameter(Mandatory = $true)][scriptblock]$ScriptBlock,
        [Parameter(Mandatory = $true)][string]$Path
    )
    try {
        & $ScriptBlock | Out-String -Width 280 | Set-Content -LiteralPath $Path -Encoding UTF8
    } catch {
        ("COMMAND_FAILED: {0}" -f $_.Exception.Message) | Set-Content -LiteralPath $Path -Encoding UTF8
    }
}

function Export-TypeLogSince {
    param(
        [Parameter(Mandatory = $true)][datetime]$StartTime,
        [Parameter(Mandatory = $true)][string]$Path
    )

    $logPaths = [System.Collections.Generic.List[string]]::new()
    $appLogPath = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
    if (Test-Path -LiteralPath $appLogPath) {
        $logPaths.Add($appLogPath) | Out-Null
    }
    $testLogRoot = Join-Path ([System.IO.Path]::GetTempPath()) "listener-type-test-logs"
    if (Test-Path -LiteralPath $testLogRoot) {
        Get-ChildItem -LiteralPath $testLogRoot -Recurse -Filter "listener-type.log" -ErrorAction SilentlyContinue |
            ForEach-Object { $logPaths.Add($_.FullName) | Out-Null }
    }
    if ($logPaths.Count -eq 0) {
        "listener-type.log not found in app log or test log roots: $appLogPath ; $testLogRoot" |
            Set-Content -LiteralPath $Path -Encoding UTF8
        return
    }

    $sinceUtc = [datetimeoffset]$StartTime.ToUniversalTime()
    $lines = [System.Collections.Generic.List[string]]::new()
    foreach ($logPath in $logPaths) {
        $keepContinuation = $false
        $fileLines = [System.Collections.Generic.List[string]]::new()
        $stream = [System.IO.File]::Open(
            $logPath,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            [System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete
        )
        try {
            $reader = [System.IO.StreamReader]::new($stream, [System.Text.Encoding]::UTF8, $true)
            try {
                while (-not $reader.EndOfStream) {
                    $line = $reader.ReadLine()
                    $timestamp = $null
                    $hasTimestamp = $line -match '^(\d{4}-\d{2}-\d{2}T[0-9:.]+Z)'
                    if ($hasTimestamp) {
                        $parsed = [datetimeoffset]::MinValue
                        if ([datetimeoffset]::TryParse($Matches[1], [ref]$parsed)) {
                            $timestamp = $parsed
                        }
                    }
                    if ($timestamp -ne $null) {
                        $keepContinuation = $timestamp -ge $sinceUtc
                    }
                    if ($keepContinuation) {
                        $fileLines.Add($line) | Out-Null
                    }
                }
            } finally {
                $reader.Dispose()
            }
        } finally {
            $stream.Dispose()
        }
        if ($fileLines.Count -gt 0) {
            $lines.Add(("--- {0} ---" -f $logPath)) | Out-Null
            foreach ($line in $fileLines) {
                $lines.Add($line) | Out-Null
            }
        }
    }

    if ($lines.Count -eq 0) {
        ("No listener-type.log lines since {0:o} ({1})" -f $sinceUtc, ($logPaths -join "; ")) |
            Set-Content -LiteralPath $Path -Encoding UTF8
    } else {
        $lines | Set-Content -LiteralPath $Path -Encoding UTF8
    }
}

function Export-WindowsEventsSince {
    param(
        [Parameter(Mandatory = $true)][datetime]$StartTime,
        [Parameter(Mandatory = $true)][string]$Path
    )

    ("Windows BLE/pairing events since {0:o}" -f $StartTime) |
        Set-Content -LiteralPath $Path -Encoding UTF8

    Add-Section -Path $Path -Title "Available logs"
    Get-WinEvent -ListLog '*Bluetooth*', '*Bth*', '*DeviceAssociation*' -ErrorAction SilentlyContinue |
        Sort-Object LogName |
        Select-Object LogName, RecordCount, IsEnabled |
        Format-Table -AutoSize |
        Out-String -Width 220 |
        Add-Content -LiteralPath $Path -Encoding UTF8

    Add-Section -Path $Path -Title "System providers"
    foreach ($provider in @(
        "BTHUSB",
        "Microsoft-Windows-DeviceAssociationService",
        "Microsoft-Windows-DeviceSetupManager"
    )) {
        Add-Section -Path $Path -Title ("System / {0}" -f $provider)
        try {
            Get-WinEvent -FilterHashtable @{
                LogName = "System"
                ProviderName = $provider
                StartTime = $StartTime
            } -ErrorAction Stop |
                Sort-Object TimeCreated |
                Select-Object TimeCreated, Id, LevelDisplayName, ProviderName,
                    @{ Name = "Message"; Expression = { ($_.Message -replace '\s+', ' ').Trim() } } |
                Format-List |
                Out-String -Width 280 |
                Add-Content -LiteralPath $Path -Encoding UTF8
        } catch {
            ("No events or query failed: {0}" -f $_.Exception.Message) |
                Add-Content -LiteralPath $Path -Encoding UTF8
        }
    }

    Add-Section -Path $Path -Title "Operational channels"
    foreach ($log in @(
        "Microsoft-Windows-Bluetooth-Policy/Operational",
        "Microsoft-Windows-Bluetooth-Bthmini/Operational",
        "Microsoft-Windows-Bluetooth-BthLEPrepairing/Operational"
    )) {
        Add-Section -Path $Path -Title $log
        try {
            Get-WinEvent -FilterHashtable @{
                LogName = $log
                StartTime = $StartTime
            } -ErrorAction Stop |
                Sort-Object TimeCreated |
                Select-Object TimeCreated, Id, LevelDisplayName, ProviderName,
                    @{ Name = "Message"; Expression = { ($_.Message -replace '\s+', ' ').Trim() } } |
                Format-List |
                Out-String -Width 280 |
                Add-Content -LiteralPath $Path -Encoding UTF8
        } catch {
            ("No events or query failed: {0}" -f $_.Exception.Message) |
                Add-Content -LiteralPath $Path -Encoding UTF8
        }
    }
}

$safeScenario = New-SafeName -Name $Scenario
$stamp = Get-Date -Format "yyyyMMdd_HHmmss"
$rootOutDir = Resolve-RepoPath $OutDir
$runDir = Join-Path $rootOutDir ("{0}_{1}" -f $stamp, $safeScenario)
New-Item -ItemType Directory -Force -Path $runDir | Out-Null

$manifestPath = Join-Path $runDir "manifest.json"
$summaryPath = Join-Path $runDir "summary.txt"
$typeLogPath = Join-Path $runDir "listener-type.since.log"
$windowsEventsPath = Join-Path $runDir "windows-events.txt"
$pnpPath = Join-Path $runDir "windows-pnp.txt"
$processPath = Join-Path $runDir "listener-type-process.txt"
$serialPath = Join-Path $runDir "firmware-serial.txt"

$sinceLocal = $Since
$sinceUtc = $Since.ToUniversalTime()

("scenario={0}" -f $Scenario) | Set-Content -LiteralPath $summaryPath -Encoding UTF8
("since_local={0:o}" -f $sinceLocal) | Add-Content -LiteralPath $summaryPath -Encoding UTF8
("since_utc={0:o}" -f $sinceUtc) | Add-Content -LiteralPath $summaryPath -Encoding UTF8
if (-not [string]::IsNullOrWhiteSpace($Observation)) {
    ("observation={0}" -f $Observation) | Add-Content -LiteralPath $summaryPath -Encoding UTF8
}

Export-CommandText -Path $processPath -ScriptBlock {
    Get-Process -Name listener-type -ErrorAction SilentlyContinue |
        Select-Object Id, ProcessName, Path, StartTime |
        Format-List
}

Export-CommandText -Path $pnpPath -ScriptBlock {
    Get-PnpDevice |
        Where-Object {
            $_.InstanceId -match 'A4CB8FF2B512|FF01577C05E0|BTHLE|BTHENUM' -and
            ($_.FriendlyName -match 'listener|Blistener|HID Keyboard|Bluetooth|BLE' -or $_.InstanceId -match 'A4CB8FF2B512|FF01577C05E0')
        } |
        Sort-Object InstanceId |
        Select-Object Status, Class, FriendlyName, InstanceId |
        Format-Table -AutoSize -Wrap
}

Export-TypeLogSince -StartTime $Since -Path $typeLogPath
Export-WindowsEventsSince -StartTime $Since -Path $windowsEventsPath

if (-not $SkipSerial) {
    $firmwareRoot = Resolve-RepoPath $FirmwareRepo
    $serialScript = Join-Path $firmwareRoot "tools\send_serial_and_capture.ps1"
    if (Test-Path -LiteralPath $serialScript) {
        try {
            & pwsh -NoProfile -File $serialScript `
                -Port $Port `
                -Baud 115200 `
                -InitialReadMs 300 `
                -CommandReadMs 1200 `
                -CommandList "~POWER:STATUS;;~LED:STATUS;;~DEVICE:SETTINGS;;~OTA:STATUS;;~DIAGLOG:LAST:80:ble_gap;;~DIAGLOG:LAST:80:ble_hid;;~DIAGLOG:LAST:80:status_led" `
                -OutputPath $serialPath | Out-Null
        } catch {
            ("SERIAL_CAPTURE_FAILED: {0}" -f $_.Exception.Message) |
                Set-Content -LiteralPath $serialPath -Encoding UTF8
        }
    } else {
        ("serial helper not found: {0}" -f $serialScript) |
            Set-Content -LiteralPath $serialPath -Encoding UTF8
    }
} else {
    "serial capture skipped" | Set-Content -LiteralPath $serialPath -Encoding UTF8
}

$typeLogText = Get-Content -LiteralPath $typeLogPath -Raw -ErrorAction SilentlyContinue
$windowsEventText = Get-Content -LiteralPath $windowsEventsPath -Raw -ErrorAction SilentlyContinue
$serialText = Get-Content -LiteralPath $serialPath -Raw -ErrorAction SilentlyContinue
$windowsPairSuccessCount = [regex]::Matches(
    $windowsEventText,
    '(?m)^Message\s+:\s+.*(?:成功地与本地适配器配对|设备关联\(配对\)成功|successfully paired|Device association.*success).*$',
    'IgnoreCase'
).Count
$windowsUnpairCount = [regex]::Matches(
    $windowsEventText,
    '(?m)^Message\s+:\s+.*(?:链接密钥|link key|不再是成对|no longer paired).*$',
    'IgnoreCase'
).Count
$windowsPairFailureCount = [regex]::Matches(
    $windowsEventText,
    '(?m)^Message\s+:\s+.*(?:连接失败|无法连接|failed to pair|pairing failed|authentication request.*rejected|身份验证请求被拒绝).*$',
    'IgnoreCase'
).Count

Add-Section -Path $summaryPath -Title "Highlights"
@(
    ("type_notify_ready={0}" -f ([bool]($typeLogText -match 'background listener notify ready|notify CCCD enabled'))),
    ("type_heartbeat_ready={0}" -f ([bool]($typeLogText -match 'Type heartbeat ready sent'))),
    ("type_pairing_prompt_count={0}" -f ([regex]::Matches($typeLogText, 'prompt|pairing|PairAsync|CustomPairing|unpairing Windows BLE device cache', 'IgnoreCase').Count)),
    ("windows_pair_success_count={0}" -f $windowsPairSuccessCount),
    ("windows_unpair_count={0}" -f $windowsUnpairCount),
    ("windows_pair_failure_count={0}" -f $windowsPairFailureCount),
    ("firmware_type_ready={0}" -f ([bool]($serialText -match 'ble=type_ready|type_ready'))),
    ("firmware_ble_repair_active={0}" -f ([bool]($serialText -match 'ble_repair_ms_left=[1-9][0-9]*|ble_repair_cue_ms_left=[1-9][0-9]*'))),
    ("firmware_ble_name_pending={0}" -f ([bool]($serialText -match 'ble_name_pending=1')))
) | Add-Content -LiteralPath $summaryPath -Encoding UTF8

$manifest = [PSCustomObject][ordered]@{
    scenario = $Scenario
    captured_at = (Get-Date).ToUniversalTime().ToString("o")
    since_local = $sinceLocal.ToString("o")
    since_utc = $sinceUtc.ToString("o")
    observation = $Observation
    run_dir = $runDir
    files = [PSCustomObject][ordered]@{
        summary = $summaryPath
        type_log = $typeLogPath
        windows_events = $windowsEventsPath
        windows_pnp = $pnpPath
        type_process = $processPath
        firmware_serial = $serialPath
    }
}
$manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $manifestPath -Encoding UTF8

Write-Output $runDir
Write-Output $summaryPath
