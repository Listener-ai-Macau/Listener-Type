## Summary

- Describe the Listener Type change and its user-visible impact.

## Verification

- [ ] `npm run build`
- [ ] `npm run check:brand`
- [ ] `npm run check:cloud`
- [ ] `npm run check:traceability`
- [ ] `cargo check --manifest-path src-tauri/Cargo.toml`
- [ ] `cargo test --manifest-path src-tauri/Cargo.toml --lib`

## Platform Notes

- macOS:
- Windows:

## Release Safety

- [ ] No upstream product branding or service endpoint was reintroduced.
- [ ] Marketplace/OAuth remain disabled unless Listener Type-owned config is present.
- [ ] Traceability docs were updated for changed source files.
- [ ] Product status and limitations remain accurate for user-visible changes.
- [ ] Recording changes include replay or hardware-log evidence for the affected timing boundary.
