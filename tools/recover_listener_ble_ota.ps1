[CmdletBinding(PositionalBinding = $false)]
param(
    [string]$DeviceName = "listener",
    [string]$BluetoothAddress = "",
    [string]$Port = "COM5",
    [string]$ManifestPath = "",
    [string]$FirmwarePath = "",
    [switch]$RunTransfer = $false,
    [switch]$KeepListenerTypeRunning = $false,
    [switch]$SkipWindowsBluetoothRestart = $false,
    [switch]$SkipGattProbe = $false,
    [int]$GattProbeSeconds = 12,
    [int]$SerialBaud = 115200,
    [int]$SerialReadSeconds = 4
)

$ErrorActionPreference = "Stop"

function Resolve-RepoRoot {
    return (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
}

function Resolve-FirmwareRepoRoot {
    $candidate = Join-Path (Split-Path -Parent (Resolve-RepoRoot)) "voice-keyboard-firmware"
    if (-not (Test-Path $candidate)) {
        throw "Unable to locate firmware repo at $candidate"
    }
    return (Resolve-Path $candidate).Path
}

function Normalize-BluetoothAddress {
    param([string]$Address)

    $normalized = ($Address -replace "[^0-9A-Fa-f]", "").ToUpperInvariant()
    if ([string]::IsNullOrWhiteSpace($normalized)) {
        return ""
    }
    if ($normalized.Length -ne 12) {
        throw "Bluetooth address must contain exactly 12 hex digits. Current value: $Address"
    }
    return $normalized
}

function Resolve-BluetoothAddress {
    param([string]$PreferredName, [string]$PreferredAddress)

    $normalized = Normalize-BluetoothAddress -Address $PreferredAddress
    if (-not [string]::IsNullOrWhiteSpace($normalized)) {
        return $normalized
    }

    $device = Get-PnpDevice -Class Bluetooth -ErrorAction SilentlyContinue |
        Where-Object { $_.FriendlyName -eq $PreferredName -and $_.InstanceId -match "DEV_([0-9A-Fa-f]{12})" } |
        Select-Object -First 1
    if ($null -eq $device) {
        return ""
    }
    return $Matches[1].ToUpperInvariant()
}

function Add-Step {
    param(
        [string]$Name,
        [string]$Status,
        [string]$Detail
    )

    $script:steps += [pscustomobject]@{
        name = $Name
        status = $Status
        detail = $Detail
        at = (Get-Date).ToString("o")
    }
    Write-Host "[$((Get-Date).ToString('HH:mm:ss'))] $Status $Name"
}

function Invoke-Step {
    param(
        [string]$Name,
        [scriptblock]$Body,
        [switch]$Optional = $false
    )

    Write-Host "[$((Get-Date).ToString('HH:mm:ss'))] START $Name"
    try {
        $output = & $Body 2>&1 | Out-String
        if ([string]::IsNullOrWhiteSpace($output)) {
            $output = "ok"
        }
        Add-Step -Name $Name -Status "PASS" -Detail $output.Trim()
        return $output
    } catch {
        $detail = $_.Exception.Message
        if ($Optional) {
            Add-Step -Name $Name -Status "SKIP" -Detail $detail
            return $detail
        }
        Add-Step -Name $Name -Status "FAIL" -Detail $detail
        throw
    }
}

function Get-ListenerBluetoothPnpText {
    param([string]$PreferredName, [string]$Address)

    $normalized = Normalize-BluetoothAddress -Address $Address
    $items = Get-PnpDevice -Class Bluetooth -ErrorAction SilentlyContinue |
        Where-Object {
            $_.FriendlyName -eq $PreferredName -or
            (-not [string]::IsNullOrWhiteSpace($normalized) -and $_.InstanceId -like "*$normalized*")
        } |
        Select-Object Status, FriendlyName, InstanceId

    if ($null -eq $items) {
        return "No matching Bluetooth PnP entries."
    }
    return ($items | Format-List | Out-String).Trim()
}

function Get-SerialPortText {
    param([string]$PortName)

    $items = Get-CimInstance Win32_PnPEntity -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -like "*($PortName)*" -or $_.DeviceID -like "*$PortName*" } |
        Select-Object DeviceID, Description, PNPDeviceID
    if ($null -eq $items) {
        return "No matching serial PnP entry for $PortName."
    }
    return ($items | Format-List | Out-String).Trim()
}

function Stop-ListenerTypeProcesses {
    $processes = Get-Process -ErrorAction SilentlyContinue |
        Where-Object {
            $_.ProcessName -eq "listener-type" -or
            $_.ProcessName -like "Listener Type*" -or
            $_.Path -like "*Listener Type*"
        }
    if ($null -eq $processes -or @($processes).Count -eq 0) {
        return "No listener-type process was running."
    }
    foreach ($process in $processes) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        "Stopped pid=$($process.Id) name=$($process.ProcessName)"
    }
}

