# Undra — working agreement for everyone (human or agent) who touches this repo

Undra is a Rust framework that owns everything under the pixels of native iOS, Android and web apps: domain logic, reactive state, the data layer, persistence, and the dev loop. The UI stays SwiftUI, Compose and React.

Read, in this order, before writing code:
1. `docs/SPEC.md` — the binding implementation specification (wire format, ABI, schema, runtime model, generated code shapes). Code that disagrees with it is wrong.
2. `.10x/adrs/` — why the big decisions were made. Touching the wire, the runtime model, the threading model or a generated public shape requires a new ADR first.
3. `.10x/handoff.md` and `.10x/status.md` — where the work stands right now.

## The constitution (non-negotiable)

R1  The schema is the only truth. A public type or function exists only if `undra-meta` can describe it. Every language's output is derived from that description.
R2  `unsafe` lives in `crates/undra-ffi` only — plus the one schema loader in `undra-cli` that `dlopen`s the built core (SPEC §13). Every block has a `// SAFETY:` comment. Every other crate has `#![forbid(unsafe_code)]`; `undra-cli` holds `deny(unsafe_code)` everywhere outside that loader.
R3  Generated code must pass native review: a Swift / Kotlin / TypeScript engineer who has never seen Rust would write it that way. Golden files lock the output.
R4  Every feature lands whole: unit tests, contract scenario coverage, a benchmark if the boundary is touched, docs on every `pub` item.
R5  Reads never cross the boundary; writes cross once per transaction. No generated getter calls into the core.
R6  Nothing escapes as a panic or an abort. Every boundary entry is guarded; every error is a typed value.
R7  Compatibility is checked at load (schema hash) and at build. Breaking the wire is a major version and an ADR.
R8  Macro errors teach: code, what, why, fix, docs link (`docs/SPEC.md` §12).
R9  Budgets are tests. Benchmarks live in `bench/` and regressions fail CI.
R10 We ship on it first: `examples/playground` uses only public APIs.
R11 Boundary changes need an ADR before code.
R12 The core is deterministic: no wall-clock, randomness or threads outside the Clock / Rng / Timer ports and `undra-runtime`.

## Engineering standards

* Rust edition 2024, MSRV 1.85. `#[unsafe(no_mangle)]` spelling. `cargo clippy --all-targets -- -D warnings` must pass. `cargo fmt` before commit.
* No `anyhow` in library crates; typed error enums with `Display` (thiserror-style, hand-written is fine).
* No `println!`/`eprintln!` in library code; route through the Log port.
* Public items are documented with a one-line summary and, where it matters, an example.
* Tests live next to the code (`#[cfg(test)]`) for units and in `tests/` for integration. Property tests use `proptest`.
* Small commits, `type(scope): summary` messages (`feat`, `fix`, `test`, `docs`, `bench`, `state`, `chore`).
* Do not add a dependency without checking it builds on `wasm32-unknown-unknown`, iOS and Android (no tokio, no reqwest, no ring in core crates). Ask in the PR description why it is needed.
* TypeScript: strict mode, ESM, no `any` in exported types, no runtime dependencies in `@undra/runtime` core.
* Kotlin: stdlib + kotlinx-coroutines only in the runtime module; Android-specific code in `android-adapters`.
* Swift: Swift 6 language mode, strict concurrency, no Objective-C.

## Toolchain and local development

Native macOS is the reference environment. `source scripts/env.sh` puts everything on
PATH (rustup, brew JDK 17 + Kotlin, the kotlinx-coroutines jar, a `DEVELOPER_DIR`
fallback when xcode-select still points at CommandLineTools). Machine setup, every
suite's run command, and the known gotchas are in `docs/ONBOARDING.md`; `undra doctor`
diagnoses a machine. Rust stable (1.99+; CI pins 1.99.0) with the wasm32/iOS/Android targets installed —
no build-std, no nightly, except Miri/ASan jobs in CI. Full Xcode is required for Swift
tests and simulators; Android work needs the SDK + NDK r27 + the `undra` AVD.

## How changes land

One piece, one worktree, one adversarial review, one merge, then clean up —
`docs/AGENT_WORKFLOW.md` is the binding process (scripts/wt.sh new/merge/rm/clean).
No piece lands on `main` unless CI is green on its branch's exact head: review → merge `main` into the branch →
`git push origin wt/<slug>` → CI green on that head → fast-forward → clean up (`scripts/wt.sh merge` refuses
otherwise; `--no-ci` is for state-only commits). An agent's piece is not done until that run is green.
State files under `.10x/` are the team's memory: after a piece merges, the integrator
updates `.10x/status.md` and `.10x/handoff.md` and commits `state(<piece>): …`;
worktree authors record their piece in `.10x/decisions/<role>/<slug>.md` and never touch
the shared state files. Reviews live in `.10x/reviews/`, decisions in `.10x/adrs/`.
