# EC11 double-click pairing ownership fix (LST-PAIR-002)

Date: 2026-07-30  
Type HEAD base: `e49e45f` + uncommitted atomic pairing worktree  
Firmware registry: LST-PAIR-001 / LST-PAIR-002 (firmware package unchanged)

## Root cause

Post-`TYPE:READY` delayed ghost-prune and EC11 Type-controlled recovery both used the
pairing-maintenance guard. Recovery saw the lock busy, returned `RetrySoon`, and fell
back into the stale-address GATT open ladder (~7 s per unreachable attempt). Serial
timeouts stacked into ~50 s of “listener 无法连接” before eventual recovery.

Not fixed by longer retries, sleeps, adapter restart, Settings detours, or fuzzy name cleanup.

## Code boundary

One Type-owned Windows pairing transaction for EC11 recovery:

1. `begin_listener_pairing_maintenance_after_wait` (≤5 s) acquires ownership once.
2. Exact known-address cleanup via `_inner` (no second public lock).
3. Direct `PairAsync` via `prompt_listener_pairing_inner(..., maintenance_already_held=true)`.
4. Optional exact BTHPORT fallback still under the same guard.
5. Final PairAsync still under the same guard; public wrappers that re-lock are not used.

Non-Type-controlled recovery keeps conservative busy/`RetrySoon` behavior.

Files (uncommitted on Type master):

- `src-tauri/src/coordinator/embedded_ble_runtime.rs`
- `src-tauri/src/coordinator_tests.rs`
- `src-tauri/src/embedded_ble/mod.rs`
- `src-tauri/src/embedded_ble/windows_ble/pairing.rs`
- `src-tauri/src/embedded_ble/windows_ble_tests.rs`

## Machine gates

| Check | Result |
| --- | --- |
| Atomic ownership review | PASS: one wait-acquire; cleanup/PairAsync/fallback use `_inner` or `maintenance_already_held` |
| `git diff --check` | PASS |
| `cargo check` | PASS |
| `cargo test --lib` | 928 passed, 19 ignored, 0 failed |
| `npm test` / `npm run build` / `check:version` | PASS |
| Installed MSI identity | PASS (see below) |
| EC11 recovery script ×3+ | See timing table |
| Embedded audio after recovery | PASS: session 65 complete `pcm_bytes=1939200` `packets=4499` `missing_packets=0` |
| Owner physical prompt | selected `失败`; note `本机没有弹窗弹出来, 别的机器有` |

## Installed package identity

| Item | Value |
| --- | --- |
| MSI | `ListenerType_1.0.4_x64_en-US.msi` |
| MSI SHA-256 | `B6DFF205F92D20A5CF35533CB249DAC6E2569B2E2A9FA2B6DEBAB47E19CBE1AD` |
| Installed EXE | `C:\Program Files\Listener Type\listener-type.exe` |
| EXE SHA-256 | `C5FDA0802304D72E992CF4B3A790CBCD4860879B30CC54543612221E73321678` |
| Release build EXE SHA-256 | same as installed |
| FileVersion / ProductVersion | `1.0.4` |
| Process path | Program Files only |

Binary markers present: `atomic Type recovery known-address cleanup`,
`pairing maintenance ownership acquired after wait`.

## EC11 generated recovery timings (COM3, installed Type)

| Run | status | trigger→READY | fresh pair→READY | adv accept | notes |
| --- | --- | ---: | ---: | ---: | --- |
| 1 | PASS | 4470 | 2570 | 70 | atomic cleanup+PairAsync |
| 2 | FAIL (script) | 4690 | 2550 | **565** | only firmware adv>250 ms; Type pairing gates OK |
| 3 | PASS | 4400 | 2570 | 69 | |
| 4 | PASS | 4900 | 2720 | 89 | re-run for third full PASS |

Contract focus (all runs including #2):

- trigger→TYPE:READY ≤ 10000 ms: **PASS**
- fresh pairing→TYPE:READY ≤ 6000 ms: **PASS**
- no multi-round stale GATT timeout ladder in Type log: **PASS** (ownership wait 0 ms → atomic cleanup → PairAsync paired)

Artifacts: `Listener-Type/.cache/validation/ec11-pair-atomic-20260730/`

## Residual baseline issues

- `check:module-budgets`: `dictation_embedded_stream.rs` > 2000 lines (pre-existing; not touched).
- `check:traceability` historical gaps (do not `--write`).
- Firmware worktree has unrelated `managed_components` deletions; not touched.
- Owner notes multi-machine residual (“本机没有弹窗…别的机器有”); this workstation machine loop is green, full product acceptance not claimed.
- CLI `--submit-embedded-audio-ble-once` can fail while background notify already owns the link; background session still proves PCM after recovery.

## Human boundary

Canonical operator prompt selected **失败** with note **本机没有弹窗弹出来, 别的机器有**.
Do not mark LST-PAIR-002 owner-accepted until a physical pass is recorded (preferably on a machine that previously showed the failure dialog).
