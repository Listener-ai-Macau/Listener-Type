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
  'const LOW_LATENCY_PREVIEW_ENDPOINT: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel";',
  "matches!(self, Self::FinalTranscript)",
  "has_final && self.role.finish_on_final_frame()",
]) {
  requireIncludes(volcengine, token, "Separate final and preview ASR streams");
}

const finalPayload = section(
  volcengine,
  "VolcengineStreamingRole::FinalTranscript => json!({",
  "VolcengineStreamingRole::LowLatencyPreview => json!({",
  "Final stream payload",
);
for (const token of [
  '"enable_nonstream": true',
  '"end_window_size": SECOND_PASS_END_WINDOW_MS',
  '"force_to_speech_time": SECOND_PASS_FORCE_TO_SPEECH_MS',
]) {
  requireIncludes(finalPayload, token, "Final stream payload");
}

const previewPayload = section(
  volcengine,
  "VolcengineStreamingRole::LowLatencyPreview => json!({",
  "});",
  "Preview stream payload",
);
for (const token of ['"enable_itn": true', '"enable_punc": true']) {
  requireIncludes(previewPayload, token, "Preview stream payload");
}
for (const token of ["enable_nonstream", "end_window_size", "force_to_speech_time"]) {
  requireExcludes(previewPayload, token, "Preview stream payload");
}

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
const fullTimeout = completion.indexOf(
  "final transcript coverage incomplete after full provider timeout",
);
const waitForFinal = completion.indexOf("tokio::time::timeout(remaining, rx)");
if (fullTimeout < 0 || waitForFinal < 0 || fullTimeout < waitForFinal) {
  fail("Final-result completion must wait for the protocol final frame before coverage failure");
}

for (const token of [
  "pub struct FinalIntermediateTranscript",
  "pub authoritative_two_pass: bool",
  "let authoritative_two_pass = candidate.authoritative_cumulative;",
  "self.emit_final_intermediate_transcript(FinalIntermediateTranscript {",
  "authoritative_two_pass,",
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

const startupConsumer = section(
  dictation,
  "struct VolcengineStartupPreviewConsumer",
  "struct VolcenginePreviewSidecar",
  "Live preview audio fan-out",
);
for (const token of [
  "final_bridge: Arc<DeferredAsrBridge>",
  "preview_sidecar: Arc<VolcenginePreviewSidecar>",
  "crate::recorder::AudioConsumer::consume_pcm_chunk(&*self.final_bridge, pcm);",
  "self.preview_sidecar.consume_pcm_chunk(pcm);",
]) {
  requireIncludes(startupConsumer, token, "Live preview audio fan-out");
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
  "PASS: provider-authoritative corrections reach the live preview, final ASR waits for protocol completion before reporting uncovered audio, and the recording path contains no fabricated PCM.",
);
