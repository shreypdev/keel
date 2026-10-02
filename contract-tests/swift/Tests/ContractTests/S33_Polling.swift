import Foundation
import PlaygroundCore
@testable import UndraRuntime
import XCTest

/// The instants at which something a store shows took each new value, seen by reading it every 5 ms on the main actor.
@MainActor
private final class TimedRecorder<Value: Equatable & Sendable> {
    private(set) var changes: [(value: Value, at: ContinuousClock.Instant)]
    private let read: @MainActor () -> Value
    private var poller: Task<Void, Never>?

    init(_ read: @escaping @MainActor () -> Value) {
        self.read = read
        changes = [(value: read(), at: ContinuousClock.now)]
        poller = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                self?.sample()
                try? await Task.sleep(for: .milliseconds(5))
            }
        }
    }

    private func sample() {
        let current = read()
        if changes.last?.value != current {
            changes.append((value: current, at: ContinuousClock.now))
        }
    }

    /// The seconds between the last two changes.
    var lastGap: Double {
        guard changes.count >= 2 else {
            return 0
        }
        let (a, b) = (changes[changes.count - 2].at, changes[changes.count - 1].at)
        let span = a.duration(to: b)
        return Double(span.components.seconds) + Double(span.components.attoseconds) / 1e18
    }

    func stop() {
        sample()
        poller?.cancel()
        poller = nil
    }

    deinit {
        poller?.cancel()
    }
}

extension ContractScenarios {
    // MARK: S33

    /// ADR-043: a query with an `interval` polls while somebody observes it and the app is active and online; the interval runs
    /// from the end of a fetch; an observer's override changes it; the last observer stops it. Real time: the harness's Timer
    /// port is a `DispatchQueue` timer, and `Lifecycle` and `Connectivity` are the events the platform would send.
    func testS33_polling() async {
        await scenario("S33", "polling") {
            let core = try self.core
            core.emitConnectivity(online: true, kind: .wifi)
            UndraLifecycle(core: core).changed(.active)
            let fetches = { try tickerFetches(ctx: core) }
            let within: Duration = .milliseconds(2_500)

            // 1. A poll after each fetch.
            let ticker = try TickerQueryHandle(ctx: core)
            let ticks = TimedRecorder { ticker.data }
            defer { ticks.stop() }
            try await waitUntil("data == 1") { ticker.data == 1 }
            try await waitUntil("data == 2", timeout: within) { ticker.data == 2 }
            // The recorder reads the store every 5 ms and this wait every 10: the gap is read once the recorder has seen the
            // second tick too. (The first tick is already there when the recorder starts: the handle's first fetch runs while it
            // is made, so the recorder's first entry is that tick, and a gap read before the second entry is 0.)
            try await waitUntil("the recorder to see the second tick") { ticks.changes.last?.value == 2 }
            let afterSecond = try fetches()
            try check(afterSecond >= 2, "ticker_fetches() after the second tick: \(afterSecond)")
            try check(ticks.lastGap >= 0.9, "the gap between the first two ticks was \(ticks.lastGap) s (expected at least 0.9 s)")

            // 2. Background pauses, Active resumes.
            UndraLifecycle(core: core).changed(.background)
            try await quietFor(milliseconds: 150)
            let inBackground = try fetches()
            try await quietFor(milliseconds: 1_500)
            try checkEqual(try fetches(), inBackground, "ticker_fetches() after 1.5 s in the background")
            let beforeActive = ticker.data ?? 0
            UndraLifecycle(core: core).changed(.active)
            try await waitUntil("data advances after Active", timeout: within) { (ticker.data ?? 0) > beforeActive }

            // 3. Offline pauses, online resumes.
            core.emitConnectivity(online: false, kind: .disconnected)
            try await quietFor(milliseconds: 150)
            let offline = try fetches()
            try await quietFor(milliseconds: 1_500)
            try checkEqual(try fetches(), offline, "ticker_fetches() after 1.5 s offline")
            let beforeOnline = ticker.data ?? 0
            core.emitConnectivity(online: true, kind: .wifi)
            try await waitUntil("data advances after going online", timeout: within) { (ticker.data ?? 0) > beforeOnline }

            // 4. A failure keeps polling and clears on success.
            setTickerFailing(failing: true, ctx: core)
            try await waitUntil("the error", timeout: within) { ticker.error == .failing }
            let failing = try fetches()
            try await waitUntil("fetches keep growing while failing", timeout: within) { (try? fetches()) ?? 0 > failing }
            let beforeRecovery = ticker.data ?? 0
            setTickerFailing(failing: false, ctx: core)
            try await waitUntil("the error clears and data advances", timeout: within) { ticker.error == nil && (ticker.data ?? 0) > beforeRecovery }

            // 5. An observer's override.
            #if UNDRA_FLOOR
            let three = UndraDuration(nanoseconds: 3_000_000_000)
            #else
            let three: Duration = .seconds(3)
            #endif
            ticker.setPollInterval(three)
            let afterOverride = ticker.data ?? 0
            try await waitUntil("the next fetch after the override", timeout: .seconds(5)) { (ticker.data ?? 0) > afterOverride }
            let afterNext = ticker.data ?? 0
            try await waitUntil("the fetch after that", timeout: .seconds(5)) { (ticker.data ?? 0) > afterNext }
            try check(ticks.lastGap >= 2.9, "the gap after setPollInterval(3 s) was \(ticks.lastGap) s (expected at least 2.9 s)")
            ticker.setPollInterval(nil)
            let afterClear = ticker.data ?? 0
            try await waitUntil("the next fetch after clearing", timeout: .seconds(5)) { (ticker.data ?? 0) > afterClear }
            let afterClearNext = ticker.data ?? 0
            try await waitUntil("the fetch after that", timeout: .seconds(5)) { (ticker.data ?? 0) > afterClearNext }
            try check(ticks.lastGap >= 0.9 && ticks.lastGap < 2.0, "the gap after setPollInterval(nil) was \(ticks.lastGap) s (expected about 1 s)")

            // 6. The last observer stops it.
            ticks.stop()
            ticker.close()
            try await quietFor(milliseconds: 150)
            let closed = try fetches()
            try await quietFor(milliseconds: 2_500)
            try checkEqual(try fetches(), closed, "ticker_fetches() 2.5 s after the handle was closed")
        }
    }
}
