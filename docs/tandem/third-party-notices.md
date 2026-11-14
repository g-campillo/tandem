# Tandem third-party notices

This directory covers the macOS dependency closures of Tandem and the upstream Codex CLI/exec-server sources. It includes normal and build dependencies and the workspace feature superset; it deliberately over-includes some components not linked into these two binaries. This is a source dependency inventory, not a binary-level SBOM.

`inventory.json` records versions, declared SPDX expressions, source repositories, notice origins and hashes. `packages/` preserves available published/upstream license and copyright files. Some crates declare standard permissive terms but include no standalone notice at their published source revision; their `ATTRIBUTION.txt` explicitly records that limitation, package authors and source copyright statements, alongside the declared standard terms. Author metadata is not asserted to be a copyright notice. No source or upstream binary was modified.

Complete unchanged source archives for components whose declaration includes MPL-2.0 are in `sources/`, including conservative build-only entries. `codex/` contains OpenAI Codex's Apache-2.0 LICENSE and NOTICE. GPUI Kit's bundled Lucide icons have their separate LICENSE-LUCIDE in the assets package notices. System fonts are used. Unused external voice/code-mode resources are not distributed in this app.

Codex dependency versions/checksums were preserved from rust-v0.169.0's lockfile. That release tag updates internal workspace package versions while its lockfile retains 0.0.0; the local audit reconciled only those workspace versions to 0.169.0 and verified all external pins remained identical. Registry declarations and original source are the provenance; this local-use package does not claim independent legal review or notarized public distribution.
