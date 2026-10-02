# Two cores in one app

The test apps of ADR-044 (per-core symbol namespacing): the playground core (`../playground/core`) built
twice, under two namespaces, and loaded into one process on every platform. Nothing is shared between the
two: each is its own image, reached through its own table (`playground_a_undra_api`,
`playground_b_undra_api`), with its own runtime, registries and threads.

```
examples/two-cores/
  a/undra.toml   [core] namespace = "playground_a"; bindings PlaygroundA, dev.undra.twocores.a, @two-cores/a
  b/undra.toml   [core] namespace = "playground_b"; bindings PlaygroundB, dev.undra.twocores.b, @two-cores/b
  a/generated/, b/generated/   the bindings `undra bindgen` writes (committed); entries UndraPlaygroundA/B
  ios/       TwoCores.app: PlaygroundACore.xcframework + PlaygroundBCore.xcframework in one binary
  android/   one APK with lib/<abi>/libplayground_a.so and libplayground_b.so
  jvm/       one JVM with both host libraries
  node/      one Node process with playground_a.wasm and playground_b.wasm
```

Each app loads both cores through their generated entries, makes a call on each (`add(2, 3)`, whose
default core is its package's own), creates a `Counter` in each and changes it (each core's change-set
reaches only its own mirror), compares their statistics, closes A and checks that B still answers and
observes while a call on A fails as unavailable. Both use the platform's default adapters, whose `Kv` is per core namespace
(ADR-044 amendment A): each core writes the same key and reads its own value back, a key only A wrote is not B's, and the
two stores are in two places (`undra/playground_a/kv` and `undra/playground_b/kv` in Application Support, `filesDir` or the
JVM's data directory, IndexedDB `undra.playground_a.kv` and `undra.playground_b.kv` on Node). It prints one `two-cores <platform>: ok ...` line per
check, then `passed`; its `run.sh` builds what it needs with the `undra` CLI and exits non-zero otherwise.

```sh
examples/two-cores/ios/run.sh                       # the booted iPhone simulator
CONFIGURATION=Release examples/two-cores/ios/run.sh # fat-LTO cores, prelinked with -u
examples/two-cores/android/run.sh                   # the attached emulator or device
examples/two-cores/jvm/run.sh
examples/two-cores/node/run.sh
```

`.github/workflows/two-cores.yml` runs all four; contract scenario S26 (`contract-tests/scenarios.md`) is
the same check in the Swift, Kotlin and TypeScript columns.

Two details a real app with two cores meets too:

* **Two local Swift packages need two directory names.** SwiftPM names a local package after its
  directory, and both generated packages live in a directory called `swift`; the iOS app refers to
  them through `ios/Packages/PlaygroundA` and `PlaygroundB` (symbolic links). A vendor's core usually
  arrives as a remote package, which has an identity of its own.
* **Default storage is per namespace.** Two cores that both use the defaults keep their `Kv`, `Fs`, `SecureStore` and `Db` data
  under `undra/<namespace>/` on every platform, so they never read each other's keys; share a store only by passing both
  cores one adapter of your own.
* **Handles are per core.** Two cores with the same history issue the same handle numbers; every
  generated object carries the core that made it, and app code passes values, not handles, between
  cores.
