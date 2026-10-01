# DevOps — dist (2026-09-30)

Piece `wt/dist` of launch-v2 (spec section 4): prebuilt `undra` binaries and the four install
channels. Runbook: `docs/RELEASING.md`.

## What was built

* `.github/workflows/release.yml`: `verify` -> `build` (matrix of four) -> `package` ->
  `release`. Tag `v*` or `workflow_dispatch` with `publish` (default false = dry run).
  Actions pinned to commit SHAs (checkout 7.0.1, setup-node 7.0.0, upload-artifact 7.0.1,
  download-artifact 8.0.1, dtolnay/rust-toolchain `stable` branch of 2026-09-03). No build
  cache, no secret in `build`/`package`; `release` alone has `contents: write`, `id-token:
  write` and the two secrets.
* `packaging/pack-release.sh` (the tarball layout, shared by the workflow and the tests),
  `packaging/npm/` (`build.mjs`, `templates/`, `test.sh`), `packaging/homebrew/`
  (`undra.rb.tmpl`, `generate.sh`), `packaging/test-install.sh`, `site/install.sh`,
  `scripts/bump-version.sh` (also `--check`, which the workflow's first job runs),
  `docs/RELEASING.md`.
* CLI: `undra --version` -> `undra <semver> (<sha7>)` (`crates/undra-cli/src/version.rs`);
  `undra init` pins `undra = { git = ..., tag = "v<cli version>" }`, uses a checkout by path when
  the project is inside one or `--undra-path`/`UNDRA_PATH` is given; `UNDRA_VERSION` (the
  registry release line) is derived from the CLI's major.minor instead of the literal `0.1`.

## Decisions and deviations from the brief

* **`package` job between build and release.** The brief has three jobs; the packaging (npm
  tarballs, formula, installs of both) has to run on a dry run too, or the first real release
  would be the first run of it. The `release` job publishes exactly the bytes `package` tested.
* **Publishing gate.** `release` runs when `verify` says `publish`: a tag push, or a manual run
  with `publish` on. `startsWith(github.ref, 'refs/tags/v') || inputs.publish` taken literally
  would publish a manual run on a tag with `publish` off, which is not a dry run. `verify` also
  refuses to publish from anything but the `v<workspace version>` tag, so a branch dispatch with
  `publish` on cannot reach npm (versions there are permanent).
* **No separate `strip` step.** `[profile.release]` has `strip = "symbols"`, applied at link time
  (and re-signed ad hoc on Apple silicon); running `strip` afterwards on macOS would invalidate
  that signature.
* **Linux runners as briefed** (`ubuntu-latest`, `ubuntu-24.04-arm`, a native arm64 runner, no
  cross toolchain). The cost: the binaries need glibc >= 2.39. `ubuntu-22.04` and
  `ubuntu-22.04-arm` would lower it to 2.35 (Ubuntu 22.04, Debian 12 and most current LTS
  hosts); recommended before announcing, one-line change per row, recorded in `RELEASING.md`.
  The build job prints the actual floor.
* **Homebrew `license any_of: ["MIT", "Apache-2.0"]`**, not the string `"MIT OR Apache-2.0"`:
  `brew audit` rejects the string as a non-standard SPDX licence. Same meaning.
* **Publish from tarballs, not directories.** `upload-artifact` drops the executable bit of the
  files in a directory; tarballs keep it. `npm publish ./x.tgz --provenance` is therefore the
  path (dry-run publish of a tarball verified locally; provenance itself cannot be).
* **Idempotent publishing.** An existing GitHub Release must hold the same `checksums.txt` or the
  run fails (immutable); npm packages already at the version are skipped (prereleases publish
  with `--tag next` and leave the tap alone); the tap commit is skipped when the formula is
  unchanged. A half-made release is finished by re-running.
* **Tap token** is passed as an `http.extraheader` for the clone and push (masked), never in a
  URL or `.git/config`.
* **musl and Windows** are refused with a one-line pointer to `cargo install` by both `install.sh`
  and the npm launcher (the platform packages carry `libc: ["glibc"]`).
* **Found while testing the default `undra init`, fixed here** (commit `fix(cli): a git
  dependency ...`): the shim and dev runner asked for `undra-ffi` by `rev = <commit>` while the
  core used `tag = ...`; Cargo treats those as two sources, so a second `undra-runtime` was linked
  and `undra bindgen` read an empty schema. `UndraSource::Git` now keeps the reference. Also:
  Cargo prints a parse error for each `templates/*/Cargo.toml` (they hold `@@X@@`) whenever it
  fetches the repository as a git dependency or `cargo install --git`; those four files are now
  `Cargo.toml.tmpl`.

## Verified locally

* `packaging/npm/test.sh` (also with `--tarballs`, relative path) and `packaging/test-install.sh`
  (also with `--release-dir`), against the rename branch's `undra` binary and a release build;
  mutation-checked (disabling the checksum comparison or the sha256-tool guard fails the
  installer test). `node --check`, `sh -n`/`bash -n`/`dash` on every script.
* The generated formula: `brew style --formula`, `brew audit --strict`, `brew install` and
  `brew test` from a throwaway local tap with `file://` URLs (Homebrew 7.0.7); tap and install
  removed afterwards.
* `scripts/bump-version.sh` on a scratch copy: refuses non-semver, bumps, `--check`, idempotent,
  and `cargo metadata --locked --offline` passes on the result.
* The workflow parses as YAML and every action is pinned to a 40-character SHA. **`actionlint`
  and `shellcheck` are not installed here and were not run.**
* Default `undra init` against a local git remote with the tag (`git config url.insteadOf`,
  `CARGO_NET_GIT_FETCH_WITH_CLI`): the project builds, `undra bindgen --check` passes with the
  schema non-empty, `undra build --platform host` works, and `cargo install --git ... undra-cli`
  installs an `undra` that prints `undra 0.1.0 (unknown)`.
* `cargo test -p undra-cli` (all suites), clippy `-D warnings`, `cargo fmt --check`.

## Not testable until the first dry run / release

* The workflow on GitHub: runner labels (`ubuntu-24.04-arm`, `macos-15`), the v7/v8 action
  majors, Rosetta on `macos-15` (the x86_64 check falls back to `lipo`), `objdump`, `ruby -c`.
* `npm publish --provenance` of a tarball, the token's scope and 2FA bypass, whether the
  unscoped name `undra` passes npm's name-similarity check, `gh release create --verify-tag`,
  the tap push, `brew install shreypdev/undra/undra` against the published formula.
* The curl installer against GitHub's real redirects and API (the tests serve the same layout
  locally and parse a realistic API reply).
* `undra init`'s git dependency against the real repository: needs the repository renamed to
  `shreypdev/undra` and the tag `v<version>` pushed.

## Known gaps outside this piece

* A project made by `undra init` outside a checkout still names registries that do not exist
  yet for iOS and Android (the Swift package URL `https://github.com/shreypdev/undra-swift`, and
  `dev.undra:runtime` on Maven Central: roadmap v1.1). Web works once `@undra/runtime` is on
  npm; every platform works with `--undra-path`.
* `undra-bindgen`'s own defaults and goldens still say `@undra/runtime ^0.1.0`; the CLI passes
  the real line, so generated projects are right. `scripts/bump-version.sh` does not touch them.
* No PR-time workflow runs `packaging/*` tests (they run in every release dry run).
