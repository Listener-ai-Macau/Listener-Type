param(
    [string]$OutputDir = "tests\artifacts\ai_diagnostics",
    [string]$FirmwareRepo,
    [string]$FirmwareDiagLog,
    [string]$FirmwareBundle,
    [int]$MaxFiles = 200,
    [int]$MaxErrorRefs = 80,
    [switch]$NoCopy
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$scriptDir = Split-Path -Parent $PSCommandPath
$repoRoot = Split-Path -Parent $scriptDir
$python = Get-Command python -ErrorAction SilentlyContinue

if (-not $python) {
    throw "python was not found on PATH; install Python or run from the project development shell."
}

$arguments = @(
    "-m", "tools.ai_diagnostics.collector",
    "--repo-root", $repoRoot,
    "--output-dir", $OutputDir,
    "--max-files", [string]$MaxFiles,
    "--max-error-refs", [string]$MaxErrorRefs
)

if ($FirmwareRepo) {
    $arguments += @("--firmware-repo", $FirmwareRepo)
}
if ($FirmwareDiagLog) {
    $arguments += @("--firmware-diag-log", $FirmwareDiagLog)
}
if ($FirmwareBundle) {
    $arguments += @("--firmware-bundle", $FirmwareBundle)
}
if ($NoCopy) {
    $arguments += "--no-copy"
}

$env:LISTENER_TYPE_DIAGNOSTIC_WRAPPER = "tools/collect_ai_diagnostics.ps1"
& $python.Source @arguments
exit $LASTEXITCODE
