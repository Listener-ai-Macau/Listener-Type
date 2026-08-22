#!/usr/bin/env node
/**
 * Listener 1.0.5 protected source contracts.
 * The historical filename is retained because release entrypoints reference it.
 * Fails if accepted thresholds regress from the current product contract.
 */
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const typeRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const firmwareRoot = resolve(typeRoot, "..", "Listener-Firmware");

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

const dictation = read("src-tauri/src/coordinator/dictation.rs");
const dictationTests = read("src-tauri/src/coordinator/dictation_tests.rs");
const typesRs = read("src-tauri/src/types.rs");
const packageJson = read("package.json");
const startupScript = read("scripts/check-type-startup-reconnect-speed.ps1");
const otaSpeedTest = read("scripts/check-ota-transfer-speed-log.test.mjs");
const firmwareOtaTs = read("src/lib/firmwareOta.test.ts");
const supportRs = read("src-tauri/src/coordinator/support.rs");
const endpointPolicy = read("src-tauri/src/coordinator/dictation_endpoint_policy.rs");
const wakePolish = read("src-tauri/src/coordinator/dictation_wake_polish.rs");
const deviceAi = read("src-tauri/src/coordinator/dictation_device_ai.rs");
const coordinatorRs = read("src-tauri/src/coordinator.rs");
const polishRs = read("src-tauri/src/polish.rs");
const volcengineAsr = read("src-tauri/src/asr/volcengine.rs");
const windowsImeSessionRs = read("src-tauri/src/windows_ime_session.rs");
const deviceSection = read("src/pages/settings/DeviceSection.tsx");
const capsuleTsx = read("src/components/Capsule.tsx");
const capsulePreviewRules = read("src/lib/capsulePreviewRules.ts");
const capsulePreviewRulesTest = read("src/lib/capsulePreviewRules.test.ts");
const multiSpeakerTimelineCatalog = read("scripts/multi-speaker-timeline-scenarios.json");
const multiSpeakerTimelineRunner = read("scripts/run-multi-speaker-timeline-regressions.mjs");
const speaker = existsSync(join(typeRoot, "src-tauri/src/speaker_verification.rs"))
  ? read("src-tauri/src/speaker_verification.rs")
  : "";
const persistence = read("src-tauri/src/persistence.rs");
const powerManager = existsSync(join(firmwareRoot, "components/power_manager/power_manager.c"))
  ? read("components/power_manager/power_manager.c", firmwareRoot)
  : "";
const voiceRecording = existsSync(
  join(firmwareRoot, "components/voice_recording_control/voice_recording_control.c"),
)
  ? read("components/voice_recording_control/voice_recording_control.c", firmwareRoot)
  : "";

// ─── Protected baseline contracts ───────────────────────────────────────────

gate("local_confirmation_800ms", () => {
  mustMatch(
    dictationTests,
    /assert_eq!\(\s*LOCAL_CONFIRMATION_START_MS,\s*800\s*\)/,
    "dictation_tests asserts 800 ms confirmation",
  );
  mustMatch(
    wakePolish + dictation + dictationTests,
    /LOCAL_CONFIRMATION_START_MS(?:\s*:\s*usize)?\s*=\s*800/,
    "LOCAL_CONFIRMATION_START_MS = 800",
  );
});

gate("target_speaker_end_1000ms", () => {
  mustMatch(
    dictation,
    /EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS:\s*u64\s*=\s*1_000/,
    "target speaker end timeout 1000 ms",
  );
  mustInclude(dictation, "target_speaker_inactive_1000ms", "auto-stop reason string");
  mustInclude(
    dictation,
    "authoritative_preview_growth_has_recent_owner_speech",
    "real owner text growth renews the independent firmware endpoint clock",
  );
  mustInclude(
    dictationTests,
    "installed_session_363_preview_growth_renews_firmware_before_one_second",
    "installed session 363 cannot stop 640 ms after continuing owner speech",
  );
  mustInclude(
    dictationTests,
    "preview_growth_firmware_refresh_rejects_punctuation_stale_and_other_speaker",
    "punctuation, stale speech, and other speakers cannot prolong the endpoint",
  );
});

gate("wake_only_body_initial_wait_3000ms", () => {
  mustMatch(
    dictation,
    /EMBEDDED_AUTOMATIC_BODY_INITIAL_WAIT_MS:\s*u64\s*=\s*3_000/,
    "automatic wake-only body initial wait 3000 ms",
  );
});

