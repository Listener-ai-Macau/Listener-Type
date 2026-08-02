param(
    [Parameter(Mandatory = $true)]
    [string]$OutputJson,
    [int]$CdpPort = 9333,
    [int]$StartupTimeoutSeconds = 30,
    [int]$MaxRenameTotalMs = 15000
)

$ErrorActionPreference = "Stop"

$installedExe = "C:\Program Files\Listener Type\listener-type.exe"
$cdpListUrl = "http://127.0.0.1:$CdpPort/json/list"
$runner = Join-Path $PSScriptRoot "windows-installed-device-settings-ui-e2e.py"
$typeLog = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"

Get-Process -Name "listener-type" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

try {
    Start-Process `
        -FilePath $installedExe `
        -WindowStyle Hidden `
        -Environment @{
            LISTENER_TYPE_WEBVIEW2_ADDITIONAL_BROWSER_ARGS = "--remote-debugging-port=$CdpPort --remote-allow-origins=*"
            LISTENER_TYPE_SUPPRESS_CAPSULE_WINDOW = "1"
        } | Out-Null

    $deadline = [DateTimeOffset]::UtcNow.AddSeconds($StartupTimeoutSeconds)
    $page = $null
    while ([DateTimeOffset]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 250
        try {
            $pages = Invoke-RestMethod -Uri $cdpListUrl -NoProxy -TimeoutSec 2
            $page = $pages |
                Where-Object {
                    $_.type -eq "page" -and
                    $_.webSocketDebuggerUrl -and
                    $_.title -eq "Listener Type" -and
                    $_.url -notmatch "[?&]window="
                } |
                Select-Object -First 1
            if ($page) {
                break
            }
        }
        catch {
            $page = $null
        }
    }
    if (-not $page) {
        throw "Listener Type main CDP page was not ready"
    }

    & python $runner `
        --remote-debugging-port $CdpPort `
        --output-json $OutputJson `
        --different-random-name-roundtrip `
        --type-log $typeLog `
        --max-rename-total-ms $MaxRenameTotalMs
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
    Get-Content -Raw -LiteralPath $OutputJson
}
finally {
    Get-Process -Name "listener-type" -ErrorAction SilentlyContinue | Stop-Process -Force
}
