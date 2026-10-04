# SDE: launch without a Homebrew tap (wt/no-homebrew, 2026-10-03)

The founder decided to launch with no Homebrew tap: `undra` is installed by the shell installer (first) or by cargo (second), and
`brew install undra` comes later through homebrew-core. ADR-063 has a dated amendment ("no Homebrew tap at launch"); no wire,
ABI, schema or generated shape changed, so no new ADR. What this removes: a repository, a token (`HOMEBREW_TAP_TOKEN`) and one
channel that could fail on launch day. The release now needs no secret beyond GitHub's own `GITHUB_TOKEN`.

## Scope rule

Only Undra's own Homebrew channel went: `brew install shreypdev/undra/undra`, `shreypdev/homebrew-undra`, `HOMEBREW_TAP_TOKEN`,
the formula template and generator, the release steps that pushed the formula, and the upgrade help that said `brew upgrade`.
Everything else that says Homebrew is about tools and stays: `undra doctor`'s `brew install <tool>` fixes, the toolchain lookups
under `/opt/homebrew`, `scripts/env.sh`, `docs/ONBOARDING.md`, the Bazel approval notes, CI's `brew install bazelisk`, getting
started's `brew install node binaryen ...`, and the historical records. Two search paths that name `/opt/homebrew/bin` stay on
purpose: the generated Gradle task and Xcode build phase look for `undra` in `~/.undra/bin`, `~/.cargo/bin`, `/opt/homebrew/bin`
and `/usr/local/bin` (a generated shape, changing it needs an ADR, R11; and the place a homebrew-core `undra` will live), and
`packaging/rehearse-launch.sh` warns when an `undra` sits in any of them because Xcode's build phase would pick it.

## What changed

| Where | Change |
|---|---|
| `packaging/homebrew/` | Removed (`undra.rb.tmpl`, `generate.sh`). Nothing tested it separately: no test under `packaging/` or `crates/undra-cli/tests` named the formula. |
| `.github/workflows/release.yml` | The package job no longer generates the formula (`ruby -c`) or uploads `homebrew-formula`; the publish job no longer downloads it, runs the secret preflight or pushes `Formula/undra.rb`, and reads no secret. Jobs renamed `Package (checksums, the npm assets)` and `Publish (the GitHub Release)`; the header says "Publishing is a GitHub Release: the CLI tarballs, the runtime tarballs and checksums.txt". The `release` environment stays (a place to restrict publishing to `v*`; it holds nothing). |
| `packaging/pack-release.sh`, `test-install.sh`, `rehearse-launch.sh` | Unchanged: none had a Homebrew part. |
| `crates/undra-cli/src/commands/upgrade.rs` | `Channel::Homebrew` and the cellar detection removed. A binary the CLI cannot place gets "the installer, or cargo if you installed it with cargo"; a path under a Homebrew prefix is such a binary (unit test: `/opt/homebrew/Cellar/...`, `/opt/homebrew/bin/undra`, Linuxbrew's cellar all give `None`, and the help names the installer first, no `brew`). `tests/upgrade.rs` asserts the installer and cargo are named and `brew` is not; the C0014 golden and `site/docs/errors.html` (generated from it) follow. |
| Site | The landing page's two install blocks are two tabs, `curl` then `cargo` (the tab script needed no change); getting started (description, five-minute block, channel table); `llms.txt`, `llms-full.txt`, `search-index.json` regenerated; the landing prose is 275 of 350 words. |
| README, `docs/SITE.md` | Install section lists two ways; the open list gains the Homebrew item; SITE.md states the order and that Homebrew is later. |
| Launch post | The "try it" block starts with the installer; the sentence after it names the cargo command. Its claims ledger: L24 rewritten (the three commands, the installer first; changed 2026-10-03), the header sentence no longer lists a tap among what does not exist, and L36's `RELEASING.md` line reference follows the new numbering. The other posts never showed the brew command (`why-undra-is-the-default-choice` O14, "nothing is on ... Homebrew yet", is still true). |
| `docs/RELEASING.md` | "Create the tap repository" and "Give the workflow a token for the tap" removed; the hardening (the `release` environment, the tag ruleset) and the `@undra` scope reservation stay, as step 2; the checklist is nine steps (check the repository; harden the repository; rehearse with a release candidate; set the version; open and merge the version pull request; push the tag; what the workflow publishes, and warming JitPack; check each channel from a clean machine; announce); every cross-reference to a step number renumbered; the `brew` lines, the tap rollback entry and the tap token's expiry entry gone; says plainly that the release needs no secret; the clean-machine check runs the installer into a temporary `UNDRA_HOME` and calls its binary by path. A maintenance bullet records the homebrew-core plan. |
| `site/data/roadmap.json` | One Next item, "Homebrew (brew install undra)": through homebrew-core once the repository is 30 days old and notable; source: ADR-063's amendment. The title has no backticks because titles render as plain text. Not a landing teaser (the word budget). |
| `.10x/` | ADR-063's amendment; checkpoint 35's founder steps and open list corrected in place in `status.md`, and the two places `handoff.md` named the tap, the token and `brew`. |

## The homebrew-core plan (the two conditions)

1. The repository is at least 30 days old. 2. It is notable: 75 stars, or 30 forks or watchers. Then a formula is a pull request to
`Homebrew/homebrew-core` (the removed template is a starting point in git history, last present at `da087cc`), `Channel::Homebrew`
returns to `undra upgrade`, and the install blocks gain the command. The conditions are the founder's, recorded as given; they were
not looked up again here.

## Verified

`cargo fmt --check`; `cargo clippy --all-targets -- -D warnings`; `cargo test -p undra-cli` (24 test binaries, all ok, including the
changed unit test, `a_project_ahead_of_this_undra_is_refused_with_the_way_out` and the C0014 golden); `bash packaging/test-install.sh`
(all checks pass); `node site/scripts/build-all.mjs` (clean, second run changes nothing); `node site/scripts/check-links.mjs` (54 pages
OK); `--words` (275 of 350); `release.yml` parsed as YAML (four jobs). `bash packaging/rehearse-launch.sh` and the final
`grep` are in the pull request.

## Not done, and why

* A real release has not run, so "the release needs no secret" is read from the workflow, not shown: the dry run
  (`gh workflow run release.yml --ref <branch> -f publish=false`) can be run once this is on `main`; it was not run from here (no
  workflow dispatch for a branch whose file is not on the default branch).
* `docs/blueprint.html` (the founding design document) shows `cargo install undra-cli  # or: brew install undra` in an
  illustrative terminal; that is the homebrew-core command, not the tap's, and it becomes true later, so it stays.
* `site/blog/undra-1-0/claims.md` L37 still says `@undra/runtime` comes from npm and Maven Central; that is older than ADR-063 and
  not this piece's.
