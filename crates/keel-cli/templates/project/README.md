# @@NAME@@

A [Keel](https://github.com/shreypdev/keel) app: the logic lives once, in a Rust core, and @@PLATFORM_LIST@@
use it through generated native bindings. The UI stays SwiftUI, Compose and React.

@@KEEL_SOURCE_NOTE@@

```
keel.toml      the project file (platforms, names, paths)
core/          the Rust core: `#[keel::api]` records, enums, errors and a `#[keel::store]`
generated/     Swift, Kotlin and TypeScript bindings of the core (`keel bindgen`)
ios/ android/ web/   one small app per platform, using the generated bindings
build/         what `keel build` produces for the apps to link (not committed)
```

## The loop

```sh
keel doctor                    # what this machine has, and what is missing
keel dev                       # serve the core over a WebSocket; it rebuilds when core/ changes
keel bindgen                   # after you change a public type or method: regenerate the bindings
keel build --release           # the libraries the apps link, with their sizes
```

`core/src/lib.rs` is the whole app logic. Everything marked `#[keel::api]` crosses into the three
languages; `keel bindgen` reads the core's schema (it builds the core, loads it and asks) and writes
`generated/`. Commit `generated/`, or check it in CI with `keel bindgen --check`.
@@README_IOS@@@@README_ANDROID@@@@README_WEB@@
## Tests

`cargo test` runs the core's tests: no device, no simulator. The core is deterministic (no clock, no
randomness, no threads of its own), so tests are plain Rust.

## More

`keel --help`, `keel <command> --help` and https://keel.dev/errors (every `error[keel::C00NN]` has a page).
