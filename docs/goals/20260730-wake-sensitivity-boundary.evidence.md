# 2026-07-30 Wake sensitivity and activation-boundary evidence

## Owner contract

- Preserve the sensitive streaming KWS and the bounded local full-phrase confirmation.
- Control false wakes without switching the primary detector to strict mode or globally raising its threshold.
- Remove only the idle activation phrase from the automatic session; preserve a later in-session `开始录音` as ordinary dictation.
- Preserve the unenrolled phrase-only open gate and all accepted OTA throughput behavior.

The durable cross-product contract is `LST-WAKE-003` in
`Listener-Firmware/docs/listener_product_requirements.md`.

## Root cause

Installed-Type log sessions 21 and 22 showed the local confirmation returning
`ExactStart` with exactly four transcript characters at 2775 ms and 5055 ms.
The streaming KWS boundary was only 1915 ms and 4315 ms. Both capsule previews
therefore began with the activation phrase. The shared 800 ms difference came
from the KWS lookback floor, not from wake sensitivity.

The fix refines the PCM cut to the local snapshot only when that snapshot
contains the wake phrase and no dictated body. A session-scoped prefix guard
removes a full or bounded-tail activation residue from automatic preview/final
text. It never searches for or removes the phrase later in the transcript.

## Machine evidence

| Check | Result |
| --- | --- |
| Focused prefix/boundary tests | PASS |
| Full Rust suite | PASS: 933 tests; hardware/network diagnostics remain ignored |
| Saved real-candidate replay | PASS: wake `3/3`; negative false trigger `0/17` |
| Formal MSVC MSI build/install | PASS |
| Installed EXE equals release payload | PASS: `7604FAC5BE9729F85EECBFE0E7B39751A884F62A38DA9E62E03CE0057BF2E8E8` |
| Installed startup | PASS: `TYPE:READY` at `2026-07-30T02:22:59.677Z`; local helper ready at `02:23:00.468Z` |
| Installed ambient-speech candidate | PASS: session 68, 6560 ms, local `Absent`, `gate_decision=Reject`, no recording capsule |

Root package identities:

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `ListenerType_1.0.3_x64_en-US.msi` | 15,245,312 | `3D780AD720B6EDCCD2BCB71F418B64474313FDC0D9CC3DC274E0F465B56C39CF` |
| `ListenerFirmware_1.0.3_ota.zip` | 1,545,390 | `51F6E00DDBAC07F7238C7D4884CCE60D1DED12B5F0CFAC6F8A586FA22A7D4355` |

## Final human boundary

Pending one physical owner pass on the installed MSI:

1. Say `开始录音，现在开始录音又开始不灵敏`.
2. Confirm the capsule starts with `现在...`, preserves the second `开始录音`,
   and opens promptly.
3. Continue ordinary nearby speech without the activation phrase and confirm no
   unexplained recording capsule appears.

The selected result and operator note must be archived next to this evidence.