// Owner-accepted 1.0.4 smooth live preview: fluid optimistic partials, no empty
// wipe on two_pass_empty, no other-speaker clock extension, progressive burst
// reveal, and same-session empty payload must not clear the capsule text.
gate("smooth_preview_optimistic_stream", () => {
  mustInclude(
    volcengineAsr,
    "optimistic partial update",
    "optimistic partial log path for live preview",
  );
  mustInclude(
    volcengineAsr,
    "local_speaker_allows_optimistic_preview",
    "Target-gated optimistic preview gate",
  );
  mustInclude(
    volcengineAsr,
    "commit_session_transcript_if_stronger",
    "Target stream promotes into session speech ledger",
  );
  mustInclude(
    volcengineAsr,
    "fn session_committed_transcript",
    "session speech ledger for finalization",
  );
});

gate("smooth_preview_empty_final_ledger", () => {
  mustInclude(volcengineAsr, "fn result_marks_two_pass_empty", "detect two_pass_empty finals");
  mustInclude(
    volcengineAsr,
    "two_pass_empty_final_seals_stream_without_erasing_session_speech",
    "two_pass_empty must keep session speech",
  );
  mustInclude(
    volcengineAsr,
    "protocol final is two_pass_empty seal-only",
    "seal-only log for empty protocol final",
  );
  mustInclude(
    dictation,
    "empty ASR final recovered from partial preview",
    "coordinator recovers empty final from partial preview",
  );
});

gate("smooth_preview_other_speaker_no_clock_extend", () => {
  mustInclude(
    volcengineAsr,
    "transient_non_target_does_not_extend_owner_endpoint_clock",
    "NonTarget must not refresh owner endpoint clock",
  );
  mustInclude(
    volcengineAsr,
    "other_person_speech_does_not_lengthen_owner_auto_end",
    "other person talking must not lengthen auto-end",
  );
  mustMatch(
    volcengineAsr,
    /SessionSpeakerClassification::Target[\s\S]{0,200}local_target_speech_end_ms/,
    "local_target_speech_end_ms only advances on Target classification",
  );
});

gate("smooth_preview_stable_attributed_endpoint", () => {
  mustInclude(
    dictationTests,
    "target_speaker_endpoint_uses_newest_stable_attributed_boundary_after_diarization_flip",
    "diarization flip must not cut on stale target boundary",
  );
  mustInclude(
    dictation,
    "stable_attributed_speech_end_ms",
    "endpoint clock chains stable attributed speech",
  );
});

gate("smooth_preview_capsule_preserve_and_burst", () => {
  mustInclude(
    capsuleTsx,
    "shouldPreserveMessageWithoutPayload",
    "same-session empty payload must preserve preview",
  );
  mustMatch(
    capsulePreviewRules,
    /maxCatchUpMs:\s*160/,
    "progressive burst reveal catch-up budget 160 ms",
  );
  // Ceiling contract: settle must stay ≤160ms. Faster (e.g. 110) is an improvement.
  mustMatch(
    capsulePreviewRules,
    /enterAnimMs:\s*(?:[1-9]|[1-9]\d|1[0-5]\d|160)\b/,
    "wake capsule geometry settle within 160 ms",
  );
  {
    const m = capsulePreviewRules.match(/enterAnimMs:\s*(\d+)/);
    if (!m || Number(m[1]) > 160) {
      throw new Error(
        `wake capsule enterAnimMs must be ≤160 (got ${m ? m[1] : "missing"})`,
      );
    }
  }
  mustInclude(
    capsulePreviewRulesTest,
    "burst reveal should stay within the frame budget",
    "burst reveal unit contract",
  );
  mustInclude(
    capsulePreviewRulesTest,
    "every reveal frame must be an exact target prefix",
    "reveal frames stay pure prefixes",
  );
});

gate("default_settings", () => {
  mustInclude(typesRs, "send_key_after_dictation: false", "auto-send default off");
  mustInclude(typesRs, "copy_dictation_to_clipboard: true", "clipboard retention default on");
  mustInclude(typesRs, "show_capsule: true", "capsule default on");
  mustInclude(
    typesRs,
    "dictation_input_source: DictationInputSource::EmbeddedBle",
    "default embedded BLE input",
  );
  mustInclude(typesRs, "restore_clipboard_after_paste: true", "restore clipboard default on");
  mustInclude(typesRs, "remove_filler_words: true", "filler removal default on");
});

gate("notify_ready_3000", () => {
  mustMatch(
    startupScript,
    /\[int\]\$MaxNotifyReadyMs\s*=\s*3000/,
    "warm start notify-ready gate 3000 ms",
  );
});