function Invoke-FirmwareSerialStatus {
    param(
        [string]$PortName,
        [int]$BaudRate,
        [int]$ReadSeconds,
        [string]$OutputPath
    )

    $serial = [System.IO.Ports.SerialPort]::new(
        $PortName,
        $BaudRate,
        [System.IO.Ports.Parity]::None,
        8,
        [System.IO.Ports.StopBits]::One)
    $serial.ReadTimeout = 500
    $serial.WriteTimeout = 1000
    $serial.DtrEnable = $false
    $serial.RtsEnable = $false

    try {
        $serial.Open()
        Start-Sleep -Milliseconds 250
        foreach ($command in @("~OTA:ABORT", "~OTA:STATUS", "~POWER:STATUS", "~DIAGLOG:COUNT")) {
            $serial.Write("$command`n")
            Start-Sleep -Milliseconds 250
        }

        $lines = New-Object System.Collections.Generic.List[string]
        $deadline = (Get-Date).AddSeconds($ReadSeconds)
        while ((Get-Date) -lt $deadline) {
            try {
                $line = $serial.ReadLine()
                if ($null -ne $line) {
                    $lines.Add($line)
                }
            } catch {
            }
        }
        $lines | Set-Content -LiteralPath $OutputPath -Encoding UTF8
        return $OutputPath
    } finally {
        if ($serial.IsOpen) {
            $serial.Close()
        }
        $serial.Dispose()
    }
}

function Invoke-ListenerTypeFirmwareOtaCli {
    param(
        [string]$ListenerRoot,
        [string]$Mode,
        [string]$Manifest,
        [string]$Firmware,
        [string]$Address
    )

    if (-not (Test-Path $Manifest)) {
        throw "ManifestPath not found: $Manifest"
    }
    if (-not (Test-Path $Firmware)) {
        throw "FirmwarePath not found: $Firmware"
    }

    $env:LISTENER_TYPE_BLE_ADDRESS = $Address
    Push-Location $ListenerRoot
    try {
        & cargo run --manifest-path src-tauri\Cargo.toml -- $Mode $Manifest $Firmware 2>&1 | Out-String
    } finally {
        Pop-Location
    }
}

$listenerRoot = Resolve-RepoRoot
$firmwareRoot = Resolve-FirmwareRepoRoot
$address = Resolve-BluetoothAddress -PreferredName $DeviceName -PreferredAddress $BluetoothAddress
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$artifactDir = Join-Path $firmwareRoot "tests\artifacts\listener_ble_ota_recovery"
New-Item -ItemType Directory -Force -Path $artifactDir | Out-Null

$logPath = Join-Path $artifactDir "recover_listener_ble_ota_$stamp.log"
$jsonPath = Join-Path $artifactDir "recover_listener_ble_ota_$stamp.json"
$summaryPath = Join-Path $artifactDir "recover_listener_ble_ota_$stamp.md"
$serialLogPath = Join-Path $artifactDir "recover_listener_ble_ota_serial_$stamp.log"
$script:steps = @()

