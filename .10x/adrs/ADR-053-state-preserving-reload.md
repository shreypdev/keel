# ADR-053: `undra dev` carries the core's state across a rebuild: the dev server snapshots the old core, restores it into the new one, and the clients resume

Status: **Accepted** (2026-10-01, piece B3 of the v1.x plan, `wt/dev-reload`; the integrator accepted it with the four decisions at the end, which supersede the text above where they differ). Touches the dev server and its
generated runner (`undra-cli`: `commands/dev.rs`, `runner.rs`, `templates/runner/main.rs`), `undra-transport`
(`ServerConfig`, `Server::suspend`, two small session hooks), the three platform runtimes (one optional callback,
`onDevNotice`; SPEC section 17) and the three dev status bars. It does **not** touch the envelope (3.2), any
payload, the C ABI, the wasm ABI, the schema, the generated code, `undra-runtime` or the snapshot layout: it uses
`Runtime::snapshot` / `Runtime::restore` (SPEC 5.9, ADR-022, ADR-023) exactly as they are, ADR-051's session
resume as it is, and a `Log` record (SPEC 5.10) for the one message that has to reach the screen. A client that
does nothing new keeps working against the new server. Constitution R11: decided here, before the code.

## Context

ADR-051 made a dropped client heal itself and made the dev server keep a dropped client's objects for ten
minutes. It left one seam on purpose: after a rebuild the new core has none of the old core's objects, so a
reconnecting client is told **session lost** (close 4001), closes its core, and the app loads a new one and starts
over. The web playground reloads its page; the iOS and Android playgrounds rebuild their screens. Editing Rust
therefore resets the app to its first screen, with its first values, every time. That is the single largest
remaining gap in the loop the plan calls the best there is: the fast path (save, one second, the app is on the new
code) loses the thing the developer was looking at.

Everything needed to keep it already exists:

* `Runtime::snapshot()` encodes every live, non-transient store (`handle`, `type_id`, the plain signals; computed
  signals are recomputed) and carries the highest generation issued (`generation_floor`), SPEC 5.9, ADR-022.
* `Runtime::restore(bytes)` rebuilds each store through its generated `restore`, **re-issues the same handles**,
  raises the process's generation counter above everything the snapshot, or the old process, ever issued (so no
  handle the app still holds can name an object created after the restore), and is all-or-nothing: on an error the
  runtime is unchanged (ADR-023). Restoring into a fresh process is the case ADR-022 was written for.
* The server's retained session (ADR-051) is a plain `(token, handles)` entry; a connection that presents the token
  with `undra_resume=1` adopts the handles and finds them, observes them again, and the core answers each `Observe`
  with the current values (SPEC 5.5).
* The dev runner is a process `undra dev` spawns and talks to over two pipes (a ready line on stdout; stdin closing
  means "stop"). `undra dev` already stops the old runner (Close 1001) before it starts the new one.

What is missing is the part in the middle: something takes the snapshot from the old core at the right moment,
holds it, gives it to the new core before any client can attach, and seeds the new server with the session the
old one held. No client has to be involved: the dev server owns the old core, so it can read the state itself,
which is also why the state survives a client that happens to be offline during the swap.

Measured on the playground core (a `Counter`, a `Todos` and the 10,000-row `BigList`; release build, in process,
best of 20): the snapshot is **209,008 bytes** (204 KiB), `snapshot()` takes **68 us** and `restore()` into a
fresh runtime **189 us**. The existing rows `snapshot/encode_100kb` (13.8 us) and `snapshot/restore_1mb`
(260 us) say the same: the runtime's part is a rounding error; what costs is moving the bytes between two
processes and the quiesce.

## Decision

### 1. When, and how the old core is quiesced

`undra dev` takes the snapshot **after a rebuild has succeeded and the new core has proved it starts, and before
the old core is stopped**: never when the change is detected. While the build runs the old core keeps serving and
the user may keep using the app; the snapshot must be the last state before the swap. A failed rebuild takes no
snapshot and changes nothing (as today).

