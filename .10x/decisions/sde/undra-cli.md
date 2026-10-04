# undra-cli - decisions (SDE, branch wt/undra-cli)

[VERIFIED by running it, 2026-09-30: the three generated app shells were built and run - SwiftUI on
the iOS 26.5 simulator against the XCFramework, Compose on an Android emulator against the JNI
library, React in Chrome against the wasm core and against an `undra dev` core.]

## What the crate is

`undra` = init, bindgen, build, dev, doctor, adopt (`crates/undra-cli`, README has the table).
`unsafe` is confined to `schema.rs` (dlopen, SAFETY on every block; `#![deny(unsafe_code)]`
elsewhere). Deps: undra-meta, undra-bindgen, serde_json, clap (+`env`), notify, libloading; dev:
tungstenite, undra-wire. No `toml` crate: `undra.toml` uses the small reader in `toml_lite.rs`.

## Decisions the playground and the integrator should reuse

1. **The core is a plain rlib that depends on `undra` only.** The C ABI is added by a generated
   *shim* crate (`target/undra/<project>/shim`: `undra-ffi` + the core, lib named `undra_core`,
   SPEC 7 profiles; cdylib for host/android/web, staticlib for ios). Android needs `--features jni`.
   The playground core can drop its `cdylib/staticlib` crate types and ffi dependency.
2. **Undra comes from one place**, discovered with `cargo metadata` (path checkout, registry, git);
   shim and dev runner use the same source, so there is one `undra-runtime` in every build.
3. **Project layout** (`undra init`): `undra.toml`, `core/`, `generated/{swift,kotlin,ts}`, `ios/`,
   `android/`, `web/`, `build/` (not committed). Generated trees are tracked in
   `generated/.undra-generated` so regeneration removes only what it wrote.
4. **Artifact locations** the shells depend on: `build/ios/UndraCore.xcframework`,
   `build/android/jniLibs/<abi>/libundra_core.so`, `build/web/undra_core.wasm`.
5. **XCFramework has no header and no module map.** The runtime's `UndraFFI` target already defines
   module `UndraFFI`; a second definition fails ("redefinition of module"). The app target links with
   `-force_load <slice>/libundra_core.a` (per-SDK `OTHER_LDFLAGS`), excludes the simulator arch the
   XCFramework lacks (`EXCLUDED_ARCHS`), and Xcode must see `UNDRA_LINK_CORE=1` (see open items).
6. **Xcode project** is objectVersion 77 with a synchronized root group (no file lists), local
   package references to `generated/swift` and the `UndraRuntime` checkout.
7. **Android**: `generated/kotlin` is a plain JVM Gradle module (`org.jetbrains.kotlin.jvm`,
   JVM 11) included from `android/settings.gradle.kts`; the runtime is a composite build
   (`includeBuild`) of `runtimes/kotlin/undra-runtime` (substitutes `dev.undra:runtime`).
8. **Web**: Vite aliases `@undra/runtime` and `@app/<core>` to their TypeScript sources (no dist
   build); the wasm is imported with `?url`; `?undra=ws://…` switches to an `undra dev` core.
9. **`undra dev`** runs the core in a child process (`undra-dev-runner`, generated), which exits when
   its stdin closes. Native Clock/Rng/Log are bound only when the core links `undra-ports`. A failing
   rebuild keeps the old core serving; restarts reuse the address.
10. **Bindgen output is normalised** (methods/constructors/port methods sorted by name, variants by
    index) because the library's canonical JSON is sorted and the dev runner's `collect_schema` is
    in declaration order; both paths generate the same files.

## Open items (for the integrator)

* `undra_schema_json` returns the canonical JSON: no doc comments, no labels. Generated code from the
  dlopen path has no docs; `undra bindgen --docs` reads the full schema from the dev runner instead.
  Suggested: make `undra-ffi`'s `schema_json()` return the full JSON (the hash stays canonical); the
  CLI already accepts both.
* `runtimes/swift/UndraRuntime/Package.swift` defaults to the link-time stand-ins; `UNDRA_LINK_CORE=1`
  is read at manifest evaluation, which Xcode GUI does not get. Suggested: make the real core the
  default when the package is a dependency.
* The Kotlin runtime has no `REMOTE` mode on Android (no `java.net.http`), so `undra dev` serves web,
  iOS and the JVM only.
* The TS runtime bug fixed on this branch (`fix(ts-runtime)`, `Mirror` default scheduler) was only
  visible in a real browser.
* Swift is distributed from a monorepo subdirectory, which SwiftPM cannot consume by URL: registry
  mode writes `https://github.com/shreypdev/undra-swift` as a placeholder.

## CLI polish after the playground (finding 8 of `playground.md`)

1. **Host dylib identity.** `undra build --platform host` links the shim with
   `-Clink-arg=-Wl,-install_name,@rpath/libundra_core.dylib` (macOS only), passed after `--` so it applies to the
   shim and not to the dependencies. rustc's default is the absolute path of the file it wrote, which leaked
   the `target/` path into anything embedding the copy. `contract-tests/swift/run.sh` re-stamps the name
   itself (`install_name_tool -id`); that step is now redundant.
