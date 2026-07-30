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
- Sensitive KWS values, the two-`Absent` KWS precision rule, explicit-`Absent`
  authority, local-only `PresentLater` rejection, owner verification and the 900 ms
  safety budget are unchanged.

## Machine evidence

- Focused helper concurrency test: PASS; a second confirmation returns busy instead of
  queueing on the helper.
- Focused coordinator busy/fallback test: PASS; busy returns before keyword fallback.
- Focused terminal policy test: PASS; short candidates and two local `Absent` results
  skip offline work, while the remaining path is capped at 500 ms.
- Complete clean-instance Rust run: 925 passed, 19 ignored, 0 failed.
- `npm run release:check`: PASS, including frontend, package, latency and OTA contracts;
  release Rust set was 925 passed, 19 ignored, 0 failed.
- Current-code replay of retained accepted `session 84`: default and recall KWS both
  detect the phrase boundary at 1.420 seconds.
- Final installed ambient `session 165`: five local `Absent` results; terminal stop and
  rejection occurred in the same logged millisecond with no offline cascade.
- Final installed ambient `session 167`: six local `Absent` results; terminal handling
  completed in 45 ms and did not open a capsule.

## Package identity

- Root Type MSI: `ListenerType_1.0.3_x64_en-US.msi`
  - bytes: 15,265,792
  - SHA-256: `FD75B3E6AE7301B9FA8A38AF41970BD661A4C151A1E045D9F210E8C28AB27724`
- Installed Program Files executable SHA-256:
  `D8A48A347FAF1941B7815722F94655D94DBC583AB99548665B49F9E02622899F`
- Root firmware ZIP is unchanged:
  `51F6E00DDBAC07F7238C7D4884CCE60D1DED12B5F0CFAC6F8A586FA22A7D4355`
- Root release-artifact identity check: PASS.

## Human boundary

The first canonical review selected `通过` with note
`哈喽哈喽，有没有什么问题？`, but its matching machine log contained one 988 ms
phrase-tail sample. That result is retained and triaged as non-final rather than used to
override the performance contract.

Final canonical Chinese review remains pending on the priority-lane installed build.
The owner performs the screen and spoken interaction; the agent only reads the resulting
log and stored review.
