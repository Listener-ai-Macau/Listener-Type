# Owner-centric audio pipeline

## Content-first candidate (2026-10-06)

`ownerActivityOnly` is an opt-in Windows BYOK candidate and defaults to false.
It separates ordinary dictation permission from identity: original PCM goes
to the ordinary cloud recognizer, all provider content is permitted, and no
speaker-present span grants a word-owner certificate. It skips the strict
extractor and cloud voiceprint projection in this mode.

The independently verified current wake is encoded with NomoPVAD's own
ERes2NetV2 encoder, producing a temporary per-session reference. Existing
CAM++ and WeSep templates are never converted by dimension. The existing
three-step persistent enrollment and unenrolled open wake remain intact.
This temporary reference still requires real-client and language validation.
The hosted gateway's bound-identity route does not implement this mode yet.

NomoPVAD runs on original PCM in causal 160 ms steps with a 2.4 second past
window. Only a new positive owner observation renews the owner endpoint and
firmware lease. Completed negatives can expire the owner clock even while
generic VAD remains Speech. Missing, failed, replayed or lagging inference
cannot certify absence. Actual editor receipt earns one three-second window
per owner advance; repeated background receipts cannot keep renewing it.
The STOP transaction rechecks this same presence evidence before dispatch.

Four public Chinese endpoint replays passed, including quiet speech, a return
inside the continuation window, and the cross-boundary counterexample.
Twenty native windows match the pinned author model. The latest ordinary-ASR
counterexample still misses the owner's short phrase, so this candidate is
not a joint repair and has not been installed or accepted on hardware.

Recovery sessions must inherit this content permission. Reconstructing an ASR
instance without `ownerActivityOnly` returned retries to the speaker filter and
could drop a later speaker's permitted words. The replay factory now copies
the mode; it still grants no owner identity, word watermark or endpoint lease.
Only this mode's one existing empty-final replay uses `enable_nonstream=false`.
The live route, transport recovery and strict-owner route retain two-pass
correction. No extra replay budget is introduced.

The public PS4 full-file payload produced a usable first-pass phrase that its
two-pass correction cleared. A new stream-only call retained three syllables
as “晚晚香”; the expected “碗碗香” spelling was not correct. Neighboring clean
and digital-quiet Chinese checks retained the syllables, while the normal
English two-pass result corrected “to” to “at” but spelled “Otto” as “Auto”.
These are separate content and spelling observations, not global acceptance.
The digital-quiet loader peak-normalizes input, so it is not real quiet-mic
evidence. A lab float-to-PCM converter needed whole-file attenuation to avoid
44 clipped samples; that lab repair does not change the product's raw capture.
PS4 remains a batch candidate. The optional Windows native adapter and its
empty-final replay hook now exist, but no validated export assets are installed.
The guarded export stopped during dependency loading when available system
memory fell below 2 GiB; it produced no export or model quality result. This
route cannot be enabled or counted as runtime acceptance yet.

The adapter loads only a parity-confirmed manifest of the pinned author revision,
checkpoint fingerprint and three exported asset fingerprints. It encodes the
session-local verified wake PCM with PS4's own ECAPA, never another encoder's
192-dimensional vector. It operates on complete sessions of at most six seconds;
longer audio remains original and is never shortened to fit the model. Missing
reference/assets, invalid results, memory pressure and worker failure preserve
the original replay PCM. It consumes the existing single empty-final replay,
with no extra ASR call budget and no new text identity or endpoint permission.

The native frontend keeps every source sample, including the final partial STFT
hop. It reconstructs the centered padded signal before taking the explicit
source length. Full-file attenuation avoids clipping without time masking.
Four component regressions cover partial-hop/tail round trips, non-clipping PCM,
invalid/overlong input and lossless fallback. These are mathematical/state
checks, not author-versus-ONNX parity or installed-device evidence. A nonempty
background-only primary transcript still does not invoke an empty-final replay;
this hook alone does not solve that observed omission case.

## Why this exists

The product contract is owner-centric, not energy-centric:

