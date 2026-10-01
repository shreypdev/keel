# ADR-034: The dev clients reconnect, and `undra dev` keeps a dropped client's objects for it

Status: Accepted (implemented on `wt/dev-loop`). Touches the dev/remote transport contract
(`docs/SPEC.md` section 11, `undra-transport`'s README) and the public API of the three platform
runtimes (a connection state and a reconnect policy on `UndraCore`; SPEC section 17). It does **not**
touch the envelope (3.2), any payload, the C ABI, the wasm ABI, the schema or generated code: a client
that does nothing new keeps working against the new server, and a new client keeps working against an
old one (it just cannot resume). Constitution R11: decided here, before the code.

## Context

`undra dev` serves the core to a client over a WebSocket. Until now a client that lost its socket was
done: every call in flight failed, the core object was closed, and the developer relaunched the app
(`undra dev` even prints "reload the app to reconnect" after each rebuild). Three things make that
the wrong default for a dev loop that is meant to be the best there is:

1. Sockets die for reasons that have nothing to do with the code: a laptop sleeps, a phone leaves the
   Wi-Fi, `adb reverse` is re-established, an app is backgrounded and the OS reaps its socket.
2. The server tears down a client's objects when its socket closes (`release_on_disconnect`, so that
   a dev core that outlives many app launches does not keep every launch's stores for ever). So even a
   client that reconnected would find its handles gone: reconnecting without keeping the objects is
   not healing, it is a faster way to a broken app.
3. Android has no remote mode at all, and not for want of wiring: the Kotlin `RemoteTransport` is
   built on `java.net.http.WebSocket`, which Android does not have. (The playground's manifest also
   has no `INTERNET` permission and no cleartext policy, and `undra dev` printed no Android address.)

## Decision

### 1. Client: reconnect, with a visible state

Each runtime's `remote` transport reconnects by itself, by default, with exponential backoff and
jitter, and tells its core what the connection is doing.

* **Backoff.** Attempt `n` (from 1) waits `min(maxDelay, initialDelay * 2^(n-1))`, then a random
  factor in `[1 - jitter, 1]` of that. Defaults: `initialDelay` 250 ms, `maxDelay` 5 s, `jitter` 0.5,
  no attempt limit (`maxAttempts` optional). A policy of "off" restores the old behaviour. Each
  attempt gets `min(handshake timeout, 5 s)`. Every runtime takes its random source and its timer
  from injectable seams so the schedule is a unit test, not a sleep.
* **State.** `UndraCore` exposes `connecting`, `connected`, `reconnecting(attempt)` and
  `closed(reason)`, where the reason is `requested` (the app closed the core), `schemaMismatch`,
  `sessionLost` or `failed`. TypeScript: `core.connection`, a `Signal` (plus `onConnectionChange`);
  Kotlin: `connectionState`, a `StateFlow`; Swift: `connectionState`, an `@Observable` `connection`
  object for SwiftUI and an `AsyncStream`. `LoadOptions` of each also takes a change callback, so an
  app sees `connecting` and every later change from the first one.
* **What a drop does.** Everything in flight fails at once with the typed outcome each runtime
  already has for a lost connection (TypeScript `UndraTransportError` `closed`; Kotlin
  `UndraException`; Swift `UndraCallError.unavailable(.connectionLost)`); nothing waits for a
  reconnect. A call made while disconnected fails the same way, immediately. Streams end with it.
* **What a reconnect does.** After the handshake the runtime re-sends `Observe` for every store
  signal the app had observed and `Release` for every handle released meanwhile. The core answers
  each `Observe` with the current values as one change-set (SPEC 5.5), so every mirror converges
  without the app doing anything.
* **When it stops.** The app closing the core, a schema-hash mismatch on the server's `Hello` (the
  existing `UndraSchemaMismatch` error, reported once, as `closed(schemaMismatch)`), and a lost
  session (below) are terminal: no retry, no loop. Everything else is retried for ever.

### 2. Server: a dropped client's objects wait for it

