# Undra Kotlin runtime

The JVM/Android runtime for Undra (`docs/SPEC.md` §11, §17.2). Dependencies: Kotlin stdlib and
kotlinx-coroutines, nothing else. Group `dev.undra`, package root `dev.undra.runtime`.

It has two layers:

* the **wire layer** (`dev.undra.runtime.wire`): everything needed to speak the binary format of SPEC §3;
* the **runtime core** (`dev.undra.runtime`): `UndraCore` and what generated code calls (SPEC §17.2), the JNI
  facade `UndraNative` (§6.1), the in-process and WebSocket transports, the mirror that applies change-sets on
  the main thread, the port registry, and the default JVM adapters (`dev.undra.runtime.adapters`, §8, §11).

## Layout

```
undra-runtime/
  settings.gradle.kts        includes :runtime, and :android-adapters when an Android SDK is found
  build.gradle.kts           group / version, Kotlin plugin declared once
  gradle/libs.versions.toml  Kotlin 2.0.21, coroutines 1.6.4, JUnit 5.10.3 (+ AGP 8.7.3, JUnit 4 and AndroidX Test for :android-adapters' tests)
  gradlew, gradle/wrapper/   Gradle 8.14.3 wrapper
  runtime/                   the library
    src/main/kotlin/dev/undra/runtime/
      UndraCore.kt LoadOptions.kt ConnectedCore.kt   the core: load / shared / call / stream / observe ...
      UndraNative.kt                                 JNI facade (static natives + Callbacks)
      Transport.kt InprocTransport.kt RemoteTransport.kt
      Mirror.kt UndraStore.kt UndraObject.kt HandleCleaner.kt UndraDispatchers.kt
      PortRegistry.kt Markers.kt Errors.kt UndraStats.kt UndraLog.kt
      adapters/                                     default JVM port adapters + standard port records
      wire/                                         the wire layer
    src/test/kotlin/dev/undra/runtime/               suites (see "Building and testing")
      support/                                      FakeTransport, FakeNative (JNI contract), WsTestServer, ...
  android-adapters/          the Android module: the adapters of the ten standard ports + the Choreographer frame pacer
  scripts/
    test-local.sh            build + test without Gradle or JUnit
    gen-vectors.py           regenerates WireVectors.kt from contract-tests/wire-vectors.json
    local/junit-stub/        a stub @Test annotation, used only by test-local.sh
```

`:android-adapters` holds everything Android-specific (the Http, Kv, SecureStore, Fs, Connectivity and Lifecycle adapters,
installed by `AndroidPlatformDefaults.install(core, context)`, and the Choreographer frame pacer); see
[android-adapters/README.md](android-adapters/README.md). `:runtime` never depends on it, and never touches an Android API at
compile time (Android is detected by reflection).

## Using it

```kotlin
// Once, at startup. UndraIds comes from the generated bindings.
UndraCore.load(LoadOptions(expectedSchemaHash = UndraIds.SCHEMA_HASH))

val todos = TodoStore()                   // generated; uses UndraCore.shared
todos.todos.collectAsState()              // Compose: plain StateFlows, updated on the main thread
todos.add("Milk")                         // suspend; cancelling the caller cancels the call in the core
todos.close()                             // or let the cleaner release it if you forget
```

