# Tandem third-party notices

This directory covers two dependency sets: Tandem's macOS cargo dependency closure, and the installed npm tree of the Claude helper that ships in `Contents/Resources/claude-helper`. The cargo part includes normal and build dependencies and the workspace feature superset, so it deliberately over-includes some components that are not linked into the app. This is a source dependency inventory, not a binary-level SBOM.

`inventory.json` records, for every package, its ecosystem (`cargo` or `npm`), version, declared license, source, repository, notice origins and hashes. It also records the hashes of Tandem's `Cargo.lock` and the helper's `package-lock.json` it was collected from, and the Claude Agent SDK version. Packaging refuses an inventory whose lockfile hashes do not match the current sources, or one with unresolved notices.

`packages/` holds the cargo crates' published or upstream license and copyright files. Some crates declare standard permissive terms but include no standalone notice at their published source revision; their `ATTRIBUTION.txt` records that limitation, the package authors and the source copyright statements, next to the declared standard terms. Author metadata is not asserted to be a copyright notice. GPUI Kit's bundled Lucide icons have their separate `LICENSE-LUCIDE` in the `gpui-kit-assets` notices. System fonts are used.

`npm/` holds the Claude helper packages' notices, one folder per package and version (a scoped name such as `@scope/name` becomes `@scope__name`). Each folder contains the license, notice and copyright files from the root of the published package, unchanged. A package that has no such file and declares MIT or Apache-2.0 instead gets an `ATTRIBUTION.txt` (name, version, declared license, repository) and the standard terms from SPDX license list v3.27.0. The lockfile also lists the SDK's optional platform packages for other systems; npm does not install them on this Mac, so they are not bundled and not listed.

Complete unchanged source archives for components whose declaration includes MPL-2.0 are in `sources/`, including conservative build-only entries.

`@anthropic-ai/claude-agent-sdk` and `@anthropic-ai/claude-agent-sdk-darwin-arm64`, which contains the bundled Claude Code binary, are Anthropic's proprietary software, not open source. Their use is subject to Anthropic's legal agreements; their `LICENSE.md`, which says so, is included under `npm/`. Tandem runs them on this Mac under the user's own Claude subscription, signed in through Claude Code. A Tandem bundle therefore contains proprietary Anthropic software: it is for local use only and must not be redistributed.

No source or upstream binary was modified. Registry declarations and the published packages are the provenance; this local-use package does not claim independent legal review or notarized public distribution.

Regenerate this directory for every release: run `npm ci` in `crates/tandem-claude/helper`, then `python3 scripts/prepare-notices.py`, then `python3 scripts/collect-notices.py --output <folder>`. Do not reuse an earlier inventory just because the private source and cache folder under `.tandem-local/m5` still has that name.
