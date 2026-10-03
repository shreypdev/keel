# Undra under Bazel

A Bazel workspace around the `undra init` to-do core (ADR-061). `bazel test //...` builds the core, generates its bindings and runs
the Kotlin, TypeScript and (on macOS) Swift code that uses them against the real core.

```
examples/bazel/
  MODULE.bazel      undra_rules (bazel/, by path), rules_rust 1.99.0 pinned by checksum, rules_kotlin, rules_swift, aspect_rules_ts/js
  undra.toml        the project file `undra build` and `undra bindgen` read; [core] namespace = "hello_core"
  Cargo.toml, core/ the core: a store, a typed error and a function
  BUILD.bazel       undra_core, undra_bindings, undra_ts_library
  kotlin/           the JVM test (JNI against libhello_core) and the ktlint test of the generated Kotlin
  ts/               the Node test (hello_core.wasm through the compiled bindings)
  swift/            the Swift test (macOS): the core in process
```

```sh
cd examples/bazel
bazel test //...                      # needs Bazelisk; .bazelversion pins Bazel
bazel build //:core_web               # bazel-bin/core_web/hello_core.wasm: 118 KB gzipped of the 120 KB budget
bazel build //:bindings               # the Swift, Kotlin and TypeScript trees, as outputs
bazel build //:mobile_ios             # macOS: the XCFramework (manual target)
```

| Target | What it proves |
|---|---|
| `//kotlin:hello_test` | the Kotlin bindings, the Kotlin runtime and `core_host` work together: a call, a store's signals, a typed error, a command, through JNI |
| `//ts:hello_test` | the same through the TypeScript bindings (type-checked by `tsc` against the compiled runtime) and `core_web` under Node |
| `//swift:hello_test` | the same in Swift, with the XCFramework's host twin linked (macOS only; skipped on Linux) |
| `//kotlin:lint_test` | ktlint 1.8 reports nothing on the generated Kotlin, and 90 findings once the exclusions `undra bindgen` writes are taken away |

## What is hermetic, and what is not

The actions run the `undra` CLI of this checkout with the Rust toolchain Bazel resolved, crates.io packages downloaded by the
checksum `Cargo.lock` records and unpacked into a Cargo directory source, and no network. The scratch directory of an action is
named by a digest of its inputs, so a core built twice is the same file. The C linker (and Xcode for iOS, the NDK and `cargo-ndk`
for Android) are the machine's.

Android is declared (`//:mobile_android`, `--config=android`) and not built here: the core's Rust toolchain for an Android platform
needs a C++ toolchain for it (`rules_android_ndk`), which fails to configure on a machine without an NDK, so it cannot be in a
module every machine builds. The guide (`docs/bazel.html`) says how to add it.

## Notes

* The Undra crates are not on crates.io yet, so the core says `undra = "0.1"` and the rules (and `.cargo/config.toml`, for a plain
  `cargo test` here) stand the checkout in for the registry with `[patch.crates-io]`.
* `.bazelrc` sets `DO_NOT_TRACK=1`: `aspect_rules_js` and `aspect_rules_ts` depend on a telemetry module that reports the rulesets a
  build uses to Aspect.
* `MODULE.bazel.lock` is committed and CI runs with `--lockfile_mode=error`.