Sequence, with `old` the serving runner and `new` the runner built from the new sources:

1. **Prepare.** `new` is started in *standby*: it builds its runtime and binds its native ports but does **not**
   listen. It prints `UNDRA-DEV standby <schema-hash>`. If it cannot start (a panic in an init hook, an
   `InitError`) nothing was touched: `old` keeps serving, with its state, and `undra dev` says "the rebuilt core
   did not start; still serving the previous build" (the same sentence as a failed build).
2. **Decide.** If the new schema hash is not the old one, or `--no-keep-state` was given, no snapshot is taken
   (section 3). Otherwise:
3. **Quiesce and snapshot `old`.** `undra dev` writes `snapshot` on `old`'s stdin. The runner calls the new
   `Server::suspend(settle)`:
   1. the server stops accepting connections (the listener is dropped, so the port is free for `new`);
   2. new `Call` frames from the attached client are no longer run (they were not going to be answered by a core
      that is about to go away; the client fails them as `Unavailable` when the socket closes, one step later);
      `Observe`, `Release` and port replies are still processed;
   3. it waits until the calls that are open on the connection have been answered, **at most `settle`** (2 s).
      Streams are not waited for. This is what lets an `async` command that is in the middle of its own port call
      finish instead of leaving its store at a transient value (`loading = true`) that nothing will ever clear;
   4. it closes the client with 1001 (`the core is reloading`), which tears the connection down: calls still open
      are cancelled (`Runtime::cancel`, as for any dropped client; the client fails them as `Unavailable`),
      observations stop, and the objects the client's constructors made are **retained** for its
      return (ADR-051), not released;
   5. it joins the connection threads and **takes the retained session out without releasing it**, returning
      `Suspended { session: Option<KeptSession { token, handles }>, settled, cancelled_calls }`.
   From here no client frame is processed, so the core's stores can no longer change from the outside; the runner
   then calls `Runtime::snapshot()`. (The core's own timers and tasks may still write for the few milliseconds
   until `undra dev` closes stdin; the snapshot is the state at the moment it was taken, which is what the new
   core resumes from.) The runner answers on stdout with one line (section 2). Nothing here reads a clock in the
   core (section 6): `settle` is a host-side deadline in the transport.