| Type | What it is |
|---|---|
| `UndraCore` | `load(LoadOptions)`, `shared`, `callSync`, `call` (suspend, cancellable), `stream` (`Flow`, credit 16 / top-up 8), `construct`, `observe`, `release`, `event`, `timerFired`, `mirror`, `registerPort`, `stats`, `snapshot` / `restore`, `close`. Open with a protected constructor, so tests can subclass it as a fake core |
| `LoadOptions` | `mode` (`INPROC` or `REMOTE`), `remoteUrl`, `adapters` (port id to `PortImpl`), `expectedSchemaHash`, `defaultAdapters`, `remoteTimeout` |
| `UndraObject`, `UndraStore` | `AutoCloseable` handles; a `java.lang.ref.Cleaner` (or a phantom-reference fallback where it does not exist, Android below API 33) releases leaked ones. `UndraStore.signal(initial)` makes the `MutableStateFlow` that `apply(signalId, op, reader)` updates. The mirror holds stores weakly: keep a reference to the store while you use its flows |
| `Mirror` | Per-handle registry of `apply` callbacks. Change-sets are applied on `UndraDispatchers.main` in batches (one hop for a burst) with per-batch coalescing of superseded full values; a throwing callback is logged and skipped, a malformed change-set is dropped whole |
| `UndraDispatchers` | `main`: `Dispatchers.Main.immediate` on Android (found by reflection), else a daemon thread named `undra-main` |
| `PortImpl(sync, methods, detach)` | What generated `<trait>PortImpl(...)` returns and `LoadOptions.adapters` / `registerPort` take. Sync ports are answered inline (they must not suspend or call Undra); async ports run off the core's threads and answer through `portReply`. `detach` (optional) runs once when the implementation stops serving its core (the core closed, or another one was registered for the port): the WebSocket, Sse and Db bindings close what they hold |
| Errors | `UndraException` (base of generated errors, and of everything below), `UndraCallError` (sealed: `CancelledByCore`, `Panicked`, `Refused`, `Unavailable`, `Malformed`; what a generated call throws besides its own `E` and `CancellationException`), `UndraUnhandledError(operation, error)` (what `LoadOptions.onError` receives), `UndraReplyException(status, body)` (+ `panicInfo`, `badRequestReason`), `UndraTransportException(reason, ...)`, `UndraProtocolException`, `UndraRestoreException(code)`, `UndraModeException`, `UndraSchemaMismatchException(expected, got)`, `UndraPortException(body)`, `WireException` (sealed) A stream that fails ends with the same set (`UndraCallError.mappedStream`): `E` for its own typed error (flag 2), and for a failure the core ends it with (flag 3) `CancelledByCore`, `Panicked` or `Refused` by the failure's status (ADR-036), `Malformed` for an item or failure body the runtime cannot read |

### Threading, in one paragraph

The native core calls back from its own threads, possibly with its lock held, into direct buffers that die when the
callback returns. So every callback **copies** what it needs and returns; nothing in the runtime calls a native method from
inside a callback (a thread-local guard turns an attempt into the core's own `E_REENTRANT` bad request, `UndraCallError.Refused`, instead of a deadlock); replies resume
suspended callers on their own dispatcher (a continuation that would run inline, such as `Dispatchers.Unconfined`, is resumed
from the `undra-delivery` thread instead); stream items go through `undra-delivery` too; change-sets hop to the main thread
through the mirror. `observe` in `INPROC` applies the initial change-set before it returns: inline on the main thread, or by
waiting (up to 5 s) for the main thread from anywhere else.

### Modes

* `INPROC` (production): the core is loaded through JNI. `System.loadLibrary` uses the name in the system property
  `undra.native.name` (default `undra_core`); `undra.native.path` is an absolute path that wins over the name. If the library
  cannot be loaded, `UndraNative.isAvailable` is `false` and `load` says how to fix it. The native runtime is process-global,
  so there is one `INPROC` core at a time per process. `close()` ends its work (ADR-034: `UndraNative.shutdown()` stops its
  tasks, timers and port calls; in-flight calls fail as closed), **and waits for it**: it returns after the core's threads are
  joined and the port callbacks running on other threads have returned, so it must not run under a lock a synchronous port
  implementation needs. A later `load` starts a fresh core.
* `REMOTE` (**development only**): a WebSocket to `undra dev` on the runtime's own client (RFC 6455 over `java.net.Socket`:
  the same code runs on a JVM and on Android; `java.net.http` is not used), envelope framing of SPEC §3.2, `Hello` handshake
  with the schema check. `callSync` and `construct` block the calling thread for a network round trip (up to
  `remoteTimeout`). No snapshots, no statistics. No call into the runtime does network I/O on the calling thread, so
  `load` and the rest may be called from Android's main thread. A dropped connection is reconnected with backoff and
  jitter (`LoadOptions.reconnect`, a `ReconnectPolicy`; `null` turns it off), what was in flight fails at once (an
  `UndraTransportException` of reason `CONNECTION_LOST`, which a generated call throws as `UndraCallError.Unavailable`, and
  which a command does not hand to `onError`), the stores are observed again, and `core.connectionState` (a
  `StateFlow<ConnectionState>`) says what it is doing; a schema change or a session the dev server lost closes the core for
  good (ADR-051, `docs/DEV_LOOP.md`).

