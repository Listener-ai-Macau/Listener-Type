# Owner-centric audio pipeline

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
