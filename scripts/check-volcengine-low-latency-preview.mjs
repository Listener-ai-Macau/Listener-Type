import { readFileSync } from "node:fs";
import { join } from "node:path";

const root = process.cwd();
const volcengine = readFileSync(
  join(root, "src-tauri", "src", "asr", "volcengine.rs"),
  "utf8",
);
const transcript = readFileSync(
  join(root, "src-tauri", "src", "asr", "volcengine_transcript.rs"),
  "utf8",
);
const dictation = [
  "dictation.rs",
  "dictation_preview.rs",
  "dictation_device_ai.rs",
  "dictation_wake_polish.rs",
  "dictation_session.rs",
  "dictation_embedded_submit.rs",
  "dictation_embedded_stream.rs",
  "dictation_tests.rs",
]
  .map((name) =>
    readFileSync(join(root, "src-tauri", "src", "coordinator", name), "utf8"),
  )
  .join("\n");

function fail(message) {
  throw new Error(message);
}

function requireIncludes(source, token, scope) {
  if (!source.includes(token)) fail(`${scope} is missing: ${token}`);
}

function requireExcludes(source, token, scope) {
  if (source.includes(token)) fail(`${scope} must not contain: ${token}`);
}

function section(source, startToken, endToken, scope) {
  const start = source.indexOf(startToken);
  if (start < 0) fail(`${scope} start not found: ${startToken}`);
  const end = source.indexOf(endToken, start + startToken.length);
  if (end < 0) fail(`${scope} end not found: ${endToken}`);
  return source.slice(start, end);
}

for (const token of [
  'const FINAL_TRANSCRIPT_ENDPOINT: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async";',
  'const BIDIRECTIONAL_TRANSCRIPT_ENDPOINT: &str',
  '"wss://openspeech.bytedance.com/api/v3/sauc/bigmodel";',
  'Self::OptimizedBidirectional => FINAL_TRANSCRIPT_ENDPOINT,',
  'Self::Bidirectional => BIDIRECTIONAL_TRANSCRIPT_ENDPOINT,',
  '"enable_nonstream": self.session_options.enable_nonstream',
  'request["end_window_size"] = Value::from(end_window_size_ms);',
  'request["force_to_speech_time"] = Value::from(force_to_speech_time_ms);',
  "self.emit_final_intermediate_transcript(FinalIntermediateTranscript {",
  "authoritative_two_pass,",
]) {
  requireIncludes(volcengine, token, "Authoritative bidirectional ASR session");
}

const defaultSessionOptions = section(
  volcengine,
  'impl Default for VolcengineSessionOptions',
  'enum AudioDeliveryReadiness',
  'Authoritative ASR default session options',
);
for (const token of [
  'endpoint: VolcengineSessionEndpoint::OptimizedBidirectional,',
  'enable_nonstream: true,',
  'end_window_size_ms: Some(SECOND_PASS_END_WINDOW_MS),',
  'force_to_speech_time_ms: Some(SECOND_PASS_FORCE_TO_SPEECH_MS),',
]) {
  requireIncludes(defaultSessionOptions, token, 'Authoritative ASR default session options');
}

for (const token of [
  "LOW_LATENCY_PREVIEW_ENDPOINT",
  "LowLatencyPreview",
  "new_low_latency_preview",
  "low_latency_preview_silent_stalled",
]) {
  requireExcludes(volcengine, token, "Authoritative bidirectional ASR session");
}

for (const token of [
  "VolcenginePreviewSidecar",
  "VolcenginePreviewTeeConsumer",
  "VolcengineStartupPreviewConsumer",
  "VOLCENGINE_PREVIEW_",
  "preview_replay_tail",
  "set_volcengine_partial_preview_callback",
]) {
  requireExcludes(dictation, token, "Live preview routing");
}

const builder = section(
  dictation,
  "fn build_volcengine_asr(",
  "async fn open_volcengine_asr(",
  "Authoritative ASR builder",
);
requireIncludes(
  builder,
  "set_volcengine_preview_callbacks(&asr, inner, session_id);",
  "Authoritative realtime ASR builder",
);