The client puts a random token in the URL of its WebSocket upgrade, the same on every connection of
one `UndraCore`: `ws://host:7443/?undra_session=<token>`. On a reconnect by a client that holds at
least one constructed object it adds `&undra_resume=1`. (A client that holds nothing has nothing to
resume and reconnects like a new one.) The token is URL material, not envelope material: the wire
does not change, and a server or client that ignores it behaves exactly as before.

`ServerConfig::resume_grace` (default zero: off, today's behaviour; `undra dev` sets ten minutes)
changes what teardown does for an attached client that sent a token:

* its observations are stopped as before, but its constructed objects are **retained**, not released;
* they are released when `resume_grace` passes, when a client with another token (or none) attaches,
  or when the server shuts down, so a dev core still holds at most one launch's objects;
* a connection that presents the retained token with `undra_resume=1` **adopts** them: they become
  its own (so its own disconnect retains them again, and a `Release` releases them), and the server
  logs `client reconnected`;
* a connection that asks to resume a token the server does not hold (the core was restarted by a
  rebuild, or the grace passed) is answered with the server's `Hello` and then closed with code
  **4001** `session lost: ...`; the server logs why. The client reports `closed(sessionLost)`: its
  handles belong to a core that no longer exists, and a new `UndraCore` has to be loaded (the web
  playground reloads its page; a native app rebuilds its root). Until B3 (a state-preserving reload)
  lands this is also what a rebuild looks like to a client, and the server's `Hello` is still the
  schema check, so a rebuild that changed the schema is `closed(schemaMismatch)` first;
* a connection that presents the token of the attached (but silent) connection takes the slot over: the
  stale socket is aborted at once instead of holding the client slot for the keepalive's 15 seconds.

`undra dev` stops the old runner with a Close frame (1001) before it starts the new one; a client
treats that like any other drop.

### 3. Android

The Kotlin runtime's WebSocket client is its own, over `java.net.Socket` (RFC 6455: the handshake,
masked client frames, fragmentation, ping and pong, close), replacing `java.net.http`. It runs the
connect and every read and write on threads of its own, so `UndraCore.load` works from Android's main
thread (it blocks it for a connect, like any dev-only synchronous call) and `observe`, `release` and
`call` never touch a socket on it. It sends a ping when the server has been silent, so a server that
vanished without a FIN is noticed. `undra dev` prints the emulator address (`10.0.2.2`) and the
`adb reverse` command, and `--android` runs `adb reverse` for every attached device.

## Alternatives considered

* **A `Resume` envelope kind or a field in `Hello`.** The cleanest signal, and a wire change: a new
  kind in every codec and golden vector, or a longer `Hello` that old decoders refuse. The URL token
  and a WebSocket close code carry the same information with no change to the wire.
* **Always retain, resume by order.** Treating the first message of the next client as the answer
  (an `Observe` of a retained handle means resume, a constructor means a fresh launch) needs no token
  and breaks on a resumed client whose first message is a free-function call.
* **Reconnect without retention.** Rejected in the context: the handles would be dead on arrival.
* **Queue calls while reconnecting.** A queued call hides an outage and replays a side effect the
  developer may no longer want. Failing fast with the typed outcome is what the apps already handle.
* **Reconnect in `UndraCore` instead of the transport.** The transport owns the socket and the
  timer; the core owns what is replayed (the observed set, the live handles). Each does its half.
* **A hand-written client only on Android.** Two WebSocket clients to keep correct; the JVM tests
  would stop covering what Android runs.

## Consequences

* The dev loop heals a dropped socket without a relaunch, on all three platforms; Android joins them.
* After a rebuild the client reconnects to a new, empty core and reports `closed(sessionLost)` instead
  of failing every call with a stale handle. B3 (snapshot before the rebuild, restore after, then
  seed the new server's retained session) is what makes that transparent; this ADR leaves the seam
  for it: the server's retained session is a plain `(token, handles)` entry.
* `release_on_disconnect` keeps its meaning for clients that send no token.
* The server keeps at most one retained session, and `undra dev` serves one client at a time as
  before; two clients taking turns lose each other's objects, as they already did.
* New public API in three runtimes (the state, the policy, a session-lost error): SPEC 17 is updated.
