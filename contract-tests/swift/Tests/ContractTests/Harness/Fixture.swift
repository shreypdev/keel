import Foundation
import KeelRuntime
import PlaygroundCore

/// The one core of this process and the fakes it runs against.
///
/// `keel_init` is once per process, so every scenario shares the core the first of them loads.
/// The fakes outlive a reload (S16 shuts the core down and loads it again), which is why they
/// are owned here and not by the adapters.
@MainActor
final class Fixture {
    static let shared = Fixture()

    let clock = ManualClock()
    let server = FakeServer()
    let kv = MemoryKv()
    let log = CapturingLog()

    private var loaded: KeelCore?

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
    func core() throws -> KeelCore {
        if let core = loaded, !core.isShutDown {
            return core
        }
        let core = try KeelCore.load(.inproc(adapters: makeAdapters(), expectedSchemaHash: KeelIds.schemaHash))
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
