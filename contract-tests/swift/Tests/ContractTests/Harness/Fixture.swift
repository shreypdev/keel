import Foundation
import UndraRuntime
import PlaygroundCore

/// The one core of this process and the fakes it runs against.
///
/// The playground core's `init` is once per core, so every scenario shares the core the first of
/// them loads (through its generated entry, `UndraPlaygroundCore`, as an app does).
/// The fakes outlive a reload (S16 shuts the core down and loads it again), which is why they
/// are owned here and not by the adapters.
@MainActor
final class Fixture {
    static let shared = Fixture()

    let clock = ManualClock()
    let server = FakeServer()
    let kv = MemoryKv()
    let log = CapturingLog()

    /// Every failure the generated commands and the stores reported through `LoadOptions.onError`
    /// (ADR-032), oldest first, across reloads of the core. Scenarios compare counts before and
    /// after, the way they do for statistics; this is the Swift column's `runtimeErrors` of the
    /// TypeScript harness.
    let unhandled = Locked<[UndraUnhandledError]>([])

    /// Every call the core made to one of the adapters below, per port, across reloads (S17.7).
    let portCalls = PortCallCounter()

    /// Where S25's databases live: a fresh temporary directory, deleted when S25 ends.
    let databases = FileManager.default.temporaryDirectory
        .appendingPathComponent("undra-contract-s25-\(UUID().uuidString)", isDirectory: true)

    private var loaded: UndraCore?
    private var realtimeServer: RealtimeServer?

    /// The harness fails the first read of the offline queue with `Locked`, as the Keychain or a
    /// data-protected file answers an app launched before the device's first unlock (scenarios.md,
    /// "Adapters"; S20 step 4 checks what the core did with it, S14 waits until the queue is read).
    private init() {
        kv.fail(.get, key: Persisted.queueKey, with: .locked, times: 1)
    }

    /// The shared local server of S23 and S24 (`contract-tests/servers/realtime-server.mjs`),
    /// started on first use and left running until the process ends (it exits with its stdin).
    func realtime() throws -> RealtimeServer {
        if let running = realtimeServer {
            return running
        }
        let started = try RealtimeServer.start()
        realtimeServer = started
        return started
    }

    /// The adapters of the harness: the fakes above, the platform defaults for `Rng` and `Timer`
    /// (a real `DispatchQueue` timer), and the default adapters of the opt-in ports (ADR-047,
    /// ADR-048): `URLSessionWebSocketAdapter` and `URLSessionSseAdapter` (S23, S24 run them against
    /// the shared local server) and `SQLiteDbAdapter` rooted in `databases` (S25), each counting its
    /// calls in `portCalls`. `Connectivity` has no adapter: the scenarios emit its events.
    func makeAdapters() -> Adapters {
        let adapters: [any UndraAdapter] = [
            clock, server, kv, log, RngAdapter(), TimerAdapter(),
            URLSessionWebSocketAdapter(), URLSessionSseAdapter(), SQLiteDbAdapter(directory: databases),
        ]
        var all = Adapters.none
        for adapter in adapters {
            all = all.replacing(CountingAdapter(inner: adapter, counter: portCalls))
        }
        return all
    }

    /// The options every load of the harness uses: its adapters and an `onError` that records into
    /// `unhandled` (the entry adds the core's table and the generated schema hash).
    func loadOptions() -> LoadOptions {
        let sink = unhandled
        return .inproc(
            adapters: makeAdapters(),
            onError: { (report: UndraUnhandledError) -> Void in
                sink.withLock { (current: inout [UndraUnhandledError]) -> Void in current.append(report) }
            }
        )
    }

    /// The loaded core, loading it (and pointing it at the fake server) on first use, and again
    /// after it was shut down.
    func core() throws -> UndraCore {
        if let core = loaded, !core.isShutDown {
            return core
        }
        let core = try UndraPlaygroundCore.load(loadOptions())
        configureRemote(RemoteConfig(baseUrl: FakeServer.baseURL), ctx: core)
        loaded = core
        return core
    }

    /// Shuts the core down. The next `core()` loads a fresh one.
    func shutDown() {
        loaded?.shutdown()
        loaded = nil
    }
}
