# Offline CPU VAD runtime

These Windows x64 libraries are from the official sherpa-onnx 1.13.1
`win-x64-shared-MD-Release-no-tts` archive. The archive SHA-256 is
`6760b0e25eaad0dadffba9029b1270778e0dbfd43f314cf070b2f9c1dcb4af25`.
The installed ONNX Runtime identifies itself as version 1.24.4. The package
includes the corresponding upstream license and third-party notices.

The runtime supports the bundled Silero VAD on CPU without a first-use network
download. Existing complete per-user speaker runtime files retain priority.
An incomplete cache is never mixed with the bundled DLLs.
