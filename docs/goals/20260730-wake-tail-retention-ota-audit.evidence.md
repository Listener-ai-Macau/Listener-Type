# Wake tail, diagnostic retention, and OTA audit evidence

Date: 2026-07-30

## Owner OTA audit

The installed pre-change Type completed the owner's full 1.0.3 OTA:

- Payload: `1,389,776/1,389,776` bytes in 2,780 data writes.
- Protocol transfer: 20,578 ms at 66.0 KiB/s.
- Offset recoveries: 0.
- Fixed/non-transfer time: 5,252 ms.
- Total: 26,305 ms.
- Device transition: one old-to-new generation boundary.
- Final response-bearing `TYPE:READY`: 941 ms after the new generation appeared.
- A recording completed normally after OTA with no missing audio packets.

The later `restoring failed OTA` line belongs to the UI's separate post-success firmware
preflight. That read-only preflight deliberately releases its temporary OTA reservation
through the bonded-recovery helper; it did not transfer bytes and is not a second failed
upgrade. The completed OTA itself passes the strict `>60.0 KiB/s`, zero-recovery,
single-reboot and final-ready gates.

## Root correction

- The sensitive live KWS runtime and auxiliary strict/offline runtime now use separate
  cache slots. Auxiliary recall can no longer evict startup's prewarmed live runtime.
- Runtime identity now includes phrase, score, threshold and prefix-variant behavior, so
  strict and sensitive modes cannot accidentally share an incompatible spotter.
- The accepted sensitive score/threshold, explicit-`Absent` authority, local-only
  `PresentLater` rejection and owner gate are unchanged.
- Logs now report both candidate-start-to-capsule and detected-phrase-end-to-capsule
  latency. The owner's slow 3.040-second activation phrase had a 176 ms system tail,
  rather than a 3,216 ms processing delay.

## Rolling diagnostic retention

Default live diagnostics are now pruned asynchronously across process restarts:

- Maximum matching WAV files: 128.
- Maximum matching WAV bytes: 32 MiB.
- Maximum age: 7 days.
- Oldest matching WAVs are removed first.
- Unrelated files and explicitly configured operator directories are not pruned.

On the first candidate after installing the new MSI, cleanup removed 1,103 old matching
files. The directory then contained exactly 128 WAV files totaling 19,052,992 bytes.

## Machine evidence

- `cargo test --lib`: 922 passed, 19 ignored, 0 failed.
- Focused runtime-cache identity test: passed.
- Focused phrase-tail latency test: passed.
- Retention age/count/bytes plan and filesystem tests: passed.
- Formal MSI build and same-version silent install: passed.
- Root artifact check: passed.
- Root MSI SHA-256:
  `AE5D60C75C856E3E7E2489D61BD7E426057F8E8698CC26EA5BD9B5E8ADB892DE`.
- Installed executable SHA-256:
  `5F9000B38E497DD50EA7A8FDBDCC7CC99E7C7018B414A200999FE572F01AFD11`.
- Firmware ZIP SHA-256 remains:
  `51F6E00DDBAC07F7238C7D4884CCE60D1DED12B5F0CFAC6F8A586FA22A7D4355`.
- Startup sensitive KWS prewarm completed in 1,508 ms; BLE notify reached ready.
- Installed session 56 ran auxiliary terminal recall and rejected `Absent`.
- The immediately following installed session 57 still reported the live detector ready
  at 15 ms of buffered PCM, proving auxiliary work did not evict the live runtime.
- Sessions 56 and 57 both rejected non-matching candidates without opening a recording
  capsule.

## Human boundary

The owner completed the canonical Chinese operator review on the installed Program Files
build:

- Selected result: `通过`.
- Operator note: `这个东西能用吗？`
- Positive installed session 65 accepted at 1,043 ms from candidate start and 323 ms
  from the detected phrase end; both configured gates passed.
- The activation phrase was removed at the automatic boundary and the post-wake sentence
  remained intact through final ASR and raw fallback insertion.
- Audio completed with zero missing packets.
- Idle/non-matching candidates were rejected without opening a capsule.

The note is triaged as a clarification request and answered affirmatively for the reviewed
scope. Its selected result and verbatim text are stored in
`20260730-wake-tail-retention-ota-audit.review.json`; detailed disposition is adjacent in
`20260730-wake-tail-retention-ota-audit.triage.md`.
