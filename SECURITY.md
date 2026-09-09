# Security Policy

## Reporting a vulnerability

Please use a **private GitHub Security Advisory** for this repository. Include
the affected version or commit, the smallest safe reproduction, impact, and
any suggested mitigation. Do not open a public issue for an unpatched
vulnerability.

If private advisories are not enabled, open a minimal public issue containing
only the label `security-contact-needed` and no exploit details, secrets,
recordings, transcripts, personal data, or private paths. The maintainers will
provide a private follow-up route.

## What not to send

Never publish API keys, OAuth tokens, signing keys, provider credentials,
diagnostic exports with private data, raw audio, transcripts, or firmware
release credentials. Redact logs before attaching them.

## Scope

This policy covers the Listener Type application, its documentation, build
scripts, Windows IME bridge, and the public release workflow. Provider outages,
account permissions, malicious model responses, and third-party firmware are
outside the application's security response unless they expose a vulnerability
in Listener Type itself.

## Supported versions

Security fixes target the latest tagged release and the default branch. Older
development snapshots may be useful for diagnosis but are not promised to
receive backports.