4. **Stop `old`**: stdin closes, the runner shuts its runtime down and exits; if it does not within five seconds it
   is killed (today's `Running::stop`).
5. **Hand over and listen.** `undra dev` writes the state to `new` (section 3), then `listen`. The runner binds the
   address (the same one: a port-0 start keeps its port across restarts, as today) and prints
   `UNDRA-DEV ready <url> <hash>`.

The first start of `undra dev`, and a rebuild whose state is not carried, differ from today only in that `new` is
started in standby first.

### 2. The mechanism: one more pair of lines on the pipes the runner already has

The runner's stdin, which until now meant only "stop when it closes", becomes a line protocol; its stdout, which
carries one `UNDRA-DEV ready` line, carries a few more. Nothing else is added: no port, no file, no new envelope
kind, no client involved.

```
undra dev -> runner   (stdin, one line each, UTF-8, "\n")
  snapshot                                        old, serving: quiesce (section 1), snapshot, answer
  state <old-hash> <token|-> <h1,h2,..|-> <hex>   new, standby: restore this snapshot; the session to seed
  reset <reason...>                               new, standby: the state is not carried, and why
  listen                                          new, standby: bind and serve
  (stdin closes)                                  stop, as today
runner -> undra dev   (stdout)
  UNDRA-DEV standby <hash>
  UNDRA-DEV ready <url> <hash>                    (as today)
  UNDRA-DEV snapshot ok <settled 0|1> <open calls> <stores> <bytes> <token|-> <h1,h2,..|-> <hex>
  UNDRA-DEV snapshot failed <reason...>
  UNDRA-DEV restored <stores> <lost objects> <bytes>
  UNDRA-DEV reset <reason...>                     the state was refused (schema, size, the core)
```

Handles are 64-bit hex; the snapshot is hex (a table-free codec that is a few lines on each side and cannot drift;
base64 would save a third of a payload that is 0.2 MB on the playground). The pipes are private to the parent and
child: no other process can read or inject a snapshot, which a loopback port or a file in a temp directory could
not promise.

**Rejected: `--snapshot-on-exit`.** Closing stdin is also how the runner learns `undra dev` died (a Ctrl-C, a
closed terminal); it would pay a snapshot for every exit and could not carry a deadline, a reply or a failure.
**Rejected: a control connection to the runner's WebSocket.** The server admits one client and filters origins;
the envelope has `Snapshot` and `Restore` kinds but no "send me a snapshot" request, and adding one is a wire
change this feature does not need.

### 3. Where the bytes live, and what is carried

The snapshot lives **in the memory of the `undra dev` process only**, from the moment the old runner answers until
the new runner has read it (milliseconds); it is dropped right after. It is never written to disk: a snapshot is
the app's state (a signed-in user, a draft message, a cached list), and a dev machine's temp directory is backed up,
indexed and left behind by crashes. A flag that persists it (to survive a restart of `undra dev` itself) is
possible later and is not part of this ADR.

**Bound: 16 MiB** of snapshot (the playground's is 204 KiB: 80 times room). Over it the runner does not send the
state: `UNDRA-DEV snapshot failed too large: 20.3 MiB, over the 16 MiB limit` and the swap proceeds with fresh state
(section 7). The bound is about the cost of hex lines on a pipe and about a dev loop that must stay quick; a core
whose state is bigger than that is better reset than stalled.

What is carried: the snapshot (every non-transient store: signals, not computeds) and the session `Suspended`
returned. What is not, and why:

| Not carried | Why | What the app sees |
|---|---|---|
| Objects that are not stores, and transient stores (query handles) | SPEC 5.9: not snapshotted | their handles are `stale_handle` (status 5): a call on one fails as `UndraCallError.Refused`, the existing path; the dev server counts them in its line ("2 objects not carried over") and the status bar says so |
| Running tasks, timers, streams, calls in flight | a task is a future, not data | in-flight calls fail as `Unavailable`; streams end with the connection; a store that must keep a background task alive across a reload starts it from its `restore = ".."` hook (which receives the `Ctx`) |
| The query cache, the offline queue | not store state; the queue is persisted through `Kv` by its own rules (ADR-037) | queries refetch when observed again |
| Transaction ids | a new process counts from 1 | no runtime orders change-sets by `txn_id` (checked in the three mirrors); each `Observe` answer is a full value |

### 4. Restore into the new runner, and the schema hash

`new` restores in standby, after its runtime exists and **before it listens**, so no client can be attached to a
half-restored core. The restore command carries the old core's schema hash; the runner compares it with its own
first:

* **equal**: `Runtime::restore(bytes)`. On success the runner prints `restored <stores> <lost> <bytes>` and keeps
  `(token, handles)` with the handles that are not restored stores removed (`lost` counts them);
* **different**: no restore attempt. The restore of a snapshot into changed types is exactly what ADR-037
  documents as dangerous today (a compatible-width change restores wrong values and returns `Ok`), and ADR-037
  (versioned snapshots, migrations by name) is not on `main`. So **a changed hash means fresh state**, and the
  runner answers `reset schema changed (was 0x..., now 0x...)`. When ADR-037 lands, this is the one place that
  changes: attempt the restore through the migration path when the hash differs;
* **refused by the core** (`RestoreError`: a store that cannot decode, an unknown store type, `BAD_SNAPSHOT`, the
  generation ceiling): `Runtime::restore` is all-or-nothing, so the new runtime is untouched; the runner answers
  `reset the core refused the snapshot: <error>` and goes on with fresh state. A panic inside a store's restore is
  contained by the runtime (R6) and arrives here as the same error.

Then `listen`. The new `Server` is built with the session as its **inherited session**: its `Resume` registry holds
`(token, handles)` from the first instant, with a new ten-minute grace. A client that comes back with `token` and
`undra_resume=1` adopts the handles exactly as in ADR-051; a client with another token (the app was relaunched), or
nothing, supersedes it and the restored stores are released, so a dev core still holds at most one launch's objects.
A new `ServerConfig::inherited_session` carries it; `Server::bind` seeds the registry before the accept thread
starts.

### 5. What the clients see

Nothing changes in the protocol: **a reload is, to a client, ADR-051's reconnect plus S15's restore.**

1. `old` closes the socket with 1001. Every runtime goes to `reconnecting(1)` and fails what is in flight with the
   typed `Unavailable`; calls made now fail the same way at once (ADR-051).
2. The retries get "connection refused" until `new` listens (a second or less after the old exit), then the
   server's `Hello`, then the session: `client reconnected ... (N object(s) kept)`.
3. The runtime sends `Observe` for every store signal the app observed (and `Release` for what it released while
   down). `new` answers each with the current values, **the restored ones**: the first change-sets are the old
   core's last state. `connected` is announced; the mirrors converge; the screen is where it was.
4. **Handles.** A constructed handle survives iff the object is a store: `restore` re-issues the same
   `(index, generation)`. The handle of a plain object or a query handle is stale: a call on it is refused with
   status 5, which every generated binding already maps to `UndraCallError.Refused` / `refused`
   (`docs/ERRORS.md`); the app re-creates the object as it would after any `Refused`. Handles created *after* the
   reload are above the restored generations (ADR-022): nothing the app held can alias them.
5. A client that was **offline** during the swap finds the state when it comes back (the session is held for
   ten minutes by `new`, which is why the state is held by the server and not by a client).
6. A client built from other bindings than the new core's (a schema change) gets the server's `Hello` and 1008, as
   always: `closed(schemaMismatch)`. A client whose session was **not** carried (section 7) is told 4001 and
   reloads its core, which is today's behaviour: the fallback for every failure is the loop as it is now.

**The message.** The new server sends one `Log` record with target **`undra::dev`** (level 2, the message is the
sentence to show) to every client that attaches within 30 seconds of its start, once per client (per session
token): a client that **resumed** gets `Reloaded, state kept` (plus `(2 objects not carried over)` when
`lost > 0`); a **new** client (one whose session was not carried and that loaded a fresh core) gets
`Reloaded, state reset: <reason>`. It is a normal `Log` envelope: SPEC 5.10 already says the inspector is a consumer of the same stream, and a client that
ignores it loses nothing. The runtimes route a record whose target is `undra::dev` to one new optional callback
**in addition to** their log sink:

* TypeScript `AttachOptions.onDevNotice?: (message: string) => void`
* Kotlin `LoadOptions.onDevNotice: ((String) -> Unit)? = null` (the last parameter, so existing calls compile)
* Swift `LoadOptions.onDevNotice: (@Sendable (String) -> Void)? = nil`, and the matching `onDevNotice:` of the
  `.remote(...)` factory next to `onConnectionChange:`

This is the whole runtime-side change: the callback follows `onConnectionChange` and `onError` in shape,
threading note and tests. Rejected: reading the log through each platform's own log route (TypeScript's `adapters.log`,
Swift's `Log` port, **Kotlin's `java.util.logging` logger, which the runtime does not let an app replace and which
the JVM holds weakly**): three mechanisms, none of them a contract. Rejected: a new state or event in
`ConnectionState` (a source break for every exhaustive `switch` in apps), and a field in `Hello` (a wire change).

**The three dev status bars** (`dev-banner.ts`, `DevStatus.kt`, `DevStatusBar.swift`, in the playground and in the
`undra init` templates) show the notice for four seconds, over the usual "Dev server: <url>" line, and then go
back. The three `schemaMismatch` texts say "The schema changed, state reset: run undra bindgen and rebuild the
app" (a mismatch can only follow a reload that could not keep state). `undra dev` prints the same facts:

```
Restarted: ws://127.0.0.1:7443  (schema hash 0x...); state kept (3 stores, 204 KiB, 2 objects not carried over)
Restarted: ws://127.0.0.1:7443  (schema hash 0x...); state reset: schema changed (was 0x...). Run `undra bindgen`, ...
```

(The existing prefix `Restarted: ws://` is kept: tests and scripts wait for it.)

### 6. Determinism and R12

`Runtime::snapshot` and `Runtime::restore` are functions of the core's state; the payload carries no time and no
randomness; nothing the core does depends on a clock because of this feature. The deadlines (`settle`, the
five-second stop, the thirty-second notice window, the ten-minute grace) are in the host-side transport and in
`undra dev`, both of which already own threads and timers (R12 puts those outside the core). The restored core's
`Clock` and `Rng` ports are the runner's native ones, as before. The runner reads a clock to print durations in
its log lines, as it already does for its log records; the core does not.

### 7. Failure matrix

| What fails | What happens | State | Shown |
|---|---|---|---|
| The rebuild | unchanged: the old core keeps serving; **no snapshot is taken**, no standby is started | kept (nothing happened) | "the rebuild failed; still serving the previous build" |
| The new runner does not start (standby never prints) | the old core keeps serving; nothing was quiesced | kept | "the rebuilt core did not start; still serving the previous build" + its output |
| The schema hash changed | no snapshot; `reset`; the old runner is stopped as today | fresh | terminal: "state reset: schema changed"; app: `schemaMismatch` "The schema changed, state reset: ..." |
| `--no-keep-state` | as above, `reset` with that reason | fresh | terminal; notice to the reloaded app |
| The old runner does not answer `snapshot` within 15 s, dies, or the snapshot is over 16 MiB | log it; swap with fresh state: `old` is stopped, `reset <why>` | fresh | terminal; notice to the reloaded app |
| A snapshot is partial (a store's encode panicked; the runtime logs it and skips the store) | the other stores are restored; the skipped store's handle is stale | partial | `lost` counts it; the runtime's error is in the output |
| The new core refuses the restore | `Runtime::restore` left it untouched; fresh state | fresh | terminal; notice |
| `new` dies between standby and listen | as any runner that dies: "the dev server stopped" | lost | as today |
| Binding the address fails (another process took it) | as today: `C0013` | lost | as today |
| The client is offline during the swap | ADR-051: it reconnects when it can and finds the state (10 min) | kept | notice on return |
| The client had no session token | its objects are released at the close, so they are not in the snapshot | fresh for it | it loads afresh (4001), as today |
| A call is open and does not finish in `settle` | cancelled with the close; a store it half-wrote keeps that value (below) | kept, possibly transient | `cancelled_calls` in the log line |

"Fresh state" always means **today's behaviour**, and the clients take it through the paths ADR-051 already
defined (4001 → load a new core; 1008 → schema mismatch). The feature can only improve on the loop as it is.

## Alternatives considered

* **A client-driven snapshot** (the client asks, or sends its mirror back): every platform would implement it, a
  client that is offline or backgrounded during the swap loses the state, and a mirror is not the core's state
  (computed signals, unobserved signals, private state). Rejected.
* **Persisting the snapshot to disk** (survives a restart of `undra dev` itself, and a crash of the runner):
  the bytes are application data; a file is a persistent copy nobody asked for. Rejected as the default; a flag may
  add it later.
* **Do nothing** (today): every save resets the app to its first screen.
* **Snapshot when the change is detected.** The old core keeps serving during the build; the user keeps tapping;
  the snapshot would miss everything after it. Rejected.
* **Restore after listening.** A client that attaches between `listen` and `restore` would observe fresh stores
  whose handles the restore then replaces, and `restore` cancels calls in flight (ADR-023). Rejected: standby.
* **Stop the old runner first, then start the new one (today's order).** A new core that cannot start would
  take the old state with it. The standby step costs one extra process start and removes the failure.
* **Make the quiesce reversible so a failed `new` could resume `old`.** The listener has to be released for `new` to
  bind; re-binding it in `old` is a second code path that is rarely exercised. Standby makes it unnecessary.
* **Always resume, or fall back to 4001 when any retained object is not restorable.** Falling back would turn
  one live query handle into a full reset. Resuming loses the query handle only. Recommended: resume; say so.
* **Restore across a changed hash on a best-effort basis.** The wrong-value hazard of ADR-037 is silent. Rejected
  until migrations exist.

## Consequences

* Editing Rust no longer loses the screen: the counter keeps its value, the to-do list its items, a 10,000-row list
  its rows, across the swap, on web, iOS and Android, with no change to an app that already follows ADR-051
  except the optional `onDevNotice` of its status bar.
* The cost is a second runner process at the moment of the swap (standby) and two pipes' worth of hex: for the
  playground 209 KB of snapshot is 418 KB on a pipe each way, well under a millisecond of copying beside the
  process start; the end-to-end swap time is measured after the implementation and recorded with the piece.
* A state that was reached by old *logic* is restored into new logic. That is what a reload is, and is the one thing a
  developer has to know: relaunch the app (a new session supersedes the carried one and the app builds fresh
  stores on the new code), or run `undra dev --no-keep-state`.
* **Known limits, stated plainly.** (a) A call that was cancelled by the swap can leave a store at a transient value
  (`loading = true`); the `settle` wait makes it rare, not impossible; re-trigger the action. (b) Tasks and timers
  do not survive: a store whose `running` signal is true because a task it started was running comes back saying
  `running` with no task, unless its restore hook starts one. (c) Query handles and plain objects need re-creating
  by the app; a screen that holds a live query handle keeps its last values but cannot refetch until it is
  re-mounted. (d) A schema change is still a reset; ADR-037 is what will change that.
* New public API: `ServerConfig::inherited_session`, `ServerConfig::attach_notices`, `Server::suspend`,
  `Suspended`, `KeptSession` (undra-transport); `onDevNotice` in three runtimes; `undra dev --no-keep-state`.
  SPEC 5.9 (a "dev reload" paragraph), 5.10 (`undra::dev` records), 11.0 and 17 are updated with the code.
* Cross-merge notes for Track A (`runtime-lifecycle`): this ADR changes **no** runtime code and no Kotlin `close()`
  path; the runtime-side edits are an additive callback and a four-line branch in each `onLog`. `Runtime::shutdown`
  is called by the runner exactly as before, after the snapshot.

## What the contracts and tests prove (R4)

* **`undra-transport`** (real server, real sockets): `suspend` ordering (a call sent after the freeze is not run;
  a call open at the freeze finishes within `settle` and its effect is in the snapshot; one that does not is
  cancelled and reported; the listener is closed afterwards; the retained session comes back un-released and is
  adoptable by a new server through `inherited_session`); the attach notices (once, to the right kind of client,
  not after the window).
* **`undra-cli`** (the real `undra dev` on a copy of the playground, a raw WebSocket client speaking the envelope,
  as `tests/dev.rs` does): change a counter, touch a source file, wait for the rebuild, reconnect with the
  session, observe, and read the same counter value as the first change-set; the same with a schema change
  (a new field): fresh values and "state reset"; a broken edit and a core that does not start leave the old one
  serving with its state. Unit tests for the runner protocol lines and for the swap's decisions and step order.
* **TypeScript runtime over the real WebSocket** against `undra dev` on the playground (a node script, the shape of
  `crates/undra-transport/interop/ts.mjs`), and the three runtimes' unit tests for `onDevNotice`.
* **Contract scenarios: none new.** What a client sees is ADR-051's reconnect (covered by its own tests in each
  runtime) followed by the change-sets S15 already specifies for a restore: "the same handles still work ... delivered
  as change-sets for the observed signals" and "a plain object's handle is refused". The only new client-visible
  item is a `Log` record and its callback, which the per-runtime unit tests pin. A contract step would test the
  same code twice.
* **On devices:** the playground on the `undra` Android emulator (remote mode) and on the iPhone 17 Pro simulator:
  change a counter, edit the Rust, the bar says "Reloaded, state kept" and the counter has not moved.
* **Budgets (R9):** no hot path or boundary entry changes; the runtime's snapshot and restore are already gated
  (`snapshot/encode_100kb`, `snapshot/restore_100kb`, `snapshot/restore_1mb`); the swap's end-to-end time is
  reported in the piece's record, not gated (it is dominated by the process start).

## Decisions (2026-10-01, the integrator)

1. **`onDevNotice` is accepted**, with three constraints. (a) Only `undra dev`'s server emits a `undra::dev` record:
   in-process and production cores never produce one, so each runtime dispatches `onDevNotice` **only from its
   `remote` transport** (a record with that target arriving through an in-process core's `Log` port is an ordinary
   log line), and the option is documented as dev-only and inert otherwise; a test per runtime asserts that an
   in-process core never fires it. (b) The notice goes **once to every client that attaches within the window**, not
   only the first (a simulator and an emulator are often attached together, taking turns on the one slot): the server
   remembers which session tokens it told, so a client that reconnects again inside the window is not told twice;
   a client with no token is told per connection. (c) The option is additive and **last** in every signature, so
   generated code and existing call sites do not move.
2. **Resume anyway and show `(N objects not carried over)`.** The stale handles keep today's status 5 path.
   `docs/DEV_LOOP.md` says what an app does with a stale query handle after a reload: run the query again (re-mount
   the screen, or construct the query again).
3. **`undra dev --no-keep-state`: yes.**
4. **The limits stand**: 16 MiB of snapshot, 2 s of `settle`, 30 s of notice window. The cap and the settle are named
   constants with a one-line reason each. Over the cap the swap falls back to fresh state and the notice says
   `Reloaded, state reset: snapshot over 16 MiB`.

Also required of the evidence: the device proof shows the notice text on the Android dev bar (the `undra` AVD,
shared: used, never killed) and on the iOS dev bar; the CLI integration test asserts the counter value after a
reload and `state reset` after a schema change; the runtime-side diff stays additive (Track A touches
`UndraCore.kt`, `core.ts` and `UndraCore.swift`).

## As built (2026-10-01)

What differs from the text above, and what was measured.

* **Measured, playground (a `Counter`, a `Todos` and the 10,000-row `BigList`)**: snapshot 209,008 bytes (204 KiB);
  `Runtime::snapshot` 68 us and `restore` into a fresh runtime 189 us (release, in process, best of 20); inside the dev runner
  (a *debug* build of the core) `restore` takes 1.6 ms, which `undra dev` prints (`restored in 1.6 ms`). From the old core
  closing its client to the new core listening: 74 ms (the dev server's own log). What an app sees: the web page was
  `reconnecting` for 171 ms; the iOS simulator's client was away 0.1 s, the Android emulator's 1.0 s (the Kotlin client's
  backoff, 250 ms at least, and its jitter, not the swap). An incremental rebuild of the playground core is 0.5 s.
* **A fix the ADR did not foresee, found on the Android emulator.** The Kotlin `RemoteTransport` dropped a frame that arrived
  right behind the server's `Hello`, because `handshakeDone` and `current` were set by the thread that waits for the Hello, not
  by the thread that reads it: the dev notice (the first frame after the `Hello` of a resumed session) never reached
  `onDevNotice`. The reader thread now sets `handshakeDone` when it reads the `Hello` and a connection being opened counts as
  current for dispatch. A test fails without the change (20 connects, a `Log` frame right behind each `Hello`). TypeScript
  and Swift set the flag on the reading thread and were not affected. SPEC 11.0 says a client must process such a frame.
* **The cap and the settle are named constants in `crates/undra-cli/src/reload.rs`** (`STATE_LIMIT_BYTES`, `SETTLE`,
  `NOTICE_WINDOW`) and are written into the generated runner; `UNDRA_DEV_STATE_LIMIT_BYTES` can only *lower* the cap (the
  integration test proves the over-the-limit path with it).
* **The TypeScript runtime over a real WebSocket** was run against `undra dev` on the playground in a browser (the playground
  page, Vite dev server, a real `UndraCore.load({ mode: "remote" })`), not from a node script: the counter kept its value, the
  bar said `Reloaded, state kept (1 object not carried over)`, and a `refetch` on the stale query handle was refused with
  `UndraCallError.Refused` (the status 5 path). The automated tests of the client side are in each runtime's suite; the
  CLI integration tests speak the envelope with a raw client.
* **`Server::suspend` hands a session over only when `resume_grace` is above zero** (as `undra dev` has it); with zero nothing
  is retained and the client's objects are released at the close.

## Review amendments (2026-10-02, the adversarial review)

The review (`.10x/reviews/2026-10-02-dev-reload-review.md`) changed four things and added tests; none touches the envelope, a payload,
the ABI, the schema, the generated code or `undra-runtime`.

* **A call the reload cut off is counted, and the notice says so.** Section 1.3.2 stops running calls during the settle;
  such a call was neither answered nor counted, and a *command* that fails as `Unavailable` while the connection is down is
  only logged (ADR-051), so a tap made during the swap vanished behind `Reloaded, state kept`. `Suspended` gains
  `dropped_calls` (calls the client sent after the server stopped running calls, including those that arrived after the
  Close was queued); the runner's answer is `snapshot ok <settled> <cancelled> <not run> <stores> <bytes> <token|->
  <handles|-> <hex>` and the hand-over is `state <old-hash> <lost calls> <token|-> <handles|-> <hex>`; the terminal line
  adds `N calls sent during the reload were not run` and the resumed notice reads `Reloaded, state kept (N calls lost in
  the reload)` (with the objects not carried over, `(1 object not carried over; 2 calls lost in the reload)`). The
  semantics are unchanged: those calls are not run and the client fails them as `Unavailable` at the close; the state is
  the state before them.
* **`undra::dev` is reserved for the server.** A core that logged under that target (`undra_info!(target: "undra::dev",
  ..)`) reached `Host::log` and so every client, and the runtimes fired `onDevNotice` for it: the core could make the dev
  bar say anything, contrary to decision 1(a). The bridge no longer forwards such a record to a client (the log sink, the
  terminal, still prints it); the notices are sent by `Session::tell`, which does not go through `Host::log`.
* **The runner's stdout is read in bounded lines** (48 MiB, three times the state limit) and need not be UTF-8: before,
  one line was read without bound, and a non-UTF-8 byte from the core's own `print!` ended the reading, which `undra dev`
  took for the runner exiting ("the dev server stopped"). A `snapshot` answer that does not parse, or is over the bound, is
  a failed snapshot at once (fresh state, with the reason) instead of a megabyte line printed to the terminal and a 15 s
  wait; a protocol line glued to the core's `print!` output without a newline is still recognised; and `undra dev` checks
  the 16 MiB limit itself before it decodes the hex.
* **A call is decided under the connection's lock.** The reader checked "frozen" and then recorded the call in two
  steps, while `suspend` set the flag and then counted the open calls: a call recorded between the count and the Close
  frame ran without being waited for (a sync write could land in the snapshot and lose its reply; an async one was
  cancelled without being counted). `Conn::begin_call` now reads the flag under the lock the count takes, so every call
  is either waited for (answered before the Close, or cancelled and counted) or not run (counted).
* **Tests added for the failure matrix with real cores**: a rebuilt core that exits at start (an init hook that calls
  `exit`), a restore the new core refuses (a restore hook that panics, the same schema hash), a second save during a
  reload (two swaps, the state carried twice, the second time from a core whose client had not come back), the generation
  floor across the process boundary, an inherited session whose grace passes, a client back after the notice window, two
  clients, and 16 MiB of state through real pipes both ways.
