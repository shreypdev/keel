# @@NAME@@

An [Undra](https://github.com/shreypdev/undra) app: the logic lives once, in a Rust core, and @@PLATFORM_LIST@@
use it through generated native bindings. The UI stays SwiftUI, Compose and React.

@@UNDRA_SOURCE_NOTE@@

```
undra.toml      the project file (platforms, names, paths)
core/          the Rust core: `#[undra::api]` records, enums, errors and a `#[undra::store]`
generated/     Swift, Kotlin and TypeScript bindings of the core (`undra bindgen`)
ios/ android/ web/   one small app per platform, using the generated bindings
build/         what `undra build` produces for the apps to link (not committed)
.github/       the CI workflow (undra.yml): the core, and a job per app
```

## The loop

```sh
undra doctor                    # what this machine has, and what is missing
undra dev                       # serve the core over a WebSocket; it rebuilds when core/ changes
undra bindgen                   # after you change a public type or method: regenerate the bindings
undra build --release           # the libraries the apps link, with their sizes (the app builds below run it for you)
undra upgrade                   # move the project to this `undra`'s version, with the migration notes
```

`undra build` is not a manual step: Gradle (`undraBuild`), Xcode (the "Build the Undra core" phase) and Vite (the
`undra()` plugin) each run it before the app builds, and skip it while the core is unchanged. They find `undra`
on `PATH` (`undra doctor` checks that).

`core/src/lib.rs` is the whole app logic. Everything marked `#[undra::api]` crosses into the three
languages; `undra bindgen` reads the core's schema (it builds the core, loads it and asks) and writes
`generated/`. Commit `generated/`, or check it in CI with `undra bindgen --check`.
@@README_IOS@@@@README_ANDROID@@@@README_WEB@@
## Tests

`cargo test` runs the core's tests: no device, no simulator. The core is deterministic (no clock, no
randomness, no threads of its own), so tests are plain Rust.

## More

`undra --help`, `undra <command> --help` and https://shreypdev.github.io/undra/docs/errors.html (every `error[undra::C00NN]` has a page).
