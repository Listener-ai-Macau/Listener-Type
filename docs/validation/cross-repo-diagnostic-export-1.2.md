# cross-repo-diagnostic-export 1.2 validation

## Scope

Desktop unified diagnostic package export for Listener Type:

- Existing diagnostic JSON is now packaged inside a ZIP.
- The ZIP includes sanitized desktop log tail, BLE connection history, audio sample metadata, up to 3 retained debug WAV audio samples when available, firmware diagnostic summary, and firmware `diag_log.bin` when the BLE diagnostic service is reachable.
- Firmware diagnostic pull uses the accepted 1.1 GATT service UUID `710af845-6d9f-6583-0c4d-9e5b3bc3093a` with control/data/count characteristics and validates per-chunk CRC32.
- If firmware is unreachable or the platform is unsupported, the export still succeeds with `firmware/diag_log_summary.json` marked `offline`.
- Save dialog defaults and backend final filename use `.zip`, device info, and timestamp.

## Visual evidence

- `docs/validation/cross-repo-diagnostic-export-1.2-about-export.png`
  - Captured from Vite dev server in Edge headless.
  - Shows the About export row updated to diagnostic ZIP wording and the export entry point.

## Validation commands

- `npm ci`
  - PASS; installed local frontend dependencies.
- `npx tsc --noEmit`
  - PASS.
- `npm run build`
  - PASS; Vite production build completed. Existing large chunk warning only.
- `cargo test --manifest-path src-tauri\Cargo.toml diagnostic --lib`
  - PASS; 9 diagnostic tests passed, including ZIP entries, offline firmware summary, firmware diag bin inclusion, audio sample manifest, and filename device/timestamp coverage.
- `cargo test --manifest-path src-tauri\Cargo.toml crc32_matches_standard_vector --lib`
  - PASS.
- `npm test`
  - PASS; frontend rule/layout/device-health/BLE recovery/OTA tests passed.
- `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check`
  - PASS.
- `git diff --check`
  - PASS; only Git CRLF conversion warnings on touched files.

## Notes

- No transcripts, final inserted text, or credential values are exported.
- Audio samples are limited to previously retained debug WAV recordings (`record_audio_for_debug` sessions); export does not start a new recording.
- No real BLE hardware pull was performed in this step; the Windows BLE implementation compiles and the firmware-unreachable fallback is covered by unit tests.
