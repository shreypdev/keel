# ios15-sample

The compatibility sample of Undra's iOS 15 / 16 mode ([ADR-045](../../.10x/adrs/ADR-045-ios-15-16-compatibility-mode.md),
[docs/IOS_15_16.md](../../docs/IOS_15_16.md)): an iOS app whose deployment target is **iOS 15.0**, over one Rust core with a
store (`Todos`, with a keyed list), a query (`tips`) and a function. Because the target is below 17, `undra bindgen` generates
the stores and the query handle as `ObservableObject`s with `@Published` properties instead of `@Observable` classes, and the
views observe them with `@ObservedObject`. CI builds it for the iOS 15.0 simulator (`scripts/ios-floor.sh`); it is not part of
the playground, which stays on iOS 17 and dogfoods Observation.

The core uses the Undra crates, and the app the Undra runtimes, from this checkout (`[undra] path = "../.."` in undra.toml).

```
undra.toml      the project file (platforms, names, paths)
core/          the Rust core: `#[undra::api]` records, enums, errors, a `#[undra::store]` and a `#[undra::query]`
generated/     Swift, Kotlin and TypeScript bindings of the core (`undra bindgen`)
ios/ android/ web/   one small app per platform, using the generated bindings
build/         what `undra build` produces for the apps to link (not committed)
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

## iOS

```sh
xcodebuild -project ios/Ios15Sample.xcodeproj -scheme Ios15Sample \
    -destination 'generic/platform=iOS Simulator' build
```

Open `ios/Ios15Sample.xcodeproj` in Xcode 16 or newer to run it. There is no `undra build` to run first: the **Build the
Undra core** Run Script phase, before Compile Sources, runs `undra build --platform ios --configuration $CONFIGURATION`
(a Debug build makes a debug core, a Release build a release one, `build/ios/Ios15SampleCore.xcframework`). Xcode skips it
while the files in `ios/Config/undra-core-inputs.xcfilelist` are older than the ones in
`ios/Config/undra-core-outputs.xcfilelist`; `undra build` keeps the input list in step with the core's sources, so
commit it. The phase finds `undra` on `PATH` or in `~/.undra/bin`, `~/.cargo/bin` and Homebrew's directories (Xcode
started from the Dock has a short `PATH`) and says how to install it when it is missing; it turns user script
sandboxing off for the target, since it runs Cargo. The app links the core's library, `libios15_sample_core.a`, by its path
in Other Linker Flags, not as a framework: Xcode reads an XCFramework while it plans the build, before the phase could
have made it. Each slice is one prelinked object whose only global symbol is the core's entry, which the bindings call
(`UndraIos15SampleCore.load()`), so it needs no `-force_load` and another Undra core can sit next to it in the app.

**Against `undra dev`.** In the scheme's Run environment variables set `UNDRA_DEV_URL` to the `ws://` URL
`undra dev` prints (a simulator can use `ws://127.0.0.1:7443`; a device needs your computer's address and
`undra dev --addr 0.0.0.0:7443`). Debug builds then use that core instead of the linked one: edit the Rust,
save, and the app reconnects and comes back to the screen it was on, with its state (`undra dev` carries the
core's state across a rebuild; the bar at the top says `Reloaded, state kept`, and shows the connection;
`core.connectionState` and `core.connection` are the API). See docs/DEV_LOOP.md in the Undra repository.

## Tests

`cargo test` runs the core's tests: no device, no simulator. The core is deterministic (no clock, no
randomness, no threads of its own), so tests are plain Rust.

## More

`undra --help`, `undra <command> --help` and https://shreypdev.github.io/undra/docs/errors.html (every `error[undra::C00NN]` has a page).
