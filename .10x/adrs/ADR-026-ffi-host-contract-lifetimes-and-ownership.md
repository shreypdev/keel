# ADR-026: the native host contract: port registrations drain, `out_reply` is always `free()`d, Log is fire and forget

Status: accepted (2026-09-30). Touches SPEC 6, 6.1, 6.3 and 7 and `keel.h` (contract text; no
signature changes, no wire change). Origin: `.10x/reviews/2026-09-30-keel-ffi-review.md`, findings
H1, M1, M2, M4, L1, L2, L3, L4, L5 (and I1 in `keel-runtime`). Constitution R11: these are boundary
changes, so they are decided here before the code.

## 1. H1 + L5: removing a port registration waits for its running callbacks

Context. `keel_port_register(id, NULL)` and `keel_shutdown` only edited a map, while a port call
copies `(cb, user)` out of it and calls them with no lock or count held. The header told the host
it could free `user` when they returned. A callback running on another thread (the Log port reached
from a malformed `keel_port_reply`, any core-thread port call) then touched freed memory
(confirmed under ASan).

Decision.
* A registration is an `Arc<PortReg>` with an in-flight count and a `retired` flag under one mutex.
  A port call *enters* it (count + 1, fails if retired, then looks the port up again) and leaves it
  when the callback has returned and the reply memory has been read and released.
* `keel_port_register(id, NULL)`, a replacing `keel_port_register` and `keel_shutdown` take the
  registration out of the map, mark it retired and **block until its count is zero**, never under
  the map's lock (so a callback may still reach other ports). Afterwards the old callback is not
  running, never starts again, and its `user` is never read again.
* Waiting for yourself is a contract violation: a removal issued from inside a callback of the same
  registration would never finish. A thread-local stack of the registrations running on the thread
  detects it; debug builds assert (contained by the boundary guard, R6), release builds skip the wait
  for the calling thread's own invocations and wait for the others. `keel_shutdown` from a callback
  was already asserted by `Runtime::shutdown` (ADR-023).
* L5: `keel_shutdown` retires the registrations inside the critical section that serialises
  `keel_init`, after the runtime has stopped (its shutdown still logs through the Log port). A
  `keel_init` on another thread waits for it, so a registration a host makes after that init returns
  cannot be wiped by the earlier shutdown. `JNI_OnUnload` shares the path.
* Cost: one `Arc` clone, two uncontended mutex operations and a thread-local push per port call:
  not measurable on `boundary/port_call/sum_on_host` (about 240 ns before and after).

Rejected: refusing (no-op) an unregister that would deadlock. A silent no-op leaves the host
believing `user` is free; an assertion in debug and "cannot wait for yourself" in release is the same
stance the runtime takes for `shutdown`.

## 2. M1: `out_reply` is always `malloc`ed and released with `free`

The `cap != 0` branch (a Rust `Vec` reclaimed from a host-written field) is deleted. A host that
filled `cap` with its capacity made the core free a C block with Rust's allocator (Miri: undefined
behaviour; silent on a `System`-backed build, heap corruption under another global allocator).
`cap` of `out_reply` is reserved and ignored. No platform used the branch (Swift, Kotlin/JNI, wasm
and every test wrote `cap = 0`).

## 3. M2: the contract is in `keel.h`

The header is the single source of truth for C hosts, so it now states, next to the typedefs:
lifetimes (above), concurrent invocation of callbacks on arbitrary threads, no unwinding, which
entries a callback may call, the waits of section 1, the Log port and the JNI per-thread note. The
allowed list was derived from the code, not from the draft: `keel_buf_free`, `keel_port_reply`,
`keel_stream_credit`, `keel_timer_fired`, `keel_stats_json` and the read-only `keel_abi_version`,
`keel_schema_hash`, `keel_schema_json` never take the core lock. `keel_call`, `keel_call_sync`,
`keel_cancel`, `keel_observe`, `keel_release`, `keel_event` and `keel_restore` take it and are refused
with `E_REENTRANT` (`keel_cancel` and `keel_event` were wrongly on the draft list). `keel_init`,
`keel_shutdown`, `keel_port_register` and `keel_snapshot` must not be called from a callback.
`abi.rs::callbacks_may_call_exactly_the_documented_entry_points` holds both sides.

## 4. M4: port call id 0 is fire and forget

The core sends every log record to the Log port as a port call with id 0 and ignores the answer. A
host that answered `1` and later called `keel_port_reply(id 0)` made the runtime warn "no port call 0
is pending", which is another Log call: about 100,000 calls a second from one warning. Real port calls
are numbered from 1; `api::port_reply` drops a well-formed reply with id 0 before the runtime sees it.
The Log port needs no particular answer.

## 5. L1: the snapshot floor survives shutdown

`keel_snapshot` with no runtime returns `count 0` and the **process-wide** generation counter as the
floor (ADR-022), not 0. `keel_runtime::object_table::process_generation_floor()` is the one read-only
accessor this needed (the counter was private).

## 6. wasm: L2, L3 (and the TypeScript check)

* L2: the wasm shim decides whether a host that returned `0` from `port_call` answered *that call*
  by the ids of the well-formed `keel_port_reply` payloads that arrived while the import ran, not by a
  global count. A reply to another pending call no longer hides a lie.
* L3: `keel_alloc` traps (level-5 log first) for a size no allocation can have instead of returning
  0, as its documentation always said; the TypeScript wasm transport also refuses a 0.

## Not done

I2 (layout assertions for `KeelBuf`) and I3/I4 of the review are untouched. L4's protocol change
(return the sync reply from `onPortCall`) is a v2 ABI matter: v1 documents the per-thread state
instead, and the shipped Kotlin `Callbacks` (`InprocTransport`) already keeps it in a `ThreadLocal`.
`KeelNative.kt` still calls an exception thrown from a callback "undefined behaviour" while the shim
describes and clears it (`jni_shim.rs`): a wording fix for the Kotlin owners.
