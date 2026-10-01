# Releasing Undra

One tag, one workflow, four install channels. `.github/workflows/release.yml` builds the `undra`
binary for macOS and Linux on x86_64 and arm64, and on a version tag publishes it as

| Channel | What users run | Where it comes from |
|---|---|---|
| Homebrew | `brew install shreypdev/undra/undra` | the formula in `shreypdev/homebrew-undra`, pointing at the GitHub Release tarballs |
| npm | `npm install -g @undra/cli` (or `undra`) | `@undra/cli` + `@undra/cli-<platform>` packages carrying the binary |
| curl | `curl -fsSL https://shreypdev.github.io/undra/install.sh \| sh` | `site/install.sh`, which downloads the GitHub Release tarball and checks its sha256 |
| cargo | `cargo install --git https://github.com/shreypdev/undra undra-cli` | the repository itself; nothing to publish |

`@undra/runtime` (the TypeScript runtime the generated web code imports) is published by the same
run. Windows and Alpine (musl) are not supported (roadmap).

## Prerequisites (the founder does these once)

1. **The repository is `shreypdev/undra`.** The rename is `gh repo rename undra` after the rename
   piece merged. The install URLs, the Homebrew formula and npm's provenance check all name
   that repository.
2. **npm.** Create the organisation `undra` on npmjs.com (free for public packages; it owns the
   `@undra` scope). Create a *granular access token* with read and write access to the `@undra`
   scope and to the unscoped package `undra` (a token for "all packages" works for the first
   publish, when the packages do not exist yet), with *bypass two-factor authentication*
   enabled (CI cannot answer a one-time code) and an expiry. Store it as the repository secret
   **`NPM_TOKEN`** (Settings, Secrets and variables, Actions). Once the first release is out,
   replace an "all packages" token with one scoped to the `@undra` organisation and the package
   `undra`, or drop the token altogether: npm's *trusted publishing* lets this workflow publish
   with its OIDC token alone (configure it per package on npmjs.com, naming this repository,
   `release.yml` and the `release` environment; it needs npm 11.5 or newer in the job, so add
   `npm install -g npm@latest` before publishing; check npm's current documentation).
3. **Homebrew tap.** Create the public repository `shreypdev/homebrew-undra` with a README, so
   it has a default branch (git cannot hold an empty `Formula/` directory; the first release
   creates it). Create a *fine-grained personal access token* for that one repository only,
   with Contents: read and write, and an expiry. Store it as the repository secret
   **`HOMEBREW_TAP_TOKEN`**.
4. **GitHub Pages** deploys `site/` (`site.yml`), which is what serves `install.sh`.

The workflow's first publishing step checks that both secrets exist and that the npm token
works, before anything is published.

**Hardening (recommended).** The publish job runs in the GitHub environment `release`. Create
it (Settings, Environments), restrict *Deployment branches and tags* to the tag pattern `v*`, and
move `NPM_TOKEN` and `HOMEBREW_TAP_TOKEN` from the repository secrets into that environment.
Then only a run on a version tag can read them, and a manual run of the workflow on some
branch cannot. Add required reviewers there if a second pair of eyes should approve each
release.

Second, decide who can create a version tag at all: a *tag ruleset* (Settings, Rules, Rulesets;
target tags matching `v*`; restrict creation, update and deletion; the bypass list is you). The
workflow refuses a `v*` tag whose commit is not on `main`, so a tag on a branch or on rewritten
history builds like a dry run and publishes nothing; the ruleset stops such a tag from being
pushed in the first place, by anyone else with push access.

## The dry run

Run it on any branch, any time (GitHub offers manual runs of a workflow only once its file is on
the default branch); it builds, packages and tests everything and publishes nothing:

```sh
gh workflow run release.yml --ref <branch> -f publish=false
gh run watch                       # pick the run
gh run download <run-id>           # the workflow artifacts, to look at them
```

A green dry run shows that: the four binaries build and report `undra <version> (<sha>)`; the
`checksums.txt` verifies; the seven npm tarballs are built, `npm install -g` of the launcher
and the Linux x64 package gives a working `undra`; `site/install.sh` downloads, verifies and
installs the same tarballs from a local web server; the Homebrew formula generates and is
valid Ruby. The job summaries list the checksums and the glibc version the Linux binaries need.

What a dry run cannot show, and the first real release therefore proves: that the npm token is
accepted and the package names are free, that npm accepts the provenance statement, that
`gh release create` and the tap push succeed, and that `brew install` of the published formula
works. The checks after a release below cover each.

## The release

```sh
scripts/bump-version.sh 1.0.0       # Cargo.toml, Cargo.lock, the TS runtime, packaging/npm; prints the files
git switch -c release/1.0.0
git commit -am "chore(release): 1.0.0"
git push -u origin release/1.0.0    # open the pull request; CI green; merge
git switch main && git pull
git tag v1.0.0 && git push origin v1.0.0
gh run watch
```

Add the release's notes to `crates/undra-cli/src/migrations.rs` in the same pull request: `undra upgrade` prints the notes of
every release a project crosses (what an app author must do, what behaves differently, what is new), and a release with
no entry prints nothing, which tells the author nothing changed. The projects `undra init` writes pin this version
(`core/Cargo.toml`'s git tag, the runtimes, `UNDRA_VERSION` of `.github/workflows/undra.yml`), so the release has to exist
for their first CI run and `cargo build` to succeed.

The tag has to be `v` plus the workspace version, on the merged commit, which has to be on
`main`. The workflow's first job refuses anything else (`scripts/bump-version.sh --check` also
fails when any version file was missed). It then:

1. **builds** `undra-cli` with `--release --locked` for the four targets (`UNDRA_BUILD_SHA` is the
   commit, so `undra --version` says which one), checks the version line, and packs
   `undra-v1.0.0-<target>.tar.gz` (the binary and the licences);
2. **packages** `checksums.txt`, the npm tarballs and the formula, and tests the npm install and
   the curl installer on them;
3. **publishes**, in this order: the GitHub Release (generated notes), the npm packages
   (`@undra/runtime`, the four platform packages, `@undra/cli`, `undra`, all with provenance),
   and the commit that updates `Formula/undra.rb` in the tap.

Every publishing step is idempotent, so a release that stopped half-way (an expired token, a
network error) is completed by re-running the failed job, or
`gh workflow run release.yml --ref v1.0.0 -f publish=true`. Nothing is ever overwritten: an
existing GitHub Release must hold the same `checksums.txt`, and an npm version that already
exists is skipped only when the registry's tarball has the same bytes (its `dist.integrity`) as
the one this run built; otherwise the run fails and the answer is a patch release. Before
anything is published, the publish job checks the downloaded artifacts against the checksums the
build and package jobs wrote, so what goes out is what was tested.

A version with a suffix (`1.0.0-rc.1`) is a prerelease: a GitHub prerelease (the installer's
"latest" ignores it), npm tag `next`, and the tap is left alone. Install one with
`UNDRA_VERSION=1.0.0-rc.1 sh install.sh` or `npm install -g @undra/cli@next`.

## After a release: check each channel

Use clean locations; the version is the one you tagged and the commit is the tag's, first seven
characters.

```sh
# GitHub Release: four tarballs and checksums.txt
gh release view v1.0.0
# Homebrew
brew update && brew install shreypdev/undra/undra && undra --version && brew test undra
brew audit --strict shreypdev/undra/undra
# npm (a scratch prefix, so nothing of yours is touched)
npm install -g --prefix "$(mktemp -d)" @undra/cli && npm view @undra/cli dist-tags
npm view undra version && npm view @undra/runtime version     # all three at 1.0.0
npm view @undra/cli@1.0.0 dist.attestations                    # the provenance attestation exists
# curl (a scratch UNDRA_HOME)
curl -fsSL https://shreypdev.github.io/undra/install.sh | UNDRA_HOME="$(mktemp -d)" sh
# cargo, and a project that pins the tag
cargo install --locked --git https://github.com/shreypdev/undra --tag v1.0.0 undra-cli
undra init smoke --platforms web --dir "$(mktemp -d)" && undra --version
```

`undra --version` prints `undra 1.0.0 (<sha>)` from brew, npm and curl and `(unknown)` from a
cargo build. In the project `undra init` made, `core/Cargo.toml` names
`tag = "v1.0.0"`, and `undra bindgen --check` inside it passes.

## Rollback

Releases are immutable; the answer to a bad release is a patch release (`scripts/bump-version.sh
1.0.1`, the same steps). Until it is out, steer users away:

* **GitHub Release**: `gh release edit v1.0.0 --prerelease` stops the installer's "latest" from
  choosing it (and `gh release edit <previous tag> --latest` pins the previous one). Do not
  delete a release or move a tag: projects made by `undra init` pin the tag.
* **npm**: a version cannot be published twice. Point `latest` back, and deprecate:
  `npm dist-tag add @undra/cli@<previous> latest` (also `undra`, `@undra/runtime`), then
  `npm deprecate @undra/cli@1.0.0 "broken, use 1.0.1"`. The launcher pins its platform packages
  to its own version, so moving the launcher's tag is enough.
* **Homebrew**: revert the formula commit in `shreypdev/homebrew-undra` and push:
  `git revert <commit> && git push`.
* **cargo**: nothing to roll back; users pin `--tag`.

## Maintenance

* **Linux binaries need glibc at least as new as the runner's.** The matrix builds on
  `ubuntu-22.04` and `ubuntu-22.04-arm` on purpose, so the floor is glibc 2.35 (Ubuntu 22.04,
  Debian 12, RHEL 9 and derivatives); the build job's summary prints the exact floor. When GitHub
  retires the 22.04 images, moving the two Linux `os:` values to 24.04 raises the floor to 2.39;
  keeping 2.35 then means building in a 22.04 container or with `cargo zigbuild`.
* **macOS binaries are not notarised.** A binary fetched by `curl`, Homebrew or npm carries no
  quarantine flag and runs; one downloaded in a browser needs `xattr -d com.apple.quarantine`.
* **Action pins.** Every third-party action in `release.yml` is a commit SHA with its version in
  a comment. To update one: `gh api repos/<owner>/<name>/git/ref/tags/<tag> --jq .object.sha`
  (for an annotated tag, follow it to the commit), edit the SHA and the comment, run the dry
  run.
* **Tokens expire.** `NPM_TOKEN` and `HOMEBREW_TAP_TOKEN` have the expiry chosen when they were
  made; the preflight step names the one that no longer works.
* **Tests for the pieces**: `bash packaging/npm/test.sh`, `bash packaging/test-install.sh`
  (both build the CLI, or use `UNDRA_BIN`), and `bash scripts/bump-version.sh --check`.
