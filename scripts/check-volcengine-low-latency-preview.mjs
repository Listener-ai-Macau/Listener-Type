import { readFileSync } from "node:fs";
import { join } from "node:path";

const repoRoot = process.cwd();
const volcenginePath = join(repoRoot, "src-tauri", "src", "asr", "volcengine.rs");
const transcriptPath = join(repoRoot, "src-tauri", "src", "asr", "volcengine_transcript.rs");
const dictationPath = join(repoRoot, "src-tauri", "src", "coordinator", "dictation.rs");

const volcengine = readFileSync(volcenginePath, "utf8");
const transcript = readFileSync(transcriptPath, "utf8");
const dictation = readFileSync(dictationPath, "utf8");

function fail(message) {
  throw new Error(message);
}

function section(source, startToken, endToken, label) {
  const start = source.indexOf(startToken);
  if (start < 0) fail(`Could not locate ${label} start token: ${startToken}`);
  const end = source.indexOf(endToken, start + startToken.length);
  if (end < 0) fail(`Could not locate ${label} end token: ${endToken}`);
  return source.slice(start, end);
}

function requireIncludes(source, token, label) {
  if (!source.includes(token)) fail(`${label} missing required token: ${token}`);
}

function requireExcludes(source, token, label) {
  if (source.includes(token)) fail(`${label} must not contain token: ${token}`);
}

requireIncludes(
  volcengine,
  'const FINAL_TRANSCRIPT_ENDPOINT: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async";',
  "Volcengine endpoint contract",
);
requireIncludes(
  volcengine,
  'const LOW_LATENCY_PREVIEW_ENDPOINT: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel";',
  "Volcengine endpoint contract",
);
requireIncludes(
  volcengine,
  "matches!(self, Self::FinalTranscript)",
  "Volcengine final ownership contract",
);
requireIncludes(
  volcengine,
  "has_final && self.role.finish_on_final_frame()",
  "Volcengine final ownership contract",
);
requireIncludes(
  volcengine,
  "preview final frame kept alive for continued low-latency preview",
  "Volcengine long-recording preview contract",
);

const finalPayload = section(
  volcengine,
  "VolcengineStreamingRole::FinalTranscript => json!({",
  "VolcengineStreamingRole::LowLatencyPreview => json!({",
  "final ASR first-frame payload",
);
for (const token of [
  '"model_name": "bigmodel"',
  '"enable_nonstream": true',
  '"enable_punc": true',
  '"show_utterances": true',
  '"result_type": "full"',
  '"end_window_size": SECOND_PASS_END_WINDOW_MS',
  '"force_to_speech_time": SECOND_PASS_FORCE_TO_SPEECH_MS',
]) {
  requireIncludes(finalPayload, token, "final ASR first-frame payload");
}

const previewPayload = section(
  volcengine,
  "VolcengineStreamingRole::LowLatencyPreview => json!({",
  "});",
  "preview ASR first-frame payload",
);
for (const token of [
  '"model_name": "bigmodel"',
  '"enable_itn": true',
  '"enable_punc": true',
  '"show_utterances": true',
  '"result_type": "full"',
]) {
  requireIncludes(previewPayload, token, "preview ASR first-frame payload");
}
for (const token of [
  '"enable_nonstream"',
  '"end_window_size"',
  '"force_to_speech_time"',
]) {
  requireExcludes(previewPayload, token, "preview ASR first-frame payload");
}

const teeSection = section(
  dictation,
  "impl crate::asr::AudioConsumer for VolcenginePreviewTeeConsumer",
  "impl Drop for VolcenginePreviewTeeConsumer",
  "Volcengine preview tee",
);
const previewFeed = teeSection.indexOf("self.preview_sidecar.consume_pcm_chunk(pcm);");
const finalFeed = teeSection.indexOf("consume_pcm_chunk(&*self.final_asr");
if (previewFeed < 0 || finalFeed < 0) {
  fail("Volcengine preview tee must feed both preview_asr and final_asr");
}
if (previewFeed > finalFeed) {
  fail("Volcengine preview tee must feed the low-latency preview before the final ASR path");
}

