# Goal: remove intermittent artificial wake latency

Date: 2026-07-30

## Contract

Fix the occasional Listener Type wake delay at its scheduling source without changing
the accepted sensitive KWS thresholds, explicit-`Absent` authority, local-only
`PresentLater` rejection, owner gate, 400 ms retry cadence or 900 ms final safety
budget. Abandoned helper work must not queue ahead of a later candidate. Terminal
ambient cleanup must not serialize the BLE actor for multiple seconds: two completed
local `Absent` results skip auxiliary offline recall, while the evidence-poor recovery
path retains a bounded 500 ms attempt.

Protect the accepted Listener Firmware, OTA LED behavior, dual-lane OTA transport,
strict `>60.0 KiB/s` release evidence and root firmware ZIP. This task does not change
firmware, OTA, pairing, UI layout, ASR models, phrases or product thresholds.

## Ordered work

1. Reproduce and identify the queueing and terminal actor stalls from installed logs.
2. Make local helper confirmation single-flight and non-queueing; retry transient busy.
3. Bound terminal cleanup while retaining the evidence-poor offline recovery chance.
4. Run focused and complete Type release gates.
5. Rebuild, install and identify the final MSI; preserve the accepted firmware ZIP.
6. Run an installed ambient-backlog-to-real-wake loop and collect owner acceptance.

The final human boundary is the canonical Chinese operator review on the installed
Program Files build. A separate priority helper lane is explicitly excluded after its
installed session-212 sensitivity regression.
