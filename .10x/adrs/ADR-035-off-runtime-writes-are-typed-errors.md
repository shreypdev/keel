# ADR-035: a signal write off its runtime's core is refused in every build, and change-sets go to the store's owner

Status: **Accepted** (2026-10-01; implemented on `wt/runtime-lifecycle`, Track A, piece A2. Proposed the same day
from the v1.x gap audit `.10x/specs/2026-10-01-v1x-gaps.md`, gaps OW-1 and OW-2; the open half of runtime review
L3). Touches SPEC 5.1 (threading: the write rule),
16.1 (`set_write_checker`, `ChangeSink`, `StoreCell`) and 16.2 (`Ctx::with_core`), `undra-signals`
(`context.rs`, `signal.rs`, `store.rs`, `sink.rs`), `undra-runtime` (`runtime.rs`: the checker, the sink, a
runtime registry) and the macros' error catalogue (one new runtime code). **No wire change, no C ABI or wasm
ABI change, no schema change, no generated platform code change.** Constitution R6 and R11: a silent drop is
replaced by a loud failure and the contract between `undra-signals` and `undra-runtime` changes, so it is decided
here before code.

## Context

SPEC 5.1 says signal writes belong on the core. ADR-020 introduced a write checker, ADR-023 made it an
allowlist (a thread that holds a runtime's core lock, a `TestRuntime` driver thread, `testing::unchecked_writes`;
`crates/undra-runtime/src/runtime.rs:173-181`). It is evaluated **in debug builds only**
(`crates/undra-signals/src/context.rs:56-74`, `signal.rs:250-256`); release builds never call it, "so the
lock-level guarantees are what protects them".

The runtime review's re-review left L3 at PARTLY: "Release: the write is accepted, 0 change-sets are delivered,
and the core holds count=2 that the host never sees." The gap audit reproduced it in a release build with a
threaded `Runtime::new`:

* a write from a plain thread (no runtime scope): applied, **never delivered** (change-sets 1 → 1). The commit
  claimed the dirty slot, `RuntimeSink::deliver` found no current runtime and returned
  (`runtime.rs:138-143`), and nothing re-sends it: host and core disagree until the signal is written again or
  re-observed;
* a write from a blocking-pool thread (the runtime is current there): applied and **delivered without the core
  lock** (1 → 2), unordered against the core's transactions (the store's delivery lock keeps only that store's
  order, ADR-020);
* the debug build panics in both cases.

Two more facts make it worse than a debug/release asymmetry:

