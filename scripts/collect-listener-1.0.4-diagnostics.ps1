<#
.SYNOPSIS
  One-click Listener 1.0.4 diagnostic bundle (privacy-bounded).

.DESCRIPTION
  Collects Type logs, BLE status snapshots, firmware boot/reset/WDT evidence when
  available, package hashes, versions, and key settings. Does NOT collect user
  recordings, transcript body text, credentials, or raw voiceprint blobs by default.

.NOTES
  Privacy boundary (default):
  - INCLUDE: app version, package SHA-256, BLE connection metadata, boot/reset/WDT
    counters, sanitized settings booleans/enums, redacted error tails, firmware
    diagnostic event sources when serial/offline log is provided.
  - EXCLUDE: *.wav / recordings/, ASR transcript bodies, inserted text, API keys,
    tokens, passwords, voiceprint embeddings, wake-diag-live WAV store contents.
#>
param(
  [string]$OutputDir = "",
  [string]$FirmwarePort = "",
  [string]$FirmwareDiagJsonl = "",
  [switch]$IncludeFirmwareSerial,
  [switch]$NoCopyLogs
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$scriptDir = Split-Path -Parent $PSCommandPath
$typeRoot = (Resolve-Path -LiteralPath (Join-Path $scriptDir "..")).Path
$listenerRoot = (Resolve-Path -LiteralPath (Join-Path $typeRoot "..")).Path
$firmwareRoot = Join-Path $listenerRoot "Listener-Firmware"

if ([string]::IsNullOrWhiteSpace($OutputDir)) {
  $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
  $OutputDir = Join-Path $typeRoot ".artifacts\listener-1.0.4-diagnostics\$stamp"
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

$privacy = [ordered]@{
  schema_version = 1
  mode           = "default_privacy_bounded"
  includes       = @(
    "Type app log tail (redacted secrets)",
    "BLE connection / notify-ready metadata",
    "package hashes and versions",
    "key settings booleans/enums (no credential values)",
    "firmware boot/reset/WDT counters when available",
    "repo identity (commit/tag)"
  )
  excludes       = @(
    "user recordings (*.wav)",
    "ASR transcript / inserted text bodies",
    "API keys, tokens, passwords",
    "voiceprint embeddings / enrollment audio",
    "wake-diag-live WAV store contents",
    "full production preferences.json credential blobs"
  )
  notes          = @(
    "Default collection never copies %APPDATA%\\Listener Type\\recordings.",
    "Credential fields are represented only as configured/unconfigured when present.",
    "Serial firmware collection is optional and disabled unless -IncludeFirmwareSerial or -FirmwareDiagJsonl."
  )
}
$privacy | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputDir "privacy.json") -Encoding utf8
@"
# Listener 1.0.4 诊断包隐私边界

## 默认采集
- Type 版本、可执行文件路径、包 SHA-256
- BLE 连接 / notify-ready 元数据
- 关键设置布尔/枚举（不含凭据值）
- 固件 boot / reset / WDT 计数（若可获得）
- 仓库 commit/tag 身份
- 日志尾部（密钥模式脱敏）

## 默认不采集
- 用户录音与 `recordings/` 目录
- ASR 正文、插入文本
- API Key / token / 密码
- 声纹 embedding 与注册音频
- wake-diag-live WAV 内容

如需更深固件事件，显式传入 `-FirmwareDiagJsonl` 或 `-IncludeFirmwareSerial -FirmwarePort COMx`。
"@ | Set-Content -LiteralPath (Join-Path $OutputDir "PRIVACY.md") -Encoding utf8

function Get-Sha256Upper {
  param([string]$Path)
  if (-not (Test-Path -LiteralPath $Path)) { return $null }
  return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToUpperInvariant()
}

function Redact-Text {
  param([string]$Text)
  if ([string]::IsNullOrEmpty($Text)) { return $Text }
  $Text = [regex]::Replace($Text, 'sk-[A-Za-z0-9_\-]{12,}', 'sk-<redacted>')
  $Text = [regex]::Replace($Text, '(?i)(api[_\- ]?key|token|secret|password)\s*[:=]\s*[''"]?[^''"\s,;]+', '$1=<redacted>')
  return $Text
}

# --- Identity & packages ---
$identity = [ordered]@{
  generated_at = (Get-Date).ToString("o")
  type_commit  = (& git -C $typeRoot rev-parse HEAD 2>$null)
  type_describe = (& git -C $typeRoot describe --tags --always 2>$null)
  firmware_commit = (& git -C $firmwareRoot rev-parse HEAD 2>$null)
  firmware_describe = (& git -C $firmwareRoot describe --tags --always 2>$null)
  packages     = [ordered]@{
    msi = [ordered]@{
      path   = (Join-Path $listenerRoot "ListenerType_1.0.4_x64_en-US.msi")
      sha256 = Get-Sha256Upper (Join-Path $listenerRoot "ListenerType_1.0.4_x64_en-US.msi")
    }
    ota = [ordered]@{
      path   = (Join-Path $listenerRoot "ListenerFirmware_1.0.4_ota.zip")
      sha256 = Get-Sha256Upper (Join-Path $listenerRoot "ListenerFirmware_1.0.4_ota.zip")
    }
  }
  installed_type = $null
}

$installedExe = "C:\Program Files\Listener Type\listener-type.exe"
if (Test-Path -LiteralPath $installedExe) {
  $identity.installed_type = [ordered]@{
    path   = $installedExe
    sha256 = Get-Sha256Upper $installedExe
    version = (Get-Item $installedExe).VersionInfo.ProductVersion
  }
}
$identity | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $OutputDir "identity.json") -Encoding utf8

