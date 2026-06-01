$ErrorActionPreference = "Stop"

$script = Join-Path $PSScriptRoot "embedded_audio_replay\run_ble_stream_smoke.ps1"
& $script @args

if ($LASTEXITCODE -is [int] -and $LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
