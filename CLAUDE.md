# Keel — working agreement for everyone (human or agent) who touches this repo

Keel is a Rust framework that owns everything under the pixels of native iOS, Android and web apps: domain logic, reactive state, the data layer, persistence, and the dev loop. The UI stays SwiftUI, Compose and React.

Read, in this order, before writing code:
1. `docs/SPEC.md` — the binding implementation specification (wire format, ABI, schema, runtime model, generated code shapes). Code that disagrees with it is wrong.
2. `.10x/adrs/` — why the big decisions were made. Touching the wire, the runtime model, the threading model or a generated public shape requires a new ADR first.
3. `.10x/handoff.md` and `.10x/status.md` — where the work stands right now.

## The constitution (non-negotiable)

R1  The schema is the only truth. A public type or function exists only if `keel-meta` can describe it. Every language's output is derived from that description.
R2  `unsafe` lives in `crates/keel-ffi` only. Every block has a `// SAFETY:` comment. Every other crate has `#![forbid(unsafe_code)]`.
R3  Generated code must pass native review: a Swift / Kotlin / TypeScript engineer who has never seen Rust would write it that way. Golden files lock the output.
R4  Every feature lands whole: unit tests, contract scenario coverage, a benchmark if the boundary is touched, docs on every `pub` item.
R5  Reads never cross the boundary; writes cross once per transaction. No generated getter calls into the core.
R6  Nothing escapes as a panic or an abort. Every boundary entry is guarded; every error is a typed value.
R7  Compatibility is checked at load (schema hash) and at build. Breaking the wire is a major version and an ADR.
R8  Macro errors teach: code, what, why, fix, docs link (`docs/SPEC.md` §12).
R9  Budgets are tests. Benchmarks live in `bench/` and regressions fail CI.
R10 We ship on it first: `examples/playground` uses only public APIs.
R11 Boundary changes need an ADR before code.
R12 The core is deterministic: no wall-clock, randomness or threads outside the Clock / Rng / Timer ports and `keel-runtime`.

## Engineering standards

* Rust edition 2024, MSRV 1.85. `#[unsafe(no_mangle)]` spelling. `cargo clippy --all-targets -- -D warnings` must pass. `cargo fmt` before commit.
* No `anyhow` in library crates; typed error enums with `Display` (thiserror-style, hand-written is fine).
* No `println!`/`eprintln!` in library code; route through the Log port.
* Public items are documented with a one-line summary and, where it matters, an example.
* Tests live next to the code (`#[cfg(test)]`) for units and in `tests/` for integration. Property tests use `proptest`.
* Small commits, `type(scope): summary` messages (`feat`, `fix`, `test`, `docs`, `bench`, `state`, `chore`).
* Do not add a dependency without checking it builds on `wasm32-unknown-unknown`, iOS and Android (no tokio, no reqwest, no ring in core crates). Ask in the PR description why it is needed.
* TypeScript: strict mode, ESM, no `any` in exported types, no runtime dependencies in `@keel/runtime` core.
* Kotlin: stdlib + kotlinx-coroutines only in the runtime module; Android-specific code in `android-adapters`.
* Swift: Swift 6 language mode, strict concurrency, no Objective-C.

## Toolchain notes for this environment

* Rust 1.95 (edition 2024 default), Node 22, Java 21, Gradle 8.14 (its `lib/` contains `kotlin-compiler-embeddable-2.0.21.jar`, `kotlin-stdlib`, `kotlinx-coroutines-core-jvm-1.6.4.jar`; use `scripts/kotlinc.sh` to compile Kotlin for JVM tests).
* The `wasm32-unknown-unknown` target and Swift toolchain are **not** available here (rustup and swift.org downloads are blocked). wasm and Swift are verified in CI (`.github/workflows`) and on a developer Mac. Write them to compile there; keep the TS runtime testable against the native core over the `remote` transport (see SPEC §11) and against a WAT stub of the wasm ABI.
* `cargo-fuzz` (nightly) is unavailable; use proptest and the in-tree byte-fuzz harness.

## Team state protocol (10x-team)

State files under `.10x/` are the team's memory. After finishing a phase or a substantial task, update `.10x/status.md`, `.10x/handoff.md` and the relevant `decisions/<role>/keel-v1.md`, then commit with `state(<phase>): …`.
