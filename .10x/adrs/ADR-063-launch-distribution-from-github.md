# ADR-063: Launch distribution from GitHub

Status: **Accepted** (2026-10-03). Decided by the founder (GitHub only for the launch; registries later, without breaking anyone;
no npm install channel for the CLI); implemented by `wt/launch-dist` (record: `.10x/decisions/sde/launch-dist.md`). It changes
generated public shapes (the dependency lines of the projects `undra init` writes, of the Swift package and Gradle module
`undra bindgen` writes, and of what `undra adopt` tells an author to add), the meaning of `[undra] version`, and the release
pipeline. It does **not** touch the wire, the C or wasm ABI, the schema, the schema hash or any runtime's behaviour. Constitution
R7 (compatibility is checked at load and at build: every dependency of a generated app names one release), R8 (the new settings
fail with taught errors), R10 (the playground and the other examples keep using the checkout and are unaffected), R11.

## Context

Measured on 2026-10-03 by running the quickstart from an empty directory (`.10x/decisions/sde/launch-site.md`): a project
`undra init` makes outside a checkout cannot be built on any platform.

* The Rust core pins this repository at tag `v0.1.0` (the workspace version), which does not exist. That one is only a matter of
  tagging: the mechanism is right.
* The iOS app (and the generated Swift package) depend on `https://github.com/shreypdev/undra-swift`, which does not exist.
* The Android app (and the generated Gradle module) depend on `dev.undra:runtime` and `dev.undra:android-adapters` from Maven
  Central: not published, and nothing in `release.yml` or `docs/RELEASING.md` publishes them. Publishing to Maven Central needs a
  verified namespace and a signing key: days of founder-only setup.
* The web app depends on `@undra/runtime` from the npm registry, which needs an npm organisation that does not exist.

So a newcomer can only build with `--undra-path <a checkout>`. The founder decided that the launch distributes everything from
`github.com/shreypdev/undra` alone, and that Maven Central and the npm registry come later as additions.

## Decision

Everything a generated app needs resolves from `github.com/shreypdev/undra` **at one release tag**, `v<version>`. No registry
account, no token on the consumer's side, no second repository.

### 1. One version, written down once per kind of file

* The projects `undra init` writes name **the exact release of the CLI** everywhere: the crates' git tag, the Swift package's
  minimum version, the Kotlin artifacts' version, the npm tarballs' URLs and the CI workflow's `UNDRA_VERSION`. Before this ADR
  the runtimes were pinned to a release line (`^0.1.0`, `0.1.0`, `upToNextMajor 0.1.0`) because a registry serves every patch of a
  line; a tag serves exactly one release, and the core and the runtimes are released together, so one exact release is both
  possible and the honest pin (R7: the runtime the bindings were generated for is the runtime the app gets).
* `[undra] version` in `undra.toml` becomes that full version (`"1.0.0"`, was `"1.0"`): `undra bindgen` writes the runtime
  requirements of the generated packages from it, and their bytes must not depend on which patch of the CLI ran (`--check`).
  A two-part value (written by an older CLI) still parses and means `<line>.0`; `undra upgrade` rewrites it to the full version.
