# Releasing Undra

One tag, one workflow, one repository (ADR-063). Everything an Undra app needs comes from
`github.com/shreypdev/undra` at a release tag `v<version>`; no registry account exists on either side.

| What | What users run or write | Where it comes from |
|---|---|---|
| CLI, Homebrew | `brew install shreypdev/undra/undra` | the formula in `shreypdev/homebrew-undra`, pointing at the GitHub Release's tarballs |
| CLI, installer | `curl -fsSL https://shreypdev.github.io/undra/install.sh \| sh` | `site/install.sh`, which downloads the Release's tarball and checks its sha256 |
| CLI, cargo | `cargo install --locked --git https://github.com/shreypdev/undra --tag v1.0.0 undra-cli` | the repository |
| Rust crates | `undra = { git = "https://github.com/shreypdev/undra", tag = "v1.0.0" }` | the repository at the tag |
| Swift runtime | `.package(url: "https://github.com/shreypdev/undra", from: "1.0.0")` | `Package.swift` at the repository's root, at the tag |
| Kotlin runtime | `com.github.shreypdev.undra:runtime:v1.0.0` with `https://jitpack.io` for that group | JitPack, which builds the tag (`jitpack.yml`) the first time it is asked |
| npm runtimes | `"@undra/runtime": "https://github.com/shreypdev/undra/releases/download/v1.0.0/undra-runtime-1.0.0.tgz"` | the GitHub Release's assets (also `undra-react-native-*.tgz`, `undra-testkit-*.tgz`) |

`undra init` writes exactly those lines for the release it belongs to; `undra upgrade` moves them. The workflow
(`.github/workflows/release.yml`) builds the CLI for macOS and Linux on x86_64 and arm64, packs the three npm
packages, tests both installs on the real artifacts, and on a version tag publishes the GitHub Release and the
tap's formula. Maven Central and the npm registry are later and additive (ADR-063: one constant and one
`undra upgrade`). Windows and Alpine (musl) are not supported (roadmap).

## The launch checklist

In order. Each step is one command or one click, and says how to see that it worked. Run the commands from a
clone of `shreypdev/undra` on `main`, with `gh` signed in as the repository's owner.

### 1. Check the repository

```sh
gh repo view shreypdev/undra --json visibility,defaultBranchRef   # PUBLIC, main
curl -fsSI https://shreypdev.github.io/undra/install.sh | head -n 1  # HTTP/2 200 (GitHub Pages, site.yml)
```

The repository must be public: SwiftPM, JitPack, Cargo and the installer read it without credentials.

### 2. Create the tap repository

```sh
gh repo create shreypdev/homebrew-undra --public --add-readme --description "Homebrew tap for Undra"
```

Verify: `gh repo view shreypdev/homebrew-undra` shows it with a default branch (the README gives it one; the
first release creates `Formula/`).

### 3. Give the workflow a token for the tap

GitHub, Settings of your account, Developer settings, Fine-grained personal access tokens, Generate: repository
access **only `shreypdev/homebrew-undra`**, permission **Contents: read and write**, an expiry. Then:

```sh
gh secret set HOMEBREW_TAP_TOKEN --repo shreypdev/undra     # paste the token
```

Verify: `gh secret list --repo shreypdev/undra` lists `HOMEBREW_TAP_TOKEN`. (No npm token: nothing is published to
npm. If `NPM_TOKEN` is still there from before, `gh secret delete NPM_TOKEN --repo shreypdev/undra`.)

Recommended hardening, once: create the environment `release` (Settings, Environments), restrict *Deployment
branches and tags* to the tag pattern `v*` and move the secret into it (`gh secret set HOMEBREW_TAP_TOKEN --env
release`); add a tag ruleset (Settings, Rules, Rulesets: tags matching `v*`, restrict creation, update and
deletion, bypass: you). The workflow already refuses a `v*` tag whose commit is not on `main`.

### 4. Rehearse with a release candidate (recommended)