- wake-up requires the configured wake phrase and the enrolled owner;
- after wake-up, only the enrolled owner can extend the recording;
- other speakers and room energy may be present without extending the recording;
- endpointing must stop after the owner has been quiet for the endpoint interval,
  even when a generic energy detector still reports sound.

The old pipeline exposes a generic `local_speech_end_ms` value to endpoint
policy. That value is an energy/VAD watermark, not an owner watermark. It is
therefore not allowed to renew an owner recording.

## Model responsibilities

### Firmware keyword detector

The firmware detector is a low-latency candidate source. It does not establish
the owner identity and it does not control the host endpoint.

### Wake phrase detector

The host KWS/ASR detector verifies the configured phrase and supplies the
phrase-aligned interval. It is independent phrase evidence, not speaker
identity evidence.

### Owner wake verifier

The enrolled voiceprint verifies the phrase-aligned candidate. A short or low
quality window is `PendingOwnerVerification`, not an immediate permanent
reject; the candidate may use a bounded follow-up window. An accepted wake
creates an immutable per-session owner profile.

The current CAM++ adapter remains the baseline, not the endpoint policy. It is
small and fast, but the product's short, single-mic, overlapping-speech wake
window is a harder operating point than ordinary diarization. A replacement
must be selected by replaying the same enrolled clips through both models and
calibrating false-reject/false-accept rates; changing a threshold without that
calibration is not a model upgrade.

### Owner activity detector

The endpoint-facing detector is speaker-conditioned. Its conceptual output is
one class per frame: `NoSpeech`, `OwnerSpeech`, or `OtherSpeech`. Generic VAD
and raw energy remain diagnostic inputs only. The current embedding classifier
is an adapter while a true personal-VAD/TS-VAD model is evaluated; it must not
leak its generic energy watermark into endpoint policy.

### Target-speaker extraction

The existing WeSep target-speaker extractor is an ASR isolation stage for
overlap. It may improve the owner transcript, but its residual energy is not a
replacement for owner activity state.

### Endpoint arbiter

The arbiter consumes only owner evidence:

```text
owner_last_speech_end
owner_activity_state
provider_owner_boundary
pending_provider_text (bounded by owner evidence)
```

`environment_last_speech_end` and `generic_local_speech_end` can never renew
the firmware lease or block endpointing. An uncertain owner classification may
receive one bounded grace period after a recent positive owner frame; once the
grace is exhausted, the arbiter must commit stop and must not remain in
`Hold`.

## State machine

```text
Idle
  -> Candidate (firmware KWS)
  -> WakePhraseConfirmed (host phrase)
  -> OwnerVerified (enrolled voiceprint)
  -> ListeningOwnerTracked
  -> EndpointPending (owner quiet)
  -> Finalizing
  -> Done
```

`OtherSpeech` and `NoSpeech` do not transition back to
`ListeningOwnerTracked`. Only a new positive owner frame or an owner-attributed
provider boundary can do that. Every transition records the source, audio
watermark, owner state, and reason so a future failure is attributable without
guessing.

## Acceptance invariants

1. Generic energy can never keep a session alive by itself.
2. A firmware KWS hit with a non-owner voice is rejected.
3. A verified owner followed by other-speaker audio stops on the owner clock;
   other-speaker audio cannot renew the lease.
4. A verified owner followed by silence stops within the configured endpoint
   interval.
5. A provider stall can delay final text delivery only while a bounded owner
   tail is present; it cannot prevent endpoint forever.
6. Wake and endpoint decisions are replayable from captured PCM and structured
   evidence, without changing global thresholds blindly.

## Runtime ownership rule (1.0.5)

Endpointing is a session reducer, not a collection of callback timers. Provider,
preview, and local-speaker callbacks only publish an observation into the
session clock. A single 50 ms watchdog evaluates the immutable latest snapshot
and is the only path allowed to dispatch `Stop`.

Each observation has a monotonic arrival time. If no new observation arrives for
one endpoint interval, the reducer marks the provider snapshot stale, clears its
provisional/tail claims, and evaluates the bounded provider-stall path. This is
what prevents a frozen `pending_unattributed_speech` or `local_speech_end_ms`
from holding a recording forever. Callback order, preview revisions, and timer
creation can no longer create competing endpoint decisions.
