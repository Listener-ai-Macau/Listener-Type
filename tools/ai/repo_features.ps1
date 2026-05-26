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
            "Windows TSF IME bridge plus platform hotkey/input insertion support"
        )
        responsibilities = @(
            "Record one-tap dictation sessions from the built-in microphone or Listener BLE device audio.",
            "Stream or batch audio to ASR providers, polish text with configured LLM providers, then insert text at the cursor.",
            "Own onboarding, settings, diagnostics, device health display, update UX, history, vocabulary, and style controls.",
            "Provide desktop-side BLE pairing/subscription, embedded audio receive/replay tooling, and user-facing recovery guidance."
        )
        major_features = @(
            "Recording workflow: press once to start and press once to stop; no hold-to-talk product mode.",
            "Input sources: microphone and Listener BLE, including embedded BLE audio from VKA1-style devices.",
            "ASR providers: Volcengine streaming, OpenAI batch, Apple Speech, Bailian realtime, Qwen local, and Foundry local paths.",
            "Text pipeline: coordinator-driven dictation, correction, polish, vocabulary hotwords, translation, QA selection ask, and insertion.",
            "Settings and diagnostics: shortcuts, provider credentials, language, permissions, advanced logs, dark mode, and device health."
        )
        key_paths = @(
            [ordered]@{ path = "src/App.tsx"; purpose = "Top-level React app state, shell, settings modal, capsule wiring." },
            [ordered]@{ path = "src/components/Capsule.tsx"; purpose = "Recording capsule and user-visible dictation state." },
            [ordered]@{ path = "src/pages/settings/RecordingSection.tsx"; purpose = "Recording mode and input-source settings." },
            [ordered]@{ path = "src/pages/settings/EmbeddedBleStatusPanel.tsx"; purpose = "Listener BLE connection and device health UI." },
            [ordered]@{ path = "src-tauri/src/coordinator.rs"; purpose = "Backend dictation orchestration and state transitions." },
            [ordered]@{ path = "src-tauri/src/recorder.rs"; purpose = "Local microphone capture." },
            [ordered]@{ path = "src-tauri/src/embedded_ble.rs"; purpose = "Windows BLE discovery, connection, and subscription handling." },
            [ordered]@{ path = "src-tauri/src/embedded_audio.rs"; purpose = "Embedded audio framing and receive path." },
            [ordered]@{ path = "src-tauri/src/asr/"; purpose = "ASR provider implementations and local ASR engines." },
            [ordered]@{ path = "tools/embedded_audio_replay/"; purpose = "BLE/audio replay and smoke tools for desktop-device integration." }
        )
        platform_assumptions = @(
            "Windows is the primary product path for BLE device audio and TSF IME insertion.",
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
            "npm run test",
            "npm run build",
            "npm run verify",
            "npm run check:dark-mode",
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
