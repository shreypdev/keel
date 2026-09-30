# keel-cli

The `keel` command: create a Keel app, generate its bindings, build its core for iOS, Android and
the web, and serve the core to a running app while you edit it (`docs/SPEC.md` section 13).

```sh
cargo install keel-cli              # or, in a checkout: cargo run -p keel-cli --
keel init todo --platforms ios,android,web
cd todo
keel doctor                         # what this machine has, what is missing, how to fix it
keel dev                            # ws://127.0.0.1:7443: the core, served; rebuilt when core/ changes
keel bindgen                        # after changing a public type: Swift, Kotlin and TypeScript again
keel build --release                # build/ios/KeelCore.xcframework, build/android/jniLibs, build/web/keel_core.wasm
```

| Command | What it does |
|---|---|
| `keel init <name>` | A project: `keel.toml`, a core crate with a working store, generated bindings, and an iOS (Xcode project, SwiftUI), Android (Gradle, Compose) and web (Vite, React) app that use them. Nothing is downloaded; the templates are in the binary. `--keel-path` uses a checkout of the Keel repository instead of released versions. |
| `keel bindgen` | Builds the core as a host library with `keel-ffi` linked in, `dlopen`s it, reads `keel_schema_json` and `keel_schema_hash`, and writes `generated/{swift,kotlin,ts}`. `--schema file.json` skips the build; `--check` fails when the files are stale (CI); `--docs` keeps the core's doc comments. |
| `keel build` | The core for `host`, `ios`, `android`, `web`, with sizes against the blueprint's budgets. |
| `keel dev` | Builds a small runner around your core, serves it with `keel-transport`, watches `core/` and swaps in the rebuilt core on the same address. A failing build keeps the old core serving. |
| `keel doctor` | Rust and its targets, Xcode, the Android SDK/NDK and `cargo-ndk`, Node, `wasm-opt`, a JDK: one line each, with the fix. Exit status 1 if something the project needs is missing. |
| `keel adopt [path]` | Adds a core to an existing app: one new `keel/` directory and `KEEL_ADOPT.md` with the exact, path-filled edits for each platform it finds. The app's own project files are never modified. |

Every failure prints what happened, why and what to do, with a stable code
(`error[keel::C0001]` ...; the list is `keel_cli::error::Code`).

## How it is put together

A project is a directory with a `keel.toml`. Its core is an ordinary library crate that depends on
`keel`. What ships to a platform is built from two crates the CLI generates under `target/keel/`:

* the **shim** links the core and the C ABI (`keel-ffi`) into a library named `keel_core` (the name
  the Kotlin runtime loads), with the release profiles of SPEC 7: `cdylib` for the host, Android
  and web, `staticlib` for iOS;
* the **dev runner** links the core and `keel-transport` into an executable. It is a separate process
  so a core that panics or is being rebuilt cannot take `keel dev` down, and it exits when its
  stdin closes, so it never outlives the CLI.

Both depend on Keel from wherever the core gets it (`cargo metadata` says), so there is exactly one
copy of `keel-runtime` in the build.

### iOS

`KeelCore.xcframework` has one static library per slice and **no header**: the Swift runtime's
`KeelFFI` target declares the C ABI as a module, and a second definition fails the build. The app
links with `-force_load` (a debug static library has many object files and the linker drops the ones
that register the core's items) and builds the runtime package with `KEEL_LINK_CORE=1` so its
link-time stand-ins do not shadow the real core. The generated Xcode project does the first;
`keel init`'s README says how to do the second.

## Tests

`cargo test -p keel-cli` runs the unit tests and the integration tests that drive the real binary:
`init` then `cargo test` on the core, `bindgen --check`, `bindgen --schema` against goldens,
`build --platform web` loaded under Node, and `keel dev` spoken to over a raw WebSocket (including an
edit that rebuilds and restarts the core). The heavy platform builds are switched on by environment
variables (see `tests/platforms.rs`): `KEEL_TEST_IOS=1`, `KEEL_TEST_IOS_APP=1`, `KEEL_TEST_ANDROID=1`,
`KEEL_TEST_ANDROID_APP=1`, `KEEL_TEST_WEB_APP=1`.
