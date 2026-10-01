# ADR-049: storage ports have an error channel, worker mode answers sync ports in the worker, and a trapped web core restarts from its last snapshot

Status: **Proposed** (2026-10-01, `wt/boundary-adrs`; Amendment C items 2 (pieces A6 and A7) and the gap
audit's PO-3, PO-4, PO-11 and PC-2, PA-5). Amends **ADR-024** (the standard surface: `Kv` and `SecureStore`
signatures, a new `StorageError`, two `FsError` variants) and **ADR-025** (which standard methods may lack an
error channel). Touches SPEC 7 (built-in sync ports in worker mode; the `random` import), 8, 9 (how
`undra-query` treats storage failures), 11 (worker protocol 3; recovery) and 17.1 (`snapshot`, `restore`,
`recovery`, `onCoreRestarted`); `undra-ports`, `undra-query`, `undra-ffi` (wasm `random`), `undra-bindgen`'s
standard table, the three runtimes' storage adapters, the TypeScript runtime's worker and transports. **No wire
change, no C ABI or wasm ABI change.** Every core's schema hash changes once (the standard surface), which
ADR-024 anticipates. Constitution R6 (nothing escapes as a panic or an abort), R11, R12.

## Context

The gap audit (`.10x/specs/2026-10-01-v1x-gaps.md`) rates three of these **blocks**:

* **PO-3: storage failures are panics.** `Kv` and `SecureStore` have no error channel
  (`crates/undra-ports/src/ports.rs:75-101`: `async fn get(&self, key: String) -> Option<Bytes>` …; the doc says
  calling one without an adapter "panics … (E0062), which traps a wasm core"). The generated proxy turns **any**
  non-`Ok` port outcome of a method without `Result` into a panic (`crates/undra-macros/src/impl_/port.rs:546-563`,
  `:630-670`): contained on native (the task is dropped, `crates/undra-runtime/src/runtime.rs:1718-1751`), a
  trap on wasm (`panic = "abort"`). Every platform adapter reports a storage failure as **status 2
  (unavailable)**: Swift's bridge answers 2 for any error that is not an `UndraPortError`
  (`runtimes/swift/.../Core/UndraCore.swift:864-907`) and the Kv/Keychain backends throw raw Foundation errors or
  `PortAdapterError` (`Adapters/KeyValueAdapters.swift:33-65`, `:139-187`, `:260-321`); Kotlin's `PortRegistry`
  likewise (`PortRegistry.kt:62-100`), and `FileKv` propagates `IOException` (`adapters/FileAdapters.kt:42-113`);
  TypeScript's IndexedDB `set`/`delete` reject on `QuotaExceededError` and the bridge answers 2 and calls
  `onError` (`adapters/kv.ts:43-54`, `core.ts:755-785`); a missing IndexedDB or `crypto.subtle` leaves the port
  unregistered (`adapters/browser.ts:119-124`). Android registers no Kv at all (`adapters/JvmAdapters.kt:53-63`).
  `undra-query` calls Kv through the panicking proxy in hydration (`shared.rs:842`, `:855`), persistence
  (`:1165-1167`) and the offline queue (`queue.rs:208`, `:246`, `:307-308`); only `list` uses a raw call with
  retries (`shared.rs:1053-1078`). `Fs` is already typed: every adapter maps failures to `FsError` → status 1
  (`FsAdapter.swift:80-163`, `FileAdapters.kt:205-230`, `adapters/fs.ts:24-39` + `ports.ts:39-51`).
* **PO-4: worker mode traps on the first Clock, Rng or Log call.** The worker answers every port call
  "async" ("The core cannot wait for the main thread, so every port call is asynchronous here",
  `runtimes/ts/@undra/runtime/src/worker.ts:116-120`). A sync port called through `port_call_sync` that gets
  `Async` is `Unavailable` (`runtime.rs:1895-1900`); the wasm shim's built-in Clock/Rng/Log answers run only
  when the import returns `2` (`crates/undra-ffi/src/wasm.rs:164-166`), so they are never reached; the
  infallible proxy panics; the core traps. Two doc comments claim the built-ins cover worker mode
  (`src/port.ts:15-18`, `transport/wasm-worker.ts:83-84`); the `clock`/`rng`/`timer` adapters given to `load`
  never reach the worker (`core.ts:244-256`).
