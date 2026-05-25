# Embedded Audio Stop Boundary 1.2 Validation

Date: 2026-05-25
Repo: Listener-Type
Branch: ai/oai-embedded-audio-stop-boundary-1.2

## Implementation

- Added `SessionStats` diagnostics for `asrBoundaryPcmBytes`, `asrBoundaryDurationSeconds`, `postStopPacketCount`, `postStopPcmBytes`, and `postStopDurationSeconds`.
- `SessionCollector` now keeps full transport reconstruction separate from the ASR boundary reconstruction.
- Batch embedded audio submission uses `reconstructed_asr_boundary_pcm()` for ASR while preserving full packet statistics.
- Streaming embedded audio marks PCM chunks received after `SESSION_STOP` and excludes those chunks from ASR input while still allowing them to complete BLE statistics.
- Existing cancel/error/missing-packet paths remain collector-driven; `SESSION_STOP` tail packets only affect diagnostics and completeness.

## Validation

- PASS: `npm run build` (created `dist`; existing Vite large chunk warning only).
- PASS: `cargo test --manifest-path src-tauri\Cargo.toml --lib --quiet` (390 passed).
- PASS: `npm test`.
- PASS: `cargo fmt --manifest-path src-tauri\Cargo.toml -- --check`.
- PASS: `git diff --check`.
- PASS: manual merge-base check: `deab11b` (BLE idle hotfix merge) is an ancestor of this branch.
- PASS: `cargo check --manifest-path src-tauri\Cargo.toml`.
- PASS: `npm run verify`.

## Self-Review

- Acceptance checked: post-stop audio is counted for diagnostics but excluded from ASR input.
- Tail diagnostics include zero-tail sessions via explicit zero values.
- Tests cover audio arriving after STOP and the ASR-input gate for post-stop chunks.
- Diff scope is limited to embedded audio stats/collector, embedded dictation routing, tests, and this validation note.
