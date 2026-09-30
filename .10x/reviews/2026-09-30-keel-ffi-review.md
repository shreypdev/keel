# keel-ffi adversarial review (v1 safety gate)

Date: 2026-09-30 · Target: `crates/keel-ffi` (api, native, jni_shim, wasm, builtin, session, guard, buf, lib) at `4820127`
Method: read every `unsafe` block against the caller-visible contract (SPEC §5.1/§5.9/§6/§6.1/§6.3/§7, `keel.h`,
`KeelNative.kt`, `wasm-main.ts`, the Swift `InprocTransport`), then attacked it with scratch harnesses in
`/private/tmp/keelffi-review/` (never in the repo): C + ASan (`c/stress.c`, `c/pingpong.c`, `c/floor.c`), a Miri
crate (`miri-lie/tests/lie.rs`), and node wasm probes (`wasm/reenter.test.mjs`, `alloc.mjs`, `bounds.mjs`).
Every finding is **CONFIRMED** (repro / ASan / Miri) or **THEORY** (exact code path given).

## Acceptance matrix as run

| Suite | Result |
|---|---|
| `cargo test -p keel-ffi` / `--features jni` | 13 unit + 29 abi pass (both) |
| `tests/wasm/run.sh` (debug and `PROFILE=release-wasm`) | raw 16/16, TS 10/10 — against a **freshly built** TS dist (see M3) |
| `tests/jni/run.sh` | 13/13 |
| `tests/swift/run.sh` | NativeCoreTests 1/1 |
| `tests/c/run.sh` | **FAILS**: `smoke.c:95 assert(snap.len == 4)` (see M3) |
| `cargo +nightly miri test -p keel-ffi --lib` | 13/13, no UB (installed nightly+miri in ~7 s) |
| Miri on `abi.rs` (`abi_version…before_init`, `sync_port_answers_through_out_reply…`) | pass, no UB in `take_host_reply`/`free`; one leak (I1) |

## HIGH

### H1 — A port callback can run after the host was told it may free `user` (use-after-free) — CONFIRMED (ASan)
`native.rs:94-100` copies `(cb, user)` out of `PORTS` and releases the lock; `native.rs:183-203` then calls it with no
lock or refcount held. `keel_port_register(id, NULL)` (`native.rs:401-434`) and `keel_shutdown` (`native.rs:322-328`)
only edit the map, so both return while a copied registration is still executing (or about to start).
The documented contract says `user` must stay valid "until the registration is removed or `keel_shutdown` returns"
(`native.rs:396-399`, `280-283`) — i.e. the host is *told* it may free right then.
- Scenario (`c/stress.c uaf-unregister` / `uaf-shutdown`): thread T calls `keel_port_reply` with a malformed payload →
  runtime logs WARN without the core lock → `NativeHost::log` → Log port callback (sleeps 200 ms). Main thread calls
  `keel_port_register(LOG, NULL)` (or `keel_shutdown`), it returns, host `free(user)`. ASan:
  `heap-use-after-free … in on_port stress.c:31 ← CSink::port_call native.rs:194 ← NativeHost::log session.rs:101 ←
  Runtime::port_reply ← keel_port_reply`. Same for any port the core thread is mid-call on when unregistered, and for
  every Log path that runs off the core lock (malformed `keel_call`, `call_id 0`, `E_REENTRANT` refusals, contained panics).
- Reply/change-set/stream callbacks are *not* affected: every path holds the core lock and re-checks `is_shut_down`
  (runtime.rs:894-906, 957-966), and teardown clears calls/objects under it — verified by reading, and 100 cycles below.
- Shipped platforms are shielded by accident: Swift retains `user` forever (`InprocTransport.swift:65-68`), Kotlin's
  `GlobalRef` lives in the `Arc`'d `JniSink`, wasm has no unregister/shutdown. Any other C/C++/Rust embedder is exposed.
