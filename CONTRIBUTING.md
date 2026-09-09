# Contributing to Listener Type

Thanks for helping improve Listener Type. It is a Tauri 2 desktop app with a
Rust backend, React/TypeScript frontend, optional Listener BLE hardware, and a
Windows TSF insertion bridge. Read the [open-source boundary](docs/OPEN_SOURCE.md)
before working on the platform submodule or release packaging.

## Before you start

1. Open an issue for a non-trivial change so the scope and platform impact are
   clear.
2. Do not include recordings, transcripts, API keys, access tokens, device
   identifiers, private backend details, or full local paths in an issue or PR.
3. Keep the local-first path working without a Listener Type backend.
4. Keep product source mapped in `specs/traceability/files.md` when adding or
   moving source files.

## Development setup

Use Node.js 22+, Rust stable, and the platform tooling required by your OS.
Clone with submodules when you have access to the pinned Denzic Platform
repository:

```bash
git clone --recurse-submodules https://github.com/Listener-ai-Macau/Listener-Type.git
cd Listener-Type
npm ci
```

The full Tauri build currently requires `third_party/denzic-platform`. If the
submodule is not publicly readable yet, follow [open-source status](docs/OPEN_SOURCE.md)
and run the documentation/static checks instead of copying a private checkout
into a pull request.

The current repository status is **BLOCKED for a self-contained public build**
until that platform repository has a compatible public license and read access.
This is an external release boundary, not a reason to bypass the dependency or
commit a private checkout.

## Checks

For normal frontend or documentation changes:

```bash
npm run check:docs
npm run check:open-source
npm run check:traceability
npm run build
npm test
```

For backend or platform changes, also run:

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --lib
```

Before packaging, updater, marketplace, or branding changes, run the focused
audits that apply:

```bash
npm run check:brand
npm run check:cloud
npm run check:traceability
npm run release:check
```

Hardware BLE, microphone permissions, cloud providers, Windows TSF, and signed
release artifacts cannot be completely verified on every contributor machine.
Record the OS, device path, and exact checks you ran in the PR instead of
claiming a test you could not perform.

## Pull requests

- Keep commits focused and explain the user-visible effect.
- Add or update tests for behavior changes.
- Update user docs when settings, shortcuts, device actions, privacy behavior,
  or recovery steps change.
- Do not commit generated `dist/`, `target/`, `.cache/`, `.artifacts/`, logs,
  installers, firmware packages, or validation output.
- Keep the PR description complete using the repository template, including
  platform notes and verification evidence.

By submitting a contribution, you agree that it may be distributed under the
MIT License in this repository. Please also follow the [Code of Conduct](CODE_OF_CONDUCT.md).