const previewCallbacks = section(
  dictation,
  "fn set_volcengine_preview_callbacks(",
  "fn build_volcengine_asr(",
  "Authoritative realtime ASR callbacks",
);
for (const token of [
  "asr.set_partial_transcript_callback",
  "asr.set_final_intermediate_transcript_callback",
]) {
  requireIncludes(previewCallbacks, token, "Authoritative realtime ASR callbacks");
}
requireIncludes(
  dictation,
  "record_embedded_audio_preview_published",
  "Published preview observability",
);
requireIncludes(
  volcengine,
  "Self::OptimizedBidirectional | Self::Bidirectional => true,",
  "Realtime ASR preview delivery",
);
requireIncludes(
  volcengine,
  ".emits_stream_preview_before_final()",
  "Realtime ASR preview delivery",
);
for (const token of [
  "optimistic_preview_text: String",
  "optimistic_preview_segments: Vec<TranscriptSegment>",
  "last_emitted_preview_text: String",
  "let owner_safe_provider_split_preview =",
  "sequential_speaker_split_gap_is_owner_safe(&state, result, target_text)",
  "transcript_candidate_from_result(if owner_safe_provider_split_preview",
  "&speaker_filtered_result.optimistic_result",
  "state.pending_unattributed_text.clear();",
  "owner-safe sequential provider split admitted to live preview",
  "state.best_transcript_text = merged.clone();",
  "state.last_partial_text = merged.clone();",
]) {
  requireIncludes(volcengine, token, "Display-only provisional preview isolation");
}
const provisionalPreview = section(
  volcengine,
  "if !has_final\n            && pending_unattributed_speech",
  "let prefer_final_optimistic = has_final",
  "Display-only provisional preview path",
);
requireExcludes(
  provisionalPreview,
  "best_transcript_text =",
  "Display-only provisional preview path",
);
requireExcludes(
  provisionalPreview,
  "last_partial_text =",
  "Display-only provisional preview path",
);

const opener = section(
  dictation,
  "async fn open_volcengine_asr(",
  "fn apply_and_publish_dictation_event(",
  "Authoritative ASR opener",
);
requireIncludes(
  opener,
  "authoritative optimized-bidirectional ASR ready; preview and final share one provider session",
  "Authoritative optimized bidirectional ASR opener",
);

for (const token of [
  "AUDIO_KEEPALIVE_INTERVAL",
  "FINAL_SILENCE_FRAMES",
  "FINAL_SILENCE_PADDING_MS",
]) {
  requireExcludes(volcengine, token, "ASR audio transport");
}
for (const token of [
  "EMBEDDED_AUDIO_ASR_PREROLL",
  "feed_embedded_asr_preroll_if_needed",
  "EMBEDDED_AUDIO_TRIM_PAD_SILENCE_MS",
  "trim_embedded_pcm_for_asr",
]) {
  requireExcludes(dictation, token, "Embedded recording path");
}

const batchSubmission = section(
  dictation,
  "async fn submit_embedded_pcm_for_dictation_with_stats",
  "fn embedded_streaming_chunk_is_asr_input",
  "Batch embedded audio submission",
);
requireIncludes(
  batchSubmission,
  "let (asr_pcm, gain_stats) = normalize_embedded_pcm_for_asr(pcm);",
  "Batch embedded audio submission",
);

const completion = section(
  volcengine,
  "pub async fn await_final_result_with_timeout",
  "fn final_partial_coverage_gap",
  "Final-result completion",
);
for (const token of [
  "const FINAL_RESULT_TIMEOUT: Duration = Duration::from_secs(12);",
  "tokio::time::timeout(timeout, &mut rx)",
  "final transcript coverage incomplete after full provider timeout",
  "VolcengineASRError::FinalResultCoverageIncomplete",
  "provider final result timed out after {} ms",
]) {
  requireIncludes(volcengine, token, "Final-result completion");
}
requireExcludes(completion, "FINAL_RESULT_UNCOVERED_AUDIO_GRACE", "Final-result completion");

for (const token of [
  "pub struct FinalIntermediateTranscript",
  "pub authoritative_two_pass: bool",
  "let authoritative_two_pass = candidate.authoritative_cumulative && !two_pass_empty_final;",
]) {
  requireIncludes(volcengine, token, "Provider-authoritative correction metadata");
}
for (const token of [
  "pub(super) authoritative_cumulative: bool",
  "result_has_authoritative_two_pass_correction",
  'source == "two_pass"',
  "authoritative_cumulative: has_authoritative_two_pass_correction",
]) {
  requireIncludes(transcript, token, "Two-pass authority classification");
}

const finalSupplement = section(
  dictation,
  "fn update_embedded_audio_partial_preview_from_final_supplement",
  "fn provider_preview_change",
  "Final supplement handoff",
);
for (const token of [
  "crate::asr::volcengine::FinalIntermediateTranscript",
  "let authoritative_two_pass = update.authoritative_two_pass;",
  "provider_preview_change(slot.as_deref(), &preview)",
]) {
  requireIncludes(finalSupplement, token, "Final supplement handoff");
}

const providerPreviewPolicy = section(
  dictation,
  "fn provider_preview_change",
  "fn stabilize_embedded_audio_partial_preview",
  "Provider preview replacement policy",
);
for (const token of [
  "current.is_some_and(|value| value.trim() == candidate)",
  "Some(candidate.to_string())",
]) {
  requireIncludes(providerPreviewPolicy, token, "Provider preview replacement policy");
}
if (providerPreviewPolicy.includes("embedded_audio_partial_preview_stability_key")) {
  fail("Provider preview replacement must not reintroduce heuristic text suppression");
}

console.log(
  "PASS: default preview and final use one authoritative optimized bidirectional ASR session; its early stream preview and later two-pass correction both reach the live preview without a replaying sidecar.",
);
