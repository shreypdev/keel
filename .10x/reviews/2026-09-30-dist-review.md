# Distribution pipeline - adversarial security review

**Date:** 2026-09-30 · **Reviewer:** fable (security) · **Piece:** `wt/dist` at `6962ace` (merged with `main` as `f7b50de`; review fixes committed on top) · **Spec:** `.10x/specs/2026-09-30-launch-v2-design.md`, Section 4 · **Decisions:** `.10x/decisions/{devops,qa}/launch-v2.md`, `.10x/decisions/devops/dist.md`

Threat model as briefed: a compromised or confused CI run publishing the wrong bytes; a token
with more power than needed; a user running the installer on an untrusted network or against a
tampered mirror; substitution of an action or a dependency; a release from a non-tag ref; a dry
run that publishes. Everything was read in the worktree; the workflow itself cannot run here, so
every `run:` block was extracted and passed through `bash -n`, the YAML was loaded, and the two
publish-time shell changes were exercised against the real npm registry with `npm publish`
stubbed. `actionlint` and `shellcheck` are not installed on this machine and were not run.

## Verdict

**The first dry run may be triggered now.** Build and package hold no secret and no write
permission; a `workflow_dispatch` with `publish=false` cannot reach the publish job (`verify`
emits `publish=false`, the job's `if` is on that output, and `success()` still applies); a
dispatch with `publish=true` on anything but the `v<workspace version>` tag is refused in the first
job; every third-party action is pinned to a commit that is the one its version comment names;
`${{ }}` appears in no `run:` block. **A real release may follow once the founder's prerequisites
exist** (`shreypdev/undra` already exists; `shreypdev/homebrew-undra` does not yet (404 today)
nor the npm org, `NPM_TOKEN` or `HOMEBREW_TAP_TOKEN`) **and the dry run is green**, with two things
done before the first tag: the `release` environment with the `v*` restriction and the tag ruleset
(both in `docs/RELEASING.md`, Hardening). Two of the fixes below (the on-`main` check and the
integrity-checked npm skip) and the `ubuntu-22.04` runner labels run for the first time on GitHub
in that dry run; the on-`main` check fires only when publishing, so the first real release is its
first exercise - if it refuses wrongly, its message says why and it is one `if` to remove.

## Findings

| # | Sev | Area | Where | Finding | Status |
|---|---|---|---|---|---|
| H | n/a | n/a | n/a | No High finding. | n/a |
| M1 | Medium | Trust boundary | `.github/workflows/release.yml`, `verify` (tag check, formerly lines 71–79) | A `v<workspace version>` tag on **any** commit published. `verify` checked that the tag equals the version, not that the tagged commit is on the default branch, so anyone with push access could tag a branch commit or rewritten history and publish it to all four channels; the `release` environment's `v*` rule does not help (the tag matches). | **Fixed**: `fetch-depth: 0` on `verify`'s checkout; when `publish=true`, `git merge-base --is-ancestor HEAD origin/$DEFAULT_BRANCH` (`github.event.repository.default_branch`) must hold, with a message naming the commit and the rule; dry runs are untouched. Exit codes of the command checked locally (on-main 0, off-main 1, unknown ref 128 → refused). GitHub-only, so the dry run cannot exercise it. **Open for the founder**: the tag ruleset (`RELEASING.md`, Hardening) so such a tag is not pushable in the first place. |
| M2 | Medium | npm idempotency | `release.yml`, npm step, `publish()` (formerly lines 355–362) | The re-run skip trusted name+version: an existing version with *different* bytes (the unscoped `undra` squatted at our exact version, or a republish after a moved tag) was logged "already on npm; skipping" and the run went green while the npm channel disagreed with the GitHub Release. | **Fixed**: the skip compares the registry's `dist.integrity` with `sha512-` + base64 of the tarball this run built; a mismatch fails with the patch-release instruction. Verified against the real registry with `npm@10.9.0`: identical bytes → skip; different bytes → error; missing version and missing package → the publish path (`npm view` exits 1 for both). |
| L1 | Low | Hash chain | `release.yml`, `release` job | The publish job trusted the artifact store: it uploaded `release/*` and `npm/*.tgz` without checking them against the checksums the build and package jobs wrote. | **Fixed**: `package` writes `npm/tarballs/SHA256SUMS` next to the tarballs it installed; `release` runs `sha256sum --check --strict` on `checksums.txt` and `SHA256SUMS` before preflight. The formula is not in the chain: it carries the release hashes and `brew install` fails loudly on a wrong one. |
| L2 | Low | Supply chain | `release.yml`, `package`, "Build the @undra/runtime package" | `npm ci` ran dependency install scripts on the release build (today only `fsevents`, macOS-only, has one; the lock holds 143 packages, all with `integrity`). | **Fixed**: `npm ci --ignore-scripts`; `npm run build` (`tsc`) verified locally with it, output present. |
| L3 | Low | Installer transport | `site/install.sh:22–25` | Setting `UNDRA_INSTALL_BASE_URL` to an **https** mirror also allowed plain http and http redirects for every transfer, the release lookup included. | **Fixed**: http is allowed only when the base URL is itself `http://`; a case added to `packaging/test-install.sh` §5 (https base + http API → refused). |
| L4 | Low | Installer swap | `site/install.sh:106` | The staged file was `$bin_dir/.undra.$$`: predictable, and `cp` follows a symlink planted under that name. Same-user trust domain (whoever can write `~/.undra/bin` can replace `undra` outright), so no escalation, but needless. | **Fixed**: `staged=$(mktemp "$bin_dir/.undra.XXXXXX")`; the upgrade test still finds exactly one file in `bin/`. |
| L5 | Low | Runners / glibc | `release.yml`, build matrix | Linux rows on `ubuntu-latest` / `ubuntu-24.04-arm` gave a glibc 2.39 floor. | **Fixed** (the integrator's decision): `ubuntu-22.04` / `ubuntu-22.04-arm` (2.35), comment on the rows, the floor print kept; `RELEASING.md` Maintenance rewritten (what to do when GitHub retires the 22.04 images); `dist.md` bullet annotated. |
| L6 | Low | Runbook / token scope | `docs/RELEASING.md`, prerequisite 2 | Guidance ended at "an all-packages token works for the first publish": no path to a narrower token or to none. | **Fixed**: after the first release, re-scope to `@undra` + `undra`, or use npm trusted publishing (OIDC, no `NPM_TOKEN`; needs npm ≥ 11.5 in the job; configured per package once the packages exist). |
| L7 | Low | Portability (informational) | `release.yml`, `"${tag_flags[@]}"`, `"${flags[@]}"` | Expanding an empty array under `set -u` is an error on bash 3.2 (macOS); the runners' bash 5 accepts it, which the original code already relied on. Surfaced while running the harness locally. | Open, informational; nothing to change for CI. |

Low fixes made directly: 6 (L1–L6). Medium fixed: M1, M2 (both in the workflow, verified as far as a machine without GitHub can). Open: the founder's tag ruleset (M1, defence in depth), L7 (informational).

## What was attacked and held

1. **Workflow trust boundaries.** Top-level `permissions: contents: read`; `release` alone has
   `contents: write` and `id-token: write`; `secrets.NPM_TOKEN` and `secrets.HOMEBREW_TAP_TOKEN`
   are referenced only in `release` (checked by loading the YAML). `verify`, `build`, `package` are
   secret-free and check out with `persist-credentials: false`; `release` checks out nothing, so no
   repository script runs with the secrets. `publish=false` → `verify` outputs `publish=false` →
   `release` skipped. Branch dispatch with `publish=true` → refused in `verify`. Tag whose name is
   not `v<workspace version>` → refused. No `${{ }}` inside any `run:` (every step's script was
   extracted: zero hits); attacker-shaped values (`github.ref_name`) travel through `env:` and are
   quoted. The tap token is an `http.extraheader` for `clone` and `push` only (not in a URL, not in
   `.git/config`), the base64 form is masked. Preflight checks both secrets before anything is
   published. `concurrency` never cancels a release in progress. Hash chain build → package →
   release: tarball `.sha256` written where the tarball was built, `checksums.txt` is their
   concatenation verified in `package`, and now re-verified in `release` (L1); the npm packages'
   binaries come out of those same tarballs (`build.mjs` also checks each binary's ELF/Mach-O
   machine against the target it claims).
2. **Action pins.** Fetched with `gh api repos/<o>/<r>/git/ref/tags/<tag>`: `actions/checkout`
   v7.0.1 → `3d3c42e5…`, `actions/upload-artifact` v7.0.1 → `043fb46d…`, `actions/download-artifact`
   v8.0.1 → `3e5f45b2…`, `actions/setup-node` v7.0.0 → `82076278…`: all lightweight tags, all four
   **match** the pins. `dtolnay/rust-toolchain@6bed0761…` is the head of its `stable` branch
   (commit "toolchain: stable", 2026-09-03), as the comment says. `toolchain: stable` floats to the
   day's stable (reproducibility, not integrity; `--locked` pins the crates).
3. **Artifact integrity, installer.** Download to a 0700 `mktemp -d` directory, never `curl | tar`;
   `checksums.txt` fetched from the same `<base>/v<version>/` as the tarball; the entry is selected
   by exact file name, must be unique and 64 hex, compared with the sha256 of the downloaded file;
   TLS only (`--proto '=https' --proto-redir '=https' --tlsv1.2`) unless the base URL is http (L3);
   `--max-time 600`; version validated before it reaches a URL; the tar extraction names the single
   member `undra` and rejects a symlink; `cp` then `mv -f` in the same directory (rename, atomic);
   no `sudo`, no `eval`, no `sh -c`. Function-on-last-line is real: `set -eu`, ten function
   definitions, `main "$@"` as line 261, nothing else at top level; test §9 cuts the script at line
   30 (syntax error, nothing runs) and at the last line (nothing runs). Every refusal in
   `test-install.sh` leaves `bin/` empty. Suite re-run after the edits: all checks pass under `sh`,
   `dash`, `bash`.
4. **npm.** `--provenance` with `id-token: write` on a public repo; both `@undra/runtime` and the
   generated packages carry a `repository.url` on `github.com/shreypdev/undra` (provenance requires
   it to match). `optionalDependencies` are the literal version (exact), asserted by `test.sh`;
   `os`/`cpu` per platform and `libc: ["glibc"]` on Linux; no `scripts` in any template (no
   `postinstall`); the unscoped `undra` is the same launcher with the same scoped
   `optionalDependencies`, not a pointer. Launcher: `execFileSync(binary, args, { stdio: "inherit"
   })`, exit code and signal passthrough, verified by `test.sh` (exit code 2 passes through).
   Publish order: runtime, four platform packages, `@undra/cli`, `undra`. Half-published set: every
   step idempotent; the skip is now integrity-checked (M2); the runbook says how to finish a run.
5. **Homebrew.** One `sha256` per platform, URLs by exact release file name, `test do` asserts the
   version in `undra --version`. The generator refuses a missing, duplicated, non-hex or
   wrong-length hash and a surviving placeholder; literal (non-regex) substitution. Re-run here: a
   formula generated from a synthetic `checksums.txt`, `ruby -c` OK, `brew style --formula` no
   offences, `brew audit --strict --formula` clean, in a throwaway local tap removed afterwards.
   (The audit pulled a `homebrew/core` tap onto this machine; it was untapped again - the machine is
   as it was.) Tap token: one repository, Contents read/write, passed as a header for two commands.
6. **CLI.** `UNDRA_RELEASE_TAG = "v" + CARGO_PKG_VERSION`; `undra init` outside a checkout writes
   `undra = { git = "https://github.com/shreypdev/undra", tag = "v<version>" }` (unit test
   `a_project_outside_a_checkout_pins_the_release_by_git_tag`); inside a checkout, or with
   `--undra-path`, a `path` dependency (`a_project_created_inside_a_checkout_uses_it_by_path`).
   `UndraSource::Git` keeps the core's own reference (`tag`/`branch`/`rev`/default, percent-decoded)
   so the shim and the core resolve to one source. `--version` → `undra <semver> (<sha7>|unknown)`
   (const-evaluated; integration test checks the shape and `-V`). `cargo install --git … undra-cli`
   is unblocked by the `Cargo.toml.tmpl` rename; no `@@` survives in any `Cargo.toml`; `grep -rni
   keel` over the templates, `packaging/`, `site/install.sh`, the runbook and the workflow: nothing.
7. **Runbook.** Prerequisites in a workable order (org before token; tap repo with a default branch
   before the PAT; Pages serves `install.sh` - `site/scripts/stage.sh` copies `site/.` whole). Dry
   run documented (`gh workflow run release.yml --ref <branch> -f publish=false`). Rollback is
   real: patch tag; `gh release edit --prerelease` so the installer's "latest" skips it; `npm
   dist-tag add … latest` + `npm deprecate`; `git revert` in the tap; never delete a release or move
   a tag (projects pin the tag). Added: the on-`main` rule, the tag ruleset, the integrity-checked
   skip, token re-scoping / trusted publishing, the 22.04 glibc note.

## Verified locally after the edits

`ruby -ryaml` load of the workflow; `bash -n` on every extracted `run:` block; `sh -n`, `dash -n`,
`bash -n` on `site/install.sh`, `packaging/pack-release.sh`, `packaging/homebrew/generate.sh`,
`packaging/test-install.sh`, `packaging/npm/test.sh`, `scripts/bump-version.sh`; `node --check` on
`build.mjs` and the launcher; `bash packaging/npm/test.sh` and `bash packaging/test-install.sh`
(with `UNDRA_BIN=target/debug/undra`): all checks pass; `npm ci --ignore-scripts && npm run build`
in `runtimes/ts/@undra/runtime`; the `publish()` harness against the registry (four branches).

## Not verifiable here (the first dry run / release proves them)

Runner labels `ubuntu-22.04-arm` and `macos-15`; the v7/v8 action majors; `fetch-depth: 0` plus
`origin/main` in `verify`; `npm publish --provenance` of a tarball and the name-similarity check
on the unscoped `undra`; `gh release create --verify-tag`; the tap push; `brew install` of the
published formula; the installer against GitHub's real redirects.
