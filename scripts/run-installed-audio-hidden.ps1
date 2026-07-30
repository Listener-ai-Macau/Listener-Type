param(
    [Parameter(Mandatory = $true)]
    [string]$OutputJson,
    [int]$CdpPort = 9280,
    [int]$StartupTimeoutSeconds = 30
)

$ErrorActionPreference = "Stop"

$installedExe = "C:\Program Files\Listener Type\listener-type.exe"
$cdpListUrl = "http://127.0.0.1:$CdpPort/json/list"
$runner = Join-Path $PSScriptRoot "run-installed-audio-cdp.mjs"
$serialRunner = Join-Path $PSScriptRoot "..\..\Listener-Firmware\tools\send_serial_and_capture.ps1"
$nodeOut = "$OutputJson.node.out"
$nodeErr = "$OutputJson.node.err"

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

    Start-Sleep -Seconds 4
    $node = Start-Process `
        -FilePath "node" `
        -WindowStyle Hidden `
        -PassThru `
        -ArgumentList @(
            "`"$runner`"",
            "--cdp-url", "`"$($page.webSocketDebuggerUrl)`"",
            "--output-json", "`"$OutputJson`"",
            "--timeout-ms", "20000"
        ) `
        -RedirectStandardOutput $nodeOut `
        -RedirectStandardError $nodeErr
    Start-Sleep -Seconds 2

    & pwsh -NoProfile -File $serialRunner `
        -Port COM3 `
        -CommandList "~VREC:CANCEL;;~VREC:TOGGLE;;~VREC:STOP" `
        -InitialReadMs 100 `
        -CommandReadMs 1000 `
        -CommandDelayMs 2500 `
        -OutputPath "$OutputJson.serial.log" | Out-Null

    $node.Refresh()
    if (-not $node.HasExited) {
        Wait-Process -Id $node.Id -Timeout 35
    }
    $node.Refresh()
    if ($node.ExitCode -ne 0) {
        throw "Installed audio CDP runner failed: $(Get-Content -Raw -LiteralPath $nodeErr)"
    }
    Get-Content -Raw -LiteralPath $OutputJson
}
finally {
    Get-Process -Name "listener-type" -ErrorAction SilentlyContinue | Stop-Process -Force
}