### JNI surface for `undra-ffi`

`dev.undra.runtime.UndraNative` is a Kotlin `object` whose `@JvmStatic external` functions compile to `public static native`
methods, so the symbols are `Java_dev_undra_runtime_UndraNative_<name>`. `NativeShapeTests` pins these descriptors:

```
abiVersion ()I      schemaHash ()J        schemaJson ()[B        init ([BLdev/undra/runtime/UndraNative$Callbacks;)I
call ([B)I          callSync ([B)[B       cancel (I)V            streamCredit (II)V
observe (JIZ)V      release (J)V          portReply ([B)V        event (II[B)V
timerFired (I)V     snapshot ()[B         restore ([B)I          statsJson ()Ljava/lang/String;
shutdown ()V        (UndraCore.close() of an in-process core ends its work through it, ADR-034)

UndraNative$Callbacks:  onReply (ILjava/nio/ByteBuffer;)V     onChangeSet (Ljava/nio/ByteBuffer;)V
                       onStream (ILjava/nio/ByteBuffer;)V     onPortCall (IIILjava/nio/ByteBuffer;)I     portSyncReply ()[B
```

`onReply` and `onStream` receive the **whole** `Reply` / `StreamItem` payload (SPEC 3.4 / 3.7, including the call id and status
or flag); `onPortCall` receives the encoded arguments only, and after returning `0` the shim reads the whole `PortReply`
payload from `portSyncReply()` on the same thread. `observe` uses `-1` for "all signals" (`u32::MAX`). A port the host has not
registered is answered `2` (unavailable), so the shim should route every port id it does not bind natively to `onPortCall`.

### Default JVM adapters

`UndraCore.load` installs `dev.undra.runtime.adapters.JvmAdapters` for every standard port not in `LoadOptions.adapters`
(`defaultAdapters = false` turns that off): `Http` over `java.net.http.HttpClient`; `Kv` and `SecureStore` over files under
`<dataDir>/kv` and `<dataDir>/secure` (SHA-256-named, atomic writes; **not** encrypted); `Fs` over `<dataDir>/fs`, confined to its
root; `Clock`, `Rng` (`SecureRandom`), `Log` (`java.util.logging`) and `Timer` (a scheduled executor). The data directory is the
system property `undra.data.dir` or `~/.undra/data`; call `JvmAdapters.standard(dir) { core.timerFired(it) }` to choose another. On Android only
Clock, Rng, Log and Timer are installed; the rest comes from `android-adapters` (`AndroidPlatformDefaults.install`). Port and method ids are `fnv1a32("port.<Trait>")`
and `fnv1a32("<Trait>.<method>")` (`StandardPorts`), and the records of SPEC §8 have hand-written codecs
(`HttpRequest`, `HttpResponse`, `HttpError`, `Header`, `FsError`, `NetKind`, `AppState`, `HttpMethod`).

### The opt-in ports: WebSocket, Sse, Db (ADR-047, ADR-048)

A core that enables the cargo features `websocket`, `sse` or `db` calls three more ports; the runtime ships their twelve
records (`WsOpened`, `WsMessage`, `WsError`, `SseEvent`, `SseError`, `DbMigration`, `DbOpened`, `DbValue`, `DbExecuted`,
`DbRows`, `DbConstraint`, `DbError`, in `dev.undra.runtime.adapters`, where generated bindings look for them), an adapter
interface an app can implement, a **binding** that turns an adapter into the port (`WebSocketPortAdapter`, `SsePortAdapter`,
`DbPortAdapter`, or `webSocketPort(adapter)`, `ssePort(adapter)`, `dbPort(adapter)`), and defaults, which `JvmAdapters.standard`
registers (`AndroidPlatformDefaults.install` on Android):

