# Open-source status and boundaries

Listener Type is intended to be maintained as an MIT-licensed desktop
application. This repository contains the application source, documentation,
tests, packaging scripts, and Windows IME sources.

The MIT license covers the repository's copyrightable source and documentation;
it does not grant permission to use the `Listener Type` name, logo, mascot,
release signatures, or other product marks to imply endorsement or official
distribution. Ask the maintainers before redistributing a branded build.

For the current product boundary, see the [1.0.5 commercial readiness note](release/1.0.5-commercial-readiness.md)
and the [change log](../CHANGELOG.md). Those documents describe what has
been verified for the current baseline and what is deliberately deferred to
1.0.6.

## Important dependency boundary

The application currently depends on the `third_party/denzic-platform` Git
submodule for BLE, OTA, audio, device-control, speaker-verification, and voice
activation host crates, plus the TypeScript OTA package. The submodule is a
separate repository and must have a compatible public license and read access
before this repository can honestly promise a fully reproducible public build.

Until that dependency is public and licensed, do not publish this repository as
a self-contained buildable distribution. A contributor can still review the
application source and documentation, but the full Tauri build requires access
to the pinned platform commit.

### Current release status: BLOCKED

At the pinned commit `0c7190ea411ff046d3410c37e1ac4d7c8d7d7f1a`, the checked-out
platform repository has no root `LICENSE` file, and the main CI workflow still
uses `DENZIC_PLATFORM_DEPLOY_KEY` for its full-build jobs. Therefore this
repository is **not yet a self-contained, reproducible public release**. The
correct release actions are to license and publish the platform repository (or
replace it with a public/vendored equivalent), then remove the private-key
requirement from the public build path. Do not work around this by copying a
private submodule into a pull request.

The root `package.json` remains marked `private` because this project is a
desktop application, not an npm package intended for registry publication.
That flag does not replace the repository license or restrict source review.

## Commercial distribution is a separate gate

An MIT source release and a sellable Listener Type product are related but not
the same approval. A commercial build also needs clear rights for the platform
submodule, firmware, models, icons, product marks, and any bundled runtime;
published checksums and signatures; a privacy notice; and support, update, and
refund terms. Do not describe the current 1.0.5 baseline as a security-grade
speaker identity system or as a universal human-interference guarantee. The
broader owner-enrollment and human-interference work is tracked for 1.0.6.

## What is not included

- Provider API keys, OAuth tokens, signing keys, or release credentials.
- User recordings, transcripts, diagnostic exports, or local model data.
- Product-owned backend infrastructure, marketplace credentials, or private
  deployment configuration.
- Firmware binaries unless a release explicitly publishes them with their own
  license and checksum.

## Release checklist

Before changing the repository's public visibility or announcing a public
release, maintainers must confirm:

1. The copyright holder and MIT license text are correct.
2. Every required submodule has a public URL, a compatible license, and a
   reproducible pinned commit.
3. CI passes without a private deploy key for ordinary fork pull requests.
4. Release artifacts are signed, checksummed, and published separately from
   source and test output.
5. The public repository contains no credentials, private workstation paths,
   user data, or unreviewed generated logs.
