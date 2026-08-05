#!/usr/bin/env node
/**
 * Listener 1.0.5 protected source contracts.
 * Reuses 1.0.4 anti-regression gates and adds long-form + stop feedback contracts.
 */
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const typeRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));

function read(rel, root = typeRoot) {
  const path = join(root, rel);
  assert.ok(existsSync(path), `missing required file: ${path}`);
  return readFileSync(path, "utf8");
}

function mustMatch(source, re, label) {
  const m = source.match(re);
  assert.ok(m, `contract missing: ${label} / ${re}`);
  return m;
}

function mustInclude(source, needle, label) {
  assert.ok(source.includes(needle), `contract missing: ${label} => ${needle}`);
}

const results = [];
function gate(id, fn) {
  try {
    fn();
    results.push({ id, status: "PASS" });
    console.log(`[PASS] ${id}`);
  } catch (error) {
    results.push({ id, status: "FAIL", error: String(error?.message || error) });
    console.error(`[FAIL] ${id}: ${error?.message || error}`);
  }
}

// Inherit frozen 1.0.4 contracts first — 1.0.5 must not regress them.
const baseline = spawnSync(
  process.execPath,
  [join(typeRoot, "scripts", "verify-listener-1.0.4-protected-contracts.mjs")],
  { cwd: typeRoot, encoding: "utf8" },
);
process.stdout.write(baseline.stdout || "");
process.stderr.write(baseline.stderr || "");
if (baseline.status !== 0) {
  console.error("[FAIL] 1.0.4 baseline protected contracts");
  process.exit(baseline.status ?? 1);
}
results.push({ id: "baseline_1.0.4_protected_contracts", status: "PASS" });
console.log("[PASS] baseline_1.0.4_protected_contracts");

const dictation = read("src-tauri/src/coordinator/dictation.rs");
const dictationTests = read("src-tauri/src/coordinator/dictation_tests.rs");
const typesRs = read("src-tauri/src/types.rs");
const typesTs = read("src/lib/types.ts");
const packageJson = read("package.json");
const recordingSection = read("src/pages/settings/RecordingSection.tsx");
const supportRs = read("src-tauri/src/coordinator/support.rs");
const capsulePreviewRules = read("src/lib/capsulePreviewRules.ts");

// Owner release line: ship current UX as **1.0.4** (not a separate 1.0.5 product).
// Extended contracts below remain the 1.0.4+ experience gates.
gate("product_version_1.0.4", () => {
  mustMatch(packageJson, /"version"\s*:\s*"1\.0\.4"/, "package.json version 1.0.4");
});

gate("standard_endpoint_still_1000ms_default", () => {
  mustMatch(
    dictation,
    /EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS:\s*u64\s*=\s*1_000/,
    "standard endpoint remains 1000 ms",
  );
  mustInclude(dictation, "target_speaker_inactive_1000ms", "standard stop reason");
  mustInclude(
    dictation,
    "fn target_speaker_end_timeout_ms",
    "timeout selected from long-form preference",
  );
});

gate("long_form_endpoint_2000ms_optional", () => {
  mustMatch(
    dictation,
    /EMBEDDED_TARGET_SPEAKER_LONG_FORM_END_TIMEOUT_MS:\s*u64\s*=\s*2_000/,
    "long-form endpoint 2000 ms",
  );
  mustInclude(dictation, "target_speaker_inactive_2000ms", "long-form stop reason");
  mustInclude(typesRs, "long_form_dictation", "Rust preference field");
  mustInclude(typesTs, "longFormDictation", "TS preference field");
  mustInclude(recordingSection, "longFormDictation", "settings toggle wired");
  mustInclude(
    dictationTests,
    "long_form_endpoint_requires_two_seconds_without_that_speaker",
    "long-form endpoint unit test",
  );
});

gate("stop_to_transcribing_immediate_feedback", () => {
  mustInclude(
    dictation,
    "stop_to_transcribing_ms",
    "stop→Transcribing latency log field",
  );
  mustInclude(
    dictation,
    "request_embedded_audio_stop_feedback(inner, stop_reason)",
    "endpoint path latches Transcribing before async BLE stop",
  );
});