gate("ota_throughput_gate_contract", () => {
  const otaRegression = existsSync(
    join(firmwareRoot, "tools", "verify_listener_103_ota_regression.py"),
  )
    ? read("tools/verify_listener_103_ota_regression.py", firmwareRoot)
    : "";
  mustInclude(
    otaRegression,
    "strictly more than 60.0 KiB/s",
    "firmware OTA regression still requires bulk_kb_s > 60.0",
  );
  mustInclude(otaSpeedTest + firmwareOtaTs, "bulk_kb_s", "Type OTA log parser retains bulk_kb_s");
  mustInclude(
    otaSpeedTest + otaRegression,
    "offset recover",
    "offset recovery accounting retained",
  );
});

gate("feature_surface", () => {
  mustInclude(typesRs, "voice_wake_phrase", "custom wake phrase field");
  mustInclude(typesRs + speaker, "speaker", "speaker/voiceprint surface");
  mustInclude(dictation, "raw_input_level_percent", "raw capsule level surface");
  assert.ok(
    existsSync(join(typeRoot, "src/lib/firmwareOta.test.ts")),
    "firmware OTA frontend tests present",
  );
});

gate("wake_idle_boundary", () => {
  // Product contract: voice activation only while non-idle with BT light on.
  // Assert the firmware-side sources directly — the desktop bundle always
  // contains "idle" somewhere, so the old combined-regex gate could never fail
  // even when the firmware checkout was missing entirely.
  assert.ok(
    powerManager.length > 0,
    "firmware power_manager.c must be readable (firmware checkout missing?)",
  );
  assert.ok(
    voiceRecording.length > 0,
    "firmware voice_recording_control.c must be readable (firmware checkout missing?)",
  );
  mustInclude(powerManager, "low_power", "firmware low-power management surface");
  mustInclude(powerManager, "idle", "firmware idle tracking surface");
  mustInclude(
    voiceRecording,
    "voice_auto_start",
    "firmware voice auto-start gate surface",
  );
  mustInclude(
    voiceRecording,
    "voice_activation",
    "firmware voice activation surface",
  );
});

gate("clean_test_data_dir_contract", () => {
  mustInclude(
    persistence,
    "listener-type-test-data-",
    "test data dir prefix outside production profile",
  );
  mustInclude(persistence, "LISTENER_TYPE_DATA_DIR", "override env for isolated runs");
  mustInclude(
    persistence,
    "default_test_data_dir_is_process_scoped_and_outside_production_profile",
    "unit test for production isolation",
  );
});

// ─── Recording-UX extension gates ──────────────────────────────────────────

gate("product_version_1.0.5", () => {
  mustMatch(packageJson, /"version"\s*:\s*"1\.0\.5"/, "package.json version 1.0.5");
});

