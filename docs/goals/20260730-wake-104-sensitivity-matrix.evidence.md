# 1.0.4 Wake sensitivity matrix evidence (in progress)

Date: 2026-07-30  
Baseline Type: `874a0c5` (v1.0.3)  
Firmware (unchanged): `0f085bb` / root OTA ZIP

## Root cause (from saved WAV + live logs)

1. **Session 288** (`wake-candidate-…-session-288-phrase-non-match.wav`, quiet rms≈36):
   - Live: stage1 KWS at **pcm_ms=1920**, then stage2 Absent×2 hard-reject, terminal Reject.
   - Offline full-buffer KWS (score=3.5 / thr=0.05): **end_seconds=3.10** on the same WAV.
   - Conclusion: early KWS + two pre-completion Absents discarded a candidate that still
     contained the full phrase ~1.2 s later; local Paraformer on the ambient-filled window
     never matched.
2. Accepted short positives (sessions 266/312) offline ends ≈0.48–0.60 s and match live
   ExactStart path — healthy baseline for clean near-mic speech.
3. Ambient non-match sessions 336–341: ladder Absent only, no capsule — false-wake control
   intact on ambient speech.

No KWS score/threshold change. No second helper lane. No fixed volume gate.

## Code change (single product fix, two coordinated levers)

| Lever | Behavior |
| --- | --- |
| Post-hit Absent horizon | KWS-path Absent only counts toward the two-Absent hard reject after `first_hit_pcm_ms + 1000 ms` of candidate audio (session-288 horizon). Earlier Absents still retry. |
| Phrase-focus confirm | KWS stage-2 still uses the LST-WAKE-009 ≤5 s tail first; on Absent, one extra confirm on the last **1.6 s** focus tail before spending reject budget. |

Preserved: score=3.5/thr=0.05, single helper, try_lock non-queue, PresentLater local-only hold,
explicit Absent authority after countable evidence, 900 ms secondary budget, 400 ms retry,
firmware/OTA/BLE/LED untouched.

## Unit / machine gates

| Check | Result |
| --- | --- |
| `kws_phrase_focus_tail_is_shorter_than_five_second_cap` | PASS |
| `kws_absent_hard_reject_waits_for_post_hit_phrase_horizon` | PASS |
| `kws_local_confirmation_uses_only_an_aligned_five_second_tail` | PASS |
| Full `cargo test --lib` | 928 passed, 19 ignored, 0 failed |

## Acceptance matrix (install pass required)

| ID | Expected | Machine log fields |
| --- | --- | --- |
| P1 normal | Accept | KWS, relation, confirm_ms, capsule latency |
| P2 soft | Accept | same |
| P3 fast | Accept | same |
| P4 far | Accept | same |
| P5 long-prefix tail | Accept PresentLater or ExactStart | same |
| N1 ambient speech | no capsule | Reject |
| N2 near-phrase | no capsule | Reject |
| N3 “开始” only | no capsule | Reject |
| N4 body with 录音 | no capsule unless full wake | Reject |
| N5 TV/other | no capsule | Reject |
| I1 in-session 开始录音 | remains transcript | LST-WAKE-001 |

## Package identity

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `ListenerType_1.0.4_x64_en-US.msi` (root + `.artifacts`) | 14,082,048 | `342DB5AEEFD207746D5C9AB44CA243093FF4C34442DCDCB0C9E6FD2A947DBA36` |
| Installed `C:\Program Files\Listener Type\listener-type.exe` | 38,962,688 | `D0D4A37A1B2F7CED4863D7842F4FD6739C64DCB98A93D25E973CBD591CBDD630` |
| `ListenerFirmware_1.0.3_ota.zip` (unchanged) | 1,545,390 | `51F6E00DDBAC07F7238C7D4884CCE60D1DED12B5F0CFAC6F8A586FA22A7D4355` |

Installed identity check: MSI payload EXE hash matches Program Files EXE.
Installed process logs: `runtime KWS config phrase=开始录音 score=3.5 threshold=0.05`,
`isolated local confirmation helper ready reason=startup`, `background listener notify ready`.
Binary contains `stage2 early Absent held (phrase horizon)`.

Type commits on master (not tag): `c7d82bc` wake fix + `ff88bf2` OTA headless version assert.
Firmware remains tagged source `0f085bb` (no firmware code change for 1.0.4 wake).

## Live positive/negative matrix

Owner voice + screen acceptance still required for P1–P5 / N1–N5 / I1.
Agent machine path complete through install + ready + helper; live spoken matrix is the owner gate.
