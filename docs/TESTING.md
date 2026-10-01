# Testing your app

Undra ships a testing kit for the three platforms and for Rust. It does three things:

* **`PreviewCore`**: your app's own core, loaded in the process with deterministic fakes as its ports and a manual clock. A SwiftUI
  `#Preview`, a Compose test, a Storybook story or a unit test runs the real logic against a server, a store and a clock that you script.
* **`RecordedCore`**: a recording of a session, played under the generated stores with no core at all. For states that are expensive to reach.
* **Record and replay of port traffic**: record what a session asked the platform (HTTP, storage, files) and what it was answered, then replay it
  deterministically in a test.

Nothing in the kit touches the wire, the C ABI or a generated shape (SPEC 17.5; ADR-055). Generated code is the generated code; the kit sits
beside the runtimes.

| | Swift | Kotlin | TypeScript | Rust |
|---|---|---|---|---|
| Package | `UndraTestKit` (a product of the `UndraRuntime` package) | `dev.undra:testkit` | `@undra/testkit` | `undra::testing` (`undra-testkit`) |
| Real core + fakes | `PreviewCore` | `PreviewCore` | `PreviewCore` | `Harness` |
| A recording as a core | `RecordedCore` | `RecordedCore` | `RecordedCore` | |
| Record port traffic | `PortRecorder` | `PortRecorder` | `PortRecorder` | `Recorder` |
| Replay it | `Replayer` | `Replayer` | `Replayer` | `Replayer` |
| The fakes | `Fakes` | `Fakes` | `createFakes()` | `undra::ports::fakes` |

## Which one to use

