# Handoff — integrator → keel-ports implementer

The tree is green everywhere (see status.md). Next piece: **keel-ports**, SPEC §8, in
worktree `wt/keel-ports`.

Hard constraints the implementer must not miss:

1. The three platform runtimes already implement the §8 records and ids in their
   adapters and tests (`runtimes/ts/@keel/runtime/src/adapters`, `runtimes/kotlin/...
   /adapters`, `runtimes/swift/.../Adapters` + each one's StandardPort/Adapter tests, and
   `contract-tests/wire-vectors.json`). The Rust crate must match **them** — field order,
   variant order and indices, method names, port ids — bit for bit.
2. Port ids and method ids are FNV-1a as SPEC §1.1 (`port.<TraitName>` /
   `<TraitName>.<method>`). The runtimes hard-code these; a mismatch fails their suites.
3. `#[keel::port]` (keel-macros) is done — use it, with `#[keel(crate = ...)]` as the
   in-workspace path override (SPEC §16.3). Records use `#[keel::api]`, errors
   `#[keel::error]`.
4. Fakes per SPEC §8 + `TestRuntime` integration (keel-runtime::testing exists).
5. `#![forbid(unsafe_code)]`, clippy -D warnings, docs on every pub item, tests beside
   code + integration tests, wasm32 build must succeed.

---

# Handoff: playground to integrator

The playground, the contract tests and three running apps are done on `wt/playground` (see
`decisions/sde/playground.md` and the last section of `status.md`). To merge: `cargo test --workspace`,
`keel bindgen -C examples/playground --docs --check`, then `contract-tests/run-all.sh` (needs `kotlinc`,
`KEEL_KOTLIN_STDLIB`, `KEEL_KOTLINX_COROUTINES`, Xcode) and, for the apps, `examples/playground/web`
`npm run smoke`, `examples/playground/ios/smoke.sh`, and the commands in `examples/playground/android/README.md`.
Next for the bench piece: use the `Bench` store (`examples/playground/README.md`, "Benchmark hooks");
finding 1 (keyed patch is O(list)) will fail the 10,000-item row. CI does not yet build `playground-core` for
wasm32 or run the contract tests: add `playground-core` to the wasm32 build loop and `contract-tests/run-all.sh ts kotlin` (macOS runner: `swift`).
