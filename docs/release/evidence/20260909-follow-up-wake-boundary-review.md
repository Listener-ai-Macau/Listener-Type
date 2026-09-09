# 2026-09-09 follow-up wake-boundary review

This is a development review record, not 1.0.5 release evidence.

## Scope

- formal baseline: `v1.0.5-open-source-baseline` (`e5e2a92`)
- follow-up source: `a347517`
- candidate under review: delayed exact-start wake-boundary handling
- related earlier follow-up: `3a08b24` final-frame owner-tail recovery

## Observed failure

The manual interference run did not produce a valid pass record. In the
representative terminal candidate, local confirmation arrived only after the
capture window had reached `4480 ms`:

- `phrase_relation=ExactStart`
- `wake_end_s=4.360`
- `post_wake_pcm_ms=0`
- the continuation session reached `Done`, but its final inserted text was
  only `14` characters

This is evidence that the old boundary could consume the first owner sentence
as wake audio. It is not evidence that the follow-up fix is accepted; a fresh
owner-plus-interference run still has to verify that the sentence is retained.

## Decision

The follow-up remains a 1.0.6 candidate. Automated Rust tests, `cargo check`,
the desktop build, and repository hygiene checks may pass while the product
still lacks human acceptance for this runtime path. Do not move this record
into the 1.0.5 baseline or update the release tag until the operator records a
successful retest.

The focused ASR replay suite now passes: `114` executable tests passed and `2`
credential/audio-backed tests were ignored. The published multi-speaker matrix
also passes all `10` catalog scenarios. The complete `cargo test --lib` suite
is still not green on this branch: `1284` passed, `19` historical coordinator
and endpoint-replay tests failed, and `31` were ignored. Those failures remain
a separate release blocker, not something to hide behind the focused tests.

## Current-branch MSI identity check

This development branch was packaged and installed for an identity check only;
it is not a new 1.0.5 acceptance record:

- `npm run tauri -- build --bundles msi`: PASS
- `msiexec /i ... /qn /norestart`: exit `0`
- MSI SHA-256: `09A180C96D8F656C7CC6855C3CAED6A1AD6733959D1987EA55CEB3A6835120E5`
- packaged MSI payload and installed executable SHA-256:
  `9026C012C35C624255780B1228C2BA12B58465333B7A664340B1E0B451FF4B55`
- `scripts/verify_latest_runtime.ps1 -RequireRunning`: PASS, two processes from
  the installed Program Files path

Tauri patches executable bundle metadata while producing the MSI, so the raw
`target/release/listener-type.exe` hash is not the installed payload identity.
The runtime verifier now extracts the current MSI payload into a temporary
directory and compares that packaged executable instead. This fixes a false
stale-binary failure; it does not replace the missing fresh human interference
run.

## Workspace inventory boundary

The repository worktree is clean, and no generated build output, runtime log,
cache, user recording, credential or local artifact was added to the release
commits. The workspace still contains older/sibling projects (`Listener-Type`,
`Listener-Type-recut-wake`, `Listener-Design`, `Listener-Hardware`, and
`Listener-Firmware`), plus separately generated MSI/firmware artifacts and
workspace `.artifacts`/`.cache` directories. They are intentionally retained
outside this repository's Git history; deleting or archiving them requires the
owner's explicit confirmation. The tracked Qwen audio files are existing
public test fixtures under `src-tauri/vendor/qwen-asr/samples`, not captured
user recordings.
