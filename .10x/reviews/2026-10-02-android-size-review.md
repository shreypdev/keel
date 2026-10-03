# android-size (ADR-052 amendment "native size gates") — adversarial review

**Date:** 2026-10-02 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:** `wt/android-size` at
`184f010` (draft PR #8; contains `main` `267b62c`) · **Read:** `CLAUDE.md` (R6, R9, R12), `docs/AGENT_WORKFLOW.md` 4, ADR-046 (panic reports
unwind on native), ADR-052 and its amendment, `.10x/decisions/sde/android-size.md`, the `[size]` rows of `bench/budgets.toml`, `scripts/native-size.sh`,
the shim template, `cargo.rs`, `builds/android.rs`, `builds/ios.rs`, `tests/symbols.rs`, `undra-runtime/src/guard.rs`, `bench.yml`.
**Method:** the gate re-run from the branch (byte-identical: 905,520 / 971,464 / 793,517); a fresh-target-directory Android build with
`CARGO_TERM_VERBOSE=true` read for every crate's `-C opt-level`; each `opt_level` value built through the CLI for the hello world and the
playground (Android and iOS); the playground's device bench re-run for `"3"`, `"s"` and `"z"` interleaved on `emulator-5554` and the iPhone 17
Pro simulator; `capture_backtrace` stubbed and the library measured; the iOS slice linked into a minimal executable; the PR's first CI run read
(Ubuntu Android bytes, the macOS `size-ios` job). **Fixes:** `8c9f758`, `27ffbe0`, `f0c4924`, `8e0be51`, `303fed8` and the merge `347a527`.

## Verdict

**Merge.** The profile does what the piece says, through the CLI, for an app: the shim is a workspace of its own and its template carries the
profile, cargo hands rustc `-C opt-level=3` for exactly `undra-wire`, `undra-signals`, `undra-runtime` and `undra-ffi` and `-C opt-level=s` for every
other crate (the app's core, `serde_json`, the shim), and `panic = "unwind"` is kept (the symbols suite's release panic reports resolve on the
emulator and the simulator). The gates measure what ships: the Android file Gradle packages, and an iOS number within 1% of what a minimal
executable gains. The speed claim reproduces on both targets. What was missing was the choice: no setting reached the user's 1.2 MB, and an
app whose own hot code lives in its core (optimised for size by this profile) had no project-level way back. Both are now one setting.

## Findings

