# Listener v1.0.6-rc1 · Type build199 / Firmware37

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## Wake admission repair

The final wake decision previously overrode an enrolled non-owner result when the phrase detector reported a hit. It now waits for an existing owner-recovery window while capture is active and rejects an unconfirmed owner when the candidate ends. The existing compatible-owner recovery rules remain; no blanket increase to the speaker or keyword thresholds was made.

The recording preview appears only after complete intent and common owner admission. The question command does not briefly display a recording capsule. A complete-keyword comparison waits for transient verifier contention within its existing budget; exploratory checks still do not queue. A bounded, process-local technical receipt records decision metadata without audio or message text.

Saved-draft naming, question preview/auto-end and original-text streaming remain available. Firmware37 is unchanged; this app update does not require a firmware flash.

## Verification and limits

The old non-owner bypass was reproduced in the actual native gate before the repair. A retained ordinary-speech false-keyword sample was exercised through the cached native model/helper and both completion paths; compatible-owner recovery cases were retained for both commands. Matching-MSI startup and read-only diagnostic IPC are checked separately from actual speech.

The two newly reported live trigger waveforms were not captured. This release fixes concrete admission/preview defects; it does not establish zero future false wakes or perfect single-microphone separation. Strong background speech, arbitrary target fields, physical/global-key and long-term scopes remain separately qualified. RC1 remains a prerelease.

US$49 is the hardware price. Type is free/open source. Recognition and optional text services require preparation and separate service usage; this release does not provide an official cloud account or allowance.