const deviceSection = read("src/pages/settings/DeviceSection.tsx");
const volcengineAsr = read("src-tauri/src/asr/volcengine.rs");
const wakePolish = read("src-tauri/src/coordinator/dictation_wake_polish.rs");
const deviceAi = read("src-tauri/src/coordinator/dictation_device_ai.rs");
const coordinatorRs = read("src-tauri/src/coordinator.rs");
const windowsImeSessionRs = read("src-tauri/src/windows_ime_session.rs");

gate("recording_automation_polish_and_long_form", () => {
  mustInclude(deviceSection, "longFormDictation", "long-form toggle in automation settings");
  mustInclude(deviceSection, "dictationPolishLabel", "polish style control in automation");
  mustInclude(deviceSection, "setActiveStylePack", "style pack activation for faithful/clean");
  mustInclude(deviceSection, "voiceprintNoisyHint", "noisy-room expectation copy");
});

// Installed empty wake 72519330: before body text exists, auto-end must not use
// snappy 1.0s on the wake clock (scared owner with "没有识别到语音").
gate("wake_no_body_uses_longer_abandon_timeout", () => {
  mustInclude(
    dictation,
    "EMBEDDED_AUTOMATIC_WAKE_NO_BODY_END_TIMEOUT_MS",
    "wake-no-body abandon timeout constant",
  );
  mustMatch(
    dictation,
    /EMBEDDED_AUTOMATIC_WAKE_NO_BODY_END_TIMEOUT_MS:\s*u64\s*=\s*3_000/,
    "wake-no-body abandon is 3000 ms",
  );
  mustInclude(
    dictation,
    "target_speaker_inactive_no_body_3000ms",
    "wake-no-body stop reason",
  );
  mustInclude(
    dictation,
    "automatic_wake_body_started",
    "body-started latch gates snappy endpoint",
  );
  mustInclude(
    dictationTests,
    "automatic_wake_no_body_uses_longer_endpoint_timeout",
    "wake-no-body unit test",
  );
});

// Installed mid-sentence cut 19df34c4: pending provisional body must block
// auto-end, and provisional growth must refresh the owner endpoint clock.
gate("no_mid_sentence_cut_pending_blocks_endpoint", () => {
  mustInclude(
    dictation,
    "let pending_blocks_endpoint = update.pending_unattributed_speech;",
    "pending unattributed speech always blocks auto-end",
  );
  mustInclude(
    dictationTests,
    "Pending provisional body must never auto-end",
    "unit test documents pending blocks endpoint",
  );
  mustInclude(
    dictationTests,
    "local_wake_still_pending",
    "local authority + pending must not endpoint",
  );
});

gate("no_mid_sentence_cut_provisional_refreshes_clock", () => {
  mustInclude(
    volcengineAsr,
    "fn refresh_local_target_from_owner_preview_activity",
    "owner preview activity refreshes local target clock",
  );
  mustInclude(
    volcengineAsr,
    "provisional body growth refreshed local target endpoint clock",
    "provisional body growth logs clock refresh",
  );
  mustInclude(
    volcengineAsr,
    "pending_activity_advanced",
    "provisional activity advances with pending text changes",
  );
});

gate("snappy_insert_when_llm_auth_or_tsf_unavailable", () => {
  // Owner sessions 7ce523e5/6a86d8e3: 401 + TSF inactive → streaming FAILED +
  // slow paste. Contract: circuit-open skips polish; TSF-not-ready skips submit.
  mustInclude(
    dictation,
    "LLM auth circuit open; inserting raw transcript without polish wait",
    "auth circuit skips polish for snappy raw insert",
  );
  mustInclude(
    dictation,
    "llm_auth_blocked",
    "polish dispatch logs auth-block gate",
  );
  mustInclude(
    coordinatorRs,
    "TSF not activated at recording start; retrying prepare at insert time",
    "retry TSF prepare once at insert when recording-start activate failed",
  );
  mustInclude(
    coordinatorRs,
    "TSF still not activated at insert; using non-TSF insert path immediately",
    "skip failed TSF submit when prepare never activated",
  );
  mustInclude(
    coordinatorRs,
    "non-TSF IME-safe Unicode insert status=Inserted",
    "IME-safe Unicode is the preferred non-TSF true-insert path",
  );
  mustInclude(
    coordinatorRs,
    "non-TSF clipboard paste path status=",
    "clipboard paste remains non-TSF reliability fallback",
  );
  mustInclude(
    windowsImeSessionRs,
    "TSF not registered",
    "skip doomed ActivateProfile when TSF DLL is not installed",
  );
});