- Fix: store `Arc<PortReg>` (or an in-flight counter + `Condvar` per registration); invocation holds a clone;
  `keel_port_register` (replace/remove) and `keel_shutdown` swap the entry out, then **wait until no invocation of the
  old registration is running**; refuse (log, no-op) when called from inside a callback on the same thread (a
  thread-local "in callback" mark in the shim) to avoid self-deadlock. Add the ASan repro as a regression test and state
  in `keel.h`: "after `keel_port_register(id, …)`/`keel_shutdown` returns, no callback of the old registration runs".

## MEDIUM

### M1 — `out_reply` with `cap != 0` is reclaimed as a Rust `Vec`: one wrong field is allocator-mismatch UB — CONFIRMED (Miri)
`native.rs:144-151`: `cap != 0` ⇒ `KeelBuf::free` ⇒ `Vec::from_raw_parts` ⇒ Rust `dealloc`. A C host that fills
`{ptr = malloc(n), len = n, cap = n}` (the natural reading of "cap") triggers it. Miri (`miri-lie/tests/lie.rs`):
`Undefined Behavior: deallocating alloc53726, which is C heap memory, using Rust heap deallocation operation`.
On macOS/Linux with the `System` allocator this silently "works" (dealloc → `free`), masking the bug until an embedder
installs a `#[global_allocator]` (mimalloc/jemalloc) → heap corruption far from the cause. The SAFETY comment
("a non-zero cap means a buffer built by `KeelBuf::from_vec`") trusts a host-written field. No test or platform uses
the `cap != 0` branch (abi.rs, smoke.c, Swift all use `malloc` + `cap = 0`).
Fix: drop the branch — always `free()`; if a Rust embedder needs it, export `keel_buf_alloc(len)` so the core owns the
allocator choice. At minimum treat `cap != 0` as a contract violation (log FATAL, leak, answer Unavailable).

### M2 — `keel.h` omits the preconditions every native SAFETY comment relies on — CONFIRMED
`keel.h` ("the single source of truth for the C ABI", lines 8-17) says only "may be invoked on the core thread, a
blocking thread, or the caller's thread". Missing: (a) callbacks + `user` valid until `keel_shutdown` returns (only in
rustdoc, `native.rs:280-283`); (b) port `cb`/`user` lifetime (H1); (c) callbacks are invoked **concurrently** and must be
thread-safe — `UserPtr: Send + Sync` (`native.rs:77-81`) cites "SPEC 6", which never says so; `c/stress.c concurrency`
measured **4 simultaneous `port_cb` invocations** from 4 host threads; (d) callbacks must not unwind (a C++/ObjC
exception through an `extern "C"` (non-`C-unwind`) fn pointer is UB); (e) the re-entrancy allowlist contradicts itself:
`keel.h` allows only `keel_buf_free` from a callback, while SPEC 5.1 and `native.rs:436-440` allow
`keel_port_reply`/`timer_fired`/`stream_credit`/`stats_json`, and SPEC 7's sync-port flow *requires* `keel_port_reply`
inside `port_call`; (f) `keel_shutdown` is not callable from a callback and not concurrently with other entries
(header says "All functions are thread-safe"). Fix: move the `# Safety` sections into `keel.h` verbatim.

### M3 — The acceptance matrix has dead legs — CONFIRMED
- `tests/c/smoke.c:95` still asserts the pre-ADR-022 4-byte empty snapshot; `run.sh` aborts
  (`Assertion failed: (snap.len == 4)`). CI never runs `tests/c/run.sh` (`.github/workflows/ci.yml` has no C job), so
  the only harness that compiles `keel.h` with `-Werror` against the Rust exports rotted unnoticed.
- `tests/wasm/run.sh:26-33` silently uses `runtimes/ts/@keel/runtime/dist` when present; the local dist predates the
  `generationFloor` codec (`grep -c generationFloor dist/wire/payloads.js` = 0 vs 4 in `src`). In CI the `wasm-ffi` job
  (ci.yml:83-96) never runs `npm ci`, so the TS-over-real-wasm leg is **skipped** ("TypeScript runtime not built").
