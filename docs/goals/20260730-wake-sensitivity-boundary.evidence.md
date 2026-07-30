# 2026-07-30 Wake sensitivity and activation-boundary evidence

## Owner contract

- Preserve the sensitive streaming KWS and the bounded local full-phrase confirmation.
- Control false wakes without switching the primary detector to strict mode or globally raising its threshold.
- When KWS misses, allow a local second chance only for a complete start-aligned phrase; a phrase later in ordinary speech cannot independently open a capsule.
- Start that second chance at 1000 ms and require a nonzero, bounded activation endpoint.
- Remove only the idle activation phrase from the automatic session; preserve a later in-session `开始录音` as ordinary dictation.
- Preserve the unenrolled phrase-only open gate and all accepted OTA throughput behavior.

The durable cross-product contracts are `LST-WAKE-003` and `LST-WAKE-004` in
`Listener-Firmware/docs/listener_product_requirements.md`.

## Root cause

Installed-Type session 69 reproduced the remaining sensitivity/boundary defect:
streaming KWS missed, the local ladder did not start until 1800 ms, and
`ExactStart` accepted with `wake_end_s=0.000`. The capsule arrived at 1764 ms and
the retained audio produced `现在开始录录音...`. Session 70 then proved that the
KWS+local path itself was healthy: a 1140 ms KWS hit, 119 ms local confirmation,
`wake_end_s=1.020`, and a 1140 ms wake-to-capsule result.

Session 72 exposed the false-wake side of the same fallback: local ASR may find
the literal phrase later in an ordinary candidate. Local `PresentLater` is now
only a verifier when KWS has also fired; it cannot activate by itself. The
local-only ladder begins at 1000 ms and retries at 1400/1800/2400/3000/5000 ms.

The installed Paraformer model returns tokens but empty timestamp arrays.
For a start-aligned local-only result, Type first attempts a full-buffer default
KWS boundary only when the local transcript contains the four-character phrase
and no body. A result later than 1.2 seconds is not trusted as the activation
boundary because the retained session 69 clip proves it can point to the second,
legitimate in-session occurrence. With body text present, Type estimates only
the phrase share of the start-aligned snapshot and clamps it to 0.55-1.20
seconds. The session-scoped text guard remains a final residue defense and never
searches for or removes a later occurrence.

This follows the public mature pattern: Apple's voice trigger uses a
high-recall first pass followed by a higher-precision second pass and
speaker/directed-speech checks; Amazon documents two-stage wake detection with
an explicit background model. The exact XiaoAI internal detector is not public,
so no undocumented Xiaomi behavior is claimed.

## Machine evidence

| Check | Result |
| --- | --- |
| Focused fusion/prefix/boundary tests | PASS: local-only start accepted, `PresentLater` held, nonzero bounded endpoint, later phrase preserved |
| Full Rust suite | PASS: 936 tests; hardware/network diagnostics remain ignored |
| Prior saved real-candidate replay | PASS: wake `3/3`; negative false trigger `0/17` |
| Fresh strict saved-candidate scan | `1/4` positives and `0/19` false triggers, confirming strict primary mode is not an acceptable sensitivity fix |
| Session 69 full-buffer diagnostic | Default and recall cascade both found `1.84 s`, the later repeated phrase; the new 1.2 s boundary guard rejects it and uses the 0.8 s start estimate |
| Formal MSVC MSI build/install | PASS |
| Installed EXE contains new fusion branch | PASS: installed binary contains `local-only PresentLater held for stage1` |
| Installed startup/recovery | PASS: local helper ready at `2026-07-30T02:49:04.952Z`; background GATT recovered without re-pair at `02:50:46.036Z` and reached `TYPE:READY` |
| Installed 1000 ms ladder | PASS: sessions 21-23 log first threshold `1000`, then bounded `1400/1800/2400/3000/5000` retries |
| Installed ambient-speech candidates | PASS: fresh sessions 21-23 (8860/3140/13700 ms) remained local `Absent`, ended `gate_decision=Reject`, and emitted no recording capsule |
| Post-acceptance Type release gate | PASS: `npm run release:check`; 917 passed, 19 ignored, 0 failed; 20 OTA helper tests passed |
| Post-acceptance Firmware release gate | PASS: version, OTA package rules, V2 board, power, status LED, BLE LED sync, diagnostics, and tooling hygiene |
| Final root artifact identity gate | PASS: MSI and OTA ZIP match their final build-source bytes and SHA-256 identities |

Root package identities:

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `ListenerType_1.0.3_x64_en-US.msi` | 15,228,928 | `3D9BA0B215C3568861374AC5C8CD7B92F266D512539FF995A9910554EC3E67E1` |
| Installed `listener-type.exe` | 38,932,480 | `D655288D47127E85C75C69EC9163FBC8ACE088FC5209854D8EE131B08581958E` |
| `ListenerFirmware_1.0.3_ota.zip` | 1,545,390 | `51F6E00DDBAC07F7238C7D4884CCE60D1DED12B5F0CFAC6F8A586FA22A7D4355` |

## Final human boundary

Completed through the visible, topmost canonical operator prompt on
2026-07-30. The owner personally ran both prompted phrases, selected `通过`, and
then confirmed in chat that the result "感觉还行".

- Prompted ordinary phrase: `我觉得今天可以开始录音测试一下。`
- Prompted wake phrase and body:
  `开始录音，现在开始录音又开始不灵敏，你再精修一下。`
- Archived selected result: `通过`
- Archived operator note:
  `测试一下。现在开始录音又开始不灵敏，你再精修一下。`
- Matching artifacts:
  `20260730-wake-sensitivity-boundary.refined.visible.review.json` and
  `20260730-wake-sensitivity-boundary.refined.visible.note.txt`

This acceptance protects the combined behavior: ordinary speech containing the
phrase later in the sentence does not independently open a capsule; a deliberate
start-aligned wake remains responsive; only the leading activation phrase is
removed and the later body occurrence remains available to dictation.

The first capture in `20260730-wake-sensitivity-boundary.review.json` remains
invalid because shell quoting recorded the MSI argument as the selected result.
The intermediate `refined.review.json` capture is also not human acceptance:
the owner did not see its window. Both are retained as superseded diagnostic
records rather than silently rewritten as acceptance.

## Public references

- Apple, [Hey Siri: An On-device DNN-powered Voice Trigger](https://machinelearning.apple.com/research/hey-siri)
- Apple, [Voice Trigger System for Siri](https://machinelearning.apple.com/research/voice-trigger)
- Amazon Science, [Monophone-based background modeling for two-stage on-device wake word detection](https://www.amazon.science/publications/monophone-based-background-modeling-for-two-stage-on-device-wake-word-detection)
- sherpa-onnx, [OfflineRecognizerResult C API](https://k2-fsa.github.io/sherpa/onnx/c-api/html/structSherpaOnnxOfflineRecognizerResult.html)
- sherpa-onnx, [Offline Paraformer model output examples](https://k2-fsa.github.io/sherpa/onnx/pretrained_models/offline-paraformer/paraformer-models.html)
