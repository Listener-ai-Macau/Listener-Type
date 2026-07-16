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
const dictation = readFileSync(
  join(root, "src-tauri", "src", "coordinator", "dictation.rs"),
  "utf8",
);

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
  'endpoint: VolcengineSessionEndpoint::OptimizedBidirectional,',
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
  "set_volcengine_final_supplemental_preview_callback(&asr, inner, session_id);",
  "Authoritative ASR builder",
);

const opener = section(
  dictation,
  "async fn open_volcengine_asr(",
  "fn apply_and_publish_dictation_event(",
  "Authoritative ASR opener",
);
requireIncludes(
  opener,
  "authoritative bidirectional ASR ready; preview and final share one provider session",
  "Authoritative ASR opener",
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
  "fn complete_from_stable_partial_after_finish_grace",
  "Final-result completion",
);
for (const token of [
  "const FINAL_RESULT_TIMEOUT: Duration = Duration::from_secs(12);",
  "waiting up to {} ms for protocol final frame",
  "tokio::time::timeout(remaining, rx)",
  "final transcript coverage incomplete after full provider timeout",
  "VolcengineASRError::FinalResultCoverageIncomplete",
]) {
  requireIncludes(volcengine, token, "Final-result completion");
}
requireExcludes(completion, "FINAL_RESULT_UNCOVERED_AUDIO_GRACE", "Final-result completion");

for (const token of [
  "pub struct FinalIntermediateTranscript",
  "pub authoritative_two_pass: bool",
  "let authoritative_two_pass = candidate.authoritative_cumulative;",
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
  "fn stabilize_embedded_audio_partial_preview",
  "Final supplement handoff",
);
for (const token of [
  "crate::asr::volcengine::FinalIntermediateTranscript",
  "let authoritative_two_pass = update.authoritative_two_pass;",
  "stabilize_embedded_audio_final_supplemental_preview_with_provider_authority(",
]) {
  requireIncludes(finalSupplement, token, "Final supplement handoff");
}

const stabilization = section(
  dictation,
  "fn stabilize_embedded_audio_final_supplemental_preview_with_provider_authority",
  "fn embedded_audio_final_supplement_adds_decorative_progress",
  "Authoritative preview correction",
);
for (const token of [
  "if authoritative_two_pass && current_key != candidate_key",
  "applied provider-authoritative two-pass preview correction",
  "return Some(candidate.to_string());",
]) {
  requireIncludes(stabilization, token, "Authoritative preview correction");
}
const authorityRewrite = stabilization.indexOf("if authoritative_two_pass && current_key != candidate_key");
const heuristicRewrite = stabilization.indexOf("embedded_audio_final_supplement_is_brief_bounded_revision");
if (authorityRewrite < 0 || heuristicRewrite < 0 || authorityRewrite > heuristicRewrite) {
  fail("Provider-authoritative preview correction must take precedence over heuristic rewrites");
}

console.log(
  "PASS: preview and final use one authoritative bidirectional ASR session; provider-authoritative two-pass corrections reach the live preview without a replaying sidecar.",
);
