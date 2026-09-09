## Summary

- Describe the Listener Type change and its user-visible impact.
- Link the issue or explain why this change is needed.

## Verification

- [ ] `npm run build`
- [ ] `npm run check:brand`
- [ ] `npm run check:cloud`
- [ ] `npm run check:traceability`
- [ ] `cargo check --manifest-path src-tauri/Cargo.toml`
- [ ] `cargo test --manifest-path src-tauri/Cargo.toml --lib`
- [ ] `npm run check:open-source` (for docs, public metadata, or repository changes)

## Platform Notes

- macOS:
- Windows:

## Release Safety

- [ ] No upstream product branding or service endpoint was reintroduced.
- [ ] Marketplace/OAuth remain disabled unless Listener Type-owned config is present.
- [ ] Traceability docs were updated for changed source files.

## Public-repository hygiene

- [ ] No API keys, OAuth tokens, recordings, transcripts, personal data, private
      workstation paths, generated logs, installers, or firmware packages are
      included.
- [ ] User-facing behavior, privacy notes, and recovery docs are updated when
      this change affects them.
- [ ] If this changes a dependency or submodule, its public URL and license
      boundary are documented.
