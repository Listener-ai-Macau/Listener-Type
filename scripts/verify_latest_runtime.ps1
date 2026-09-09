[CmdletBinding()]
param(
    [switch]$RequireRunning
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$source = Join-Path $repoRoot 'src-tauri\target\release\listener-type.exe'
$msi = Join-Path $repoRoot 'src-tauri\target\release\bundle\msi\Listener Type_1.0.5_x64_en-US.msi'
$installed = 'C:\Program Files\Listener Type\listener-type.exe'

if (-not (Test-Path -LiteralPath $source)) { throw "active release binary missing: $source" }
if (-not (Test-Path -LiteralPath $installed)) { throw "installed binary missing: $installed" }

$sourceItem = Get-Item -LiteralPath $source
$installedItem = Get-Item -LiteralPath $installed
$expectedHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
$identitySource = $source

# Tauri patches bundle metadata after compiling the release executable. The
# bytes in the MSI payload (and therefore Program Files) can legitimately
# differ from target/release/listener-type.exe. When the current MSI exists,
# extract its payload to a private temp directory and use that packaged exe as
# the release identity. This keeps runtime verification strict without
# treating a valid MSI installation as stale.
if (Test-Path -LiteralPath $msi) {
    $extractRoot = Join-Path ([IO.Path]::GetTempPath()) ("listener-runtime-verify-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $extractRoot -Force | Out-Null
    try {
        $extractLog = Join-Path $extractRoot 'msiexec.log'
        $arguments = "/a `"$msi`" /qn TARGETDIR=`"$extractRoot`" /l*v `"$extractLog`""
        $extractProcess = Start-Process msiexec.exe -ArgumentList $arguments -Wait -PassThru
        if ($extractProcess.ExitCode -ne 0) {
            throw "could not extract current MSI payload for identity verification: exit=$($extractProcess.ExitCode)"
        }
        $payload = Get-ChildItem -LiteralPath $extractRoot -Recurse -Filter 'listener-type.exe' -File |
            Select-Object -First 1
        if ($null -eq $payload) {
            throw "current MSI payload does not contain listener-type.exe: $msi"
        }
        $expectedHash = (Get-FileHash -LiteralPath $payload.FullName -Algorithm SHA256).Hash
        $identitySource = $msi
    } finally {
        if (Test-Path -LiteralPath $extractRoot) {
            Remove-Item -LiteralPath $extractRoot -Recurse -Force
        }
    }
}

$installedHash = (Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash
if ($expectedHash -ne $installedHash) {
    throw "installed binary is not the active release: identity=$identitySource expected=$expectedHash installed=$installedHash"
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
    if ($runningHash -ne $expectedHash) {
        throw "running process $($process.ProcessId) has stale hash $runningHash"
    }
}

[pscustomobject]@{
    Source = $identitySource
    Installed = $installed
    ProductVersion = $installedItem.VersionInfo.ProductVersion
    Sha256 = $installedHash
    RunningProcesses = $processes.Count
    ProcessIds = ($processes.ProcessId -join ',')
    Status = 'PASS'
}
