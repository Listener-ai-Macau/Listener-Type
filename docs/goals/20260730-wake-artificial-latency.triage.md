# Operator note triage

Date: 2026-07-30

## Earlier single-helper review

- Selected result: `通过`.
- Operator note: `哈喽哈喽，有没有什么问题？`
- Machine disposition: installed `session 173` accepted correctly with zero missing
  packets, but phrase-tail-to-capsule latency was 988 ms. Later sessions 174 and 175
  measured 85 ms and 29 ms.
- Resolution: retain as positive usability evidence, not final release acceptance.

## Rejected priority-lane review

- Selected result: `失败`.
- Machine disposition: installed `session 212` repeatedly classified short snapshots
  as explicit `Absent` and delayed acceptance until terminal confirmation.
- Resolution: revert the priority helper and preemption experiment. Preserve the
  non-queueing single helper and bounded terminal cleanup. Require a fresh installed
  positive review before final acceptance.

## Prompt clarification

- Selected result: `失败`.
- Operator note: `这个弹窗到底是哪里来的`
- Classification: the note asks about the canonical `aiw operator-prompt` window and
  contains no spoken wake trial.
- Resolution: explain that the window belongs to the workflow, not Listener Type.
  Do not classify it as a product failure or acceptance.

## Final bounded-tail review

- Selected result: `失败`.
- Operator note: empty.
- Matching machine evidence: installed `session 264` accepted an `ExactStart` phrase
  with 344 ms local confirmation, 475 ms total gate compute and 0 ms reported
  phrase-tail-to-capsule latency.
- Resolution: latency correction is machine-positive, but human product acceptance
  remains failed. Do not declare sensitivity and false-wake accuracy accepted until a
  fixed owner positive/near-phrase/ambient matrix passes.

No firmware, OTA, pairing, LED or transcript defect is opened by these notes.
