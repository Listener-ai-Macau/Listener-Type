[CmdletBinding()]
param(
    [switch]$RequireRunning,
    [string]$ExpectedSha256 = ''
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$source = Join-Path $repoRoot 'src-tauri\target\x86_64-pc-windows-msvc\release\listener-type.exe'
$installed = 'C:\Program Files\Listener Type\listener-type.exe'

if (-not (Test-Path -LiteralPath $installed)) { throw "installed binary missing: $installed" }

$expectedHash = $ExpectedSha256.Trim().ToUpperInvariant()
$usingExplicitHash = -not [string]::IsNullOrWhiteSpace($expectedHash)
if (-not $usingExplicitHash -and -not (Test-Path -LiteralPath $source)) {
    throw "active release binary missing: $source"
}

$installedItem = Get-Item -LiteralPath $installed
$sourceItem = if ($usingExplicitHash) { $null } else { Get-Item -LiteralPath $source }
$sourceHash = if ($usingExplicitHash) {
    $expectedHash
} else {
    (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
}
$installedHash = (Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash
if ($sourceHash -ne $installedHash) {
    $expectedSource = if ($usingExplicitHash) { 'explicit expected hash' } else { $source }
    throw "installed binary is not the active release: source=$expectedSource expected=$sourceHash installed=$installedHash"
}
if ($sourceItem -and $sourceItem.VersionInfo.ProductVersion -ne $installedItem.VersionInfo.ProductVersion) {
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
    Source = if ($usingExplicitHash) { 'explicit expected hash' } else { $source }
    Installed = $installed
    ProductVersion = $installedItem.VersionInfo.ProductVersion
    Sha256 = $installedHash
    ExpectedSha256 = $sourceHash
    RunningProcesses = $processes.Count
    ProcessIds = ($processes.ProcessId -join ',')
    Status = 'PASS'
}