* **PC-2: a trapped web core is dead for good.** A trap sets the transport's `#dead`, rejects pending calls,
  fails streams and calls `onClose` (`transport/wasm-main.ts:418-432`, `core.ts:536-566`); `LoadOptions.onError`
  never hears of it (`core.ts:238-240`). TypeScript's `UndraCore` has no `snapshot()`/`restore()`; the runtime
  never calls `undra_snapshot` and never sends `Restore` (`wasm-main.ts:75`, `:285-294`) — though SPEC 7 says
  "Panics call the `log` import with level 5 … before trapping so the host can restart from snapshot", and Swift
  and Kotlin have both (`UndraCore.swift:535-549`, `UndraCore.kt:210-219`).
* **PO-11 (hurts)**: without WebCrypto the `random` import's error is swallowed and the built-in `Rng.fill`
  returns **zeros** with status 0 (`wasm-main.ts:482-485`, `crates/undra-ffi/src/builtin.rs:97-101`), so
  idempotency keys collide.

## Decision

### 1. Storage ports have an error channel

1. **`StorageError`** joins the standard surface (an `#[undra::error]` enum with `From<PortError>`, ADR-025's
   pattern):

   ```rust
   pub enum StorageError {
       Unavailable(String),   // no adapter, no backend in this context (no IndexedDB, no secure context, no Keystore)
       Full,                  // quota or disk exhausted
       Locked,                // protected data unreadable now (Keychain before first unlock, a key that needs user auth)
       Corrupt(String),       // stored bytes or ciphertext that cannot be read back; the key is still there
       Io(String),            // anything else, with the platform's message
   }
   // From<PortError>: Unavailable → Unavailable("the Kv port has no adapter registered"), Cancelled → Io("cancelled"),
   //                  Decode(e) → Corrupt("malformed port reply: …"), Failed(bytes) → the decoded StorageError (else Io)
   ```

2. **New signatures** for `Kv` and `SecureStore` (identical shapes): `get -> Result<Option<Bytes>,
   StorageError>`, `set -> Result<(), StorageError>`, `delete -> Result<(), StorageError>`, `list ->
   Result<Vec<String>, StorageError>`. `FsError` gains `Full` and `Unavailable(String)` (its `From<PortError>`
   maps `Unavailable` there instead of `Io`). After this, **every standard port method that can fail at run time
   has an error channel**; the ones without (`Clock`, `Rng`, `Log`, `Timer`, and the event ports) are answered
   by built-ins or cannot fail (decision 3), which ADR-025's "register the optional ports … or give their methods
   a `Result`" now holds for the standard surface by construction.
3. **Adapters return typed failures** (an `UndraPortError` carrying `StorageError`, status 1), never a raw throw:

   | Failure | Swift | Kotlin (JVM / Android, C4a) | TypeScript |
   |---|---|---|---|
   | no backend | — (always present) | Keystore missing → `Unavailable` | no `indexedDB` / no `crypto.subtle` → **registered anyway**, answering `Unavailable("needs IndexedDB" / "needs a secure context")` |
   | quota / disk | `NSFileWriteOutOfSpaceError`, `ENOSPC` → `Full` | `IOException` with `ENOSPC` → `Full` | `QuotaExceededError` → `Full` |
   | locked | `errSecInteractionNotAllowed`, data-protection `NSFileReadNoPermissionError` before first unlock → `Locked` | `UserNotAuthenticatedException` → `Locked` | — |
   | unreadable | a stored file that does not decode, Keychain item of the wrong class → `Corrupt` | `WireException` in `get`, `KeyPermanentlyInvalidatedException` → `Corrupt` | `OperationError` on decrypt, bad stored format → `Corrupt` |
   | other | `Io(error.localizedDescription)` | `Io(message)` | `Io(message)` |

   The bridges keep answering 2 for a non-typed throw (a bug in an adapter), now with an ERROR log naming the
   adapter.
