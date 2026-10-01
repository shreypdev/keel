# undra-cli

The `undra` command: create an Undra app, generate its bindings, build its core for iOS, Android and
the web, and serve the core to a running app while you edit it (`docs/SPEC.md` section 13).

```sh
cargo install undra-cli              # or, in a checkout: cargo run -p undra-cli --
undra init todo --platforms ios,android,web
cd todo
undra doctor                         # what this machine has, what is missing, how to fix it
undra dev                            # ws://127.0.0.1:7443: the core, served; rebuilt when core/ changes
undra bindgen                        # after changing a public type: Swift, Kotlin and TypeScript again
undra build --release                # build/ios/UndraCore.xcframework, build/android/jniLibs, build/web/undra_core.wasm
```

| Command | What it does |
|---|---|
| `undra init <name>` | A project: `undra.toml`, a core crate with a working store, generated bindings, and an iOS (Xcode project, SwiftUI), Android (Gradle, Compose) and web (Vite, React) app that use them. Nothing is downloaded; the templates are in the binary. `--undra-path` uses a checkout of the Undra repository instead of released versions. |
| `undra bindgen` | Builds the core as a host library with `undra-ffi` linked in, `dlopen`s it, reads `undra_schema_json` and `undra_schema_hash`, and writes `generated/{swift,kotlin,ts}`. `--schema file.json` skips the build; `--check` fails when the files are stale (CI); `--docs` keeps the core's doc comments. |
| `undra build` | The core for `host`, `ios`, `android`, `web`, with sizes against the blueprint's budgets. |
| `undra dev` | Builds a small runner around your core, serves it with `undra-transport`, watches `core/` and swaps in the rebuilt core on the same address. A failing build keeps the old core serving. |
| `undra doctor` | Rust and its targets, Xcode, the Android SDK/NDK and `cargo-ndk`, Node, `wasm-opt`, a JDK: one line each, with the fix. Exit status 1 if something the project needs is missing. |
| `undra adopt [path]` | Adds a core to an existing app: one new `undra/` directory and `UNDRA_ADOPT.md` with the exact, path-filled edits for each platform it finds. The app's own project files are never modified. |

Every failure prints what happened, why and what to do, with a stable code
(`error[undra::C0001]` ...; the list is `undra_cli::error::Code`).

## How it is put together

A project is a directory with an `undra.toml`. Its core is an ordinary library crate that depends on
`undra`. What ships to a platform is built from two crates the CLI generates under `target/undra/`:

* the **shim** links the core and the C ABI (`undra-ffi`) into a library named `undra_core` (the name
  the Kotlin runtime loads), with the release profiles of SPEC 7: `cdylib` for the host, Android
  and web, `staticlib` for iOS;
* the **dev runner** links the core and `undra-transport` into an executable. It is a separate process
  so a core that panics or is being rebuilt cannot take `undra dev` down, and it exits when its
  stdin closes, so it never outlives the CLI.

Both depend on Undra from wherever the core gets it (`cargo metadata` says), so there is exactly one
copy of `undra-runtime` in the build.

### iOS

`UndraCore.xcframework` has one static library per slice and **no header**: the Swift runtime's
`UndraFFI` target declares the C ABI as a module, and a second definition fails the build. The app
links with `-force_load` (a debug static library has many object files and the linker drops the ones
that register the core's items) and builds the runtime package with `UNDRA_LINK_CORE=1` so its
link-time stand-ins do not shadow the real core. The generated Xcode project does the first;
`undra init`'s README says how to do the second.

## Tests

`cargo test -p undra-cli` runs the unit tests and the integration tests that drive the real binary:
`init` then `cargo test` on the core, `bindgen --check`, `bindgen --schema` against goldens,
`build --platform web` loaded under Node, and `undra dev` spoken to over a raw WebSocket (including an
edit that rebuilds and restarts the core). The heavy platform builds are switched on by environment
variables (see `tests/platforms.rs`): `UNDRA_TEST_IOS=1`, `UNDRA_TEST_IOS_APP=1`, `UNDRA_TEST_ANDROID=1`,
`UNDRA_TEST_ANDROID_APP=1`, `UNDRA_TEST_WEB_APP=1`.
