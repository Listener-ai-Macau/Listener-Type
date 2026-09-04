[CmdletBinding()]
param(
    [switch]$RequireRunning
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$source = Join-Path $repoRoot 'src-tauri\target\release\listener-type.exe'
$installed = 'C:\Program Files\Listener Type\listener-type.exe'

if (-not (Test-Path -LiteralPath $source)) { throw "active release binary missing: $source" }
if (-not (Test-Path -LiteralPath $installed)) { throw "installed binary missing: $installed" }

$sourceItem = Get-Item -LiteralPath $source
$installedItem = Get-Item -LiteralPath $installed
$sourceHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
$installedHash = (Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash
if ($sourceHash -ne $installedHash) {
    throw "installed binary is not the active release: source=$sourceHash installed=$installedHash"
}
if ($sourceItem.VersionInfo.ProductVersion -ne $installedItem.VersionInfo.ProductVersion) {
    throw "version mismatch: source=$($sourceItem.VersionInfo.ProductVersion) installed=$($installedItem.VersionInfo.ProductVersion)"
}

$expectedPath = [IO.Path]::GetFullPath($installed)
$processes = @(Get-CimInstance Win32_Process -Filter "Name='listener-type.exe'" | Where-Object {
    $_.ExecutablePath -and ([IO.Path]::GetFullPath($_.ExecutablePath) -eq $expectedPath)
})
$foreign = @(Get-CimInstance Win32_Process -Filter "Name='listener-type.exe'" | Where-Object {
    -not $_.ExecutablePath -or ([IO.Path]::GetFullPath($_.ExecutablePath) -ne $expectedPath)
})
if ($foreign.Count -gt 0) {
    throw "listener-type process from an unexpected path is running: $($foreign.ExecutablePath -join ', ')"
}
if ($RequireRunning -and $processes.Count -eq 0) {
    throw 'the latest installed binary is not running'
}
foreach ($process in $processes) {
    $runningHash = (Get-FileHash -LiteralPath $process.ExecutablePath -Algorithm SHA256).Hash
    if ($runningHash -ne $sourceHash) {
        throw "running process $($process.ProcessId) has stale hash $runningHash"
    }
}

[pscustomobject]@{
    Source = $source
    Installed = $installed
    ProductVersion = $installedItem.VersionInfo.ProductVersion
    Sha256 = $installedHash
    RunningProcesses = $processes.Count
    ProcessIds = ($processes.ProcessId -join ',')
    Status = 'PASS'
}