gate("multi_speaker_owner_isolation", () => {
  // 1.0.5: dual-gate isolation — only owner text enters ledger/final.
  mustInclude(
    volcengineAsr,
    "owner_isolation_frozen",
    "owner isolation freeze state exists",
  );
  mustInclude(
    volcengineAsr,
    "freeze_owner_isolation_ledger",
    "NonTarget restabilize freezes owner ledger",
  );
  mustInclude(
    volcengineAsr,
    "clamp_to_owner_isolation_ceiling",
    "merges clamp to owner ceiling while frozen",
  );
  mustInclude(
    volcengineAsr,
    "target_votes > 0 && target_votes > non_target_votes",
    "local evidence requires strict Target majority",
  );
  mustInclude(
    volcengineAsr,
    "isolation_freeze_blocks_polluted_final_over_owner_ceiling",
    "isolation freeze unit test",
  );
  mustInclude(
    volcengineAsr,
    "room_speech_raw_stream_does_not_inflate_optimistic_while_local_non_target",
    "room stream optimistic freeze unit test",
  );
});

gate("snappy_success_capsule_close", () => {
  // Owner: 结尾拖. Text is already on screen; old 1050ms Done linger felt slow.
  // Keep a brief checkmark, then out — ≤400ms hide + ≤120ms exit.
  mustMatch(
    supportRs,
    /CAPSULE_SUCCESS_HIDE_DELAY_MS:\s*u64\s*=\s*([1-9]\d{0,2}|[1-3]\d{2})\b/,
    "success hide delay is sub-second",
  );
  {
    const m = supportRs.match(/CAPSULE_SUCCESS_HIDE_DELAY_MS:\s*u64\s*=\s*(\d+)/);
    if (!m || Number(m[1]) > 400) {
      throw new Error(
        `CAPSULE_SUCCESS_HIDE_DELAY_MS must be ≤400 (got ${m ? m[1] : "missing"})`,
      );
    }
  }
  mustMatch(
    capsulePreviewRules,
    /lingerMs:\s*(\d+)/,
    "frontend linger mirrors success hide",
  );
  {
    const m = capsulePreviewRules.match(/lingerMs:\s*(\d+)/);
    if (!m || Number(m[1]) > 400) {
      throw new Error(`lingerMs must be ≤400 (got ${m ? m[1] : "missing"})`);
    }
  }
  mustMatch(
    capsulePreviewRules,
    /exitAnimMs:\s*(\d+)/,
    "frontend exit animation present",
  );
  {
    const m = capsulePreviewRules.match(/exitAnimMs:\s*(\d+)/);
    if (!m || Number(m[1]) > 120) {
      throw new Error(`exitAnimMs must be ≤120 (got ${m ? m[1] : "missing"})`);
    }
  }
});

gate("insert_failure_clipboard_visible_copy", () => {
  // Success path is silent: text already on screen; loud "已粘贴原文/仍在剪贴板"
  // made working sessions feel broken (LLM key 401). Failures still guide paste.
  mustInclude(
    wakePolish,
    "InsertStatus::Inserted | InsertStatus::PasteSent => None",
    "successful type/paste capsule is silent",
  );
  mustInclude(
    wakePolish,
    "上屏失败，内容在剪贴板，请 Ctrl+V",
    "failed insert with retained clipboard is explicit",
  );
  mustInclude(
    wakePolish,
    "润色不可用，已复制原文，请 Ctrl+V",
    "polish failure + copy fallback is explicit",
  );
  mustInclude(
    dictationTests,
    "default_done_message_treats_raw_insert_after_polish_failure_as_successful_fallback",
    "done-message unit test",
  );
});

gate("stop_to_done_latency_observability", () => {
  mustInclude(dictation, "stop_to_done_ms=", "stop→Done latency log field");
  mustInclude(
    deviceAi,
    "fn take_stop_to_done_ms",
    "stop feedback Instant is consumed at completion",
  );
  mustInclude(
    coordinatorRs,
    "dictation_stop_feedback_at",
    "Inner stores stop feedback Instant for UX metrics",
  );
});

const failed = results.filter((r) => r.status === "FAIL");
if (failed.length) {
  console.error(`\n1.0.5 contracts FAILED: ${failed.map((f) => f.id).join(", ")}`);
  process.exit(1);
}
console.log(`\n1.0.5 contracts PASS (${results.length} gates)`);
