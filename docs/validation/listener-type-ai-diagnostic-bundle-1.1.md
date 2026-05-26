# Listener-Type AI Diagnostic Bundle Validation

Date: 2026-05-26
Branch: ai/oai-listener-type-ai-diagnostic-bundle-1.1

## Scope

- Added `tools/collect_ai_diagnostics.ps1` as the one-command Listener-Type diagnostic bundle entry point.
- Added `tools/ai_diagnostics/collector.py` to write deterministic `manifest.json` and `diagnostic_bundle.json` under timestamped `tests/artifacts/ai_diagnostics*` output directories.
- Captures command/source metadata, git/workflow metadata, environment summary, Listener-Type logs/artifacts, BLE/audio artifact summaries, recent warning/error refs, file paths, hashes, and copied or referenced source files.
- Supports offline repository/log scanning with no hardware.
- Supports firmware integration by ingesting a firmware diagnostic bundle, or by preserving firmware repo/diag log metadata and using a firmware collector when one is available.
- Updated repo feature context and ignored generated `tests/artifacts/` output.

## Commands

- `python -m compileall -q tools` - PASS
- `pwsh -NoProfile -Command '$tokens=$null; $errors=$null; $null = [System.Management.Automation.Language.Parser]::ParseFile((Resolve-Path .\tools\collect_ai_diagnostics.ps1), [ref]$tokens, [ref]$errors); if ($errors.Count -gt 0) { $errors | ForEach-Object { $_.Message }; exit 1 }'` - PASS
- `$env:AI_AGENT_ID='oai'; pwsh -NoProfile -File .\tools\collect_ai_diagnostics.ps1 -OutputDir .\tests\artifacts\ai_diagnostics_smoke` - PASS
- `$env:AI_AGENT_ID='oai'; pwsh -NoProfile -File .\tools\collect_ai_diagnostics.ps1 -OutputDir .\tests\artifacts\ai_diagnostics_firmware_smoke -FirmwareBundle .\tests\fixtures\ai_diagnostics\sample_firmware_bundle.json` - PASS
- `pwsh -NoProfile -File .\tools\ai\repo_features.ps1 -Check` - PASS
- `git diff --check` - PASS, Git reported only existing LF-to-CRLF working-copy warnings for `.gitignore` and `tools/ai/repo_features.ps1`
- `git diff --check origin/main...HEAD` - PASS after rework commit; verifies the committed branch diff has no EOF whitespace issue.

## Rework Evidence

- Removed the trailing blank line at EOF from `tools/ai_diagnostics/__init__.py`.
- Re-ran the collector smoke and firmware-bundle ingest smoke after the whitespace fix.
- Added committed-diff whitespace validation with `git diff --check origin/main...HEAD` so the review check matches the branch diff, not only the clean working tree.

## Evidence

- Offline smoke bundle: `tests\artifacts\ai_diagnostics_smoke\20260526T074626Z`
- Firmware ingest smoke bundle: `tests\artifacts\ai_diagnostics_firmware_smoke\20260526T074627Z`
- Offline smoke summary: 6 source files, 2 logs, 2 BLE/audio artifacts, 14 warning/error refs, workflow env/branch/assignee all `oai`.
- Firmware smoke summary: 6 source files, firmware decoder status `ingested_firmware_bundle`, firmware bundle parse status `ok`, 14 warning/error refs.

## Notes

- The collector excludes generated `tests/artifacts/ai_diagnostics*` directories from discovery so repeated runs do not recursively ingest previous diagnostic bundles.
- Firmware event decoding is intentionally not reimplemented in Listener-Type; firmware logs are summarized as raw metadata unless a firmware-owned diagnostic bundle/tool is provided.
