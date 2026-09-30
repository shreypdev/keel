# SDE: keel-runtime review fixes (branch `wt/runtime-fixes`)

Closes the confirmed findings of `.10x/reviews/2026-09-30-keel-runtime-review.md`
(H1, M1, M2, M3, L1 to L9; notes N5 fixed in passing). Decisions: ADR-022 (generations and the
snapshot floor), ADR-023 (delivery, re-entry, restore, shutdown, write checker).

What changed, by finding (test names are the regression tests):

| Finding | Change | Tests |
|---|---|---|
| H1, L8 | one generation counter (process-wide; `TestRuntime` owns a private one), `Snapshot.generation_floor` after `count`, restore resumes at max(current, floor, snapshot gens), `u32::MAX` refused, exhaustion panics FATAL | `tests/generations.rs` (`h1_*`, `l8_*`), abi `restore_never_reissues_a_generation_the_host_may_hold`, platform codec vectors |
| M1, L9 | `StoreCell::observe_and_deliver` (keel-signals) used by `Runtime::observe` and restore phase 3, inside `txn`, one change-set per store | `tests/delivery.rs` (`m1_*`, `l9_*`), `keel-signals/tests/observe_and_deliver.rs` (`rt_m1_*`, `rt_l9_*`) |
| M2 | per-runtime `HostCall` marker around every `Host` callback; `enter_core`/`poll`/`run_pending` refuse | `tests/reentry.rs` (`m2_*`) |
| M3 | restore cancels calls/streams whose receiver it replaced (status 3 / flag-2 item) | `tests/restore_calls.rs` (`m3_*`) |
| L1, L5 | shutdown answers and cancels in-flight work first, clears subscribers/bindings, executor refuses late spawns, late `spawn`/`sleep`/`port_call`/`event` are logged no-ops, debug assertion against shutdown from the core or a callback | `tests/lifecycle.rs` (`l1_*`, `l5_*`) |
| L3, L4 | write checker is an allowlist (core-lock holder, test driver thread, `testing::unchecked_writes`); `TestRuntime` uses the real blocking pool and settles it | `tests/write_context.rs` (`l3_*`, `l4_*`) |
| L6 | `cancel_task` drops on the core (lock held, or queued + nudge) | `tests/lifecycle.rs` (`l6_*`) |
| L7 | abandoned port ids capped at 4096 (FIFO, WARN) | `tests/ports.rs` `l7_*`, unit tests in `ports.rs` |
| L2 | documented in SPEC section 6 | none (docs) |

Things a maintainer must know:

- The snapshot layout changed (`count u32, generation_floor u32, stores`): `keel-wire`, the
  Swift, Kotlin and TypeScript codecs, `keel_snapshot`'s pre-init answer (8 zero bytes) and all
  vectors moved together. Platforms never parse snapshots in production paths (they pass bytes
  through), only in their codec tests.
- Generations are no longer small per-slot numbers. Tests that need exact handles use
  `TestRuntime`, whose counter is private.
- Test harnesses that call dispatchers directly on the test thread and write signals must call
  `keel_runtime::testing::drive_from_this_thread()` (keel-macros does); off-core writers in tests
  use `testing::unchecked_writes`; embedders write by `ctx.spawn`-ing onto the runtime
  (keel-transport tests do, via `on_core`).
- Not done on purpose: `Runtime::extension` values are not cleared at shutdown (`&T` contract);
  N1-N4, N6-N10 of the review are open; InitHooks still run before the embedder can bind ports (L2
  is documentation only).
