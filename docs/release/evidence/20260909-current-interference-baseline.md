# Current interference baseline — 2026-09-09

This is the first recorded baseline for the current development candidate after
the owner-presence/interference changes from `c2e9dcc`.

## Reproduction

- Branch: `fix/speech-decision-kernel`
- Source commit at launch: `c2e9dcc9e48aed3d6828a0b224cdb8e76bd417e9`
- Candidate id: `dev-current-20260909`
- Test helper: `scripts/show-cn-interference-development-acceptance.ps1`
- Interference: looped non-owner synthetic Chinese WAV
- Operator action: read the owner prompt while the interference was playing,
  then wait for the capsule to finish

## Result

- Operator selection: `passed`
- Voice-activation candidates observed: `2469201512`, `2469201513`,
  `2469201514`
- Accepted candidate: `2469201513`
- Recording capsule observed: yes
- Done state observed: yes
- Final insertion: `Inserted`
- Final completion log: `target_restored=true`, `user_stop=true`
- Target-speaker log: owner track retained and non-owner chunks suppressed
- Interference acceptance JSON: `.artifacts/acceptance/20260909-current-interference.json`

## Evidence hashes

- Candidate executable SHA-256:
  `083452B0465F5DE6F33958FBFD5B51BCF2FCBF691E164B481B6E68B780685F03`
- Interference WAV SHA-256:
  `75416445E3965C114B37D4847617C5D6083CFBAC02ABED2C08B4F37E167E3256`
- Spoken prompt SHA-256:
  `FBFDCF08E08736827FEB1858B8757B90995A8AAD6CCC810297781272CF1678F9`

## Scope and limitation

This proves one owner-vs-computer-playback overlap path on this machine. It is
not evidence that every human interferer, distance, microphone angle, language,
or prosody is handled. The body filter is intended to be speaker-identity
based; the wake phrase detector remains phrase-specific. A follow-up human A/B
matrix is required before calling the behavior general.