| You want | Use |
|---|---|
| A screen in a state that is slow to reach (a long list, a deep flow) | `RecordedCore`, or script it with `PreviewCore` |
| The screen to react (add an item, see it appear) | `PreviewCore` |
| The cache to go stale after thirty seconds | `PreviewCore` and `advance` |
| A test that fails when the core makes a different request than it did | `Replayer` |
| A preview in Android Studio | `RecordedCore` (its preview pane runs on a desktop JVM, which cannot load the app's native core) |

## `PreviewCore`

```swift
// Swift: a SwiftUI preview
#Preview("Todos, three items") {
    let preview = try! PreviewCore.load(expectedSchemaHash: UndraIds.schemaHash, seed: seed)
    let todos = try! Todos(ctx: preview.core)
    return TodosScreen(todos: todos)
        .task { for title in ["Buy milk", "Walk the dog"] { _ = try? await todos.add(title: title) } }
}
```

```kotlin
// Kotlin: an instrumented or JVM test
val preview = PreviewCore.load(UndraIds.SCHEMA_HASH, Seed.fromJson(seedJson))
val todos = Todos(preview.core)
preview.fakes.http.respond("https://api.test/todos", httpResponse(200, "[]"))
preview.advance(31_000)       // the cached list goes stale; the core refetches
```

```ts
// TypeScript: a Storybook story, a Vitest test
const preview = await PreviewCore.load({ wasm: "/undra_core.wasm", expectedSchemaHash: UndraIds.schemaHash, seed });
const inbox = await RemoteTodosQueryHandle.create("inbox", preview.core);
await preview.advance(31_000);
```

What it installs: `FakeClock` (the `Clock` and the `Timer`), `SeededRng`, `FakeHttp`, `MemKv`, `MemSecureStore`, `MemFs`, `CaptureLog`, and two scripted
event sources for `Connectivity` and `Lifecycle` (`preview.fakes.connectivity.goOffline()`). They behave exactly as the Rust fakes in
`undra::ports::fakes` do, and a checked-in conformance file keeps it so (see "The fakes").

* **The clock.** `preview.clock.nowMs` reads it, `setNowMs(_)` jumps the wall clock, `advance(ms)` moves time forward one deadline at a time:
  at each deadline the clock reads exactly that instant, the due timer fires into the core, and the core settles before time moves on.
  On the web, `ctx.sleep` in the core follows this clock. **In a native core (Swift, Kotlin) it does not**: the runtime runs its own `ctx.sleep`
  on its timer thread in real time (the C ABI never hands sleeps to the host, SPEC 5.8). The manual clock there moves what the core *reads*
  (`Clock`: staleness, timestamps) and timers armed through the `Timer` port. A delay a native core sleeps through is waited for, not advanced.
* **Settling.** The core runs on its own thread (native) or after microtasks (web), so a preview waits for it to be idle: `settle()` and every
  `advance` do. "Idle" is observed (the core's counters stand still and no port call is pending), so raise the quiet window on a busy machine.
* **One core per process.** `load` shuts down the shared core first (a refreshed preview does the same) unless `replaceCurrent` is off.
* **Loading order.** The core's start-up work (the query cache reads the `Kv` port) runs concurrently with the host registering its ports in Swift
  (SPEC 6: a port registered after `undra_init` is racy against the hooks). The kit registers the stores first; do not rely on a persisted cache
  being hydrated in a preview. Seed the data through the `Http` fake instead.

### The seed

One JSON document seeds the fakes in every kit and in Rust (`testkit/fixtures/seed.json` is an example):

```json
{
  "version": 1,
  "now_ms": 1700000000000,
  "rng_seed": 42,
  "kv": { "greeting": "hello", "blob": { "hex": "00ff" } },
  "secure_store": { "token": "t-123" },
  "fs": { "notes/a.txt": "hello" },
  "http": [
    { "url": "https://api.test/lists/inbox/todos", "method": "get", "status": 200,
      "headers": [["content-type", "application/json"]], "body": "[]" },
    { "url_prefix": "https://api.test/slow", "error": "timeout" }
  ],
  "connectivity": { "online": true, "kind": "wifi" },
  "lifecycle": "active"
}
```

A string value is its UTF-8 bytes, `{"hex": ".."}` raw bytes. HTTP rules are tried in order, the first match answers; an `error` is `"timeout"`,
`"cancelled"`, `{"network": ".."}` or `{"invalid_url": ".."}`. Every key is optional. A malformed seed is a typed error naming the path (`http[1].status`).
Each kit also builds seeds natively (`Seed(...)`, `createFakes()` and `fakes.http.respond(..)`).

## `RecordedCore`

```swift
let recorded = try RecordedCore.load(Recording(json: text), expectedSchemaHash: UndraIds.schemaHash)
let todos = try Todos(ctx: recorded.core)      // the recorded constructor reply
recorded.advance(ms: 300)                      // the change-sets up to 300 ms into the session
```

The generated stores, the mirror and `UndraCore` are the real ones; the transport under them plays the recording.

* **State.** Recorded change-sets are released by a manual playhead (`advance(ms)`, `playAll()`; it only moves forward, load again to start over).
  The playhead starts at the time of the first recorded change-set, so observing a store shows the state the recording first saw
  (`startAtMs` chooses another). A store observed late is brought up to the playhead: everything recorded for it so far, in order.
* **Calls.** A constructor, method, function or page call is answered by the next recorded reply for the same target, in order (by default
  whatever the arguments; `exact` compares them). The reply's call id is rewritten to the live call. A recorded stream plays its items. A call the
  recording has no (more) reply for is a typed refusal that names it (`UndraCallError.refused`); `repeat_last` answers again with the last
  recorded reply instead.
* **Schema.** A recording of another schema is refused with `UndraSchemaMismatchError`, like any core.
* **Not replayed.** Port traffic, events and timers (a recorded core has no ports), snapshots, and interaction with no recording. For an app that has to
  react, use `PreviewCore`.

## The recording format

One JSON document, `undra.recording`, version 1. The same shape holds change-sets and port traffic; a reader uses the events it needs.

```json
{
  "format": "undra.recording",
  "version": 1,
  "schema_hash": "0xefd907be3070520a",
  "source": "dev-server",
  "platform": "ios",
  "events": [
    {"t":100,"kind":"call","target":"constructor","type":4125302698,"method":898850282,"call":1,"args":""},
    {"t":100,"kind":"reply","call":1,"status":"ok","body":"0000000001000000"},
    {"t":100,"kind":"observe","handle":"0x0000000100000000","signal":4294967295,"on":true},
    {"t":100,"kind":"change_set","txn":1,"entries":[{"handle":"0x0000000100000000","signal":0,"op":"full","value":"00000000"}]},
    {"t":120,"kind":"port_call","port":515815688,"method":1796529958,"call":3,"args":"..","name":"Http.request"},
    {"t":160,"kind":"port_reply","call":3,"status":"ok","body":".."}
  ]
}
```

* `t` is whole milliseconds since the session started. Payloads are lower-case hex of the bytes SPEC 3 defines; handles are `"0x.."` strings (a u64 does
  not fit a JavaScript number); ids are numbers. `schema_hash` is the schema the bytes belong to. `source` and `platform` are informational.
* Events: `call` (`target`: `function`, `method`, `constructor`, `page`), `reply` (`status`: `ok`, `error`, `panic`, `cancelled`, `stream_opened`,
  `bad_request`), `change_set` (entries `{handle, signal, op: full|patch|lazy_invalidated, value}`), `stream_item` (`flag`: `item`, `end`, `error`,
  `failed`), `port_call`, `port_reply` (`status`: `ok`, `error`, `unavailable`), `event`, `timer_fired`, `observe`, `release`, `cancel`.
  The ids of a port call are authoritative; `name` (`"Http.request"`) is only there for the standard ports, for the person reading the file.
* **Canonical.** Fixed key order, one event per line, no wall-clock time, no host paths: equal sessions are equal bytes. Every kit's writer is held to
  the same bytes by `testkit/fixtures/*.json` (read, write, compare), so a recording made on one platform is read by all of them.
* A reader refuses another `format` or `version` with a typed error that names the field and the event.

## Capturing a session

**`undra dev --record <file>`** is the cheapest capture. The dev server sees every envelope in both directions for iOS, Android and web alike, so one
tap records the whole session: calls, replies, change-sets, stream items, port calls and replies, events, timer reports, observes. The dev runner also
wraps its own `Clock` and `Rng` (they are native bindings that never cross the socket), so their readings are in the file. A reload starts a new core
and so a new file: `<file>`, then `<name>-2.<ext>`, `<name>-3.<ext>` (a recording belongs to one schema hash). The file is rewritten twice a second
while it grows and once more when `undra dev` stops.

**`PortRecorder`** (all three runtimes) wraps the real adapters in a session that is not a dev one (a device, a test) and records port traffic only:

```swift
let recorder = PortRecorder(schemaHash: UndraIds.schemaHash, platform: "ios")
let core = try UndraCore.load(.inproc(adapters: recorder.wrap(.platformDefault), expectedSchemaHash: UndraIds.schemaHash))
// ... use the app ...
try recorder.toJSON().write(toFile: "session.json", atomically: true, encoding: .utf8)
```

**In Rust**, `Recorder` is a sink for envelopes (`record_envelope(kind, payload)`) and a `Host` decorator (`TestRuntime::with_host(config, |h| recorder.host(h))`),
so a test that plays the platform records the whole session, which is how `testkit/fixtures/session-todos.json` was made
(`examples/playground/core/tests/testkit.rs`; `UNDRA_BLESS=1 cargo test -p playground-core --test testkit` rewrites it).

## Replaying port traffic

`Replayer` answers a core's port calls from a recording. The recording's `port_call` events, per port, are the script. A call that is the next
recorded one of its port (same method and, by default, the same arguments) gets the recorded reply; anything else is a typed error:

| Error | Meaning |
|---|---|
| `mismatch` | the core called another method, or the same method with other arguments, than the recording's next call of the port |
| `exhausted` | the core called a port more often than the recording did |
| `unconsumed` | the replay ended with recorded calls the core never made |

A deviation consumes nothing (one wrong call does not shift every later answer) and is answered "unavailable". `finish()` throws with every error of
the run; `errors()` reads them as they happen. `ArgsPolicy.ignore` matches on port and method only, for arguments that change between runs. Time is
not replayed: answers are immediate, and the run is deterministic. In Rust:

```rust
let replayer = Arc::new(Replayer::new(&Recording::from_json(&text)?));
replayer.install(t.host());                 // the RecordingHost's default port script
// ... run the core ...
replayer.finish().expect("the core made the calls it made when it was recorded");
```

## The fakes

`undra::ports::fakes` is the reference. A core behind the C ABI or wasm cannot have Rust fakes installed in it (there is no entry for that, and a
new one would be an ABI change), and its ports are implemented by the host anyway, so each kit carries the same fakes natively. They are held
together by `testkit/conformance/fakes.json`: scenarios (seeded random bytes, timers firing in deadline order, key order by UTF-8 bytes, the file
system's error semantics, the HTTP matcher) run against the Rust fakes, and the file records what they answered. Each kit replays it against its own
fakes in its test suite. The Rust test fails when the checked-in file is stale; `UNDRA_BLESS=1 cargo test -p undra-testkit --test conformance`
rewrites it, and the three kits then fail until they agree.

## In the playground

The playground uses the kit through public APIs only (constitution R10):

* `ios/PlaygroundApp/Previews/ScreenPreviews.swift`: three SwiftUI `#Preview`s (a recording played to the end, the real core on scripted ports, the
  remote list answered by the seeded server). Built with the app.
* `android/app/src/debug/kotlin/.../ScreenPreviews.kt`: two Compose `@Preview`s over `RecordedCore` (the same recording at the end and at 400 ms).
  `dev.undra:testkit` and the fixtures are debug-only.
* `web/src/stories/Todos.stories.tsx`: four stories in Component Story Format (Storybook loads them unchanged; the kit does not depend on it),
  rendered by `stories.html` with Vite alone: `npm run dev`, then `/stories.html`. The playground's `npm run build` builds the page.

## Limits

* A native core's `ctx.sleep` runs in real time (see "The clock").
* `RecordedCore` replays what was recorded; it does not simulate the core. It does not rewind.
* A recording of a dev session holds no timer firings of the core's own (the runner's core owns its timers); the `Clock` readings are there.
* `undra dev --record` writes every `Clock` and `Rng` reading of a core that polls them often: use it for sessions, not benchmarks.
* Link the kit into debug builds. It is dead code in release, and it ships nothing the runtime does not.