gate("standard_endpoint_still_1000ms_default", () => {
  mustMatch(
    dictation,
    /EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS:\s*u64\s*=\s*1_000/,
    "standard endpoint remains 1000 ms",
  );
  mustInclude(dictation, "target_speaker_inactive_1000ms", "standard stop reason");
  mustInclude(
    dictation + endpointPolicy,
    "fn target_speaker_end_timeout_ms_for_preview",
    "body endpoint helper",
  );
  for (const forbidden of [
    "EMBEDDED_TARGET_SPEAKER_INCOMPLETE_BODY_END_TIMEOUT_MS",
    "EMBEDDED_TARGET_SPEAKER_UNCERTAIN_TAIL_END_TIMEOUT_MS",
    "EMBEDDED_TARGET_SPEAKER_SEMANTIC_CONTINUATION_END_TIMEOUT_MS",
    "EMBEDDED_TARGET_SPEAKER_SHORT_BODY_END_TIMEOUT_MS",
    "target_speaker_inactive_1500ms",
    "target_speaker_guarded_tail_2000ms",
    "target_speaker_inactive_2500ms",
  ]) {
    if (dictation.includes(forbidden)) {
      throw new Error(`body endpoint must not retain slow preview ladder: ${forbidden}`);
    }
  }
  mustInclude(
    dictationTests,
    "body_preview_endpoint_extends_only_explicit_dangling_continuations",
    "complete previews keep one second while explicit dangling continuations stay bounded",
  );
  mustInclude(
    dictationTests,
    "incomplete_and_short_body_previews_keep_one_second_endpoint",
    "incomplete and short-body one-second unit test",
  );
  mustInclude(
    dictationTests,
    "owner_identity_uncertainty_does_not_slow_the_one_second_endpoint",
    "uncertain voiceprint one-second unit test",
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

gate("recording_automation_settings_copy", () => {
  mustInclude(deviceSection, "voiceprintNoisyHint", "noisy-room expectation copy");
});

gate("dual_bank_voiceprint_enrollment", () => {
  mustInclude(speaker, "TemplatePurpose::WakePhrase", "fixed-phrase template bank");
  mustInclude(speaker, "TemplatePurpose::FreeSpeech", "free-speech template bank");
  mustMatch(
    speaker,
    /DUAL_TEMPLATE_WINDOWS_PER_BANK:\s*usize\s*=\s*3/,
    "three embeddings per enrollment bank",
  );
  mustMatch(
    speaker,
    /ENROLLMENT_WAKE_MIN_ACTIVE_FRAMES_PER_STEP:\s*usize\s*=\s*6/,
    "each of the three guided wake samples contains at least 600 ms active speech",
  );
  mustInclude(
    speaker,
    "let session = wake.clone();",
    "the same three guided samples seed the wake and session banks without a fourth prompt",
  );
  mustInclude(
    speaker,
    "runtime_evaluates_listener_labeled_speaker_corpus",
    "owner/non-owner model evaluation gate",
  );
  mustInclude(speaker, "duration_slices", "short/medium/long evaluation slices");
  mustInclude(speaker, "quality_slices", "clean/noisy/far-field evaluation slices");
  mustInclude(
    speaker,
    "applied.owner_samples >= 5 && applied.non_owner_samples >= 5",
    "minimum per-slice owner/non-owner corpus coverage",
  );
  mustInclude(
    speaker,
    "let started = std::time::Instant::now();",
    "model-only local inference timing starts immediately before embedding",
  );
  mustInclude(
    speaker,
    "template.session_embeddings.clone()",
    "session tracking uses free-speech templates",
  );
  mustInclude(
    speaker,
    "enrollment_accepts_three_minimum_length_phrases_with_volume_variation",
    "three guided phrase samples pass per-step quality gating across normal volume variation",
  );
  mustInclude(
    speaker,
    "completed_enrollment_cancels_late_host_stop",
    "an early device endpoint cancels the delayed host STOP",
  );
  mustInclude(
    speaker,
    "session_speaker_signal_metrics(speech)?",
    "session exclusion uses active speech duration rather than pause-spanning duration",
  );
  mustInclude(
    speaker,
    "session_speaker_exclusion_uses_active_speech_not_pause_spanning_duration",
    "sparse speech with a long pause cannot become confident NonTarget evidence",
  );
});

gate("multi_speaker_timeline_matrix", () => {
  const catalog = JSON.parse(multiSpeakerTimelineCatalog);
  assert.equal(catalog.schema, "listener.multi_speaker_timeline_scenarios");
  assert.ok(catalog.scenarios.length >= 10, "at least ten explicit multi-speaker scenarios");
  assert.ok(
    catalog.scenarios.filter((scenario) => scenario.kind === "overlap").length >= 3,
    "at least three overlap scenarios",
  );
  mustInclude(
    multiSpeakerTimelineRunner,
    "transcript_pass && scenario.endpoint_pass && scenario.lifecycle_pass",
    "each timeline requires transcript, endpoint, and lifecycle evidence",
  );
  mustInclude(
    multiSpeakerTimelineRunner,
    "transcript_body_retained: false",
    "timeline evidence excludes transcript body text",
  );
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
  mustInclude(
    dictationTests,
    "host_started_wake_guard_survives_embedded_session_begin",
    "host-start / continuation wake guard must survive session begin",
  );
  mustInclude(
    dictation,
    "if !automatic_wake_session_active(inner, current_session_id)",
    "begin_embedded_audio preserves host-started wake guard",
  );
});

// Installed mid-sentence cut 19df34c4: pending owner/unknown provisional body
// must block auto-end. Confirmed nearby speech is the only exception, otherwise
// another person can keep the owner's session open indefinitely.
gate("pending_owner_blocks_but_confirmed_other_does_not", () => {
  mustMatch(
    dictation,
    /let pending_blocks_endpoint = update\.pending_unattributed_speech\s*&& !\(recent_local_speech_is_non_target && provider_other_speaker_advanced\);/,
    "pending speech blocks unless local and provider evidence confirm another speaker",
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
  mustInclude(
    dictationTests,
    "confirmed_other_speaker_does_not_extend_endpoint_via_provider_attribution",
    "confirmed nearby speech must not hold the owner endpoint",
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
    "LLM circuit open (auth={llm_auth_blocked} stall={llm_stall_blocked}); inserting raw transcript without polish wait",
    "auth/stall circuits skip polish for snappy raw insert",
  );
  mustInclude(
    dictation,
    "llm_auth_blocked",
    "polish dispatch logs auth-block gate",
  );
  mustInclude(
    dictation,
    "llm_stall_blocked",
    "polish dispatch logs stall-block gate",
  );
  mustInclude(
    coordinatorRs,
    "LLM_STALL_OPEN_AFTER_FAILURES: u32 = 2",
    "stall circuit opens after two consecutive failures",
  );
  mustInclude(
    coordinatorRs,
    "LLM_STALL_OPEN_DURATION: Duration = Duration::from_secs(120)",
    "stall circuit cools down 120s then half-opens",
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

gate("polish_first_content_fallback_1500ms", () => {
  mustMatch(
    polishRs,
    /POLISH_STREAM_START_TIMEOUT:\s*Duration\s*=\s*Duration::from_millis\(1_500\)/,
    "polish first-content deadline is 1500 ms",
  );
  mustInclude(
    polishRs,
    "first_content_deadline.saturating_duration_since",
    "role-only and keepalive chunks cannot reset the first-content deadline",
  );
  mustInclude(
    polishRs,
    "polish_streaming_fails_fast_when_response_headers_are_delayed",
    "delayed response-header regression",
  );
  mustInclude(
    polishRs,
    "polish_streaming_fails_fast_when_provider_never_emits_content",
    "content-free HTTP 200/SSE regression",
  );
});

gate("multi_speaker_owner_isolation", () => {
  // Dual-gate isolation — only owner text enters ledger/final.
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
  mustInclude(
    volcengineAsr,
    "installed_session_768_recovers_raw_body_without_prior_stable_attribution",
    "wake-only provider final must recover the recognized owner body",
  );
  mustInclude(
    volcengineAsr,
    "local_consecutive_transcript_hard_non_target",
    "transcript isolation must not reuse endpoint identity hysteresis",
  );
  mustInclude(
    volcengineAsr,
    "moderate_same_owner_negative_pair_remains_advisory",
    "same-owner low-score regression remains protected",
  );
  mustInclude(
    volcengineAsr,
    "short_hard_mismatch_pair_isolates_transcript_without_changing_endpoint_clock",
    "short extreme mismatch isolates only transcript growth",
  );
  mustInclude(
    volcengineAsr,
    "stage_owner_isolation_checkpoint",
    "first extreme mismatch checkpoints the authoritative owner ledger",
  );
  mustInclude(
    volcengineAsr,
    "freeze_owner_isolation_from_staged_checkpoint",
    "confirmed mismatch rolls every transcript ledger back to the checkpoint",
  );
  mustInclude(
    volcengineAsr,
    "isolated_extreme_mismatch_discards_staged_checkpoint_without_rollback",
    "one transient mismatch cannot roll back continued owner speech",
  );
  mustInclude(
    readFileSync(join(typeRoot, "src-tauri", "src", "asr", "volcengine_untimed_merge.rs"), "utf8"),
    "installed_session_230_growing_cumulative_revisions_never_inflate_preview",
    "growing untimed provider revisions cannot inflate then collapse the preview",
  );
});

gate("multi_speaker_unit_tests_green", () => {
  // String contracts cannot catch semantic drift between the isolation
  // implementation and its tests (the two b29d803-born red tests proved it).
  // Actually run the volcengine suite in the release gate.
  const run = spawnSync(
    "cargo",
    ["test", "--release", "--lib", "asr::volcengine"],
    { cwd: join(typeRoot, "src-tauri"), encoding: "utf8" },
  );
  if (run.error) {
    throw new Error(`cargo test failed to start: ${run.error.message}`);
  }
  if (run.status !== 0) {
    const tail = (run.stdout || "").split("\n").slice(-12).join("\n");
    throw new Error(`asr::volcengine tests red:\n${tail}`);
  }
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
const summary = {
  schema: "listener.1.0.5.protected_contracts",
  schema_version: 1,
  result: failed.length === 0 ? "PASS" : "FAIL",
  type_root: typeRoot,
  firmware_root: firmwareRoot,
  gates: results,
};
console.log(JSON.stringify(summary, null, 2));
if (failed.length) {
  console.error(`\n1.0.5 contracts FAILED: ${failed.map((f) => f.id).join(", ")}`);
  process.exit(1);
}
console.log(`\n1.0.5 contracts PASS (${results.length} gates)`);