A prerelease exercises every channel that only a real tag can (SwiftPM against github.com, JitPack's build, the
Release's assets, the installer) before `v1.0.0` exists, and announces nothing.

```sh
scripts/bump-version.sh 1.0.0-rc.1
git switch -c release/1.0.0-rc.1 && git commit -qam "chore(release): 1.0.0-rc.1"
git push -u origin release/1.0.0-rc.1 && gh pr create --fill     # four "All green", then squash-merge
git switch main && git pull && git tag v1.0.0-rc.1 && git push origin v1.0.0-rc.1
gh run watch
```

What it publishes: a GitHub **prerelease** `v1.0.0-rc.1` with the four CLI tarballs, the three npm tarballs and
`checksums.txt`; nothing else. The tap is left alone, the installer's "latest" ignores prereleases, no post, no
formula, no registry. Then run steps 8 and 9 with `1.0.0-rc.1` for `1.0.0` (install the CLI with
`curl -fsSL https://shreypdev.github.io/undra/install.sh | UNDRA_VERSION=1.0.0-rc.1 sh`): a project made by
that CLI pins `v1.0.0-rc.1` everywhere. A failure here costs a `1.0.0-rc.2`, not a broken `1.0.0`. The
prerelease and its tag can stay; nothing points at them.

The same rehearsal without any account runs on one machine: `bash packaging/rehearse-launch.sh` (a copy of the
repository as the remote, the assets on a local web server, JitPack's command into a local Maven repository; the
summary says per platform whether the app built without the checkout), or the workflow *Launch rehearsal* on
GitHub (`gh workflow run launch-rehearsal.yml`).

### 5. Set the version

```sh
scripts/bump-version.sh 1.0.0
```

It sets the workspace and `Cargo.lock`, the three npm packages and their locks, and every `"@undra/runtime"`
range, and lists the files. In the same change, re-key the first entry of `crates/undra-cli/src/migrations.rs`
from `0.1.0` to `1.0.0` ("Since v1.0"): `undra upgrade` prints the notes of every release a project crosses, and
the projects of a `0.1.0` CLI must cross this one. Verify: `bash scripts/bump-version.sh --check 1.0.0` says
`every version file says 1.0.0`.

### 6. Open and merge the version pull request

```sh
git switch -c release/1.0.0 && git commit -qam "chore(release): 1.0.0"
git push -u origin release/1.0.0 && gh pr create --fill
gh pr checks --watch                # the four "All green"
gh pr merge --squash
```

Verify: `git switch main && git pull && bash scripts/bump-version.sh --check` says `1.0.0`.

### 7. Push the tag

```sh
git tag v1.0.0 && git push origin v1.0.0
gh run watch                        # Release: verify, four builds, package, publish
```

### 8. What the workflow publishes, and warming JitPack

The run, in order: **verify** (the tag is `v` plus the workspace version, every version file agrees, the commit is
on `main`); **build** the CLI for four targets (`undra --version` says `undra 1.0.0 (<sha>)`); **package**:
`checksums.txt`, the three npm tarballs, an install of them by URL with no registry, the curl installer against the
tarballs, the Homebrew formula; **publish**: the GitHub Release `v1.0.0` (generated notes, the seven assets and
`checksums.txt`), then the commit `undra 1.0.0` of `Formula/undra.rb` in the tap. Nothing else is published: the
crates and the Swift package are the tag itself.

```sh
gh release view v1.0.0              # undra-v1.0.0-<4 targets>.tar.gz, undra-{runtime,react-native,testkit}-1.0.0.tgz, checksums.txt
```

JitPack builds the Kotlin modules the first time anyone asks for them. Ask yourself, before announcing:

```sh
curl -fsS -o /dev/null -w '%{http_code}\n' https://jitpack.io/com/github/shreypdev/undra/runtime/v1.0.0/runtime-v1.0.0.pom
# The first request starts the build and may time out: repeat it until it prints 200 (a few minutes). Then the log:
curl -fsS https://jitpack.io/com/github/shreypdev/undra/v1.0.0/build.log | tail -n 20
```

The log ends with `Published: runtime testkit android-adapters android-work undra-compose okhttp-adapters
(com.github.shreypdev.undra, v1.0.0)` (`runtimes/kotlin/undra-runtime/scripts/jitpack-install.sh`) and JitPack's
list of build artifacts. The same is at https://jitpack.io/#shreypdev/undra, the tag's row and its log icon. Check
one Android module too: the same `curl` for `android-adapters/v1.0.0/android-adapters-v1.0.0.pom` prints 200.

### 9. Check each channel from a clean machine

Use clean locations; the version is the tag's and the commit its first seven characters.

```sh
brew update && brew install shreypdev/undra/undra && undra --version && brew test undra   # undra 1.0.0 (<sha>)
brew audit --strict shreypdev/undra/undra
curl -fsSL https://shreypdev.github.io/undra/install.sh | UNDRA_HOME="$(mktemp -d)" sh
cargo install --locked --git https://github.com/shreypdev/undra --tag v1.0.0 undra-cli    # says (unknown) for the sha

cd "$(mktemp -d)" && undra init smoke && cd smoke
grep 'undra = ' core/Cargo.toml                       # tag = "v1.0.0"
(cd web && npm install && npm run build)              # @undra/runtime from the Release's asset
xcodebuild -project ios/Smoke.xcodeproj -scheme Smoke -destination 'generic/platform=iOS Simulator' build
(cd android && ./gradlew :app:assembleDebug)          # com.github.shreypdev.undra:*:v1.0.0 from JitPack
undra bindgen --check                                 # the bindings init wrote are current
```

The iOS build resolves the Swift package `undra` 1.0.0 from github.com (Xcode's package list shows it); the first
resolution clones the repository (about 47 MiB).

### 10. Announce

Only now: the post, the links. Everything a reader runs exists.

## When a channel fails

A tag is immutable and projects pin it: never move or delete one. Fix forward with `v1.0.1` (steps 5 to 9 with
`1.0.1`), and meanwhile steer users away:

* **The workflow stopped half-way** (an expired token, a network error): every publish step is idempotent. Re-run
  the failed job (`gh run rerun <run-id> --failed`) or `gh workflow run release.yml --ref v1.0.0 -f publish=true`.
  An existing Release must hold the same `checksums.txt`, else the run fails: cut a patch.
* **A broken GitHub Release**: `gh release edit v1.0.0 --prerelease` stops the installer's "latest" from choosing it
  (`gh release edit <previous tag> --latest` pins the previous one). Do not delete it: projects pin its assets.
* **Homebrew**: revert the formula commit in `shreypdev/homebrew-undra` and push (`git revert <commit> && git push`).
* **JitPack's build failed**: read the log (step 8). A failure on JitPack's side (a timeout, a missing SDK
  component) can be retried from https://jitpack.io/#shreypdev/undra (the tag's row); a failure in the repository is
  a patch release. Android apps of `v1.0.0` cannot build until a tag builds on JitPack.
* **The Swift package or the crates**: a broken tag is a patch release; `undra upgrade` moves projects to it.
* **cargo**: nothing to roll back; users pin `--tag`.

## The dry run

Run it on any branch, any time (GitHub offers manual runs of a workflow only once its file is on the default
branch); it builds, packages and tests everything and publishes nothing:

```sh
gh workflow run release.yml --ref <branch> -f publish=false
gh run watch                       # pick the run
gh run download <run-id>           # the workflow artifacts, to look at them
```

A green dry run shows that the four binaries build and report `undra <version> (<sha>)`; `checksums.txt` verifies;
the three npm tarballs install by URL from a local web server with no registry, every peer satisfied;
`site/install.sh` downloads, verifies and installs the CLI's tarball from a local web server; the Homebrew formula
generates and is valid Ruby. The job summaries list the checksums and the glibc version the Linux binaries need.
What only a real tag shows: that `gh release create` and the tap push succeed, that JitPack builds the tag, that
SwiftPM resolves the repository from github.com, and that `brew install` works: steps 4, 8 and 9.

## Maintenance

* **Linux binaries need glibc at least as new as the runner's.** The matrix builds on `ubuntu-22.04` and
  `ubuntu-22.04-arm` on purpose, so the floor is glibc 2.35 (Ubuntu 22.04, Debian 12, RHEL 9 and derivatives); the
  build job's summary prints the exact floor. When GitHub retires the 22.04 images, moving the two Linux `os:` values
  to 24.04 raises the floor to 2.39; keeping 2.35 then means building in a 22.04 container or with `cargo zigbuild`.
* **macOS binaries are not notarised.** A binary fetched by `curl` or Homebrew carries no quarantine flag and runs; one
  downloaded in a browser needs `xattr -d com.apple.quarantine`.
* **Action pins.** Every third-party action in `release.yml` is a commit SHA with its version in a comment. To update
  one: `gh api repos/<owner>/<name>/git/ref/tags/<tag> --jq .object.sha` (for an annotated tag, follow it to the
  commit), edit the SHA and the comment, run the dry run.
* **The tap token expires.** `HOMEBREW_TAP_TOKEN` has the expiry chosen when it was made; the preflight step says when
  it is missing or cannot push.
* **Moving to the registries** (ADR-063): Maven Central is the `MAVEN` constant of `crates/undra-cli/src/dist.rs`
  (`dev.undra`, no version prefix, no extra repository) plus a publishing job; the npm registry is the npm packages
  published as well as attached. `undra upgrade` moves existing projects either way.
* **Tests for the pieces**: `bash scripts/bump-version.test.sh`, `bash packaging/test-install.sh` (builds the CLI, or
  `UNDRA_BIN`), `bash packaging/pack-npm.sh --out <dir> && bash packaging/test-npm-assets.sh --release-dir <dir>`,
  and the whole thing: `bash packaging/rehearse-launch.sh`.