# --- Type log tail (redacted) ---
$appLog = Join-Path $env:LOCALAPPDATA "Listener Type\Logs\listener-type.log"
$logOut = Join-Path $OutputDir "listener-type.log.tail.txt"
$bleMeta = [ordered]@{
  log_path = $appLog
  exists   = (Test-Path -LiteralPath $appLog)
  notify_ready_samples = @()
  ble_error_samples = @()
  ota_samples = @()
  wdt_or_reset_samples = @()
}

if ($bleMeta.exists -and -not $NoCopyLogs.IsPresent) {
  $lines = Get-Content -LiteralPath $appLog -Tail 400 -ErrorAction SilentlyContinue
  $redacted = foreach ($line in $lines) { Redact-Text $line }
  $redacted | Set-Content -LiteralPath $logOut -Encoding utf8
  foreach ($line in $redacted) {
    if ($line -match "notify ready|TYPE:READY|background listener") {
      $bleMeta.notify_ready_samples += $line
    }
    if ($line -match "BLE|Gatt|CCCD|AccessDenied") {
      if ($line -match "error|fail|timeout|denied|panic") {
        $bleMeta.ble_error_samples += $line
      }
    }
    if ($line -match "OTA|bulk_kb_s|offset recover") {
      $bleMeta.ota_samples += $line
    }
    if ($line -match "WDT|reset_reason|ESP_RST|crash_count") {
      $bleMeta.wdt_or_reset_samples += $line
    }
  }
  # Cap sample sizes
  $bleMeta.notify_ready_samples = @($bleMeta.notify_ready_samples | Select-Object -Last 20)
  $bleMeta.ble_error_samples = @($bleMeta.ble_error_samples | Select-Object -Last 30)
  $bleMeta.ota_samples = @($bleMeta.ota_samples | Select-Object -Last 20)
  $bleMeta.wdt_or_reset_samples = @($bleMeta.wdt_or_reset_samples | Select-Object -Last 20)
}
$bleMeta | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputDir "ble-status.json") -Encoding utf8

# --- Key settings (booleans only; never copy raw credentials) ---
$prefsPath = Join-Path $env:APPDATA "Listener Type\preferences.json"
$settingsOut = [ordered]@{
  preferences_path = $prefsPath
  exists           = (Test-Path -LiteralPath $prefsPath)
  settings         = $null
  note             = "Only non-secret product toggles are exported."
}
if ($settingsOut.exists) {
  try {
    $prefs = Get-Content -Raw -LiteralPath $prefsPath | ConvertFrom-Json
    $settingsOut.settings = [ordered]@{
      show_capsule                 = $prefs.showCapsule
      send_key_after_dictation     = $prefs.sendKeyAfterDictation
      copy_dictation_to_clipboard  = $prefs.copyDictationToClipboard
      restore_clipboard_after_paste = $prefs.restoreClipboardAfterPaste
      remove_filler_words          = $prefs.removeFillerWords
      dictation_input_source       = $prefs.dictationInputSource
      mute_during_recording        = $prefs.muteDuringRecording
      launch_at_login              = $prefs.launchAtLogin
      voice_wake_phrase_configured = -not [string]::IsNullOrWhiteSpace([string]$prefs.voiceWakePhrase)
      active_asr_provider          = $prefs.activeAsrProvider
      active_llm_provider          = $prefs.activeLlmProvider
      # Credential presence only — never values
      has_asr_credentials_hint     = $null
      has_llm_credentials_hint     = $null
    }
  } catch {
    $settingsOut.error = $_.Exception.Message
  }
}
$settingsOut | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $OutputDir "key-settings.json") -Encoding utf8

