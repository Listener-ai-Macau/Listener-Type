# Voice-activation origin and fallback evidence

Date: 2026-07-30

## Observed timeline

- Session 150 started with origin `VoiceActivation`; no matching button or hotkey event
  appears in the runtime timeline.
- Stage 1 reported a keyword hit at `pcm_ms=9300`.
- The local verifier returned `Absent` once and scheduled the bounded retry.
- At `waited_ms=917`, the previous timeout path exceeded its 900 ms budget and accepted
  the candidate as `KeywordModel`, reversing the earlier explicit absence.
- Session 151 also started with origin `VoiceActivation`; no matching button or hotkey
  event appears in its timeline.
- Its keyword hit at `pcm_ms=1005` was followed by a local `ExactStart` confirmation and
  activation at `pcm_ms=1025`.

These traces rule out a physical button event as the software origin. They do not, by
themselves, prove whether session 151 contained a real spoken activation phrase. Final
classification of the retained 1.185-second candidate remains a human listening boundary.

## Root correction

The shared secondary-fallback decision now permits keyword-only fail-open only when the
secondary verifier is unavailable or times out before returning any explicit `Absent`
result. Once an explicit absence exists, timeout, helper unavailability and task failure
all preserve that evidence and keep the bounded confirmation path from opening a capsule.

No KWS score, phrase threshold, 900 ms budget, 400 ms retry schedule, unenrolled open gate
or 1200/1500 ms latency budget changed. A fixed amplitude threshold was intentionally
rejected because retained quiet true-positive candidates overlap the incident sample.

## Machine evidence

- Platform BLE host tests: 28 passed.
- Platform voice-activation tests: 13 passed.
- Platform observability tests: 6 passed.
- Platform full `tools/verify.ps1`: passed, including 20/20 C tests and Rust/doc tests.
- Listener Type `cargo test --manifest-path src-tauri/Cargo.toml --lib`:
  918 passed, 19 ignored, 0 failed.
- Focused Type regression:
  `explicit_absent_blocks_keyword_only_secondary_fallback` passed.
- Shared Platform implementation commit:
  `ad2cc2ee6381ec0cdefe93a8cc2341c2c4f0e733`.

## Installed build

- Type source commit: `e3cff98873cdb065e830615b8f94f9330b5d9134`.
- Formal MSI:
  `<release-root>/ListenerType_1.0.3_x64_en-US.msi`.
- MSI SHA-256:
  `9CD5080A8E6CDD0F3F8352A398ED14DB348B5EB783B272303D0429F7B7215532`.
- Installed executable:
  `C:\Program Files\Listener Type\listener-type.exe`.
- Installed executable/MSI payload SHA-256:
  `CA7B62FC9BA9265E68A4D7A94E5BEEFE503059CC38EF61097EB0FDC25170180E`.
- Installed process identity: PID 5820, Program Files executable.
- Startup trace: background BLE `notify ready` and response-bearing `TYPE:READY`,
  followed by the isolated local confirmation helper becoming ready.

## Human boundary

Listen to:

`%LOCALAPPDATA%\Listener Type\Logs\wake-diag-live\wake-candidate-133-session-151-accepted.wav`

Record whether the activation phrase is audibly present. Energy statistics alone are not
accepted evidence for changing wake sensitivity.

## Final human acceptance

The owner exercised the installed build and accepted the combined sensitivity/false-wake
behavior in the canonical Chinese review window:

- Selected result: `通过`.
- Operator note: `足够灵敏，未开始时没有主动观察到录音胶囊。还行吧。`
- Artifact:
  `docs/goals/20260730-wake-sensitivity-boundary.origin-fallback.review.json`.

The note is triaged as acceptance of the current installed behavior with no new defect or
follow-up requirement. It does not classify the older session 151 WAV by listening, so
that historical acoustic ambiguity remains explicitly separate from final product
acceptance.