Fix: `snap.len == 8` + zero count/floor; add the C harness (plus H1 repro under `-fsanitize=address`) to the macOS job;
make `wasm/run.sh` always build the TS dist into a temp dir, and add `npm ci` to `wasm-ffi`.

### M4 — An asynchronously answered Log port loops forever — CONFIRMED
`session.rs:93-101` sends every core log record as a port call with `port_call_id 0` and ignores `Async`. A host whose
port layer answers `1` and later `keel_port_reply(id 0)` makes `Runtime::port_reply` log WARN "no port call 0 is
pending" → a new Log port call → … `c/pingpong.c`: **~100,000 Log calls/s**, unbounded, from one initial warning. Nothing
in `keel.h` says Log must be answered `0`/`2`. Fix: reserve id 0 (fire-and-forget) and drop replies to it silently in
the shim/runtime, or never log "unknown port call" for id 0; document Log as sync-only in `keel.h`.

## LOW

- **L1 Post-shutdown snapshot has floor 0 — CONFIRMED.** `api.rs:253-267` answers `[0;8]` whenever no runtime is up.
  `c/floor.c` (fixture core): running with 3 counters → `floor=3`; after `keel_shutdown` → `floor=0`. A C host that
  snapshots between shutdown and init loses ADR-022's "never re-issued … in a fresh process" guarantee. Fix: floor from
  the process-wide `PROCESS_GENERATIONS` (object_table.rs:231; expose it), or return an empty `KeelBuf` after a shutdown.
- **L2 wasm "did the host reply before returning 0?" is a global counter — THEORY.** `wasm.rs:151-160`: a host that,
  inside `port_call`, answers a *different* pending call and returns 0 is treated as `Async`; an async port future then
  waits forever instead of failing. Fix: check that *this* `port_call_id` is no longer pending.
- **L3 `keel_alloc` returns 0 instead of trapping — CONFIRMED.** `wasm.rs:260-263`: `keel_alloc(0x7fff_fff9)` → 0 (doc:
  "Traps on out-of-memory"); `wasm-main.ts #invoke` never checks and would write at linear address 0 (bottom of the
  shadow stack). Needs a ≥2 GiB payload, hence Low. Fix: `handle_alloc_error(layout)` for the oversize case.
- **L4 JNI sync-port protocol is two calls with hidden per-thread state — design.** `jni_shim.rs:169-188` calls
  `onPortCall` then `portSyncReply()`. `InprocTransport.kt` is correct (a `ThreadLocal` slot, and it copies every direct
  buffer before returning), but `KeelNative.Callbacks` is public and invites a shared-field race; direct-buffer
  non-retention is doc-only (unenforceable without copying). `KeelNative.kt` also calls a thrown exception "undefined
  behaviour" — the shim describes and clears it (`jni_shim.rs:86-95`). Fix: return the reply `ByteArray?` from
  `onPortCall` (v2), or document thread confinement on `portSyncReply`.
- **L5 `keel_shutdown` clears `PORTS` after releasing `INSTALLED` — THEORY.** `native.rs:323-327`: a `keel_init` +
  `keel_port_register` on another thread inside that window is wiped; the runtime keeps the port bound Foreign and
  answers Unavailable. Fix: clear inside the `session::stop` critical section (fold into the H1 drain).

## INFO

- **I1** Each init/shutdown cycle with the fixture core leaks 80 B: Miri `memory leaked … Arc<EventsInner>`
  (Runtime::build → Events::default); `c/stress.c` against `libkeel_core`: heap +80 B/cycle (1000 cycles ≈ +96 KB).
  Cause is keel-runtime `Subscription::detach` = `mem::forget` of a `Weak` (ports.rs:622-626). The lib-only core is flat.