const previewSidecar = section(
  dictation,
  "struct VolcenginePreviewSidecar",
  "fn set_volcengine_partial_preview_callback",
  "Volcengine preview sidecar watchdog",
);
for (const token of [
  "VOLCENGINE_PREVIEW_STALL_RESTART_MS",
  "VOLCENGINE_PREVIEW_STALL_MIN_FRAMES",
  "VOLCENGINE_PREVIEW_REPLAY_BYTES",
  "VOLCENGINE_PREVIEW_MAX_RESTARTS",
  "open_initial_preview_in_background",
  "low_latency_preview_silent_stalled",
  "restart_silent_preview_if_needed",
  "restart_preview(preview, attempt).await",
  "VolcengineStreamingASR::new_low_latency_preview",
  "set_volcengine_partial_preview_callback(&replacement",
  "old_preview.cancel();",
  "replay_bytes",
]) {
  requireIncludes(previewSidecar, token, "Volcengine preview sidecar watchdog");
}
requireIncludes(
  dictation,
  "const VOLCENGINE_PREVIEW_REPLAY_MS: usize = 1_500;",
  "Volcengine preview sidecar replay must stay small enough to avoid flooding the ASR writer",
);
requireIncludes(
  dictation,
  "const VOLCENGINE_PREVIEW_MAX_RESTARTS: usize = 1;",
  "Volcengine preview sidecar restart must be bounded so it cannot dominate the final ASR path",
);
const restartInFlightGuard = previewSidecar.indexOf("self.restarting.swap(true, Ordering::SeqCst)");
const restartQuotaIncrement = previewSidecar.indexOf("self.restart_count.fetch_add(1, Ordering::SeqCst)");
if (restartInFlightGuard < 0 || restartQuotaIncrement < 0) {
  fail("Volcengine preview sidecar watchdog must guard in-flight restarts and count restart attempts");
}
if (restartInFlightGuard > restartQuotaIncrement) {
  fail("Volcengine preview sidecar watchdog must not burn restart quota while a restart is already in flight");
}
requireIncludes(
  previewSidecar,
  "self.restarting.store(false, Ordering::SeqCst);",
  "Volcengine preview sidecar watchdog restart limit cleanup",
);

const pairBuilder = section(
  dictation,
  "fn build_volcengine_asr_pair",
  "async fn open_volcengine_asr_pair",
  "Volcengine ASR pair builder",
);
for (const token of [
  "VolcengineStreamingASR::new(",
  "VolcengineStreamingASR::new_low_latency_preview(",
  "set_volcengine_partial_preview_callback(&final_asr",
  "set_volcengine_partial_preview_callback(&preview_asr",
  "VolcenginePreviewSidecar::new(",
  "VolcenginePreviewTeeConsumer",
]) {
  requireIncludes(pairBuilder, token, "Volcengine ASR pair builder");
}

const pairOpen = section(
  dictation,
  "async fn open_volcengine_asr_pair",
  "fn apply_and_publish_dictation_event",
  "Volcengine ASR pair opener",
);
for (const token of [
  "final_asr.open_session().await?",
  "final ASR ready; preview sidecar will open asynchronously",
  "pair.preview_sidecar.open_initial_preview_in_background();",
  "Ok(())",
]) {
  requireIncludes(pairOpen, token, "Volcengine ASR pair opener");
}
requireExcludes(
  pairOpen,
  "tokio::join!",
  "Volcengine ASR pair opener must not wait for preview before final ASR can start",
);
const finalOpen = pairOpen.indexOf("final_asr.open_session().await?");
const previewBackgroundOpen = pairOpen.indexOf(
  "pair.preview_sidecar.open_initial_preview_in_background();",
);
if (finalOpen < 0 || previewBackgroundOpen < 0 || finalOpen > previewBackgroundOpen) {
  fail("Volcengine final ASR must be ready before the preview sidecar starts asynchronously");
}

requireIncludes(
  dictation,
  "ActiveAsr::Volcengine(Arc::clone(&volcengine.final_asr))",
  "Volcengine final ASR storage",
);
requireExcludes(
  dictation,
  "ActiveAsr::Volcengine(Arc::clone(&volcengine.preview_asr))",
  "Volcengine final ASR storage",
);

for (const token of [
  "response_frames_seen",
  "partial_updates_seen",
  "pub fn low_latency_preview_silent_stalled",
]) {
  requireIncludes(volcengine, token, "Volcengine preview progress tracking");
}

const unstableInitialPartial = section(
  transcript,
  "pub(super) fn is_unstable_initial_partial",
  "fn is_same_prefix_streaming_revision",
  "Volcengine initial preview filter",
);
requireIncludes(
  unstableInitialPartial,
  "is_initial_hesitation_partial(&compact)",
  "Volcengine initial preview filter",
);
requireExcludes(
  unstableInitialPartial,
  "char_count <= 3",
  "Volcengine initial preview filter",
);
for (const token of [
  'assert!(!is_unstable_initial_partial("", "短"));',
  'assert!(!is_unstable_initial_partial("", "短句"));',
  'assert!(!is_unstable_initial_partial("", "测试"));',
  "trim_repeated_short_final_tail_removes_prefix_echo_after_sentence",
]) {
  requireIncludes(transcript, token, "Volcengine initial preview filter tests");
}

console.log(
  "PASS: Volcengine preview remains a lean low-latency sidecar, early Chinese content is not blanket-filtered, and final transcript stays on the accurate async/two-pass path.",
);
