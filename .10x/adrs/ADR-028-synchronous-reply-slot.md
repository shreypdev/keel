# ADR-028: the synchronous call path allocates nothing: a per-thread reply slot

Status: accepted (implemented on `wt/fast-dispatch`). Touches SPEC 5.6 and 16.2, the dispatcher
emission of `keel-macros`, `Runtime::call_sync` (+ the new `call_sync_with`) in `keel-runtime`. No wire
change, no ABI change, no schema change, no generated platform code change (bindgen goldens and the
Kotlin / TypeScript / Swift suites see byte-identical replies). Constitution R11: the dispatch
contract between the macros and the runtime changes, so it is decided here before the code.

## Context

`bench/RESULTS.md` (finding 2) measured `Runtime::call_sync` of `add(i64, i64)` at 77.8 ns on an
Apple M5 Pro against a 60 ns iOS (A15) budget, with about 60% of the time in `malloc` / `free` (a
`mach_absolute_time` read that this OS's allocator does inside every allocation is the top symbol).
A synchronous call allocated at least three times before the host saw a byte:

1. the dispatcher encoded the result into a fresh `Vec<u8>` (`Encode::encode_to_vec`);
2. it boxed a `DispatchResult` into the `DispatchOutcome` (`Box<dyn Any + Send>`, 32 bytes);
3. the runtime built the `Reply` payload (`call_id`, status, body) in a third `Vec`, which the ABI
   then moved into a `KeelBuf` (and the caller frees).

Constitution R5 says reads never cross the boundary and writes cross once per transaction; it is
about the platform side, but the same spirit applies to the core's side of the crossing: nothing
between payload decode and reply bytes needs the heap. The one allocation that is owed is the
`KeelBuf` (`keel.h`: the caller frees what `keel_call_sync` returns).

## Decision

1. **A per-thread reply slot** (`keel-runtime/src/sync_out.rs`): one reusable `Vec<u8>` and a tiny
   `Cell` state machine in a `thread_local!` (`Free`, `Armed(runtime id)`, `Written`, `Reading`).
   `Runtime::call_sync_with` arms it for the call, and the dispatcher's answer is encoded straight
   into it as the complete `Reply` payload (`call_id u32`, `status u8`, body).
2. **`Runtime::sync_ok(&value, Encode::encode)` / `sync_err(&error, Encode::encode)`** are what a
   generated dispatcher calls for a synchronous result. Armed for this runtime: the reply is written
   into the slot and the returned `DispatchOutcome` holds a **zero-sized** marker (`Written`), which
   boxes without allocating. Not armed: they return `DispatchOutcome::new(DispatchResult::Sync(..))`
   exactly as before. `DispatchOutcome`, `DispatchCall`, `DispatchFn`, every `DispatchResult` variant
   and `keel-meta` are unchanged.
3. **`Runtime::call_sync_with(payload, |reply| ..)`** lends the slot to the caller's closure, after the
   core lock is released; it is the allocation-free entry. `Runtime::call_sync` keeps its signature and
   copies the lent bytes into the `Vec` it returns: its one allocation, which is the one the C ABI owes
   (`keel_call_sync` moves that `Vec` into the `KeelBuf`; no second copy, no change in `keel-ffi`'s code).
4. **Fallback is the contract.** The slot is used only when it is `Free` at the start of the call and
   `Armed` for the answering runtime. Everything else allocates as before: `keel_call` (the async entry,
   which does not arm it), `DispatchLayer`s and hand-written dispatchers that still return
   `DispatchResult`, a dispatcher called directly by a test, a call made from inside the `read` closure
   or from a method that was itself called through the slot (E_REENTRANT guards the same runtime; a
   second runtime called from a method of the first sees a busy slot and allocates), and a thread whose
   thread-local is already destroyed. The two paths are byte-identical; `testing::call_sync_reference`
   runs a call through the allocating path (by keeping the slot busy) so tests compare them.
5. **Panic guard unchanged.** The dispatcher still runs inside `guard::guarded`; a panic (including one
   in the value's `Encode` after the slot took the call) becomes status 2 built by the old path, the
   lease that owns the slot frees it on drop on every path, and a buffer a panic abandoned is simply
   regrown. `call_sync_with`'s closure runs outside the guard (it is caller code; the boundary shells
   guard it like everything else).
6. **Bounded memory.** The slot keeps at most 64 KiB between calls; a larger reply is served and its
   buffer released, so one huge reply does not pin memory on every thread that ever made a call.
7. **The encoder is a parameter** (`fn(&T, &mut Writer)`, generated code passes the path
   `::keel::wire::Encode::encode`), not an `Encode` bound on `sync_ok`. A bound adds a rustc "required
   by a bound in `Runtime::sync_ok`" note to the E0001 diagnostic of a type that cannot cross the
   boundary, which would change a user-visible diagnostic (R8, the `m5_method_returns_an_object` UI
   test) for no benefit; with the path, the diagnostics are byte-identical to before.

Also on the same path (measured, small, no contract change): the dispatch table's `u32` ids use a
one-multiply hasher instead of SipHash; the per-call "is this method async-shaped" scan over the
object's method list is a precomputed set; the routing lookup reads the receiver's type id without
taking a reference to the object (`ObjectTable::type_of`) and `ObjectTable::get` takes one `Arc`
reference instead of two; the panic guard's depth counter and last-report slot share one thread-local.

## Alternatives considered

* **Status quo.** Rejected: the allocator is ~60% of the call and the row misses its budget on a
  core faster than the target device's.
* **A new `DispatchResult::SyncWritten` variant.** `DispatchOutcome` boxes its payload, and a
  `DispatchResult` is 32 bytes, so the box would still allocate; the variant would also still need the
  writer plumbed to the dispatcher.
* **`out: &mut Writer` (or a slot handle) in `DispatchCall`, or a changed `DispatchFn` signature.** Puts
  `keel-runtime`'s buffer type into `keel-meta`'s public types (`DispatchCall` is `Copy + Eq`), changes
  every hand-written dispatcher, layer and test fixture, and is a bigger blast radius than the thread
  local for the same win.
* **A second generated entry point per object for sync methods.** Doubles generated dispatch code
  (the hello-world size row is at 82% of its budget already) and needs a second function pointer in
  `ObjectMeta` / `FunctionMeta`.
* **A caller-stack `Writer` reached through a thread-local raw pointer.** Needs `unsafe` outside
  `keel-ffi` (R2). The buffer-moves-in-and-out `Cell` design is safe Rust and cannot alias.
* **A `Mutex<Writer>` inside `Runtime`.** A lock pair per call for what is per-thread state; the core
  lock is already held but guards no buffer the borrow checker can see.
* **Pooling `KeelBuf`s to remove the ABI allocation.** `keel_buf_free` would return them to a pool
  that needs a lock shared by every host thread; a mutex pair costs about what `malloc` / `free` of 16
  bytes does. `keel.h` promises the caller frees; that stays true and simple.

## Consequences

* Measured on the Apple M5 Pro host (shared machine; parent commit and new build measured back to back,
  best of three rounds; see `bench/RESULTS.md`): `dispatch/call_sync/add` 73.8 to 43.9 ns, the
  allocation-free `call_sync_with` variant 31.5 ns, `boundary/call_sync/add` (C ABI, one `KeelBuf`)
  79.3 to 49.8 ns, `echo_record1k` 268 to 140 ns. An A15 core is slower than this one, so the blueprint
  row (60 ns on iOS) is **within on the host and open on the device**: it passes there only if an A15
  core runs the path within 1.37x of this core's time; the verdict belongs to the device phase. The
  status 5 / 2 paths (allocating anyway) are unchanged.
* `crates/keel-ffi/tests/sync_alloc.rs` counts allocations with a global allocator: zero per
  `call_sync_with`, one per `call_sync`, one per `keel_call_sync`, for generated methods (ok, typed
  error, free function).
* `keel_call` (the async entry) still allocates its reply; giving it the same slot is a follow-up
  (the reply callback already receives a slice), not needed for the synchronous budget.
* A dispatcher must call `sync_ok` / `sync_err` at most once and return that outcome. Returning
  anything else after calling them is harmless (the runtime takes the reply from what was returned and
  frees the slot), but nothing is gained.
* wasm32 (`panic = "abort"`, one thread): the thread-local is a static; nothing changes but the
  allocation count. No new dependency, no `unsafe` (R2), no clock, randomness or thread (R12).
