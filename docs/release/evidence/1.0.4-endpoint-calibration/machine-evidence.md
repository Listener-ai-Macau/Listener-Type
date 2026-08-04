# Listener 1.0.4 Endpoint Acceptance

The canonical Chinese operator result in this directory accepted session `6b47e960-d329-4e48-99ec-74668b127bf8` on 2026-08-04.

Machine correlation for that accepted session recorded a non-empty final transcript, zero missing BLE packets, no erroneous `target_speaker_inactive_1000ms` dispatch during continuing body speech, and normal completion after terminal silence. The configured endpoint remained exactly 1000 ms; no fixed grace tail was added.

The later final 1.0.4 package preserved that backend contract. Its frontend tests and production build passed, the full Rust library suite passed with 1018 tests and 4 ignored external/diagnostic tests, and the root MSI identity is `A7DFA087076DF7691ACB33E78360D16A9D01EB8AACB1FA437EC32FF46DB5E26F`.
