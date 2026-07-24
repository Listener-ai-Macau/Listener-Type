# Listener Voice Activation Evaluation

Status: feasible, separate opt-in feature; do not enable by default.

## Recommendation

Implement speech-triggered start/stop on Listener firmware, not as an
always-open desktop microphone. Keep the existing recording button as an
override. Start with plugged-in operation only; battery operation needs its
own measured power budget and owner acceptance.

The current ESP32 WebRTC AFE explicitly sets `vad_init = false`. Enabling VAD
inside the existing AFE must not make VAD a gate that suppresses quiet speech.
Use it only as a classifier beside the audio path, or use an independently
validated energy/noise classifier.

## Proposed Behavior

- Setting is opt-in and defaults off.
- Monitoring has a distinct, privacy-visible device state.
- Keep 600 ms of pre-roll so the first syllable survives the trigger.
- Start after at least 300 ms of sustained speech evidence.
- Stop after 1,000 ms of sustained silence, with a 250 ms tail.
- Reject sessions shorter than 500 ms unless the user presses the button.
- Cap an automatic session at 60 seconds.
- Add a 1,500 ms cooldown after automatic stop.
- A button press always starts or stops immediately and resets VAD state.
- Playback, notification sounds, reconnects, OTA, and error recovery never
  trigger a recording.

These values are initial acceptance candidates, not accepted product
thresholds. Tune them from retained numeric evidence, never from transcript or
audio retention outside the existing privacy contract.

## Architecture

Create a platform `voice_activation` pure state machine with inputs for
speech confidence, elapsed time, power mode, user override, transport
readiness, and blockers. Outputs are monitor, start, continue, stop, reject,
and cooldown decisions.

Firmware adapters own microphone/AFE access, the pre-roll ring, timers, power
state, LEDs, and BLE recording-control effects. Type owns the setting, privacy
copy, diagnostics, and presentation. The platform core must not own audio,
FreeRTOS, BLE, storage, or LED effects.

## Acceptance Gates

- Speech onset to recording start: no more than 250 ms after the 300 ms
  confirmation window; the 600 ms pre-roll preserves onset audio.
- Speech end to stop request: 1,000 to 1,250 ms.
- No missed starts in 20 quiet and 20 ordinary-volume spoken trials.
- No false starts in 30 minutes each of quiet office, typing, desk movement,
  and local media playback.
- No duplicate start/stop event per speech segment.
- Existing button recording latency, transcript accuracy, transport
  throughput, and cancellation behavior remain unchanged.
- Measure plugged and battery current before allowing battery monitoring.

## Decision Needed

Before implementation, the owner must approve privacy indication, whether
battery monitoring is allowed, and the trigger/stop thresholds. A focused
physical review remains required because false triggers and perceived cutoff
quality are not fully machine-observable.
