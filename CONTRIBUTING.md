# Contributing

Listener Type is a Tauri 2 desktop app with a Rust backend and a React /
TypeScript frontend.

## Development

```bash
git submodule update --init --recursive
npm ci
npm run build
npm test
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --lib --no-run
```

The submodule command checks out the exact Denzic Platform revision recorded by this repository.

Run the release audits before proposing packaging or updater changes:

```bash
npm run check:brand
npm run check:cloud
npm run check:traceability
npm run release:check
```

## Repository structure

- `src/` contains the React product UI and desktop interaction layer.
- `src-tauri/` contains recording, recognition, device, insertion, updater, and platform integrations.
- `docs/features/` maps engineering behavior to implementation and validation.
- `docs/product/` and `docs/quickstart/` describe the supported user contract.
- `specs/traceability/` tracks product source ownership and must stay current.

Start user-facing work from the product contract, then update the implementation and its traceability entry together. Recording changes should preserve a single session owner and include evidence from replay or hardware logs for the affected timing boundary.

## Pull Requests

- Keep local-first behavior intact.
- Do not commit provider credentials, recordings, transcripts, generated build
  output, validation logs, or release artifacts.
- Keep `specs/traceability/files.md` current when adding or moving product
  source files.
- Use issue or PR text for validation evidence instead of committing
  `docs/validation` or `tests/artifacts` output.
- Describe current limits honestly; do not present experimental wake, speaker
  isolation, or TSF paths as generally available.