| Port | Adapter interface | JVM default | Android default |
|---|---|---|---|
| `WebSocket` | `WebSocketAdapter` / `WebSocketConnection` (`messages: Flow<WsMessage>`) | `ClientWebSocketAdapter`: the runtime's own RFC 6455 client (text checked to be UTF-8, subprotocols, headers, a read gate) | the same class |
| `Sse` | `SseAdapter` / `SseStream` (`events: Flow<SseEvent>`), parsed by `SseParser` (HTML standard) | `JdkHttpSseAdapter` (`java.net.http`; the JDK's `HttpURLConnection` cannot abort a blocked read) | `UrlConnectionSseAdapter` (`HttpURLConnection`) |
| `Db` | `DbAdapter` / `DbConnection` (one statement per call, `SqlText` splits scripts) | `JdbcDbAdapter` over `java.sql` in `<dataDir>/db` (the app adds `org.xerial:sqlite-jdbc`; without it every open is `Unavailable`) | `AndroidDbAdapter` (`android.database.sqlite`) |

The bindings own the ids, the pull (`receive` / `next` answer at most `max`, a burst as one reply; the default adapters read
ahead at most the room of the binding's buffer, so a core that stops reading stops the socket and TCP pushes back), the Db
migrations, the per-database serial queue and the transaction slot (`DbPortAdapter(busyTimeoutMillis = ...)` shortens the 5 s
busy timeout for tests). Errors are the ports' typed errors; SQLite failures map by result code (`DbError.fromSqliteCode`).

## The wire layer

| Type | What it is |
|---|---|
| `UndraWriter` | Growable little-endian writer: `writeU8`..`writeU64`, signed variants, `writeF32/F64`, `writeBool`, `writeStr` (UTF-8, no temporary array), `writeBytes`, `writeRaw`, `writeLen`, `toByteArray()` |
| `UndraReader` | Cursor over a `ByteArray` window or a `ByteBuffer` (read in place, meant for JNI direct buffers). `readLen` bounds every count by the bytes left, `readStr` decodes strict UTF-8, `finish()` rejects trailing bytes |
| `WireException` | Sealed. One subclass per SPEC `WireError` variant, plus `DuplicateKey`, `NegativeDuration`, `PatchOutOfBounds` |
| `UndraCodec<T>`, `Codecs` | `bool`, `u8`..`u64`, `i8`..`i64`, `f32`, `f64`, `unit`, `string`, `bytes`, `duration`, `timestamp`, `uuid`, `handle`, and `option`, `vec`, `map`, `result` |
| `Handle`, `Timestamp`, `UndraResult` | Value types: object handle (index / generation), epoch milliseconds, `Ok` / `Err` |
| `Envelope` | The 23-byte transport frame, `Envelope.Kind` (16 kinds), `encode` / `decode` |
| `Payloads` | Typed `encode` / `decode` for every message body, including `ChangeSet.forEachEntry` |
| `KeyedPatch`, `PatchOp` | Decode / encode / apply keyed list patches |
| `Fnv` | `fnv1a32` / `fnv1a64` for type, method and schema ids |

```kotlin
val w = UndraWriter()
Codecs.vec(Codecs.string).encode(w, listOf("a", "b"))
val bytes = w.toByteArray()

val r = UndraReader(bytes)                  // or UndraReader(directByteBuffer) inside a JNI callback
val items = Codecs.vec(Codecs.string).decode(r)
r.finish()

// Host loop, core to host: no allocation per entry.
Payloads.ChangeSet.forEachEntry(changeSetBytes) { handle, signalId, op, value ->
    when (op) {
        Payloads.ChangeOp.FULL -> store.apply(handle, signalId, Codecs.vec(Todo).decode(value))
        Payloads.ChangeOp.PATCH -> store.patch(handle, signalId, KeyedPatch.decodePatch(value, Todo))
        Payloads.ChangeOp.INVALIDATED -> store.invalidate(handle, signalId)
    }
}
```

### Guarantees

* **Decoding only ever throws `WireException`.** Truncated, oversized, garbage or hostile bytes never
  produce `IndexOutOfBounds`, `NegativeArraySize` or an `OutOfMemoryError`: every length or count goes
  through `UndraReader.readLen`, which checks it against the bytes that remain before anything is allocated.
  A 5,000-iteration byte fuzz (random, structured and mutated inputs, plus every truncation and a bit flip
  at every byte of a valid corpus) enforces this, and also checks that array and direct-buffer readers agree.
* **Direct buffers are read in place.** `UndraReader(ByteBuffer)` does not copy or modify the buffer. It is only
  valid while the buffer is: decode inside the callback. `readBytes`, `readRaw` and the `Payloads` bodies are
  copies, so results outlive the buffer; strings are materialized.
* **Errors carry offsets.** `at` is relative to the start of the reader's window.
* **Canonical maps.** `Codecs.map` sorts entries by the encoded key bytes, so equal maps always encode to the
  same bytes. Note that for strings this orders by the little-endian length prefix first.

### Documented limitations

* A `Vec` (or `Map`) whose items encode to zero bytes (`Unit`, an empty record) cannot be decoded when the
  count exceeds the bytes left: `readLen` cannot tell a legitimate count from a hostile one.
* `Codecs.option` is `T?` for non-null `T`, so `Option<Option<T>>` does not type-check.
* `kotlin.time.Duration` keeps nanosecond precision only up to about 146 years, so larger durations lose their
  sub-millisecond part. Negative durations are rejected (the core's `Duration` is unsigned), as are
  `Duration.INFINITE` and anything above `Long.MAX_VALUE` nanoseconds.
* `Timestamp.toInstant` / `ofInstant` use `java.time`, which Android provides from API 26 or through
  core-library desugaring.
* Strings with unpaired surrogates cannot be encoded (`IllegalArgumentException`); they are never silently
  replaced.

## Building and testing

There are two ways to run the same tests.

### 1. Without Gradle (works in the sandbox and anywhere with a JDK 11+ and `kotlinc`)

```sh
source scripts/env.sh                      # puts kotlinc on PATH and sets UNDRA_KOTLINX_COROUTINES (repo-local toolchain)
runtimes/kotlin/undra-runtime/scripts/test-local.sh
```

It checks that `WireVectors.kt` is up to date, compiles `runtime/src/main` (explicit API mode, warnings are
errors) and `runtime/src/test` with `kotlinc` from `PATH`, and runs every suite through `TestMain.kt`, a
reflection-free runner. It exits non-zero on any failure and prints each failing case with its stack frames; a
case that cannot run here (the JNI smoke test without the native library) is reported as skipped.
Each phase is incremental (`scripts/test-local.sh check|main|test|run`), so a slow machine or a per-command time
limit can run them one at a time.
Knobs: `UNDRA_FUZZ_ITERATIONS=50000`, `UNDRA_FUZZ_SEED=123`, `UNDRA_WERROR=0`, `UNDRA_FORCE=1`, `UNDRA_BUILD_DIR=<dir>`
(default `build/local`), `UNDRA_SKIP_GOLDEN=1`, `UNDRA_KOTLINX_COROUTINES`, `UNDRA_KOTLIN_STDLIB`, `UNDRA_SQLITE_JDBC`, and for the JNI smoke
test `UNDRA_NATIVE_LIB_DIR`, `UNDRA_NATIVE_NAME`, `UNDRA_NATIVE_PATH`.

What runs (see `TestMain.kt`): the wire suites, then

| Suite | Covers |
|---|---|
| `GoldenFullTests` | the generated Kotlin of the bindgen `full` golden case, compiled against this runtime together with bindgen's `FullTest.kt` (objects, stores, ports, queries through a fake `UndraCore` subclass) |
| `CoreCallTests`, `StreamTests` | call / callSync / cancellation / call ids / errors / construct / close / blocking waits; stream ordering, credit 16 and top-up 8, back-pressure, cancellation, failures |
| `MirrorTests`, `StoreTests` | batching, coalescing, ordering, isolation, awaiting the initial values; real `UndraStore` subclasses driven by a fake core, keyed patches, resync, cleaner release |
| `PortTests` | sync and async ports, typed errors, timeouts, closing, default adapters, timers, the re-entrancy guard |
| `InprocTransportTests` | the transport against `FakeNative`, an in-memory JNI shim that recycles direct buffers on return and flags native calls made from callbacks |
| `RemoteTransportTests` | the WebSocket transport against `WsTestServer`, a small RFC 6455 server: handshake, schema mismatch, framing and fragmentation, streams, ports, logs, drops and protocol errors |
| `AdapterTests`, `FileAdapterTests`, `HttpAdapterTests` | port ids, record codecs, Clock / Rng / Log / Timer, Kv / SecureStore / Fs (traversal and symlink escapes), Http against a JDK `HttpServer` |
| `PortsV2RecordTests`, `PortsV2TextTests` | the twelve records of the opt-in ports (the Rust unit tests' exact bytes, round trips, `#[error]` texts, ids), SQLite result codes; the event-stream parser and SQL statement splitting / parameter counts |
| `PortsV2BindingTests` | the WebSocket, Sse and Db bindings over scripted adapters: ids, the window and one pending pull, burst coalescing, ends, close and detach, migrations, transactions and `Busy`, the serial queue |
| `RealtimeAdapterTests` | the default WebSocket and Sse adapters against `contract-tests/servers/realtime-server.mjs` (Node): echo, subprotocols and headers, refusals with status, peer close, drop, invalid UTF-8, a flood under a stalled reader, the SSE feed and resume, `/sse/hang` closed. Skipped without Node (failed with `UNDRA_REQUIRE_TOOLCHAINS=1`) |
| `JdbcDbAdapterTests` | `JdbcDbAdapter` through the binding against real SQLite: constraint kinds, busy, a corrupt file, migrations, typed cells, one statement, the parameter count. Needs `UNDRA_SQLITE_JDBC` (the driver jar, on the test class path only); skipped without it (failed with `UNDRA_REQUIRE_TOOLCHAINS=1`) |
| `ErrorTests`, `StatsTests`, `CleanerTests`, `DispatcherTests` | exception shapes, the statistics parser, both cleaner backends, main-thread and delivery dispatchers |
| `NativeShapeTests`, `NativeSmokeTests` | the JNI descriptors of SPEC 6.1; a smoke test against the real `undra_core`, skipped until it is loadable |

### 2. With Gradle and JUnit 5

```sh
cd runtimes/kotlin/undra-runtime
./gradlew :runtime:test
```

Each test class is a `Suite` (`runtime/src/test/kotlin/dev/undra/runtime/testing/Suite.kt`): its named cases run
through `runAll()`, and a single `@Test fun allCases()` delegates to it, so JUnit reports one result per class
with every failing case listed. Adding a class means extending `Suite`, adding `@Test fun allCases() =
assertPassed()`, and registering it in `TestMain.kt` (`test-local.sh` fails if a suite is not registered). A case
calls `skip(reason)` when the environment lacks something it needs.

The Gradle build adds the bindgen `full` golden sources and `FullTest.kt` to the test source set when the repository
layout is present, and `-Pundra.native.dir=<dir>` / `-Pundra.native.name=<name>` point the JNI smoke test at the native
library.

The Gradle build was written on a machine without network access to plugin repositories, so it has not been
executed end to end; the settings script, the version catalog and the root build script were confirmed to
parse. Expect at most small fixes on first run.

### Test data

`contract-tests/wire-vectors.json` is the cross-language source of truth. `scripts/gen-vectors.py` turns it into
`WireVectors.kt` (checked in, marked generated); `python3 scripts/gen-vectors.py --check` verifies it is fresh.
`WireVectorsTest` checks every vector in both directions over array, window and direct-buffer readers, and fails
on any vector `type` it does not know.
