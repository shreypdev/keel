import Foundation
import UndraRuntime

/// The app's own core, loaded in this process with the deterministic ``Fakes`` as its ports and a manual clock, for a SwiftUI preview, a unit
/// test or a UI test: the real logic, scripted ports, and time that moves when you say so.
///
/// ```swift
/// #Preview("Todos, three items") {
///     let preview = try! PreviewCore.load(UndraPlaygroundCore.load, seed: try! Seed(json: seedJSON))
///     TodosScreen(todos: try! Todos())          // the generated stores use the bindings' own core, which is the preview's
/// }
/// ```
///
/// The core is loaded through the entry of its bindings (`UndraPlaygroundCore.load`, ADR-044), so it is also what the bindings' stores use by default.
/// A process holds one in-process core per namespace. ``load(_:seed:fakes:adapters:replaceCurrent:onError:)`` shuts the shared one down first
/// (a refreshed preview does the same), unless `replaceCurrent` is `false`.
///
/// The manual clock moves the `Clock` port and the timers armed through the `Timer` port. A native core runs its own `ctx.sleep` on the runtime's
/// timer thread, in real time (the C ABI never hands sleeps to the host), so a delay the core sleeps through is waited for, not advanced; on web
/// the sleeps follow the manual clock too.
public final class PreviewCore: @unchecked Sendable {
    /// The core.
    public let core: UndraCore
    /// The fakes it runs on: script `fakes.http`, seed `fakes.kv`, read `fakes.log`.
    public let fakes: Fakes

    private init(core: UndraCore, fakes: Fakes) {
        self.core = core
        self.fakes = fakes
    }

    /// The manual clock: ``FakeClock/nowMs`` reads it, ``FakeClock/setNowMs(_:)`` jumps the wall clock, ``advance(ms:)`` moves time.
    public var clock: FakeClock {
        return fakes.clock
    }

    /// Loads the core with the fakes installed.
    ///
    /// - Parameters:
    ///   - entry: the load function of the bindings the app was generated with (`UndraPlaygroundCore.load`): it knows the core's table and its schema
    ///     hash, and remembers the core for the generated stores.
    ///   - seed: the starting state of the fakes, applied before the core starts.
    ///   - fakes: fakes to use instead of fresh ones (for example ones a test already holds).
    ///   - adapters: further ports on top of the fakes (an app's own port, or a ``Replayer``'s adapters).
    ///   - replaceCurrent: shut the shared core down first, if there is one (a process holds one in-process core).
    @discardableResult
    public static func load(
        _ entry: (LoadOptions) throws -> UndraCore,
        seed: Seed? = nil,
        fakes: Fakes = Fakes(),
        adapters: Adapters = .none,
        replaceCurrent: Bool = true,
        onError: (@Sendable (UndraUnhandledError) -> Void)? = nil
    ) throws -> PreviewCore {
        try seed?.apply(to: fakes)
        if replaceCurrent {
            UndraCore.current?.shutdown()
        }
        var all = fakes.adapters()
        for adapter in adapters.all {
            all = all.replacing(adapter)
        }
        let core = try entry(.inproc(adapters: all, onError: onError))
        fakes.clock.onTimerFired = { [weak core] id in core?.timerFired(id) }
        return PreviewCore(core: core, fakes: fakes)
    }

    /// Lets the core catch up and the stores see what it produced: waits until the core's counters have stood still for `quietMs` and it has
    /// answered every port call, then applies what the mirror holds. The core runs on a thread of its own, so "idle" is observed, not known:
    /// raise `quietMs` on a machine that is busy.
    @MainActor
    public func settle(quietMs: Int = 20, timeoutMs: Int = 5_000) async {
        let deadline = ContinuousClock.now + .milliseconds(timeoutMs)
        var quiet = 0
        var last = ""
        while ContinuousClock.now < deadline {
            let stats = core.stats()
            let polls = stats.values["polls"] ?? -1
            let now = "\(polls):\(stats.values["pending_port_calls"] ?? 0):\(stats.coreActiveCalls):\(stats.hostPendingCalls)"
            if now == last && (stats.values["pending_port_calls"] ?? 0) <= 0 {
                quiet += 1
            } else {
                quiet = 0
            }
            last = now
            if quiet >= max(1, quietMs / 2) {
                break
            }
            try? await Task.sleep(for: .milliseconds(2))
        }
        core.mirror.flush()
    }

    /// Moves the manual clock forward by `ms`, one deadline at a time: at each deadline the clock reads exactly that instant, the due timer fires
    /// into the core, and the core settles before time moves on, so a task that sleeps again inside the window is served within the same call.
    /// Returns how many timers fired.
    @MainActor
    @discardableResult
    public func advance(ms: Int64) async -> Int {
        var left = max(0, ms)
        var fired = 0
        await settle()
        while let due = fakes.clock.nextDueInMs(), due <= left {
            guard fakes.clock.fireNext(limitMs: left) != nil else {
                break
            }
            left -= due
            fired += 1
            await settle()
        }
        fakes.clock.moveBy(ms: left)
        await settle()
        return fired
    }

    /// Shuts the core down.
    public func close() {
        fakes.clock.onTimerFired = nil
        core.shutdown()
    }
}
