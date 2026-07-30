# Operator note triage

Date: 2026-07-30

- Selected result: `通过`.
- Operator note: `这个东西能用吗？`
- Classification: clarification request, not a reported defect.
- Resolution: yes for the reviewed scope. Installed session 65 accepted the intended
  activation, opened the capsule in 1,043 ms from candidate start and 323 ms from the
  detected phrase end, completed lossless audio with no missing packets, preserved the
  post-wake sentence, and inserted the raw fallback text after the separately configured
  polish provider returned HTTP 401.
- OTA clarification: the owner's preceding firmware OTA completed all bytes at 66.0
  KiB/s with zero recovery, one generation reboot and final `TYPE:READY`. The later
  `restoring failed OTA` text was emitted by a read-only post-success preflight cleanup,
  not by a failed transfer or rollback.

No new wake, retention, OTA, pairing, firmware or LED defect is opened by this note.
The existing invalid polish-provider credential remains a separate configuration issue.