| # | Sev | Finding | Status |
|---|---|---|---|
| M1 | Medium | **No knob, so U4 was left open.** The amendment deferred `z` to a follow-up though the change is small; the only route to `z` (or back to `3`) was `CARGO_PROFILE_RELEASE_MOBILE_*` on every machine, which Xcode and Gradle builds do not carry. | **Fixed** (`8c9f758`): `opt_level` in `[ios]` and `[android]` of `undra.toml`, `"s"` (default, `release-mobile`), `"z"` (`release-mobile-z`: `z` for every crate, unwind kept), `"3"` (`release`); `C0002` for anything else; `undra init` writes it commented out; the debug-size hint reads the chosen profile's directory. Tests fail on the old code (the key was "unknown"). Measured through the knob, hello world arm64-v8a 965,968 / 898,176 / 766,528, playground 2,842,448 / 2,493,376 / 2,018,792 (merged tree). |
| M2 | Medium | **The hot-crate list was argued for Undra's code only.** The app's own crate and its dependencies get `s` (and so do the call-path crates' generics instantiated for the app's types, and the fat-LTO pass itself, which runs at the shim's level); nothing measured or offered a way back for an app whose hot loop is its own. `serde_json` in a core is only `Schema::canonical_json` at load (`undra-meta`, the cold row; the wire is Undra's own codec, `Decimal` is `undra-wire`'s), so the list is right for Undra's paths; the playground's own `serde_json` use parses HTTP bodies. | **Fixed** by `opt_level = "3"` and the ADR's precise statement of what the override keeps (`27ffbe0`). The device rows and host rows already include the app-side code at `s`. |
| L1 | Low | ADR: "the per-package override is honoured under fat LTO because the size attribute is per function" was half of it (the LTO pipeline runs at `s`; generics compile in the instantiating crate). | Fixed (`27ffbe0`), with the verbose-build evidence. |
| L2 | Low | ADR: Ubuntu and macOS "agree to a few dozen bytes". The first hosted run measured +376 / −296 bytes (Android) and +16 (iOS, Xcode 16.4 vs 26.6). | Fixed (`27ffbe0`, and the CI numbers in the amendment). |
| L3 | Low | The blog post's size row linked `bench/results/android-size.jsonl`, which this piece deletes (404 once on `main`; `check-links` does not follow GitHub URLs). | Fixed (`8e0be51`). |
| L4 | Low | The record was measured on `main` b909739; `main`'s cold-restore piece (in the stack this lands on) takes 7 to 11 KB off every core. | Re-recorded on the merged tree (`f0c4924`): 898,176 / 960,776 / 784,086, gates 943,084 / 1,008,814 / 823,290. |
| L5 | Low | `symbols.rs`: the iOS comment cited "−2,324 in another" run as alignment noise; in that run the library *without* symbols was 2,340 bytes larger than in the other, i.e. a different tree, not line-table noise. The bound itself (a thousandth, 2.3 KB, against a measured +16 bytes) is a claim about the code. | Fixed (`test(symbols)` commit): the comment states the two measured runs, +16 and the review's −8 (2,340,336 / 2,340,328). |
| L6 | Low | `size-ios` waited 36 minutes for a macOS runner and ran 2.5; it is now on the path of `Bench / All green`. | Recorded in the amendment; acceptable (CI has macOS jobs already). |
| L7 | Low | The amendment's reason for not packing relocations ("the CLI cannot promise the loader") is weaker than stated: the CLI knows `min_sdk` (default 26), and the packed format is read from API 23 (THEORY: not load-tested here). | Recorded as the follow-up, with the load test it needs. |
| L8 | Low | `scripts/bench-device.sh --device android` with one emulator (`wc -l` padding). | Open, not this piece's file (as the record says). |

No High finding: the profile applies through the CLI, R6/ADR-046 hold (no crate code changed; `panic = "unwind"` in both mobile profiles, pinned by
`shim.rs`), and both gates measure the shipped artefact.

## The questions of the brief

**1. Does the profile apply through the shim?** Yes, verified from the bytes (the same template through the CLI gives 905,520 at the piece's tree) and
from cargo: `-C opt-level=3` for `undra_ffi`, `undra_runtime`, `undra_signals`, `undra_wire`; `s` for the 30 others, `hello_core` and `serde_json` among them; the
shim crate (`cdylib`, `-C lto`) at `s`. The root `Cargo.toml` table is not what an app builds (no script uses it either); harmless.

**2. 1.2 MB.** Decided now: the knob is in. A 1.6 MB core (U4, speed profile) is about **1.42 MB** at the default and **about 1.18 MB at `"z"`** (1.14 to 1.21 for
what the CLI prints as 1.6 MB), splitting it into the hello world's part and the core's own part and scaling each as measured. The middle ground, `z` with the
call path at 3, measured for bytes (hello 883,472, playground 2,197,976 at the piece's tree) but not for speed: a follow-up, not a value.

**3. The symboliser.** `std`'s, not ours: with `capture_backtrace` returning `""` the hello world's arm64 library is 6,616 bytes smaller and all 127
`gimli`/`addr2line` symbols remain (the default panic hook links them). Only `-Z build-std` removes them. Our 6.6 KB is the `PanicReport.backtrace` text ADR-046
defines; not removed.

**4. Gate honesty.** Android: the stripped, 16 KB aligned `.so` Gradle packages. iOS: a minimal executable that takes the address of `hello_core_undra_api` gains
786,457 bytes of sections and 6,904 of `__LINKEDIT`; the row said 793,517 (+0.9%, conservative). The method is in the script header, the row comment and the
ADR. The Ubuntu `size` job needs the NDK (r27 via `sdkmanager`) and cargo-ndk: 29 s + 25 s + 58 s for the gate. Both jobs are in `all-green.needs`.

**5. `symbols.rs`.** What was relaxed: `.rela.dyn` and `.data.rel.ro` moved from "exactly equal" into the code pool compared within a thousandth (measured: 4
relocations and 48 bytes of relro data move with which identical functions are folded at `s`), and the iOS linked app may be a thousandth larger. The unstripped
twin still fails `not_the_plain_library` (it has `.debug_*` and `.symtab`: the layout check, as before; the suite's own assertion passed). The review's run
reproduces the stated differences exactly on arm64-v8a: `.rela.dyn` −96 bytes (4 relocations), `.data.rel.ro` −48, `.text` +216, `.eh_frame` +264; on x86_64 only
the code moves (`.text` −256); every other section is equal. A thousandth of the pool is about 2.3 KB, the measured net movement 352 bytes: the bound is about the
code, with an order of magnitude of room, and a symbol table or debug section left in would be caught by the layout check whatever its size.

**6. Speed.** Re-run interleaved (variants installed in turn, rotating order; Android: a discarded warm-up run and 10 s of rest after each install). Medians,
`"s"`/`"3"` and `"z"`/`"3"`:

| row | emulator, 8 runs: s | z | simulator, 5 runs: s | z | noise (emu / sim) |
|---|---|---|---|---|---|
| sync call | 0.99x | 1.58x | 0.99x | 1.33x | 1.02x / 1.01x |
| 1 KB record | 1.03x | 1.17x | 1.00x | 1.18x | 1.00x / 1.00x |
| keyed insert 10k | 1.00x | 0.97x | 0.99x | 1.04x | 1.03x / 1.01x |
| change-set 100 | 1.01x | 1.15x | 0.99x | 1.13x | 1.02x / 1.03x |
| drain frame | 1.02x | 0.95x | 0.95x | 0.77x | 1.00x / 1.19x |
| cold load | 1.08x | 1.10x | 0.94x | 0.92x | 1.18x / 1.09x |

The implementer's 0.83x was the emulator's cold load (0.85x the drain frame): rows whose own noise was 1.07x and 1.18x, so noise, not a win; my medians put
both at 1.02x to 1.08x. The shared emulator ran 3 to 4x slower in absolute terms than the committed 2026-10-01 runs (a call 1,068 ns, not 266) but steadily (noise
1.00x to 1.03x on the call rows); a first batch without the rest after install had 2x to 3x noise and is discarded. The host `[bench]` budgets pass
(`cargo test -p undra-bench --release`). **Device budgets: out of scope**, decided: no CI job runs the device bench, the web shape (five times the measured)
would not see a 1.6x, and enforcing it needs code in the Swift and Kotlin bench harnesses that parallel pieces are editing.

## What was run

`scripts/native-size.sh` (before and after the merge; `--record` after); `cargo fmt`; `cargo clippy -p undra-cli --all-targets -D warnings`; `cargo test -p
undra-cli --lib` (422); `UNDRA_REQUIRE_TOOLCHAINS=1 cargo test -p undra-cli --no-fail-fast -- --test-threads=1` (every target green: `symbols` 5 of 5 with the Android frames on `emulator-5554` and the iOS frames through the app's dSYM, `debugging`, the build, init and
upgrade suites); `cargo test -p undra-bench
--release` (budgets 6 passed); `node site/scripts/build-all.mjs`, `check-links.mjs` and `--words` (clean, 347 of 350 words).

## What I could not verify

* A physical phone: the speed rows are the emulator and the simulator on a shared Mac.
* A hosted run of the new knob's profile (CI measures the default; `"z"` and `"3"` were built here for both platforms, Android ABIs and the iOS slices).
* That API 23's loader reads `--pack-dyn-relocs=android` output (L7).

## After the review (integrator, 2026-10-03)

The founder asked that every changed line be needed before this merges. Read against that:

* **Removed:** the `[profile.release-mobile]` tables in the root `Cargo.toml` (19 lines). The review called them harmless; nothing builds with them (an app builds the shim's profile), so they were a second copy to keep in step for no reader.
* **L8 fixed:** `scripts/bench-device.sh` compared `wc -l` to `1` as text, and macOS pads the count with spaces, so one emulator read as "several"; the count is trimmed now.
* **Kept, with the reason each exists:** `scripts/native-size.sh` (325 lines, the size of `scripts/wasm-size.sh`: it builds the template through the CLI, checks 16 KB alignment and that no builder path is in the library, and links the iOS slice the way an app does, because the `.a` is not what ships); `bench/src/budget.rs` (+206: a size table counts gzipped bytes or the bytes of the shipped file, and a table that mixes the two is an error); the CLI's `opt_level` setting and the two profiles of the shim (the review's M1); the `size-ios` job. The rest is the ADR amendment's tables, the records and regenerated site files.
* What GitHub showed as fifteen thousand lines was this piece plus the three it was stacked on (the SSE adapter, the OkHttp module, Bazel), all on `main` already; its own change is 35 files.