Start-Transcript -LiteralPath $logPath -Force | Out-Null
$status = "FAIL"
try {
    Write-Host "recover_listener_ble_ota: listener_root=$listenerRoot"
    Write-Host "recover_listener_ble_ota: firmware_root=$firmwareRoot"
    Write-Host "recover_listener_ble_ota: target=$DeviceName addr=$address port=$Port"

    Invoke-Step -Name "initial_bluetooth_pnp" -Body { Get-ListenerBluetoothPnpText -PreferredName $DeviceName -Address $address } | Out-Null
    Invoke-Step -Name "initial_serial_ports" -Body { Get-SerialPortText -PortName $Port } | Out-Null

    if ($KeepListenerTypeRunning) {
        Add-Step -Name "stop_listener_type_processes" -Status "SKIP" -Detail "KeepListenerTypeRunning was set."
    } else {
        Invoke-Step -Name "stop_listener_type_processes" -Body { Stop-ListenerTypeProcesses } | Out-Null
    }

    Invoke-Step -Name "firmware_serial_abort_and_status" -Body {
        Invoke-FirmwareSerialStatus -PortName $Port -BaudRate $SerialBaud -ReadSeconds $SerialReadSeconds -OutputPath $serialLogPath
    } | Out-Null

    if ($SkipWindowsBluetoothRestart) {
        Add-Step -Name "restart_windows_bluetooth" -Status "SKIP" -Detail "SkipWindowsBluetoothRestart was set."
    } else {
        $restartScript = Join-Path $firmwareRoot "tools\restart_windows_bluetooth.ps1"
        Invoke-Step -Name "restart_windows_bluetooth" -Body {
            $output = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $restartScript 2>&1 | Out-String
            if ($LASTEXITCODE -ne 0) {
                throw "restart_windows_bluetooth failed exit_code=$LASTEXITCODE`n$output"
            }
            $output
        } | Out-Null
    }

    if ($SkipGattProbe) {
        Add-Step -Name "ble_gatt_maintain_connection_probe" -Status "SKIP" -Detail "SkipGattProbe was set."
    } else {
        $ensureScript = Join-Path $firmwareRoot "tools\ensure_ble_hid_connection.ps1"
        Invoke-Step -Name "ble_gatt_maintain_connection_probe" -Body {
            $output = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $ensureScript `
                -DeviceName $DeviceName `
                -BluetoothAddress $address `
                -DurationSeconds $GattProbeSeconds `
                -PollIntervalSeconds 2 `
                -ExitOnReady 2>&1 | Out-String
            if ($LASTEXITCODE -ne 0) {
                throw "ensure_ble_hid_connection failed exit_code=$LASTEXITCODE`n$output"
            }
            $output
        } | Out-Null
    }

    Invoke-Step -Name "post_recovery_bluetooth_pnp" -Body { Get-ListenerBluetoothPnpText -PreferredName $DeviceName -Address $address } | Out-Null

    if ([string]::IsNullOrWhiteSpace($ManifestPath) -or [string]::IsNullOrWhiteSpace($FirmwarePath)) {
        Add-Step -Name "listener_type_ota_preflight" -Status "SKIP" -Detail "ManifestPath/FirmwarePath not provided."
    } else {
        Invoke-Step -Name "listener_type_ota_preflight" -Body {
            Invoke-ListenerTypeFirmwareOtaCli `
                -ListenerRoot $listenerRoot `
                -Mode "--firmware-ota-preflight" `
                -Manifest $ManifestPath `
                -Firmware $FirmwarePath `
                -Address $address
        } | Out-Null
    }

    if ($RunTransfer) {
        Invoke-Step -Name "listener_type_ota_transfer" -Body {
            Invoke-ListenerTypeFirmwareOtaCli `
                -ListenerRoot $listenerRoot `
                -Mode "--firmware-ota-transfer" `
                -Manifest $ManifestPath `
                -Firmware $FirmwarePath `
                -Address $address
        } | Out-Null
    } else {
        Add-Step -Name "listener_type_ota_transfer" -Status "SKIP" -Detail "RunTransfer was not set."
    }

    $failed = @($script:steps | Where-Object { $_.status -eq "FAIL" })
    $status = if ($failed.Count -eq 0) { "PASS" } else { "FAIL" }
} finally {
    $report = [ordered]@{
        status = $status
        created_at = (Get-Date).ToString("o")
        device_name = $DeviceName
        bluetooth_address = $address
        port = $Port
        listener_root = $listenerRoot
        firmware_repo_root = $firmwareRoot
        log = $logPath
        serial_log = $serialLogPath
        steps = $script:steps
    }
    $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $jsonPath -Encoding UTF8

    $failedSteps = @($script:steps | Where-Object { $_.status -eq "FAIL" } | ForEach-Object { $_.name })
    $skippedSteps = @($script:steps | Where-Object { $_.status -eq "SKIP" } | ForEach-Object { $_.name })
    $lines = @(
        "# Listener BLE OTA Recovery $stamp",
        "",
        ("- Status: ``{0}``" -f $status),
        ("- Device: ``{0}`` / ``{1}``" -f $DeviceName, $address),
        ("- Port: ``{0}``" -f $Port),
        ("- Log: ``{0}``" -f $logPath),
        ("- Serial log: ``{0}``" -f $serialLogPath),
        ("- JSON: ``{0}``" -f $jsonPath),
        "",
        "## Failed Steps",
        "",
        $(if ($failedSteps.Count -gt 0) { $failedSteps | ForEach-Object { "- ``$_``" } } else { "- none" }),
        "",
        "## Skipped Steps",
        "",
        $(if ($skippedSteps.Count -gt 0) { $skippedSteps | ForEach-Object { "- ``$_``" } } else { "- none" }),
        "",
        "## Steps",
        ""
    )
    foreach ($step in $script:steps) {
        $lines += ("- ``{0}`` ``{1}``" -f $step.status, $step.name)
    }
    $lines | Set-Content -LiteralPath $summaryPath -Encoding UTF8

    Write-Host "[$((Get-Date).ToString('HH:mm:ss'))] Summary: $summaryPath"
    Write-Host "[$((Get-Date).ToString('HH:mm:ss'))] JSON: $jsonPath"
    Stop-Transcript | Out-Null
}

if ($status -eq "PASS") {
    exit 0
}
exit 1
