---
name: write-release-notes
description: Write Punktfunk stable release notes from git history. Use when cutting a version, writing docs/releases/vX.Y.Z.md, a CHANGELOG.md release card, Play whatsnew, /release-notes, or bumping a stable tag.
---

# Write release notes

Procedure for a stable `vX.Y.Z`. Voice and shape: `docs/writing.md` §2.
Ordinary PRs do not run this and do not edit `CHANGELOG.md`.

## Inputs

1. Previous stable tag (example: `v0.34.0`).
2. `git log --no-merges --format='%s%n%n%b%n---' vPrev..HEAD` — **do not pipe through `head`**. Skip `chore` / `ci` / `test` / `docs` unless the body names a user-facing fact. Cluster by **user theme**, not crate. `BREAKING CHANGE:` footers feed Before you update.
3. A fat `## Unreleased` in `CHANGELOG.md` is optional corroboration **after** git log, not the source. Then replace it with a **short card** (lead + versions + Breaking + knobs). Retitle `## Unreleased` → `## vX.Y.Z` only in the **version-bump** commit. Do not start a new dump. The 0.35 cut may still read the dump: those commits lack `BREAKING CHANGE:` footers.
4. Versioned surfaces from **code**, vs the previous tag (do not invent numbers):
   - Wire: `WIRE_VERSION` in `crates/punktfunk-core/src/lib.rs`
   - C ABI: `ABI_VERSION` / `PUNKTFUNK_ABI_VERSION` in that file and `include/punktfunk_core.h`
   - Driver protocol: `MIN_DRIVER_PROTOCOL_VERSION` in `crates/pf-driver-proto/src/lib.rs`
   - Gamepad channel, plugin schema, OpenAPI (`api/openapi.json` info.version), gamescope `+pfhdrN`, SDK / plugin-kit tags — copy the rows the previous CHANGELOG section already lists; mark unchanged.

## Outputs (same bump commit; drafts may exist earlier)

1. `docs/releases/vX.Y.Z.md` — humans, Gitea body, Discord.
   Discord (`scripts/ci/discord-announce.sh`) posts **everything before the first `## `**. Put the 3–8 highlight bullets in that lead-in. A later `## Highlights` heading is optional duplication; prefer no heading so Discord gets the scan.
2. `docs/releases/whatsnew/vX.Y.Z.txt` — Android only, 500 **characters** (`len()`, not `wc -c`), `whatsnew/TEMPLATE.txt`.
3. `CHANGELOG.md` card: lead, version table, Breaking, short **Knobs / embedder** list (env, JNI arity, CLI) for actions that do not move a version integer. No Added/Changed/Fixed diary.
4. Stop. A human reads the lead-in before the tag.

Voice: `docs/writing.md` §2. Do not paste `git log`. Do not invent version numbers.

Canary / `-rc`: no file here (existing `docs/releases/README.md` rule).
