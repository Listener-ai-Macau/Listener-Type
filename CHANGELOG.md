# Changelog

This file records user-visible release boundaries. It does not promise that
every historical development snapshot is a supported release.

## 1.0.5 — current baseline

Listener Type 1.0.5 is the current desktop and Listener keyboard baseline.

### Included

- Local-first dictation with raw, light, structured, formal and translation
  output modes.
- Windows and macOS desktop hotkeys, clipboard fallback and Windows TSF
  insertion where supported.
- Listener BLE keyboard controls, EC11 actions, four LED zones, low-power
  behavior and recovery guidance.
- Local provider credentials, local history/settings, optional local ASR and
  configurable cloud ASR/LLM providers.
- Owner-presence and interference protections across the wake candidate,
  recording session, endpoint and final-text paths.
- A recorded owner-plus-computer-playback interference baseline. See
  [the evidence record](docs/release/evidence/20260909-current-interference-baseline.md).
- Public repository hygiene, contribution, support, security and code-of-
  conduct documentation.

### Deliberately deferred to 1.0.6

- Generalizing owner voice enrollment beyond the current wake-phrase-oriented
  capture flow.
- A broader human-to-human interference matrix covering distance, room layout,
  accents, overlap patterns and multiple real speakers.
- Further wake-word recall tuning after the current 1.0.5 baseline.

1.0.5 should not be described as a universal human-interference guarantee. It
is a usable daily-input baseline with explicit environmental and dependency
boundaries.

## Unreleased / 1.0.6 direction

The next release may improve the deferred items above only after a separate
machine-measured owner/interferer evaluation. Changes must preserve the 1.0.5
baseline unless a new acceptance record replaces it.

The current development branch also contains two follow-up candidates that
are not part of the accepted 1.0.5 baseline yet:

- a final-frame owner-recovery guard that keeps a verified late owner tail
  instead of truncating it to the last preview shown before auto-stop;
- a delayed-wake boundary guard that keeps the first owner sentence when
  interference postpones exact-start wake confirmation until the terminal
  capture window.

Both candidates have automated regression coverage. They still require a
fresh human interference run before being advertised as release behavior.
