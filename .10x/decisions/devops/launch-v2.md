# DevOps - launch-v2 (2026-09-30)

* `release.yml`: tag `v*` or `workflow_dispatch` (`publish` input, default false = full
  dry run). Matrix: aarch64/x86_64 apple-darwin, x86_64/aarch64 linux-gnu. Steps: verify
  tag == workspace version == npm version → build release, strip → tarball + sha256 →
  `checksums.txt` → GitHub Release (generated notes) → `npm publish --provenance` (needs
  `NPM_TOKEN`, `id-token: write`) → update the formula in `shreypdev/homebrew-undra`
  (`HOMEBREW_TAP_TOKEN`). Publishing steps are skipped when `publish` is false or the ref
  is not a tag.
* `site.yml`: adds the playground build (wasm32 target, `binaryen`, `undra build
  --platform web`, `vite build --base=/undra/playground/`), the generated-file freshness
  check, the link checker, staging to `_site/`, and a post-deploy IndexNow submission.
* Repository rename `shreypdev/keel` → `shreypdev/undra` right after the rename merges
  (GitHub redirects the old git and web URLs; the Pages site moves to `/undra/`).
* Secrets the founder creates: `NPM_TOKEN` (automation token of the `undra` org),
  `HOMEBREW_TAP_TOKEN` (fine-grained PAT, contents:write on the tap repo only).
* Rollback: releases are immutable; a bad release is followed by a patch tag. The npm
  `latest` dist-tag can be moved back with `npm dist-tag`; the formula is a git revert in
  the tap.
