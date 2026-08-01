param(
    [Parameter(Mandatory = $true)]
    [string]$Package,

    [Parameter(Mandatory = $true)]
    [string]$OutputJson,

    [string]$ExePath = "C:\Program Files\Listener Type\listener-type.exe",

    [int]$CdpPort = 9222,

    [int]$StartupTimeoutSeconds = 15
)

$ErrorActionPreference = "Stop"

$installedExe = $ExePath
$cdpListUrl = "http://127.0.0.1:$CdpPort/json/list"
$runner = Join-Path $PSScriptRoot "run-installed-ota-cdp.mjs"

if (-not (Test-Path -LiteralPath $installedExe)) {
    throw "Installed Listener Type executable not found: $installedExe"
}
if (-not (Test-Path -LiteralPath $Package)) {
    throw "OTA package not found: $Package"
}

Get-Process -Name "listener-type" -ErrorAction SilentlyContinue |
    Stop-Process -Force
Start-Sleep -Milliseconds 500

$mainProcess = $null
try {
    $mainProcess = Start-Process `
        -FilePath $installedExe `
        -WindowStyle Hidden `
        -PassThru `
        -Environment @{
            LISTENER_TYPE_WEBVIEW2_ADDITIONAL_BROWSER_ARGS = "--remote-debugging-port=$CdpPort --remote-allow-origins=*"
            LISTENER_TYPE_SUPPRESS_CAPSULE_WINDOW = "1"
        }

    $deadline = [DateTimeOffset]::UtcNow.AddSeconds($StartupTimeoutSeconds)
    $pages = $null
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
            $pages = $null
            $page = $null
        }
    }

    if (-not $page) {
        $availablePages = ($pages |
            Where-Object { $_.type -eq "page" } |
            ForEach-Object { "title=$($_.title) url=$($_.url)" }) -join "; "
        throw "Listener Type main CDP page was not ready within $StartupTimeoutSeconds seconds; pages=$availablePages"
    }

    & node $runner `
        --cdp-url $page.webSocketDebuggerUrl `
        --package $Package `
        --output-json $OutputJson
    if ($LASTEXITCODE -ne 0) {
        throw "Installed OTA CDP runner failed with exit code $LASTEXITCODE"
    }
}
finally {
    Get-Process -Name "listener-type" -ErrorAction SilentlyContinue |
        Stop-Process -Force
}