4. **`undra-query` treats storage as best-effort and never loses a queue.**
   * A failed **persist write** keeps the entry in memory, logs a WARN once per (operation, reason), counts
     `persist.write_failed` in `stats_json`, and tries again at the next write; `Full` pauses new persisted entries
     until a write succeeds.
   * A failed **hydration read** of a cache entry starts that entry empty (it can be fetched again).
   * A failed **queue read** (`Locked`, `Io`, `Unavailable` — an iOS app launched in the background before first
     unlock, ADR-046) leaves the client **not hydrated for the queue**: it does not replay, and it **never
     writes the queue key** until a read succeeds (writing would overwrite the unreadable queue); it retries on
     `Active`, on the next background run and after a backoff. New offline mutations wait in memory meanwhile. A
     `Corrupt` queue is moved, bytes intact, to ADR-037's dead-letter key and reported.
   * The raw `list` retry loop (`shared.rs:1053-1078`) becomes an ordinary typed call.
5. Apps that use `ctx.kv()` directly now handle a `Result` (nothing is published).

### 2. Worker mode answers sync ports inside the worker

1. **Built-ins.** The worker's `port_call` handler returns **`2`** for any port it was not given an
   implementation for **in the worker** — so the shim's built-in Clock/Rng/Log answers run (`wasm.rs:164-166`)
   on the worker's own `Date.now()`, `crypto.getRandomValues` (available in workers) and the `log` import (which
   the worker already forwards as Log envelopes, `worker.ts:148-150`) — and keeps returning `1` (async, crossing to
   the main thread) only for ports registered on the main thread whose methods are asynchronous. The worker
   learns which ports are async from the host's registrations: the set is sent in `init` and kept current by a
   `{t:"ports", asyncPorts}` message whenever the host registers or removes a port after load.