* `scripts/bump-version.sh <version>` (the existing mechanism, extended) sets the workspace and its lock file, the three npm
  packages (`@undra/runtime`, `@undra/react-native`, `@undra/testkit`), their lock files and the peer ranges by which the latter two
  name the runtime. The CLI's pins come from `CARGO_PKG_VERSION` (nothing to edit); the Swift package has no version (SwiftPM reads
  the tag); the Kotlin artifacts take theirs from the tag (JitPack's `VERSION`, below), so no Gradle file carries a release
  number. `--check` covers every file the script writes, and the release workflow's first job runs it. A test runs the script on a
  copy of the repository with a throwaway version (`scripts/bump-version.test.sh`).

### 2. Rust: the repository's tag (unchanged)

`undra = { git = "https://github.com/shreypdev/undra", tag = "v1.0.0" }`. crates.io stays later (its own ADR).

### 3. Swift: a `Package.swift` at the repository root

The repository root gets a manifest (package name `undra`, the identity SwiftPM derives from the URL) that exposes the same
library products as `runtimes/swift/UndraRuntime/Package.swift` (`UndraRuntime`, `UndraTestKit`) through `path:` targets into
`runtimes/swift/UndraRuntime/Sources/...`, without the test targets. An app adds `https://github.com/shreypdev/undra` with
`from: "1.0.0"` (SwiftPM reads `v1.0.0` tags); the Xcode project `undra init` writes has that `XCRemoteSwiftPackageReference`
(`upToNextMajorVersion`, minimum the CLI's release), and the generated Swift package depends on the same URL and version
(`.product(name: "UndraRuntime", package: "undra")`). The in-repository package stays what the examples and the Swift tests use; a
test (`crates/undra-cli/tests/swift_manifests.rs`) keeps the two manifests' products, targets, platforms and language mode in step.

**Clone size.** SwiftPM clones the whole repository once per machine (then checks out the tag): measured on 2026-10-03, a bare
clone of the full history is **47 MiB** (1,805 commits) and a checkout 45.5 MiB (3,697 files; `examples/` 14.2 MiB, of which the
playground's proof screenshots are about 5 MiB, `crates/` 11.0 MiB, `runtimes/` 7.3 MiB). That is the price of one repository;
revisit (a mirror filled by the release, below) if a clone passes about 150 MiB or a user reports it.

### 4. Kotlin and Android: JitPack builds the Gradle modules from the tag

Coordinates `com.github.shreypdev.undra:<module>:v<version>` for `runtime`, `testkit` (JVM libraries), `android-adapters`,
`android-work`, `undra-compose` and `okhttp-adapters` (Android libraries, release variant with sources). The app's
`settings.gradle.kts` declares the repository for that one group only:

```kotlin
maven {
    url = uri("https://jitpack.io")
    content { includeGroup("com.github.shreypdev.undra") }
}
```

so no other dependency is ever looked up on JitPack. `jitpack.yml` at the root (JDK 17) runs
`runtimes/kotlin/undra-runtime/scripts/jitpack-install.sh`, which runs the Kotlin build's `publishToMavenLocal` with the group
and version JitPack asks for (`$GROUP.$ARTIFACT`, `$VERSION`: `com.github.shreypdev.undra`, `v1.0.0`) and fails, naming the
module, when one of the six is missing (an Android SDK JitPack did not provide). Every module applies `maven-publish`. In the
repository the build keeps its development identity (`dev.undra`, `0.1.0-SNAPSHOT`), which the checkout's composite builds
substitute and which is already the Maven Central name.

The CLI writes these coordinates from **one constant**, `dist::MAVEN` (group, version prefix, repository). Moving to Maven Central
later is that one line (`group: "dev.undra"`, no prefix, no extra repository), and `undra upgrade` already moves any Undra
coordinate it recognises (either group, any module of the six) to the constant's.

### 5. Web and React Native: the npm tarballs are assets of the GitHub Release

The release attaches `undra-runtime-<v>.tgz`, `undra-react-native-<v>.tgz` and `undra-testkit-<v>.tgz` (`npm pack` of each,
built and tested by the workflow before anything is published), and the projects depend on the asset's URL:

```json
"@undra/runtime": "https://github.com/shreypdev/undra/releases/download/v1.0.0/undra-runtime-1.0.0.tgz"
```

npm, pnpm and yarn install a tarball URL without a registry account, and the lock file records its integrity. `@undra/react-native`
and `@undra/testkit` keep naming the runtime as a **peer** (`^<version>`, set by the version script): npm checks a peer range
against the installed package's version wherever it came from, so an app that lists both URLs satisfies it, and a registry is
never consulted for `@undra/runtime`. The rehearsal installs the three tarballs from a local web server into a project with no
registry access at all and runs `npm ls` on it. `undra upgrade` rewrites the URLs.

### 6. The CLI's channels: Homebrew, the installer, cargo

The npm wrapper of the CLI (`@undra/cli`, its four platform packages, the unscoped `undra`) is removed: `packaging/npm`, its
jobs in `release.yml`, its test, the `NPM_TOKEN` secret and every mention. `undra upgrade`'s C0014 help names the channel the
running binary came from when its path says so (a Homebrew cellar, `~/.undra/bin`, `~/.cargo/bin`) and all three otherwise.

### 7. Mirror and rehearsal settings

Three environment variables replace the GitHub addresses in what `undra init`, `undra adopt` and `undra bindgen` write, for a
rehearsal against a local copy (`packaging/rehearse-launch.sh`) or a company mirror: `UNDRA_DIST_GIT_URL` (the repository: the
crates and the Swift package), `UNDRA_DIST_RELEASE_URL` (where `v<version>/<asset>` is downloaded) and `UNDRA_DIST_MAVEN_REPO`
(the Maven repository of the Kotlin artifacts). Each must be an `https://`, `http://` or `file://` URL (`ssh://` too for the git
one) without spaces or quotes; anything else stops the command with `C0009`, naming the variable, why and how to fix it. They are
not stored in the project: a command run without them writes GitHub's addresses.

## Alternatives considered

| Alternative | Pros | Cons | Why not (now) |
|---|---|---|---|
| A mirror repository `shreypdev/undra-swift` filled by the release | A small clone for SwiftPM | A second repository, a token with write access to it, a release step that can half-fail, and the same tag twice | 47 MiB is acceptable; revisit past about 150 MiB |
| A binary XCFramework of the Swift runtime | No source compile | Swift packages ship source; one more artifact to build, sign and checksum; debugging into the runtime gets worse | Source is what SwiftPM expects |
| Maven Central now | The conventional registry, no extra repository line | Namespace verification and a signing key: days of founder-only setup before launch | Later: the one-line change of section 4 |
| GitHub Packages for the Kotlin artifacts | Same host | Consumers need a personal access token even to read public packages | Unacceptable for a quickstart |
| The npm registry now | `npm update` works, no URL in package.json | Needs the `@undra` organisation and a token | Later, additive: `undra upgrade` moves URLs to ranges then |
| A git dependency with a subdirectory for npm | No release asset | npm cannot install a subdirectory of a git repository, and the runtime needs a build step | Does not work |

## Consequences

### Positive

* A project made by a released `undra` builds on iOS, Android and the web with nothing but GitHub, and every dependency names the
  same release.
* No consumer needs an account or a token; the founder needs one secret (the tap's) instead of two.
* The registries come later without breaking anyone: `undra upgrade` moves the pins.

### Negative

* JitPack builds an artifact the first time someone asks for it (minutes), and its availability is a third party's. The launch
  checklist asks for that first build itself, before announcing; Maven Central is the remedy if JitPack is unreliable.
* A tarball URL in `package.json` is not a range: `npm update` does not move it (`undra upgrade` does), and Dependabot does not see
  it.
* SwiftPM clones the whole repository (47 MiB), once per machine.
* A tag is now what every channel serves: it must never move (it never could: the crates were already pinned by tag).

### Neutral

* In-repository builds (the examples, the playground, the contract tests) keep their path and composite-build dependencies.
* The generated `Package.swift` and `build.gradle.kts` of a released project change their dependency line; a project made before
  this ADR is moved by `undra upgrade` (pins) and `undra bindgen` (the generated files).