- **I2** No layout assertion for `KeelBuf` anywhere (16/8 on 64-bit, 12/4 on wasm32); add `const` asserts in Rust and
  `_Static_assert` in `keel.h`. Correct by construction today (`#[repr(C)]`, `-Werror` C build links).
- **I3** `JNI_OnUnload` cannot fire while a runtime is up (the `GlobalRef` on `Callbacks` pins the class loader).
- **I4** wasm with a bogus `len > isize::MAX`: debug traps via `from_raw_parts` ub-checks, release reads garbage
  *inside* linear memory (`keel_call(16, 0xffffff00)` → 5); out-of-memory pointers trap. Sandbox-contained.

## Verified clean (attacked, held)

- **100 init → shutdown cycles** (C, counting callbacks, sync + async + observe + snapshot/restore each round): every
  reply/log delivered, threads back to 1, lib-only heap flat after warm-up. Idempotent re-init and double shutdown fine.
- **Reply/change-set/stream after shutdown**: unreachable (core lock + `is_shut_down` re-check; see H1 note).
- **wasm re-entrancy**: every export (`stats_json`, `snapshot`, `restore`, `schema_*`, `stream_credit`, `timer_fired`,
  `port_reply`, `poll`, `call`, `call_sync`, `observe`, `release`, `cancel`, `event`) called from inside all 9 imports,
  debug and release-wasm: **no trap**, no RefCell/lock double-borrow; core-lock entries refused with `E_REENTRANT`
  (224 logs), `restore` from a callback → 6. The shim holds no borrow across any import (`WasmHost` is stateless;
  `PORT_REPLIES`/`MONOTONIC` are atomics). `_initialize` twice is a no-op.
- **wasm allocator misuse traps rather than corrupts**: wrong-length `keel_free`, double `keel_free`, double
  `keel_buf_free` all trap (dlmalloc check) in both profiles; `keel_alloc(0)` gives distinct non-null blocks.
  `wasm-main.ts` pairs alloc/free lengths and frees every `KeelBuf*` in a `finally`.
- **Native buffers**: `from_vec`/`free`/`take_host_reply(cap == 0)` Miri-clean; `out_reply` is pre-set to EMPTY, so a
  host that returns 0 without writing is Unavailable, not UB. No double-free/leak path in the shim itself.
- **JNI**: every Java call goes through `on_java` (local frame, exception check/describe/clear); `PopLocalFrame` is
  legal with a pending exception; daemon attach is detached at thread exit (jni 0.21 TLS guard); `find_class` runs on
  the Java `init` thread (correct loader on Android); `portSyncReply` slot is per thread.
- **Panic containment**: every native/JNI entry is guarded directly or via `api::*`; unguarded ones (`keel_abi_version`,
  `keel_buf_free`, `PORTS.clear`) cannot panic. wasm panic hook passes live pointers to `log`, then traps (tested).
- **Codes/codecs**: platforms treat any non-zero `init`/`restore` code generically (no enum drift); the 8-zero-byte
  empty snapshot decodes in Swift (`PayloadTests` hex `0000000000000000`), Kotlin and TS.

## Verdict

**keel-ffi is sound for the three shipped platform runtimes** (Swift, Kotlin/JNI, TS/wasm) — Miri, ASan and the
stress runs found no UB on the paths they use. **It is not yet sound as a C ABI for third-party hosts**: H1 is a
confirmed use-after-free under the contract as written, and M1 turns a one-field host mistake into UB. Fix first:

1. **H1** — refcount + drain port registrations on unregister/replace/shutdown (and document it in `keel.h`).
2. **M1 + M2** — make `out_reply` always `free()`d (or add `keel_buf_alloc`), and put the lifetime / concurrency /
   no-unwind / re-entrancy preconditions into `keel.h`.
3. **M3** — repair `smoke.c` (8-byte snapshot), run the C harness (with the H1 ASan repro) and the TS-over-wasm leg in CI.

## Re-review (fix round merged at `f92d11e`)