* **Routing is by thread, not by store.** `current_or_global()` (`runtime.rs:122-125`) picks the runtime the
  writing thread is inside, else the global one. A write to runtime A's store from inside a call on runtime B is
  delivered to B's host under A's handle (L3's second half). The checker has the same blind spot: "holds *a*
  core lock" passes for the wrong runtime.
* **Dev and production differ.** `undra dev` sessions use `Runtime::new` (no global; `crates/undra-transport/src/session.rs:592`),
  so an off-core write is dropped; the FFI uses `Runtime::init` (global; `crates/undra-ffi/src/session.rs:147`),
  so the same write is delivered. A bug that is invisible in production is invisible differently in development.

## Decision

1. **The rule holds in every build.** A write that has consequences (the signal is attached to a store or has
   dependents — the same condition as today) is allowed only on a thread that holds **the owning runtime's**
   core lock, or a `TestRuntime` driver thread, or inside `testing::unchecked_writes`. Anything else is refused
   *before* the value changes, in release as in debug.
2. **The owner is recorded.** `StoreCell` gains `owner: AtomicU64` (a runtime id, `0` until published), set by
   the runtime wherever it sets the handle (`insert_object`, `insert_store`, restore) through
   `StoreCell::set_owner`. The checker becomes `set_write_checker(f: fn(owner: u64) -> bool)`: for an attached
   signal the cell's owner is passed and the runtime answers "does this thread hold *that* runtime's core lock"
   (`HELD` contains it); for an unattached signal with dependents `owner` is `0` and any core lock suffices (as
   today: such a signal belongs to no store yet).
3. **A refused write is loud and typed.**
   * `Signal::set`, `update`, `replace` and the recorded list operations panic with the teaching message (code
     **E0065**, "a signal of a store owned by runtime N was written from a thread that does not hold its core
     lock", the why, and the fix: return the value to a task or call, use `ctx.spawn`, or `Ctx::with_core`). Inside
     a dispatched call or task the panic is contained as every panic is (status 2 to the caller, an ERROR log);
     on a user thread it unwinds that thread, which is the loudest correct outcome for a contract violation.
     Before panicking the refusal is logged at ERROR through the owning runtime, so a host sees it even when the
     panicking thread is not one Undra watches.
   * `Signal::try_set(value) -> Result<(), WriteError>` and `Signal::try_update(f) -> Result<(), WriteError>`
     return `WriteError::OffCore { owner }` instead (`Display`, `Error`), for code that can recover; and
     `Signal::can_write(&self) -> bool` answers the question without writing.
4. **Change-sets go to the store's owner.** `ChangeSink` gains `fn deliver_from(&self, owner: u64, change_set:
   &[u8]) { self.deliver(change_set) }` (defaulted, so other sinks are unaffected); the commit passes the cell's
   owner. `RuntimeSink` routes through a process registry (`RUNTIMES: RwLock<HashMap<u64, Weak<Runtime>>>`,
   registered at build, removed on drop) and never through "current or global". A cell with owner `0` is
   unpublished and delivers nothing (today's "writes before `set_handle` are plain writes"). The same lookup
   serves `round_cap_hit`.
5. **A sanctioned path for host threads.** `Ctx::with_core<R>(&self, f: impl FnOnce() -> R) -> Result<R,
   Reentrant>` takes the core lock on the calling thread (refused with `E_REENTRANT` from a host callback or the
   core itself, like every core-lock entry), makes the runtime current, runs `f` inside a transaction and
   releases. An embedder thread that must write synchronously uses it; everything else sends the value to the
   core (`ctx.spawn`, a call, a task awaiting `spawn_blocking`'s result).
6. **Cost.** The checker is read through a `OnceLock<fn(u64) -> bool>` (one atomic load) instead of today's
   `RwLock<Option<fn>>`, and `write_allowed` is three thread-local reads; the writer already takes the value's lock
   and announces the change, so the check is a small fraction of a write. It is measured, not assumed (brief, step
   7).
7. **Unchanged:** local signals (unattached, no dependents) are free; `TestRuntime` driver threads and
   `unchecked_writes` keep their exemptions; a write made by a computed or an effect runs on the committing
   thread, which holds the core lock whenever the commit started on the core.

## Alternatives considered

* **Keep release lenient, log instead of refusing.** The value would still diverge (the dropped case) or race
  (the pool case); a log line after the fact is not "never a silent drop" in any useful sense, and the core and
  host would still disagree.
* **Marshal the write onto the core** (an off-core `set` enqueues a task that performs it). No error at all, but
  the writer's own read-after-write breaks (`s.set(5); s.get()` returns the old value on that thread), and writes
  from two threads still interleave in an order neither chose. A surprise that looks like a feature; rejected.
  `Ctx::with_core` and `ctx.spawn` give the same capability explicitly.
* **Accept `Ctx::enter()` scopes as allowed writers.** That is what ADR-020 had; ADR-023 removed it because a
  scope without the core lock is unordered against the core. Not reopened.
* **Return `Result` from `set` itself.** Every signal write in every core would need `?` or `let _ =` for an error
  that only a misuse produces; the panicking default plus `try_*` keeps the common path clean (the
  `Vec::push`/`try_reserve` precedent).

## Consequences

* An off-core write fails at the write, with a code and a fix, in every build; `undra dev` and production behave
  the same.
* Multi-runtime embedders (a transport server with sessions, tests with several runtimes) get correct routing.
* Code that wrote signals from host threads in release builds and happened to work (global runtime, single
  writer) now fails loudly; the fix is `Ctx::with_core` or a hop to the core. The in-repo callers are found by
  running every suite in release (`cargo test --release --workspace` and the contract grid).
* `set_write_checker`'s signature and `ChangeSink` grow (SPEC 16.1); both are internal contracts between
  `undra-signals` and `undra-runtime`.
* E0065 joins SPEC §12 and the error-codes page (D1 regenerates it).

## Implementation brief

1. `crates/undra-signals/src/context.rs`: `static CHECKER: OnceLock<fn(u64) -> bool>` (a test-only reset path
   for `clear_write_checker`), `check_write(owner) -> Result<(), WriteError>` compiled in every build; keep the
   "skip while panicking" rule. `WriteError` in `error.rs`.
2. `crates/undra-signals/src/signal.rs:250`: `write_with` calls `check_write` with the binding's owner (0 when
   unattached) before taking the value lock; on `Err` it logs through the sink hook (step 4) and panics with the
   E0065 text. Add `try_set`, `try_update`, `can_write` (and `try_` forms of the list operations in
   `signal/list.rs` only if review asks).
3. `crates/undra-signals/src/store.rs`: `owner` on `StoreCell` (`set_owner`, `owner`), passed to the sink as
   `deliver_from(owner, ..)` at `:741`; `ChangeSink::deliver_from` and an `off_core_write(owner)` reporting hook
   (defaulted) in `sink.rs`.
4. `crates/undra-runtime/src/runtime.rs`: `RUNTIMES` registry (insert in `build`, remove in `Drop`);
   `RuntimeSink::deliver_from` routes by owner; `write_allowed(owner)` checks `HELD` for that id (or any id when
   `owner == 0`); `insert_object`/`insert_store`/restore call `set_owner(self.id)`; `Ctx::with_core` over
   `enter_core`. Remove the "current or global" fallback from delivery (keep it for `log_fatal_current`).
5. `crates/undra-macros`: E0065 text in the diagnostic catalogue (`impl_/diag.rs`) for the runtime message and
   the docs anchor; SPEC §12 row.
6. Tests: the audit probe as a regression test in both profiles (`cargo test` and `cargo test --release`): a
   plain-thread write and a pool write to an observed store each panic with E0065 and leave the value unchanged;
   `try_set` returns `WriteError::OffCore`; `Ctx::with_core` from a host thread delivers exactly one change-set;
   a write to runtime A's store from a call on runtime B is refused (and with `with_core` on A, delivered to A's
   host); `with_core` from a host callback is `E_REENTRANT`.
7. Bench: `signals/changeset_100/*`, `signals/keyed_*` and `dispatch/call_sync/*` within their budgets; add
   `signals/set_attached` (one write + commit, no observer) to pin the check's cost.
8. SPEC 5.1 (the rule, both builds), 16.1 (checker signature, `StoreCell::set_owner`, `ChangeSink::deliver_from`,
   `try_set`/`try_update`/`can_write`), 16.2 (`Ctx::with_core`), §12 (E0065); the runtime review's L3 marked closed.

## Dependencies

Supersedes the release-build half of ADR-023 §5 ("release builds do not evaluate it") and ADR-020's routing by
thread. Independent of ADR-034/036/037; lands in `wt/runtime-lifecycle`.