2. **Toolchain notes carry a platform** (`Concern::{Apple, Android}`); a build prints only its own.
3. **jniLibs.** The template path stays module-relative (`../../build/android/jniLibs` from `android/app`, what
   Gradle needs) and now says so in a comment. The real failure was silent: Gradle ignores a missing source
   directory and the APK ships no core. After an Android build `builds/gradle.rs` reads the app module's script
   (line-level, like `detect`): same directory, silent; another directory inside the project, the libraries are
   copied there too and the warning names the line to fix; no `srcDir` or one outside the project, a warning
   with the line to add; unresolvable (variables, `$rootDir`), left alone. `detect` no longer mistakes a root
   script that only declares the plugin (`apply false`) for the app module.
4. **Debug Android cores are 42 MB per ABI.** Debug stays the default (dev loop). A debug build ends with one
   `hint:` line: the size, and the release size and ratio when an earlier release build is in the target
   directory (else "20x or more smaller"). `--release` is the documented packaging path (init README, adopt
   guide, `undra build --help`, the playground READMEs).
5. **`generated/kotlin/.gitignore` is written by the CLI**, next to the `build.gradle.kts` it already writes
   (the module wrapper is the CLI's, the `.kt` sources are the generator's); `undra-bindgen` is untouched and its
   goldens do not change. Anchored patterns (`/build/`, `/.gradle/`, `/.kotlin/`) so a package directory named
   `build` is not hidden. Existing projects see `undra bindgen --check` fail once, until `undra bindgen` is run.
6. **Target directory** (`Session::target_dir`): `CARGO_TARGET_DIR`, else the target directory `cargo metadata`
   reports for the workspace the core is a member of (when that workspace is not the core crate alone), else
   `<project>/target`. The shim's `Cargo.lock` is seeded from the project's lock file or that workspace's, so the
   shim resolves the versions the workspace was tested with (no index update). The playground no longer builds
   a second 1.4 GB dependency tree in `examples/playground/target`.

## The loadable cdylib is non-incremental (ADR-029)

`undra build --platform host`/`--platform android` and `undra bindgen`'s dlopen path produce/consume a
cdylib that is *loaded* with no link-time reference to it. On a clean **macOS** dev build the app
core's `inventory` registrations (the whole schema) and undra-ffi's `#[no_mangle]` JNI exports
(`JNI_OnLoad`, `Java_dev_undra_runtime_UndraNative_*`) were dead-stripped: they live in dependency
rlibs, rustc links rlibs with `--start-lib` (lazy) semantics, and incremental compilation (the dev
default) partitions them into codegen units nothing references. `undra_schema_hash` then returned the
empty-schema hash `0x98754cbea76a32b2` (bindings, stamped from the dev runner at the real
`0x0f95cc4a…`, disagreed with it → contract S16 mismatch) and `System.loadLibrary` failed with
`UnsatisfiedLinkError` (contract Kotlin column). Swift hid it (its test binary links the dylib and
references undra symbols, retaining them); ELF (Linux/Android `.so`) keeps them regardless; the iOS
staticlib is the same class, already fixed with `-force_load` (builds/ios.rs); release hid it (it is
already non-incremental + LTO).

Fix (ADR-029): the shim's `[profile.dev]` is `incremental = false`, and `undra build` compiles the
**host** library in a target directory of its own (`crate::shim::host_lib_target_dir` =
`<target>/undra/<project>/host-lib`) with `CARGO_INCREMENTAL=0`. Non-incremental keeps the objects
(an incremental build partitions the dependency rlibs into codegen units the macOS `--start-lib`
linker drops); the private directory stops `undra build` reusing a stripping-prone incremental rlib
that a plain `cargo build`/`cargo test` left in the shared target (decision 6). Only the host library
needs the private dir - Android `.so` is ELF (symbols kept) and iOS is `-force_load`ed, so both keep
sharing the target. Chosen over the tries that were not robust across the clean / `cargo build` /
`cargo test --workspace` / inherited-`CARGO_INCREMENTAL=1` matrix: `incremental = false` alone (cargo
reused the polluting rlib), `codegen-units = 1` (a `cargo test --workspace`-polluted target still
stripped), fat LTO (`failed to get bitcode ...` in a shared target with non-bitcode rlibs),
`-force_load`/`-all_load` (defeated by `--start-lib`), a `#[used]` anchor (cannot name the anonymous
statics). Cost: the host library's deps compile once in its own dir (~5 s clean, <1 s cached), a
second copy of the Undra crates scoped to the host library. `undra dev` is unaffected (dev *runner*,
an executable, in the shared target). Regression: `crates/undra-cli/tests/schema_retention.rs` (gated
`#[ignore]`, in CI on Linux + macOS) builds the playground through the real `undra` binary and asserts
the loaded `libundra_core` has the full schema and exports `JNI_OnLoad` + the `UndraNative` natives;
the three-column contract suite is the end-to-end acceptance.
