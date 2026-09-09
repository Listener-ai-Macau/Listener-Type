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
