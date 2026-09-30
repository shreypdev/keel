# Brief for implementation agents (local workflow)

You are one engineer on the Keel team. You own exactly the piece named in your task and nothing else.

## Where the code lives and how you touch it

The repository lives on Shrey's Mac and is reached ONLY through the `mcp__remote-devices__device_bash` tool. Your `Read`, `Write`, `Edit`, `Glob`, `Grep` and `Bash` tools operate on a different machine and are useless for this repo: do not use them for project files.

* Main repo: `$HOME/mnt/src/keel` (main branch; do not commit there yourself).
* Your worktree: the path your task names, `$HOME/mnt/src/.work/<slug>`, on branch `wt/<slug>`. Work only there.
* Every `device_bash` call is a fresh shell with a hard limit of about 180 s, after which every process it started is killed (no background jobs survive). Therefore:
  * start every call with `cd $HOME/mnt/src/.work/<slug> && source scripts/env.sh` (Rust nightly with wasm32 via `-Z build-std`, kotlinc, node, java are on PATH after that);
  * run cargo through the slice runner: `scripts/lb.sh cargo test -p <crate>`; when it prints `SLICE-TIMEOUT`, run the exact same command again (compilation progress is cached) until it prints `DONE`; build one crate at a time (`-p`) to keep slices short;
  * keep any single command under ~150 s; split long test suites with filters if needed.
* Read files with `cat -n`, `sed -n 'a,bp'`, `grep -n`. Write files with heredocs (`cat > path <<'EOF' … EOF`). Edit files with small Python scripts (read → `str.replace` with an assert that the anchor occurs exactly once → write) or `sed -i` for one-liners. Never retype a whole file from memory to change three lines. Always `cat -n` the region after an edit to verify it.
* Never run `rm -rf` outside your worktree's `target/` and `node_modules/`.
* Commit on your branch with `git add -A && git commit -m "type(scope): summary"` (small, coherent commits). Do not merge, rebase onto, or push to `main`; the integrator merges.

## Before you start
1. Read `CLAUDE.md` and the sections of `docs/SPEC.md` your task names. The spec is binding; if you believe it is wrong, implement it as written and list the concern in your report.
2. Read the existing code you depend on (its README and public API) before writing against it.

## Quality bar
* Rust: edition 2024, `#![forbid(unsafe_code)]` (except `keel-ffi`), `cargo clippy -p <crate> --all-targets -- -D warnings` clean, `cargo fmt`, every `pub` item documented, tests next to code plus `tests/` integration tests, proptest where inputs are open-ended. No new dependencies beyond the workspace list unless the task allows it. Must also compile for `wasm32-unknown-unknown` when the crate is part of the core (`scripts/lb.sh cargo build -p <crate> --target wasm32-unknown-unknown -Z build-std=std,panic_abort`).
* TypeScript: strict, ESM, zero runtime dependencies in `@keel/runtime`, vitest.
* Kotlin: stdlib + kotlinx-coroutines only; compile with `kotlinc` (on PATH via env.sh); tests run through the package's `scripts/test-local.sh`.
* Swift: Swift 6 language mode, strict concurrency; no compiler here, so desk-check twice and list uncertain constructs in your report.
* Never leave `TODO`, `unimplemented!()`, `todo!()` or stubs in committed code.
* This is a framework other engineers will trust blindly: precise names, typed descriptive errors, edge cases (empty, max, malformed, unicode), no needless allocation on hot paths.

## When you finish
Run the full check for your piece and make sure the tree is committed. Report, in this order, in under 60 lines:
1. Branch and worktree path.
2. What you built (files, public API summary).
3. Test summary (counts, what they cover, which checks ran green).
4. Deviations from the spec or the task, and why.
5. Open questions or concerns for the reviewer.
