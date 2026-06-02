param(
    [switch]$Json,
    [switch]$Check,
    [switch]$UpdateFromAccepted,
    [string]$Plan,
    [string]$StepId,
    [string]$Commit,
    [string]$RepoRoot
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$scriptDir = Split-Path -Parent $PSCommandPath
$defaultRepoRoot = Split-Path -Parent (Split-Path -Parent $scriptDir)
$resolvedRepoRoot = if ($RepoRoot) { $RepoRoot } else { $defaultRepoRoot }
$acceptedLogPath = Join-Path $scriptDir "repo_features.accepted.jsonl"

function New-FeatureSnapshot {
    return [ordered]@{
        schema_version = 1
        repository = "Listener-Type"
        purpose = "Desktop voice input app for Listener devices and local microphone dictation."
        stack = @(
            "Tauri 2 desktop shell with Rust backend",
            "React, TypeScript, and Vite frontend",
            "Windows direct/clipboard insertion with an optional TSF IME bridge for explicit validation"
        )
        responsibilities = @(
            "Record one-tap dictation sessions from the built-in microphone or Listener BLE device audio.",
            "Stream or batch audio to ASR providers, polish text with configured LLM providers, then insert text at the cursor.",
            "Own onboarding, settings, credential vault, diagnostics, device health display, update UX, history, vocabulary, and style controls.",
            "Provide desktop-side BLE pairing/subscription, embedded audio receive/replay tooling, CLI diagnostics, and user-facing recovery guidance."
        )
        major_features = @(
            "Recording workflow: press once to start and press once to stop; no hold-to-talk product mode.",
            "Input sources: microphone and Listener BLE, including embedded BLE audio from VKA1-style devices.",
            "ASR providers: Volcengine streaming, OpenAI batch, Apple Speech, Bailian realtime, macOS Qwen local, and Windows Foundry Local Whisper.",
            "Text pipeline: coordinator-driven dictation, correction, polish, vocabulary hotwords, translation, QA selection ask, and insertion.",
            "Windows insertion: direct/clipboard fallback paths are default; optional native TSF IME bridge remains available for explicit validation.",
            "Device controls: Listener keyboard KEY1-KEY4 fallback shortcuts map to configurable safe actions; EC11 rotation can be set to system volume, screen brightness, or disabled and is synced to firmware over BLE.",
            "Release shell: Tauri updater, background update gate, tray menu, autostart, single-instance behavior, and package metadata.",
            "Settings and diagnostics: shortcuts, provider credentials in OS keyring/local storage, language, permissions, advanced logs, diagnostic export, dark mode, and device health.",
            "Developer/product tools: embedded audio file/BLE CLI replay, firmware OTA package/preflight validation, BLE stream smoke, Foundry runtime probes, and updater manifest checks."
        )
        key_paths = @(
            [ordered]@{ path = "src/App.tsx"; purpose = "Top-level React app state, shell, settings modal, capsule wiring." },
            [ordered]@{ path = "src/components/Capsule.tsx"; purpose = "Recording capsule and user-visible dictation state." },
            [ordered]@{ path = "src/components/AutoUpdate*.tsx"; purpose = "Manual/background update UI and gate." },
            [ordered]@{ path = "src/pages/settings/RecordingSection.tsx"; purpose = "Recording mode and input-source settings." },
            [ordered]@{ path = "src/components/EmbeddedBleStatusPanel.tsx"; purpose = "Listener BLE connection and device health UI." },
            [ordered]@{ path = "src/pages/settings/ShortcutsSection.tsx"; purpose = "Desktop and Listener device key shortcut/action settings." },
            [ordered]@{ path = "src/pages/settings/ProvidersSection.tsx"; purpose = "Cloud and local provider selection, credential setup, and model settings." },
            [ordered]@{ path = "src-tauri/src/coordinator.rs"; purpose = "Backend dictation orchestration and state transitions." },
            [ordered]@{ path = "src-tauri/src/shortcut_dispatch.rs"; purpose = "Safe desktop shortcut dispatch for configured device-key actions." },
            [ordered]@{ path = "src-tauri/src/recorder.rs"; purpose = "Local microphone capture." },
            [ordered]@{ path = "src-tauri/src/audio_mute.rs"; purpose = "Optional system audio mute behavior while recording." },
            [ordered]@{ path = "src-tauri/src/embedded_ble.rs"; purpose = "Windows BLE discovery, connection, and subscription handling." },
            [ordered]@{ path = "src-tauri/src/embedded_audio.rs"; purpose = "Embedded audio framing and receive path." },
            [ordered]@{ path = "src-tauri/src/asr/"; purpose = "ASR provider implementations and local ASR engines." },
            [ordered]@{ path = "src-tauri/src/commands.rs"; purpose = "Tauri command surface for settings, credentials, diagnostics, provider state, BLE, local ASR, and update helpers." },
            [ordered]@{ path = "src-tauri/src/persistence.rs"; purpose = "Settings, history, dictionaries, provider credentials, and OS credential-vault persistence." },
            [ordered]@{ path = "src-tauri/src/windows_ime_*.rs"; purpose = "Windows IME IPC, profile, protocol, and session bridge." },
            [ordered]@{ path = "windows-ime/"; purpose = "Optional native TSF text service; default Windows packages do not register it." },
            [ordered]@{ path = "src-tauri/nsis/listener-type-ime-cleanup-hooks.nsh"; purpose = "Default installer hook that unregisters/removes legacy TSF IME files without installing a new input method." },
            [ordered]@{ path = "src-tauri/nsis/listener-type-ime-hooks.nsh"; purpose = "Optional TSF IME register/unregister hooks for dedicated validation builds." },
            [ordered]@{ path = "src-tauri/src/cli.rs"; purpose = "Headless embedded audio file/BLE replay, firmware OTA, and diagnostic entry points." },
            [ordered]@{ path = "src-tauri/src/firmware_ota.rs"; purpose = "Shared firmware OTA package validation and headless release-gate runner." },
            [ordered]@{ path = "src-tauri/src/marketplace_backend.rs"; purpose = "Style marketplace REST contract and Rust HTTP client error handling." },
            [ordered]@{ path = "src-tauri/src/github_oauth.rs"; purpose = "GitHub OAuth device-flow, token refresh, and authenticated user lookup for marketplace identity." },
            [ordered]@{ path = "src/lib/localAsr.ts"; purpose = "Frontend wrappers for Qwen3-ASR and Foundry Local runtime/model commands." },
            [ordered]@{ path = "tools/collect_ai_diagnostics.ps1"; purpose = "One-command AI diagnostic bundle collector for logs, artifacts, BLE/audio summaries, workflow metadata, and optional firmware diagnostic input." },
            [ordered]@{ path = "tools/firmware_ota_headless/"; purpose = "Standalone firmware OTA package/preflight validation helper for release-gate artifacts." },
            [ordered]@{ path = "tools/embedded_audio_replay/"; purpose = "BLE/audio replay and smoke tools for desktop-device integration." },
            [ordered]@{ path = "tools/foundry_asr_probe/"; purpose = "Foundry Local Whisper diagnostic probe outside the full Tauri app." },
            [ordered]@{ path = "docs/features/"; purpose = "Human-readable feature index and per-feature source maps." }
        )
        platform_assumptions = @(
            "Windows is the primary product path for BLE device audio; default installers should not add a system input method.",
            "macOS support exists for local microphone, Accessibility insertion, and Apple Speech paths.",
            "Secrets stay in the OS credential vault or local app storage; the app remains local-first by default."
        )
        boundaries = @(
            "Firmware behavior, BLE GATT implementation, button scanning, and device logs live in voice-keyboard-firmware.",
            "Industrial design, CAD, review renders, and manufacturing constraints live in voice-keyboard-design.",
            "Workflow plans, claims, review state, and cross-repo orchestration live in ai-collaboration-workflow."
        )
        validation_commands = @(
            "pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check",
            "pwsh -NoProfile -File .\tools\collect_ai_diagnostics.ps1 -OutputDir .\tests\artifacts\ai_diagnostics_smoke",
            "npm run test",
            "npm run build",
            "npm run verify",
            "npm run check:dark-mode",
            "cargo test --manifest-path src-tauri\Cargo.toml --lib --no-run",
            "cargo test --manifest-path tools\embedded_audio_replay\Cargo.toml",
            "cargo test --manifest-path tools\firmware_ota_headless\Cargo.toml",
            "node scripts\write-updater-manifest.test.mjs",
            "node scripts\windows-package-msvc.test.mjs",
            "powershell -ExecutionPolicy Bypass -File scripts\windows-ime-build.ps1",
            "git diff --check"
        )
        update_policy = "Record accepted changes only when they alter important desktop responsibilities, user-visible workflows, provider/device support, or validation entry points."
    }
}

function Get-AcceptedChanges {
    if (-not (Test-Path $acceptedLogPath)) {
        return @()
    }

    $changes = @()
    foreach ($line in Get-Content -Path $acceptedLogPath) {
        if ([string]::IsNullOrWhiteSpace($line)) {
            continue
        }
        try {
            $changes += ($line | ConvertFrom-Json)
        } catch {
            $changes += [ordered]@{ parse_error = $_.Exception.Message; raw = $line }
        }
    }
    return $changes
}

function Get-FeatureSnapshot {
    $snapshot = New-FeatureSnapshot
    $snapshot["recent_accepted_changes"] = @(Get-AcceptedChanges)
    return $snapshot
}

function Test-FeatureSnapshot {
    param([Parameter(Mandatory = $true)]$Snapshot)

    $errors = @()
    $jsonText = $Snapshot | ConvertTo-Json -Depth 10
    $scriptText = Get-Content -Path $PSCommandPath -Raw

    foreach ($term in @("Tauri", "Rust", "React", "BLE", "embedded audio", "ASR", "diagnostics", "settings")) {
        if ($jsonText -notmatch [regex]::Escape($term)) {
            $errors += "missing required Listener-Type feature term: $term"
        }
    }

    if (@($Snapshot["responsibilities"]).Count -lt 3) {
        $errors += "responsibilities must contain at least 3 entries"
    }
    if (@($Snapshot["key_paths"]).Count -lt 8) {
        $errors += "key_paths must contain at least 8 entries"
    }
    if (@($Snapshot["validation_commands"]).Count -lt 4) {
        $errors += "validation_commands must contain at least 4 entries"
    }
    if ($scriptText.Length -gt 16000) {
        $errors += "script is too long: $($scriptText.Length) characters"
    }

    return $errors
}

function Write-HumanSnapshot {
    param([Parameter(Mandatory = $true)]$Snapshot)

    Write-Output "# $($Snapshot["repository"])"
    Write-Output $Snapshot["purpose"]
    Write-Output ""
    Write-Output "Repo root: $resolvedRepoRoot"

    foreach ($section in @(
        @{ title = "Stack"; key = "stack" },
        @{ title = "Responsibilities"; key = "responsibilities" },
        @{ title = "Major Features"; key = "major_features" },
        @{ title = "Platform Assumptions"; key = "platform_assumptions" },
        @{ title = "Boundaries"; key = "boundaries" },
        @{ title = "Validation"; key = "validation_commands" }
    )) {
        Write-Output ""
        Write-Output "## $($section.title)"
        foreach ($item in $Snapshot[$section.key]) {
            Write-Output "  - $item"
        }
    }

    Write-Output ""
    Write-Output "## Key Paths"
    foreach ($entry in $Snapshot["key_paths"]) {
        Write-Output ("  - {0}: {1}" -f $entry["path"], $entry["purpose"])
    }

    if (@($Snapshot["recent_accepted_changes"]).Count -gt 0) {
        Write-Output ""
        Write-Output "## Recent Accepted Important Changes"
        foreach ($change in $Snapshot["recent_accepted_changes"]) {
            Write-Output ("  - {0}/{1} {2}" -f $change.plan, $change.step_id, $change.commit)
        }
    }

    Write-Output ""
    Write-Output "Update policy: $($Snapshot["update_policy"])"
}

if ($UpdateFromAccepted) {
    if (-not $Plan -or -not $StepId -or -not $Commit) {
        throw "-UpdateFromAccepted requires -Plan, -StepId, and -Commit."
    }

    $record = [ordered]@{
        recorded_at = (Get-Date).ToString("o")
        plan = $Plan
        step_id = $StepId
        commit = $Commit
        repo_root = $resolvedRepoRoot
        note = "Keep this entry only if the accepted work changed important Listener-Type responsibilities, workflows, providers, device support, or validation."
    }
    $record | ConvertTo-Json -Depth 6 -Compress | Add-Content -Path $acceptedLogPath -Encoding UTF8
    Write-Output "Recorded accepted feature update hint: $acceptedLogPath"
    exit 0
}

$snapshot = Get-FeatureSnapshot

if ($Check) {
    $errors = Test-FeatureSnapshot -Snapshot $snapshot
    if (@($errors).Count -gt 0) {
        throw ("repo_features check failed:`n - " + ($errors -join "`n - "))
    }
    Write-Output "PASS: Listener-Type repo feature script is present, concise, and covers desktop core responsibilities."
    exit 0
}

if ($Json) {
    $snapshot | ConvertTo-Json -Depth 10
    exit 0
}

Write-HumanSnapshot -Snapshot $snapshot
