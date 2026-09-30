# Keel v1 — status

Updated: 2026-09-30 (integrator takeover, branch `claude/keel-framework-takeover-66c4ea`)

## Phase

Implementation (Phase 4 of the 10x flow). Strategy/design live in `docs/blueprint.html`
and `docs/SPEC.md`; the plan of record is `docs/HANDOFF.md`.

## Test truth (all green as of the update above)

| Suite | Result |
|---|---|
| Rust `cargo test --workspace` | 1,079 passed / 0 failed |
| TS `npm test` (runtimes/ts/@keel/runtime) | 830 passed (20 files) |
| Kotlin `scripts/test-local.sh` | 454 cases, 0 failed, 2 skipped (JNI smoke awaits keel-ffi) |
| Swift `swift test` (needs full Xcode; env.sh sets DEVELOPER_DIR) | 313 passed / 0 failed |

## Done

- Merged the four outstanding branches into the takeover line: `wt/integrate`,
  `wt/ts-core`, `wt/kotlin-core`, `wt/swift-core`.
- First native Swift compile: one `@unchecked Sendable` restatement fixed; suite green.
- Kotlin stream-cancellation test de-raced (collector now parks so cancel lands mid-stream).
- Toolchain installed on this Mac: rustup 1.98.1 + ios/android/wasm targets, JDK 17,
  Kotlin 2.4.20, Gradle, binaryen; kotlinx-coroutines jar in `../.tools/lib`.
- `scripts/env.sh` finds the native macOS toolchain (rustup, brew, Xcode DEVELOPER_DIR).
- `.10x/` state recreated (the original was never committed); `docs/HANDOFF.md` rewritten
  as the plan of record.

## In progress

- keel-ports (SPEC §8) — worktree `wt/keel-ports`.

## Remaining (ordered, see docs/HANDOFF.md §2)

keel-ports → keel-query → keel-ffi → keel-transport → keel-cli → playground (3 apps) →
contract scenarios on 3 platforms → bench/RESULTS.md + CI → adversarial reviews closed.

## Environment notes

- `sudo` is unavailable to the agent: xcode-select stays on CommandLineTools; use
  `DEVELOPER_DIR` (env.sh does). Android SDK/NDK/AVD not yet installed (needed at the
  playground step). cargo-ndk not yet installed.
