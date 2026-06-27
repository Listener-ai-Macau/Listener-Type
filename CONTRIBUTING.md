# Contributing

Listener Type is a Tauri 2 desktop app with a Rust backend and a React /
TypeScript frontend.

## Development

```bash
npm ci
npm run build
npm test
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --lib --no-run
```

Run the release audits before proposing packaging or updater changes:

```bash
npm run check:brand
npm run check:cloud
npm run check:traceability
npm run release:check
```

## Pull Requests

- Keep local-first behavior intact.
- Do not commit provider credentials, recordings, transcripts, generated build
  output, validation logs, or release artifacts.
- Keep `specs/traceability/files.md` current when adding or moving product
  source files.
- Use issue or PR text for validation evidence instead of committing
  `docs/validation` or `tests/artifacts` output.
