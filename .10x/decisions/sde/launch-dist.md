# SDE — launch-dist: launch distribution from GitHub (wt/launch-dist, 2026-10-03)

ADR-063 (Accepted): everything a project made by a released `undra` needs resolves from `github.com/shreypdev/undra` at one
release tag; no registry account on either side; Maven Central and the npm registry later, additive. The founder decided GitHub
only and no npm channel for the CLI; this piece implements it, fixes two product defects the site agent found, and proves the
whole thing with a rehearsal that needs no account.

## What landed, per item of the brief

1. **One version.** `scripts/bump-version.sh` (the existing mechanism) now also sets `@undra/testkit`,
   `@undra/react-native` (and their locks) and every `"@undra/runtime": "^<version>"` range of the repository (the peers, the
   generated packages, the bindgen goldens; not `tests/fixtures`, which are older releases on purpose); the npm wrapper's
   templates are gone from it. Nothing else carries the number: the Swift package and the Kotlin artifacts take the tag, the CLI's
   pins come from `CARGO_PKG_VERSION`. `[undra] version` is now the full release (`1.0.0`, a two-part value still reads as
   `<line>.0`). `scripts/bump-version.test.sh` runs the script on a copy of the tracked tree with `9.8.7-test.1` (also with
   macOS's bash 3.2 and without git); CI's Rust job runs it. The generated `package.json`'s runtime range was the literal
   `^0.1.0` in `undra-bindgen`: now `^<generator version>`, so adopt's `npm install <runtime asset> <generated package>` keeps
   resolving after 1.0.0. Not bumped to 1.0.0 here.
2. **Swift.** `Package.swift` at the root (package `undra`, products `UndraRuntime` and `UndraTestKit`, the three targets by
   `path:` into `runtimes/swift/UndraRuntime/Sources`, no tests). `crates/undra-cli/tests/swift_manifests.rs` keeps the two
   manifests in step (text, and `swift package dump-package` where Swift is); the macOS CI job builds the root package
   (`swift build`, 6 s here). Init, bindgen (`.package(url: "https://github.com/shreypdev/undra", from: "<v>")`,
   `.product(name: "UndraRuntime", package: "undra")`), adopt and upgrade use the repository URL. **Clone size:** a bare clone of
   the full history is 47 MiB (1,805 commits); a checkout 45.5 MiB (3,697 files; `examples/` 14.2, `crates/` 11.0, `runtimes/`
   7.3, `site/` 3.1 MiB).
3. **Kotlin.** The six modules apply `maven-publish` (JVM: `java` component with sources; Android: the `release` single
   variant with sources). The build keeps `dev.undra` / `0.1.0-SNAPSHOT` (composite builds), `-PundraGroup`/`-PundraVersion`
   override. `jitpack.yml` (JDK 17) runs `runtimes/kotlin/undra-runtime/scripts/jitpack-install.sh`: `publishToMavenLocal`
   as `$GROUP.$ARTIFACT:<module>:$VERSION` and a named failure for any of the six missing. One constant,
   `crates/undra-cli/src/dist.rs` `MAVEN` (group, version prefix `v`, repository JitPack); templates, bindgen, adopt and upgrade
   go through it. The template's `settings.gradle.kts` declares JitPack with `content { includeGroup(...) }`; upgrade adds it
   when a project's pins move to that group.
4. **Web and React Native.** `packaging/pack-npm.sh` packs the three packages; `release.yml` attaches them to the Release (in
   `checksums.txt`) and, before publishing, `packaging/test-npm-assets.sh` installs them by URL from a local web server into a
   project with a registry that does not exist: no `@undra` lookup by name, `npm ls --all` clean. Decision: the React Native
   host and the testkit keep `@undra/runtime` as a **peer** (`^<version>`); npm checks it against the installed package's
   version wherever it came from. Shown to fail (`ERESOLVE`) with a mismatched peer range. `undra upgrade` rewrites the URLs
   (and registry ranges of before into URLs).
5. **Channels.** `packaging/npm` is gone, with its build and test steps, the npm publish step and `NPM_TOKEN`. C0014's help
   names `brew upgrade undra`, the installer or `cargo install ... --force` by where the running binary is (a Homebrew cellar,
   `$UNDRA_HOME/bin` or `~/.undra/bin`, `$CARGO_HOME/bin` or `~/.cargo/bin`), all three otherwise (the golden: all three).
   A prerelease needs no tap token, so an rc can run before the tap exists.
6. **Defects.** (a) Reproduced on a generated project in Chromium: with `?undra=`, a core edit made `undra dev` restart the core
   with its state, then the Vite plugin's `full-reload` broadcast reloaded the page, which attached as a new client and started
   fresh. Cause: the plugin's broadcast alone (with the plugin's new HMR filter switched off, Vite logged no change for the
   wasm: it does not watch the build directory). Fix: under `vite dev` the plugin adds `virtual:undra/dev-reload` to the page and
   sends `undra:core-rebuilt`; the page reloads unless `?undra=`/`VITE_UNDRA_DEV_URL` says it runs the served core; a page
   without the module gets the plain reload; `handleHotUpdate` keeps Vite from reloading over the build directory. Verified: the
   served page kept its items across a rebuild (console says why), the wasm page reloaded onto the new core. (b) The devtools
   page's icon is `site/favicon.svg` inline, with a test.