2. **App sync ports and overrides run in the worker.** `LoadOptions.worker.ports` (a module URL) is imported by
   the worker before `undra_init`; its default export maps port ids to implementations (generated adapters, as
   `registerPort` takes on the main thread). Sync ports there are answered synchronously in the worker; this is
   also how an app overrides `Clock`, `Rng` or `Timer` in worker mode (today impossible). A sync port registered
   on the **main thread** in worker mode is a load-time error naming it and the fix ("register it in
   `worker.ports`"), instead of a trap at its first call.
3. **Protocol.** The worker protocol becomes version 3 (ADR-031's version 2 plus fields): `init` gains
   `asyncPorts: number[]` and `portsModule?: string`, and a host-to-worker `{t:"ports", asyncPorts}` message keeps
   the set current; the rest of version 2 — the batched `envelopes` message,
   transfer, ordering — is unchanged. Both sides ship in one package, so no negotiation beyond the announced
   version.
4. **No `SharedArrayBuffer`** (rejected below). Nothing in the TypeScript runtime uses it today.
5. **Randomness never degrades silently** (PO-11). The `random` import throws when no CSPRNG exists; the shim
   then answers the built-in `Rng.fill` **unavailable** instead of zeros, and `Rng` keeps no error channel
   (randomness must not fail), so the proxy's E0062 panic — a loud trap with a FATAL record that names the cause —
   replaces silently colliding idempotency keys. `UndraCore.load` checks `crypto.getRandomValues` up front and
   rejects with `UndraTransportError("unsupported", "WebCrypto is required")` before instantiating.

### 3. A trapped web core restarts from its last snapshot (opt-in)

1. **Snapshot parity first (C4c).** TypeScript `UndraCore` gains `snapshot(): Promise<Uint8Array>` and
   `restore(bytes): Promise<void>` on every mode: the `undra_snapshot`/`undra_restore` exports in `wasm-main`, two
   new worker messages (`{t:"snapshot", id}` → `{t:"snapshot", id, data}`, `{t:"restore", id, data}` →
   `{t:"restored", id, code}`) in `wasm-worker`, the existing Snapshot/Restore envelopes (kinds 15/16) in `remote`.
2. **Recovery is opt-in**: `LoadOptions.recovery: true | { snapshotEveryMs = 1000, maxSnapshotBytes = 4 MiB,
   maxRestarts = 3, perMs = 60_000 }`. Off by default, because it costs a snapshot (time and a copy) up to once
   a second while stores change, and because it changes what a trap means to the app.
3. **Snapshots** are taken after a drain that applied store change-sets, at most once per `snapshotEveryMs`, in
   an idle callback where one exists (`requestIdleCallback`), and kept as a JavaScript `ArrayBuffer` outside wasm
   memory; in worker mode the worker takes and keeps it (off the main thread). A snapshot larger than
   `maxSnapshotBytes` is not kept (the previous one stays) and is reported once.
4. **On a trap**, in this order:
   1. ADR-046's panic report (from the FATAL `undra::panic` record and the trap's stack) goes to `onPanic`;
   2. every in-flight call rejects and every open stream ends with **`UndraTransportError("restarted")`** — the
      call may or may not have applied before the trap, so it is reported as failed, never retried silently;
   3. the runtime instantiates the **same compiled module** again (no recompile), runs `_initialize` and
      `undra_init`, re-registers the ports (their implementations are unchanged on the host side), and
      **restores the last snapshot** (stores keep their handles, ADR-022; ADR-037's identity check runs; ADR-040's
      derived handles come back stale);
   4. the mirror re-observes every registered store with its signals; the core answers with the restored values;
   5. query handles, which are transient by design (`crates/undra-query/src/lib.rs:74-75`, "the platform
      re-creates them after a restore"), are **re-created**: generated query handles record their constructor
      call, and the runtime re-runs it and moves the wrapper (and its mirror registration) to the new handle;
      other objects that went stale stay stale and fail with a typed refusal;
   6. `LoadOptions.onCoreRestarted({ report, restoredFromAgeMs, rejectedCalls, staleObjects })` is called, and
      `onError` receives the same as an `UndraCoreRestarted` value.
   Past `maxRestarts` within `perMs`, the core stays dead and `onClose` reports the trap, as today.
5. **What is lost**, stated in the docs: store writes made after the last snapshot (at most `snapshotEveryMs` of
   them), in-flight calls and streams, non-store objects (except query handles, which come back), the core's
   running tasks and timers (a store that needs one restarts it in its `restore` hook, as for any restore), and
   query cache entries that were not persisted (refetched when their handles come back). Persisted query entries
   and the offline queue are in `Kv` and survive untouched.
6. **Native platforms** contain panics without trapping (SPEC 5.6); recovery is web-only. Persisting a snapshot
   across a page reload is B3/A5 territory (ADR-037 makes it safe) and not part of this ADR.

## Alternatives considered

* **Keep `Kv` infallible and make adapters never fail** (swallow errors, return `None`). A failed write that
  reports success is data loss the app cannot see; a locked Keychain read that returns `None` makes the queue
  logic overwrite real data.
* **Give `Kv` a `Result` but let `undra-query` propagate storage failures to the mutation's caller.** A
  persistence hiccup would fail user actions that succeeded on the server; best-effort with counters, and a queue
  that is never overwritten, is the right default.
* **`SharedArrayBuffer` + `Atomics.wait` so the worker can block on main-thread sync ports.** Requires
  cross-origin isolation (COOP/COEP headers) that many sites cannot serve, stalls every core task behind the main
  thread's longest task, and deadlocks the day anything on the main thread waits synchronously for the worker.
  Running sync ports in the worker has none of these.
* **Answer every unknown sync port in the worker "unavailable" and stop there.** Fixes the trap for the
  built-ins but leaves no way to override `Clock`/`Rng` or to provide an app sync port in worker mode.
* **Always-on recovery.** A per-second snapshot is a cost some apps will not want, and a silent restart can hide
  a bug; opt-in with a typed `onCoreRestarted` is honest.
* **Restart without a snapshot** (a fresh core, the app rebuilds its state). Loses every store value; the
  snapshot machinery exists and ADR-022/023/037 make restore safe.
* **Wasm exception handling (`panic = "unwind"`) so panics never trap.** Not available on stable Rust without
  `-Z build-std` (the gap audit, PC-2); revisit when it is, and keep recovery for real traps.

## Consequences

* No standard port can take a core down through an adapter failure; `wasm-worker` mode works with the
  built-ins; a web core that traps can come back with its state (opt-in). The playground's web app turns
  recovery on and shows the restart in its debug panel.
* The standard surface changes (`StorageError`, `Kv`, `SecureStore`, `FsError`): with ADR-046's additions and
  the gap audit's pending standard changes (PO-6's `HttpError::TooLarge`, PO-8's `Fs::delete_dir`), it should
  ship as **one standard-surface revision**, so every core's hash and every runtime's standard types move once.
* TypeScript reaches snapshot parity with Swift and Kotlin (C4c).

## Risks

* **Adapter error mapping is per platform and easy to get subtly wrong** (Keychain `OSStatus` values, Android
  Keystore exceptions). A shared adapter test suite (the gap audit's PO-8 idea) runs the same failure cases on
  every platform with injected failures.
* **Recovery can loop** on a deterministic panic (the restored state panics again); the restart budget stops it
  and reports.
* **Re-created query handles** change their `handle` value; code that kept the raw handle (not the wrapper)
  breaks. Generated code never does; the raw `UndraCore` API documents it.

## Implementation brief

1. `crates/undra-ports`: `StorageError` (+ `From<PortError>`, `Display`), the new `Kv`/`SecureStore` signatures,
   `FsError::{Full, Unavailable}`; fakes (`MemKv` with injectable failures, a `FailingKv` for tests).
2. `crates/undra-bindgen`: `stdlib` table and its cross-check test (`tests/stdlib.rs`); the `stdlib` golden.
3. `crates/undra-query`: every Kv call handles `Result` (decision 1.4): `shared.rs` hydration and persistence,
   `queue.rs` read/write with the "never overwrite an unreadable queue" state, counters in `stats_json`.
4. Runtimes' adapters and standard types: Swift (`KeyValueAdapters.swift`, `FsAdapter.swift`, `StandardPorts.swift`),
   Kotlin (`FileAdapters.kt`, `StandardRecords.kt`, `StandardPorts.kt`, and C4a's Android adapters), TypeScript
   (`adapters/kv.ts`, `secure.ts`, `fs.ts`, `types.ts`, `codecs.ts`, `browser.ts` registering storage ports that
   answer `Unavailable`); a shared failure-injection suite per runtime.
5. `crates/undra-ffi/src/wasm.rs` + `builtin.rs`: `Rng.fill` unavailable when `random` throws; TS `random`
   import throws instead of swallowing; `load` checks WebCrypto.
6. TypeScript worker: `worker.ts` returns 2 for ports not registered as async on the host and for sync ports
   without a worker implementation; `worker.ports` module loading; protocol 3 (`worker-protocol.ts`); fix the two
   doc comments; the load-time error for main-thread sync ports in worker mode.
7. TypeScript recovery: `snapshot()`/`restore()` on `UndraCore` and the transports; the snapshot scheduler; the
   trap path (`wasm-main.ts` `#classify`, `core.ts` `#lost`) gains the restart sequence; re-creatable query
   handles in the generated TS (the constructor call is recorded) and in `UndraStore`; `onCoreRestarted`;
   `UndraTransportError("restarted")`.
8. Contract scenarios (provisional numbers): **S29 "storage failures are typed"** (a Kv adapter that fails `Full`
   then recovers: no panic or trap, a WARN and a counter, the entry persisted on the next write; an unreadable
   queue is not overwritten and replays once readable) on all columns; **S30 "worker sync ports"** (the playground
   core in `wasm-worker` mode with a query observed: Clock, Rng and Log answered in the worker; an app sync port
   from `worker.ports`) on the TS column; **S31 "web recovery"** (TS: a method that panics on purpose; the call
   rejects `restarted`; stores show the snapshot's values; a query handle comes back; the fourth trap within a
   minute leaves the core dead).
9. Bench: `ts/snapshot_take_100kb` (wasm-main, main thread; budget 2 ms on desktop Chromium) and
   `ts/recovery_restart_100kb` (re-instantiate + restore + re-observe 50 signals; budget 50 ms); rows in
   `bench/RESULTS.md`.
10. Docs: SPEC 7, 8, 9, 11, 17.1; ADR-024 and ADR-025 get dated amendment notes pointing here; the web
    cookbook page ("worker mode", "recovering from a crash").

## Dependencies

ADR-037 (the snapshot's identity makes a recovery restore safe across an update and moves a corrupt queue to
the dead-letter key), ADR-046 (the panic report precedes a restart; background runs retry an unreadable queue),
C4a (Android storage adapters return `StorageError` from day one), C4c (TypeScript snapshot parity is decision
3.1). PO-4's fix (decision 2.1) is small and can land first, in the parity piece, as Amendment C allows.
