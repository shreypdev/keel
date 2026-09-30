# Brief for implementation agents

You are one engineer on the Keel team. You own exactly the piece named in your task and nothing else.

## Before you start
1. Read `CLAUDE.md` (constitution and standards) and the sections of `docs/SPEC.md` your task names. The spec is binding; if you believe it is wrong, implement it as written and list the concern in your report.
2. Look at `contract-tests/wire-vectors.json` if your task touches the wire.
3. Your working directory is a git worktree of the Keel repository on its own branch. Work only inside the paths your task names. Do not edit other crates or packages. Edit the root `Cargo.toml` only to add a missing `[workspace.dependencies]` entry, and say so in your report.

## While you work
* Small, coherent commits with `type(scope): summary` messages (`feat`, `fix`, `test`, `docs`, `chore`).
* Rust: edition 2024, `#![forbid(unsafe_code)]` (except `keel-ffi`), `cargo clippy -p <crate> --all-targets -- -D warnings` clean, `cargo fmt`, every `pub` item documented, unit tests next to code, integration tests in `tests/`. Use `-p <crate>` to keep builds fast. No new dependencies beyond the workspace list unless the task allows it.
* TypeScript: strict, ESM, zero runtime dependencies in the runtime package, vitest for tests.
* Kotlin: compile with `scripts/kotlinc.sh` (works with or without a real `kotlinc`); stdlib + kotlinx-coroutines only.
* Swift: Swift 6 language mode, strict concurrency; there is no Swift compiler in this environment, so write conservatively, avoid clever generics, and desk-check every file twice.
* Never leave `TODO`, `unimplemented!()` or `todo!()` in committed code. If something is out of scope, leave a documented, tested, explicit limitation instead.
* Quality bar: this is a framework other engineers will trust blindly. Names are precise, errors are typed and descriptive, tests cover edge cases (empty, max, malformed, unicode), and performance-sensitive paths avoid needless allocation.

## When you finish
Run the full check for your piece (build, clippy/typecheck, tests) and make sure the tree is committed. Then report, in this order:
1. Branch name (`git rev-parse --abbrev-ref HEAD`) and worktree path.
2. What you built (files, public API summary).
3. Test summary (counts, what they cover).
4. Deviations from the spec or the task, and why.
5. Open questions or concerns for the reviewer.
Keep the report under 60 lines.
