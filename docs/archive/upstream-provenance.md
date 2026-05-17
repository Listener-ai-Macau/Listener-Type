# Upstream Documentation Provenance

This file records the upstream documentation reviewed and adapted into Listener Type. It is the only documentation area where upstream project names are intentionally retained for migration traceability.

## Reviewed Sources

- `ref/openless/README.md`
- `ref/openless/README.zh.md`
- `ref/openless/USAGE.md`
- `ref/openless/openless-all/README.md`
- `ref/openless/docs/volcengine-setup.md`
- `ref/openless/docs/style-pack-marketplace.md`
- `ref/openless/docs/tauri-csp.md`
- `ref/openless/docs/auto-update-download-acceleration.md`
- `ref/openless/docs/windows-tauri-test-agent-research.md`
- `ref/openless/docs/windows-upstream-pr-workflow.md`
- `ref/openless/docs/github-tracking/*`
- `ref/openless/docs/windows-ui-tracking/*`
- `ref/openless/docs/windows-lifecycle-tracking/*`
- `ref/openless/.github/*`

## Listener Type Adaptation

- Product name, package name, bundle id, credential service, updater repository, TSF GUIDs and installer names were changed to Listener Type.
- Production marketplace and OAuth defaults were disabled pending Listener Type-owned backend infrastructure.
- User docs were rewritten under `docs/`.
- Architecture/design docs were rebuilt under `specs/`.
- Code tracking is maintained by `specs/traceability/files.md` and `scripts/check-traceability.mjs`.