7. **`docs/RELEASING.md`** is the launch checklist (below), with the rc rehearsal, JitPack warm-up, per-channel checks and
   fix-forward.
8. **Docs and site.** Getting started shows the four dependency lines; the roadmap's Next item is "Maven Central and the npm
   registry", "Everything from GitHub at one tag (ADR-063)" is Shipped (Platforms); the launch post's line 108 and its claim L29
   (only those) say what is true now. SPEC 13, the CLI page, README, React Native, testing, the network-stack recipe, ONBOARDING
   and the Kotlin modules' READMEs follow. 54 pages OK, landing 275/350.

## Found by the rehearsal

The first run failed on the web: `npm run build` of a released project could not resolve `@undra/runtime` from
`generated/ts` (outside `web/`, so Node's lookup never reaches `web/node_modules`; checkout projects alias the sources, so nothing
had shown it). A released project now has `paths` in `web/tsconfig.json` and `resolve.dedupe` in `vite.config.ts`; each alone
fails (checked). This was broken before this piece too, registry or not.

## The rehearsal (`packaging/rehearse-launch.sh`, this Mac, warm caches)

```
Rehearsal of Undra 0.1.0-rehearsal.1
  remote (bare clone, tag)     ok   v0.1.0-rehearsal.1 at file://…/remote/undra.git
  undra CLI                    ok   undra 0.1.0-rehearsal.1 (unknown) (3 s)
  npm assets (3 tarballs)      ok   http://127.0.0.1:<port>/v0.1.0-rehearsal.1 (5 s)
  Kotlin modules (JitPack)     ok   com.github.shreypdev.undra:*:v0.1.0-rehearsal.1 (6 s)
  undra init                   ok   core pins v0.1.0-rehearsal.1 of the remote
  web (npm run build)          ok   built without the checkout in 13 s
  iOS (xcodebuild, simulator)  ok   built without the checkout in 22 s
  iOS: Swift package           ok   undra 0.1.0-rehearsal.1 from the bare clone (Package.resolved)
  Android (assembleDebug)      ok   built without the checkout in 20 s
Every platform built from the rehearsal release alone.
```

The snapshot is moved away before `undra init`, and the project files and build logs are checked for any path to it or to the
checkout. CI: `launch-rehearsal.yml` (macos-15), by hand and on pull requests touching the templates, `dist.rs`, the version
script, `packaging/`, `jitpack.yml`, `Package.swift` or the Kotlin build files; not a required check (about 20 to 30 runner
minutes cold).

## Not verifiable without a real tag

JitPack's hosted build (its environment, the Android SDK components it has, its group mapping); SwiftPM against github.com (the
tags, the 47 MiB clone); the Homebrew tap push and `brew install`; `gh release create` with the `.tgz` assets. The checklist's
step 4 (`v1.0.0-rc.1`) exercises all of them before `v1.0.0`.

## Left

* `docs/RELEASING.md` step 5's migration re-key (`0.1.0` -> `1.0.0`) stays a manual edit in the version PR.
* The launch post's claim L27 cites `vite.ts` lines 18-20 for the reload; the lines moved by six (the claim is still true).