# --- Optional Type AI diagnostics collector (already redacts secrets) ---
$typeCollector = Join-Path $typeRoot "tools\collect_ai_diagnostics.ps1"
if (Test-Path -LiteralPath $typeCollector) {
  $typeBundle = Join-Path $OutputDir "type-ai-diagnostics"
  try {
    $fwArgs = @("-OutputDir", $typeBundle, "-FirmwareRepo", $firmwareRoot)
    if ($NoCopyLogs.IsPresent) { $fwArgs += "-NoCopy" }
    # Collector resolves `tools.ai_diagnostics` relative to Type repo cwd.
    Push-Location $typeRoot
    try {
      & pwsh -NoProfile -File $typeCollector @fwArgs
      if ((Test-Path variable:global:LASTEXITCODE) -and [int]$global:LASTEXITCODE -ne 0) {
        "type collector exit=$global:LASTEXITCODE" |
          Set-Content -LiteralPath (Join-Path $OutputDir "type-ai-diagnostics.error.txt") -Encoding utf8
      }
    } finally {
      Pop-Location
    }
  } catch {
    $_ | Out-File -FilePath (Join-Path $OutputDir "type-ai-diagnostics.error.txt") -Encoding utf8
  }
}

# --- Optional firmware serial / offline diag ---
$fwDiagDir = Join-Path $OutputDir "firmware-diagnostics"
New-Item -ItemType Directory -Force -Path $fwDiagDir | Out-Null
$fwCollect = Join-Path $firmwareRoot "tools\collect_ai_diagnostics.ps1"
if (-not [string]::IsNullOrWhiteSpace($FirmwareDiagJsonl) -and (Test-Path -LiteralPath $fwCollect)) {
  try {
    & pwsh -NoProfile -File $fwCollect -InputJsonl $FirmwareDiagJsonl -OutputDir $fwDiagDir
  } catch {
    $_ | Out-File -FilePath (Join-Path $fwDiagDir "error.txt") -Encoding utf8
  }
} elseif ($IncludeFirmwareSerial.IsPresent -and -not [string]::IsNullOrWhiteSpace($FirmwarePort) -and (Test-Path -LiteralPath $fwCollect)) {
  try {
    & pwsh -NoProfile -File $fwCollect -Port $FirmwarePort -OutputDir $fwDiagDir -RecentEventCount 120
  } catch {
    $_ | Out-File -FilePath (Join-Path $fwDiagDir "error.txt") -Encoding utf8
  }
} else {
  "Firmware event collection skipped (provide -FirmwareDiagJsonl or -IncludeFirmwareSerial -FirmwarePort COMx)." |
    Set-Content -LiteralPath (Join-Path $fwDiagDir "skipped.txt") -Encoding utf8
}

# --- Manifest ---
$files = @(Get-ChildItem -LiteralPath $OutputDir -Recurse -File | ForEach-Object {
    [ordered]@{
      path   = $_.FullName.Substring($OutputDir.Length).TrimStart('\', '/')
      bytes  = $_.Length
      sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
    }
  })

$manifest = [ordered]@{
  schema         = "listener.1.0.4.diagnostic_bundle"
  schema_version = 1
  result         = "PASS"
  output_dir     = $OutputDir
  privacy        = "privacy.json"
  privacy_doc    = "PRIVACY.md"
  files          = $files
}
$manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $OutputDir "manifest.json") -Encoding utf8

Write-Host "PASS: diagnostic bundle written to $OutputDir"
Write-Host "Privacy boundary: $(Join-Path $OutputDir 'PRIVACY.md')"
exit 0