I rebuilt every scratch harness against the merged checkout. The matrix is green:
- `cargo test -p keel-ffi`: 28 unit + 32 abi + 10 host_contract.
- `KEEL_C_SANITIZE=1 c/run.sh`: smoke and lifetime both ok.
- wasm: raw 19/19 and TS 10/10 (fresh dist), debug and release-wasm.
- JNI 13/13, Swift 1/1, Miri `--lib` 28/28.
- The wasm re-entrancy hammer still never traps.

| Finding | Verdict | Evidence (own harness) |
|---|---|---|
| H1 | **CLOSED for the reported race; see N1** | `stress.c uaf-shutdown/uaf-unregister` under ASan: no report. Both calls now return only after the callback ends. |
| M1 | CLOSED | `miri-lie` (`cap = 5` on a malloc block, callback asserted to run): no UB. The Vec branch is gone. |
| M2 | CLOSED | `keel.h` host contract 1–6 and SPEC §6 cover lifetime, concurrency, no-unwind, the callable-from-callback list and Log id 0. |
| M3 | CLOSED | `smoke.c` checks the 8-byte snapshot. CI runs `c/run.sh` with ASan, `npm ci`, a fresh TS dist and Miri. |
| M4 | CLOSED | `pingpong.c`: 1 Log call in 1 s (was about 100k/s). |
| L1 | CLOSED | `floor.c`: floor 3 after `keel_shutdown` (was 0). |
| L2 | CLOSED | `l2.mjs`: the host answers call 1 inside call 2's import and returns 0. Call 2 now fails instead of staying pending. (On wasm that failure is the Echo port panic, which traps by design.) |
| L3 | CLOSED | `keel_alloc(0x7fff_fff9)` and `keel_alloc(0x8000_0000)` trap in both profiles; `wasm-main.ts:105-106` throws on 0. |
| L4 | CLOSED (docs) | `keel.h` trailer and SPEC §6.1 name the thread-local `portSyncReply` rule. |
| L5 | CLOSED (as scoped) | `probe2 drain-allowed`: a `keel_init` issued mid-drain waits, then returns 0. SPEC promises only registrations made after that init. |
| I1 | CLOSED | 3,000 fixture init/shutdown cycles: heap flat at 17,040 B from cycle 500 on (was +80 B per cycle). |

**Answers to the four questions** (probes in `c/probe2.c`, fixture core, ASan):
- **(a) Long callback, concurrent removal.**
  - A `port_cb` that never returns blocks removal and `keel_shutdown` forever, with no timeout. This is documented (keel.h §5, SPEC §6 point 5, rustdoc).
  - Two threads removing the same port do not deadlock on the Condvar. But the second returns without waiting: see **N1**.
- **(b) Callback A removing B while B runs elsewhere.**
  - This waits correctly: `cross-wait` returned after 222 ms, with no deadlock.
  - A and B each removing the other from inside their callbacks hangs: see **N2**.
- **(c) Lock ordering: no new cycle.**
  - `retire_all` runs after `rt.shutdown()` has joined the runtime threads, and it holds only `INSTALLED`.
  - `drain-allowed` made every call keel.h allows from a callback, in the middle of the drain: `stats_json`, `schema_json`, `schema_hash`, `abi_version`, `port_reply`, `timer_fired`, `stream_credit`. All completed and shutdown returned.
- **(d) Id 0.**
  - `PortTable::begin` skips 0 on wrap (`ports.rs:229-240`), and only `NativeHost::log` sends id 0.
  - So dropping id-0 replies can only swallow answers to fire-and-forget Log calls. A host that echoes an id it was never given loses only the WARN.

