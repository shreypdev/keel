# ADR-029: the host library is built non-incrementally, in its own target directory, so schema registrations and JNI exports survive

Status: accepted (2026-09-30). Touches the generated shim's dev build profile
(`crates/keel-cli/templates/shim/Cargo.toml`) and where the host library is built
(`crates/keel-cli/src/builds/host.rs`, `crate::shim::host_lib_target_dir`). No wire, ABI, schema or
generated-shape change. Constitution R11: how the cdylib is built/linked is a boundary/build
contract, so it is decided here before the code.

## Context

`keel build` links the app core (a plain rlib that depends on `keel`) and `keel-ffi` into one
`libkeel_core.{dylib,so}` cdylib (the generated *shim*; SPEC 13). The platform runtimes then
**load** that library with no link-time reference to it:

* `keel bindgen` `dlopen`s it and calls `keel_schema_json` to read the schema (SPEC 13);
* the Kotlin runtime `System.loadLibrary`s it and binds the JNI natives of `KeelNative` (SPEC 6.1);
* the Swift runtime links it, but as a dependency of a test/app binary that *references* keel
  symbols, which is why Swift never saw this.

Two kinds of symbol in that cdylib are reachable only through the loader, never from the shim's own
root module: the app core's `#[keel::api]`/store/port/error/query `inventory::submit!` statics - the
whole schema (SPEC 2.4), in the **app-core dependency rlib** - and keel-ffi's `#[no_mangle]` JNI
exports (`JNI_OnLoad`, `Java_dev_keel_runtime_KeelNative_*`, SPEC 6.1), in the **keel-ffi dependency
rlib**. rustc links dependency rlibs with `--start-lib`/`--end-lib` (lazy archive) semantics, so on
**macOS** the linker keeps only the objects something references. An **incremental** build partitions
those rlibs into many tiny codegen units; the macOS linker drops the unreferenced ones. `keel_schema_hash`
then reports the empty schema `0x98754cbea76a32b2` (bindings, stamped with the real hash from the dev
runner, disagree with it - contract S16) and `System.loadLibrary` fails with `UnsatisfiedLinkError`
(contract Kotlin column). `#[used]` keeps a symbol within its object but does not force the object
into the link, and `-force_load`/`-all_load` do not override `--start-lib`. ELF (Linux/Android `.so`)
keeps the symbols; the release profile and the iOS **staticlib** (`-force_load`, `builds/ios.rs`)
never showed it because both are non-incremental.

Two things had to be true together, and each was subtle:

1. **The build must be non-incremental.** Incremental is the dev-profile default. It is also not
   part of a crate's fingerprint, so it cannot be turned off just by depending on the shim.
2. **The host build must not reuse a bad rlib.** `keel build` shares the project's target directory
   (SDE decision 6, so the core and Keel crates compile once). A plain `cargo build`/`cargo test`
   there leaves an *incremental* core/keel-ffi rlib, and `keel build` reused it and stripped,
   regardless of the shim's own profile, because cargo reused the cached artifact.

## Decision

* The shim's `[profile.dev]` sets `incremental = false`.
* `keel build` builds the **host** library in a target directory of its own
  (`crate::shim::host_lib_target_dir` = `<target>/keel/<project>/host-lib`, still under the resolved
  target so `CARGO_TARGET_DIR` is honoured), and sets `CARGO_INCREMENTAL=0` for that invocation
  (belt against an inherited `CARGO_INCREMENTAL=1`, which would otherwise override the profile).

Together these make the host library deterministic: the app core and keel-ffi are always compiled
fresh, non-incrementally, unaffected by whatever a plain `cargo build`/`cargo test` left in the
project's target. Only the **host** library needs the private directory: Android's `.so` is ELF (the
linker keeps the symbols) and iOS's staticlib is `-force_load`ed by the app, so both keep sharing the
project's target (decision 6 stands for them).

Chosen over the alternatives, each of which was tried and rejected against the full matrix (clean,
`cargo build`-polluted, `cargo test --workspace`-polluted, and inherited `CARGO_INCREMENTAL=1`):

* **`incremental = false` alone** fixes a clean build but not a polluted shared target: cargo reused
  the incremental core rlib a `cargo test` had left.
* **`codegen-units = 1`** isolated by fingerprint and passed a `cargo build`-polluted target, but a
  `cargo test --workspace`-polluted one still stripped, and it was not robust to an inherited
  `CARGO_INCREMENTAL=1`.
* **Fat LTO** (what release uses) is robust in a clean build but breaks in a shared target with
  `failed to get bitcode from object file for LTO` when a plain `cargo build` has left non-bitcode
  rlibs, exactly the state `keel build` runs in.
* **`-force_load`/`-all_load` link args** do not work (rustc's `--start-lib` grouping) and are
  per-linker; a **`#[used]` anchor** cannot name the core's anonymous `inventory` statics.

Cost: the host library's dependency graph compiles once in its own directory (about 5 s here from
clean, under 1 s incrementally once cached; opt-level stays at the dev default). It is a second copy
of the Keel crates next to the project's, scoped to the host library only, the price for a library
that is correct no matter what else shares the target. `keel dev` is unaffected: it serves the core
from the dev **runner**, an executable (in the shared target), whose `inventory` constructors always
run, which is how `keel bindgen --docs` already reads the full schema.

## Regression

`crates/keel-cli/tests/schema_retention.rs` (gated `#[ignore]`, run in CI on Linux and macOS) builds
the playground core through the real `keel` binary and asserts, on the *loaded* `libkeel_core`, that
the schema is not the empty one and that `JNI_OnLoad` and `Java_dev_keel_runtime_KeelNative_*` are
exported. It fails on macOS if the host library stops being built cleanly. The three-column contract
suite (`contract-tests/run-all.sh`) is the end-to-end acceptance: the Kotlin column
`System.loadLibrary`s this cdylib and checks the schema hash (S16).
