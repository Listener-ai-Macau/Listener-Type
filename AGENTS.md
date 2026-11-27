# Listener Type repository guide

Start with [CONTRIBUTING.md](CONTRIBUTING.md) for build and validation commands and [docs/README.md](docs/README.md) for the documentation index.

- The React frontend is in `src/`, native code in `src-tauri/`, and the Windows IME bridge in `windows-ime/`.
- Initialize `third_party/denzic-platform` to the pinned revision before installing dependencies.
- Use [feature traceability](specs/traceability/features.md) when changing product behavior. Preserve the recording session owner and verify affected capture and insertion behavior.
- Keep credentials, recordings, transcripts, work logs and generated binaries out of contributions. Follow [SECURITY.md](SECURITY.md) for security reports.