**N1 (HIGH, the H1 class again; CONFIRMED by ASan): a removal that loses the race returns without draining.**
- **Cause.** `Registry::remove`/`install` (`registry.rs`) wait only for the registration *they* took out of the map. If `retire_all` (shutdown) or another `remove` took it first, the second caller finds `None` and returns at once.
- **Repro.** `probe2 remove-vs-shutdown` and `remove-vs-remove`:
  1. A Log callback is running on thread T.
  2. Thread Y calls `keel_shutdown` (it is now draining), or thread X calls `keel_port_register(LOG, NULL)`.
  3. Main calls `keel_port_register(LOG, NULL)`. It returns in 0.0 ms while the callback still runs.
  4. Main does `free(user)`. ASan reports `heap-use-after-free in on_log ← CSink::port_call ← NativeHost::log`.
- **Contract broken.** keel.h contract 1 ("when one of them returns, no invocation of the old registration is running"). Also SPEC §6 point 5, which allows `keel_shutdown` concurrently with other entries.
- **Fix.** Every removal of an id (and `retire_all`) must wait for *all* retired registrations of that id that are still in flight. For example, keep them in a per-id draining list until their count reaches 0, instead of dropping them from reach.
- **Scope.** Swift never removes ports, and Kotlin and wasm do not use the registry, so the shipped platforms are unaffected.

**N2 (LOW, CONFIRMED): mutual removal from callbacks is a silent hang.**
- **What happens.** `probe2 mutual`: the Sum callback (thread 1, under the core lock) removes Log while the Log callback (thread 2) removes Sum. Both drains wait for each other, and both are still stuck after 3 s. This happens in debug and in release. Before the fix it did not hang.
- **Why it is not caught.** keel.h §4 and §5 forbid `keel_port_register` from any callback. But the debug assertion only catches self-removal (`drain`'s `own` count).
- **Fix.** Assert (debug) and refuse with a FATAL log (release) whenever `RUNNING` is non-empty on the calling thread, not only for the same serial.

**Verdict.** Every original finding is closed. N1 reopens H1's use-after-free under concurrent removal, so fix N1 before the C ABI goes to third-party hosts. N2 is cheap hardening.

## Integrator resolution of the re-review (same day)

* **N1 fixed** by the integrator: a shared per-id draining list. Every removal (remove,
  replacing install, retire_all) publishes the registrations it takes, and waits for ALL
  draining entries of the id — the loser of a removal race included. Regression tests:
  `registry::tests::n1_the_loser_of_a_removal_race_still_waits_for_the_callback`,
  `n1_two_removers_of_one_port_both_wait`.
* **N2 fixed**: a removal from a thread inside ANY port callback is a debug assertion and,
  in release, returns without waiting plus a FATAL log (keel.h forbids the call; waiting
  could only deadlock). Tests: `n2_*` (debug and release variants).

### N1/N2 verification (reviewer, against `8f85447`; debug and release fixture cores)

- **N1: CLOSED.**
  - `probe2 remove-vs-shutdown` and `remove-vs-remove` under ASan: the losing `keel_port_register(LOG, NULL)` now blocks about 195 ms, until the Log callback ends (running = 0). Then `free(user)`. No ASan report, in debug or release.
  - Both removers, and the shutdown, return at the same moment.
- **N2: CLOSED.**
  - `probe2 mutual` returns in both profiles (under 140 ms, was a hang).
  - Debug: the assertion is contained by the entry's panic guard.
  - Release: the removal returns without waiting.
  - `probe2 n2-diag` (Sum callback removes the Echo port): the FATAL record reaches the host's Log port in both profiles.
  - *Info:* in the mutual case itself the FATAL is never seen. One thread has just removed the Log port, and the other is inside the Log path, where the recursion guard drops the record. keel.h already forbids the call, so no action is needed.
- **No regression:**
  - `stress.c uaf-shutdown` and `uaf-unregister` (ASan): clean.
  - 3,000 fixture init/shutdown cycles: heap flat at 17,168 B from cycle 500 to 3,000, so the draining list empties.
  - `port_cb` concurrency is still 4.
  - `c/run.sh` (ASan): ok.
  - `registry` tests: 10/10 in debug and release.
