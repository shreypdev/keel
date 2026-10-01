import Foundation
import UndraRuntime
import PlaygroundCore

/// The one core of this process and the fakes it runs against.
///
/// `undra_init` is once per process, so every scenario shares the core the first of them loads.
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

    private var loaded: UndraCore?

    private init() {}

    /// The adapters of the harness: the fakes above and the platform defaults for `Rng` and `Timer`
    /// (a real `DispatchQueue` timer). `Connectivity` has no adapter: the scenarios emit its events.
    func makeAdapters() -> Adapters {
        return Adapters.none
            .replacing(clock)
            .replacing(server)
            .replacing(kv)
            .replacing(log)
            .replacing(RngAdapter())
            .replacing(TimerAdapter())
    }

    /// The loaded core, loading it (and pointing it at the fake server) on first use.
    func core() throws -> UndraCore {
        if let core = loaded, !core.isShutDown {
            return core
        }
        let sink = unhandled
        let core = try UndraCore.load(.inproc(
            adapters: makeAdapters(),
            expectedSchemaHash: UndraIds.schemaHash,
            onError: { (report: UndraUnhandledError) -> Void in
                sink.withLock { (current: inout [UndraUnhandledError]) -> Void in current.append(report) }
            }
        ))
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
