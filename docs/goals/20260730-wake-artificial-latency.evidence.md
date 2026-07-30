# Artificial wake latency evidence

Date: 2026-07-30

## Root cause and correction

- Installed pre-fix `session 84` hit streaming KWS at 1,140 ms but waited behind an
  earlier local helper request, then keyword-fallback opened the capsule at 1,922 ms;
  detected-phrase-end-to-capsule was 1,082 ms.
- Discarded `spawn_blocking` confirmations could continue and queue indefinitely on the
  helper process mutex. Confirmation now uses non-queueing single-flight acquisition.
  Busy returns immediately and the live candidate retries without entering the
  unavailable keyword-only fallback.
- Installed first-fix `session 131` exposed a second serialized stall: after its stop
  packet, an ambient-only terminal offline recall kept the BLE actor occupied for about
  4.3 seconds.
- The final implementation records all completed midstream local `Absent` results.
  Two results skip terminal offline recall. When that evidence is unavailable, the
  auxiliary recovery path remains available but releases the actor after 500 ms.
- Restored installed `session 232` exposed the remaining late-hit cost: streaming KWS
  located the phrase at 20.28 seconds, but stage-2 received the entire 20.65-second
  ambient candidate. It did not finish inside the 900 ms safety budget, so keyword
  timeout fail-open produced a 719 ms phrase-tail-to-capsule delay.
- KWS-triggered stage-2 now receives only the aligned most-recent 5,000 ms PCM window.
  Short candidates and all exploratory local confirmations remain byte-for-byte
  unchanged. The absolute streaming-KWS boundary still controls transcript offset and
  latency reporting.
- Sensitive KWS values, the two-`Absent` KWS precision rule, explicit-`Absent`
  authority, local-only `PresentLater` rejection, owner verification and the 900 ms
  safety budget are unchanged.

## Rejected experiment

- A separate prewarmed KWS-priority helper and exploratory-task preemption were tested
  after one 988 ms outlier.
- Installed `session 212` produced repeated explicit `Absent` results on short snapshots
  and delayed a valid activation until terminal full-candidate confirmation at about
  17.08 seconds. The owner marked the matching canonical review as failed.
- The experiment was reverted. Product requirement `LST-WAKE-008` prohibits restoring
  it without a new machine loop that preserves sensitivity.

## Machine evidence

- Focused helper concurrency test: PASS; a second confirmation returns busy instead of
  queueing on the helper.
- Focused coordinator busy/fallback test: PASS; busy returns before keyword fallback.
- Focused terminal policy test: PASS; short candidates and two local `Absent` results
  skip offline work, while the remaining path is capped at 500 ms.
- Complete restored-code Rust run after the late-hit correction: 926 passed,
  19 ignored, 0 failed.
- Final `npm run release:check`: PASS, including frontend, release packaging,
  recording latency, BLE recovery and OTA contracts; its Rust run also reported
  926 passed, 19 ignored and 0 failed.
- Final installed ambient `session 165`: five local `Absent` results; terminal stop and
  rejection occurred in the same logged millisecond with no offline cascade.
- Final installed ambient `session 167`: six local `Absent` results; terminal handling
  completed in 45 ms and did not open a capsule.
- Restored installed Type BLE status probe: PASS in 334 ms, connected to `Blistener`
  with firmware 1.0.3 and audio/OTA readiness present.

## Restored package identity

- Root Type MSI: `ListenerType_1.0.3_x64_en-US.msi`
  - bytes: 15,265,792
  - SHA-256: `F3582510D3FA7B954A54C55F906947D3277B246CAE178B8902E4E3D484F7B531`
- Installed Program Files executable SHA-256:
  `EEFC1331A07ABB5E6BC1DA82F9E753EB58FD023FF8E644FBBECEF359B4EE56BA`
- Root firmware ZIP is unchanged:
  `51F6E00DDBAC07F7238C7D4884CCE60D1DED12B5F0CFAC6F8A586FA22A7D4355`
- Root release-artifact identity check: PASS.
- Final installed Type BLE status probe: PASS in 248 ms with firmware 1.0.3 and
  audio/OTA readiness present.

## Human boundary

The first canonical review selected `通过` with note
`哈喽哈喽，有没有什么问题？`, but its matching machine log contained one 988 ms
phrase-tail sample. It remains non-final. The priority-lane review was a failure and is
stored separately. A later prompt result asking where the prompt came from is triaged as
tooling clarification without a product trial. Final acceptance remains pending on the
installed bounded-tail single-helper build.
