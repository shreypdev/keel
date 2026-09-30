# keel-cli — decisions (SDE, branch wt/keel-cli)

[VERIFIED by running it, 2026-09-30: the three generated app shells were built and run — SwiftUI on
the iOS 26.5 simulator against the XCFramework, Compose on an Android emulator against the JNI
library, React in Chrome against the wasm core and against a `keel dev` core.]

## What the crate is

`keel` = init, bindgen, build, dev, doctor, adopt (`crates/keel-cli`, README has the table).
`unsafe` is confined to `schema.rs` (dlopen, SAFETY on every block; `#![deny(unsafe_code)]`
elsewhere). Deps: keel-meta, keel-bindgen, serde_json, clap (+`env`), notify, libloading; dev:
tungstenite, keel-wire. No `toml` crate: `keel.toml` uses the small reader in `toml_lite.rs`.

## Decisions the playground and the integrator should reuse

1. **The core is a plain rlib that depends on `keel` only.** The C ABI is added by a generated
   *shim* crate (`target/keel/<project>/shim`: `keel-ffi` + the core, lib named `keel_core`,
   SPEC 7 profiles; cdylib for host/android/web, staticlib for ios). Android needs `--features jni`.
   The playground core can drop its `cdylib/staticlib` crate types and ffi dependency.
2. **Keel comes from one place**, discovered with `cargo metadata` (path checkout, registry, git);
   shim and dev runner use the same source, so there is one `keel-runtime` in every build.
3. **Project layout** (`keel init`): `keel.toml`, `core/`, `generated/{swift,kotlin,ts}`, `ios/`,
   `android/`, `web/`, `build/` (not committed). Generated trees are tracked in
   `generated/.keel-generated` so regeneration removes only what it wrote.
4. **Artifact locations** the shells depend on: `build/ios/KeelCore.xcframework`,
   `build/android/jniLibs/<abi>/libkeel_core.so`, `build/web/keel_core.wasm`.
5. **XCFramework has no header and no module map.** The runtime's `KeelFFI` target already defines
   module `KeelFFI`; a second definition fails ("redefinition of module"). The app target links with
   `-force_load <slice>/libkeel_core.a` (per-SDK `OTHER_LDFLAGS`), excludes the simulator arch the
   XCFramework lacks (`EXCLUDED_ARCHS`), and Xcode must see `KEEL_LINK_CORE=1` (see open items).
6. **Xcode project** is objectVersion 77 with a synchronized root group (no file lists), local
   package references to `generated/swift` and the `KeelRuntime` checkout.
7. **Android**: `generated/kotlin` is a plain JVM Gradle module (`org.jetbrains.kotlin.jvm`,
   JVM 11) included from `android/settings.gradle.kts`; the runtime is a composite build
   (`includeBuild`) of `runtimes/kotlin/keel-runtime` (substitutes `dev.keel:runtime`).
8. **Web**: Vite aliases `@keel/runtime` and `@app/<core>` to their TypeScript sources (no dist
   build); the wasm is imported with `?url`; `?keel=ws://…` switches to a `keel dev` core.
9. **`keel dev`** runs the core in a child process (`keel-dev-runner`, generated), which exits when
   its stdin closes. Native Clock/Rng/Log are bound only when the core links `keel-ports`. A failing
   rebuild keeps the old core serving; restarts reuse the address.
10. **Bindgen output is normalised** (methods/constructors/port methods sorted by name, variants by
    index) because the library's canonical JSON is sorted and the dev runner's `collect_schema` is
    in declaration order; both paths generate the same files.

## Open items (for the integrator)

* `keel_schema_json` returns the canonical JSON: no doc comments, no labels. Generated code from the
  dlopen path has no docs; `keel bindgen --docs` reads the full schema from the dev runner instead.
  Suggested: make `keel-ffi`'s `schema_json()` return the full JSON (the hash stays canonical); the
  CLI already accepts both.
* `runtimes/swift/KeelRuntime/Package.swift` defaults to the link-time stand-ins; `KEEL_LINK_CORE=1`
  is read at manifest evaluation, which Xcode GUI does not get. Suggested: make the real core the
  default when the package is a dependency.
* The Kotlin runtime has no `REMOTE` mode on Android (no `java.net.http`), so `keel dev` serves web,
  iOS and the JVM only.
* The TS runtime bug fixed on this branch (`fix(ts-runtime)`, `Mirror` default scheduler) was only
  visible in a real browser.
* Swift is distributed from a monorepo subdirectory, which SwiftPM cannot consume by URL: registry
  mode writes `https://github.com/shreypdev/keel-swift` as a placeholder.
