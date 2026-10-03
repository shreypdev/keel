# SDE — android-size: a size-tuned mobile profile and native size gates (wt/android-size, 2026-10-02)

ADR-052's amendment "Amendment: native size gates (2026-10-02)" has the numbers; this is the piece's record. User feedback U4: a core at 1.6 MB per Android ABI
against a 1.2 MB budget. The piece is a profile, a gate and a record; no crate's code changed.

## What landed

* **`[profile.release-mobile]`** in the generated shim (`crates/undra-cli/templates/shim/Cargo.toml.tmpl`; the copy first put in the root `Cargo.toml` was removed before the merge: nothing built with it): `inherits = "release"`,
  `opt-level = "s"`, `panic = "unwind"` kept (R6, ADR-046: native panics are contained by `catch_unwind`), and **`undra-wire`, `undra-signals`, `undra-runtime`
  and `undra-ffi` at `opt-level = 3`** (`[profile.release-mobile.package.<name>]`): the call path keeps the speed profile's optimiser. `undra build --platform ios,android
  --release` builds it (`Profile::ReleaseMobile`, `Profile::mobile(release)` in `cargo.rs`; `builds/android.rs` passes `--profile release-mobile` through `cargo ndk`,
  `builds/ios.rs` through `cargo rustc`); the host build is still `release`. Line tables, the `llvm-strip` of the shipped copy, the unstripped twin, `--no-symbols` and the
  home-directory remap are unchanged. Output directory `target/<triple>/release-mobile/`; the Android debug-size hint reads it. Unit tests: the profile names and
  arguments (`cargo.rs`), the template (`shim.rs`: inherits `release`, `opt-level = "s"`, `panic = "unwind"`, never `abort`, the four crates at 3, `release` still 3).
* **`scripts/native-size.sh`**: builds the `undra init` template with the CLI and measures `libhello_core.so` per ABI (stripped, 16 KB alignment checked from the ELF
  program headers, no builder paths in it) and the iOS device slice (linked with `-force_load -dead_strip`, `strip -x`, the sections of `__TEXT`, `__DATA_CONST`, `__DATA`;
  the `.a` and the slice object's sections are in the JSON, ungated, because the `.a` grows at `z` while the code shrinks). `--record` writes `bench/results/native-size.jsonl`
  and the tables together. Exit 2 when it cannot measure.
* **Three `[size]` rows** in `bench/budgets.toml` (`android/hello-arm64-v8a`, `android/hello-x86_64`, `ios/hello-arm64`): `budget_bytes` (the design's 1.2 MB / 900 KB),
  `measured_bytes` (the record), `tolerance = 0.05`: the gate (the record's `ceiling`) is min(budget, record + 5%), the web rows' shape. `bench/src/budget.rs` reads raw-byte
  tables (`SizeUnit`) next to the gzip ones and its record test covers `web-size.jsonl` and `native-size.jsonl`. `android-size.jsonl` is replaced by `native-size.jsonl`; the
  site's `android-size` slot and two new slots (`android-x86-size`, `ios-size`), the landing card, README and the docs read it.
* **CI** (`bench.yml`): the `size` job installs cargo-ndk and NDK r27 and runs the Android half after the web one; `size-ios` is a `macos-15` job. `scripts/ci-local.rb`
  skips the cargo-ndk provisioning step of `bench/size` as it does `ci/android`'s. The job's display name keeps saying "Web size" (a branch rule may name it).
* **Docs**: ADR-052's amendment, SPEC 13 and 14, `docs/ONBOARDING.md` (a suite row and a section), README, `site/docs/production.html` (hand-written HTML, not generated: a new
  "Size of the shipped core" section), `site/docs/cli.html`, the CLI's `--release` help, the getting-started page, `docs/SITE.md`.

## Measured (hello world, bytes; the ADR has the playground, x86_64, the iOS archive and every linker lever)

| | arm64-v8a | x86_64 | iOS, linked |
|---|---|---|---|
| `main` b909739, `opt-level = 3` | 987,720 | 1,054,336 | 859,845 |
| `s` everywhere | 849,760 (−14.0%) | 898,024 (−14.8%) | 744,149 (−13.5%) |
| **the profile: `s`, call path at 3** | **905,520 (−8.3%)** | **971,464 (−7.9%)** | **793,517 (−7.7%)** |
| `z` everywhere | 774,480 (−21.6%) | 841,240 (−20.2%) | 608,574 (−29.2%) |
| the playground, the profile | 2,498,856 (−12.7%) | 2,599,592 (−14.1%) | 2,317,056 (−12.1%) |

## Why not `s` everywhere, and not `z`

Speed was checked in three places (ADR-052's amendment has the tables): the device bench on the emulator and the simulator (interleaved runs, best-of-N, noise stated), and the host
budgets test built under each profile (88 rows). `z` is out: the call path 1.34x to 1.50x slower. `s` everywhere is inside 10% on every device row but makes the core's own
operations 17% slower (median over 88 rows; one loop 3x), which the device rows hide; the call-path crates at 3 bring that to 3% for 6 points of size on a hello world (3.5 on the
playground). The first design was `s` everywhere; the host rows are what changed it.

## What did not change, and why

* `panic = "abort"` on native: R6. `-Wl,--gc-sections`: rustc already passes it (measured: no change). `--icf=safe`: no change. `--icf=all` (0.5%) and `--pack-dyn-relocs=android`
  (3.1%): measured, not taken (function identity; the loader of the app's oldest Android). The machine outliner: +12.7 KB on x86_64.
* No code removed: the 68 KB of `gimli`/`addr2line` in a hello world is the standard library's panic hook, present in a bare 260 KB `cdylib` that only calls `catch_unwind`; nothing
  debug-only is linked into a release core (the devtools hub is in the CLI).

## What could not be verified here

* The Ubuntu runner's numbers (the Android half of the gate was measured on macOS; same NDK and rustc; the 5% tolerance absorbs path-dependent constants), and a hosted run of
  either new job.
* A physical device: the speed rows are the emulator (arm64-v8a, hardware-virtualized) and the iPhone 17 Pro simulator, on a Mac shared with other agents (load average 3 to 35), so the
  device numbers are best-of-N ratios with the noise floor stated, not a claim about a phone. The emulator cannot separate `s` from the base at all; the host rows and the quiet simulator can.
* That `-Wl,--pack-dyn-relocs=android` loads on the oldest Android an app supports (not applied for that reason).

## Open

* The `[android] opt_level` / `[ios] opt_level` knob landed in the review (`.10x/reviews/2026-10-02-android-size-review.md`, ADR-052's "The knob"): `"s"` (the default),
  `"z"` (`release-mobile-z`, `z` for every crate) or `"3"` (`release`). Still open: `z` on the cold crates only with the call path at 3 (its bytes are in the ADR, its speed is not).
* Relocation packing behind `min_sdk` (−3 to −4%).
* The runtime's own size (`undra-runtime::runtime` 65 KB, `undra-meta`'s `Schema::canonical_json` 12 KB), each a piece of its own.
* `scripts/bench-device.sh --device android` fails on macOS when exactly one emulator is running (`wc -l` pads its count, so `[ "$(... | wc -l)" = 1 ]` is false): pass
  `--target emulator-5554`. Not fixed here (another piece's file).
