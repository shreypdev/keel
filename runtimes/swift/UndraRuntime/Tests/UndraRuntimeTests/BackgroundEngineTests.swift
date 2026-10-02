import Foundation
import XCTest
@testable import UndraRuntime

// ADR-046, decision 3.4: what the iOS background integration decides, against fakes of the OS (BGTask,
// BGTaskScheduler, UIApplication) and of the core, so it runs here on macOS. `UndraBackground` itself
// (iOS only) is these decisions bound to BackgroundTasks and UIApplication.

// MARK: - Fakes

final class FakeBackgroundTask: BackgroundTaskHandle, @unchecked Sendable {
    let identifier: String
    private let state = Guarded<(handler: (@Sendable () -> Void)?, completions: [Bool])>((handler: nil, completions: []))

    init(_ identifier: String) {
        self.identifier = identifier
    }

    var expirationHandler: (@Sendable () -> Void)? {
        get { state.withLock { $0.handler } }
        set { state.withLock { $0.handler = newValue } }
    }

    func setTaskCompleted(success: Bool) {
        state.withLock { $0.completions.append(success) }
    }

    /// What `setTaskCompleted` was called with, in order.
    var completions: [Bool] {
        return state.withLock { $0.completions }
    }

    /// The OS taking the window back.
    func expire() {
        expirationHandler?()
    }
}

final class FakeBackgroundScheduler: BackgroundScheduler, @unchecked Sendable {
    struct SubmitFailure: Error, CustomStringConvertible {
        var description: String { "BGTaskSchedulerErrorDomain error 1 (unavailable)" }
    }

    private struct State {
        var handlers: [String: @Sendable (any BackgroundTaskHandle) -> Void] = [:]
        var order: [String] = []
        var submitted: [BackgroundTaskRequest] = []
        var accept = true
        var failSubmits = false
    }

    private let state = Guarded<State>(State())

    func register(identifier: String, handler: @escaping @Sendable (any BackgroundTaskHandle) -> Void) -> Bool {
        return state.withLock { (s: inout State) -> Bool in
            s.order.append(identifier)
            s.handlers[identifier] = handler
            return s.accept
        }
    }

    func submit(_ request: BackgroundTaskRequest) throws {
        let fail = state.withLock { (s: inout State) -> Bool in
            s.submitted.append(request)
            return s.failSubmits
        }
        if fail {
            throw SubmitFailure()
        }
    }

    var registered: [String] { state.withLock { $0.order } }
    var submitted: [BackgroundTaskRequest] { state.withLock { $0.submitted } }

    func refuseRegistrations() { state.withLock { $0.accept = false } }
    func failSubmits() { state.withLock { $0.failSubmits = true } }

    /// The OS launching the handler of `identifier` with a fresh task.
    @discardableResult
    func launch(_ identifier: String) -> FakeBackgroundTask? {
        guard let handler = state.withLock({ $0.handlers[identifier] }) else {
            return nil
        }
        let task = FakeBackgroundTask(identifier)
        handler(task)
        return task
    }
}

final class FakeBackgroundApplication: BackgroundApplication, @unchecked Sendable {
    private struct State {
        var begun: [String] = []
        var ended: [Int] = []
        var expirations: [Int: @Sendable () -> Void] = [:]
        var next = 100
        var refuse = false
    }

    private let state = Guarded<State>(State())

    func beginBackgroundTask(named name: String, expiration: @escaping @Sendable () -> Void) -> Int? {
        return state.withLock { (s: inout State) -> Int? in
            s.begun.append(name)
            if s.refuse {
                return nil
            }
            s.next += 1
            s.expirations[s.next] = expiration
            return s.next
        }
    }

    func endBackgroundTask(_ token: Int) {
        state.withLock { $0.ended.append(token) }
    }

    var begun: [String] { state.withLock { $0.begun } }
    var ended: [Int] { state.withLock { $0.ended } }

    func refuse() { state.withLock { $0.refuse = true } }

    /// The OS taking the grace back.
    func expire(_ token: Int) {
        state.withLock { $0.expirations[token] }?()
    }
}

final class FakeBackgroundCore: BackgroundCore, @unchecked Sendable {
    enum Behavior {
        case report(UndraBackgroundReport)
        case fail(any Error)
        /// Runs until its task is cancelled.
        case hang
    }

    private struct State {
        var runs: [TimeInterval] = []
        var behavior = Behavior.report(UndraBackgroundReport(finished: true))
        var pending = 0
        var shutDown = false
    }

    private let state = Guarded<State>(State())

    init(pending: Int = 0, _ behavior: Behavior = .report(UndraBackgroundReport(finished: true))) {
        state.withLock {
            $0.pending = pending
            $0.behavior = behavior
        }
    }

    var isShutDown: Bool { state.withLock { $0.shutDown } }
    var runs: [TimeInterval] { state.withLock { $0.runs } }

    func shutDown() { state.withLock { $0.shutDown = true } }

    func pendingBackgroundWork() -> Int {
        return state.withLock { $0.pending }
    }

    func runInBackground(deadline: TimeInterval) async throws -> UndraBackgroundReport {
        let behavior = state.withLock { (s: inout State) -> Behavior in
            s.runs.append(deadline)
            return s.behavior
        }
        switch behavior {
        case .report(let report):
            return report
        case .fail(let error):
            throw error
        case .hang:
            try await Task.sleep(nanoseconds: 60_000_000_000)
            throw TestFailure(description: "the hanging run was never cancelled")
        }
    }
}

/// Collects what an engine logs.
final class EngineLog: @unchecked Sendable {
    private let lines = Guarded<[String]>([])

    func add(_ line: String) {
        lines.withLock { $0.append(line) }
    }

    var all: [String] { lines.withLock { $0 } }
}

// MARK: - The engine

@MainActor
final class BackgroundEngineTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_700_000_000)

    private struct Rig {
        let engine: BackgroundEngine
        let scheduler: FakeBackgroundScheduler
        let application: FakeBackgroundApplication
        let log: EngineLog
        let loads: Guarded<Int>
    }

    /// An engine for `taskIdentifier` whose loader returns `core` (or throws `loadError`).
    private func rig(
        _ core: FakeBackgroundCore,
        taskIdentifier: String = "com.example.app.undra",
        loadError: (any Error)? = nil
    ) -> Rig {
        let scheduler = FakeBackgroundScheduler()
        let application = FakeBackgroundApplication()
        let log = EngineLog()
        let loads = Guarded<Int>(0)
        let fixed = now
        let engine = BackgroundEngine(
            taskIdentifier: taskIdentifier,
            scheduler: scheduler,
            application: application,
            now: { fixed },
            log: { log.add($0) },
            loader: {
                loads.withLock { $0 += 1 }
                if let error = loadError {
                    throw error
                }
                return core
            }
        )
        addTeardownBlock {
            engine.stop()
        }
        return Rig(engine: engine, scheduler: scheduler, application: application, log: log, loads: loads)
    }

    // MARK: Registering

    func testRegisteringAddsTheProcessingAndTheRefreshWindowUnderTheirSuffixes() {
        let r = rig(FakeBackgroundCore())
        XCTAssertTrue(r.engine.register())
        XCTAssertEqual(r.scheduler.registered, ["com.example.app.undra.processing", "com.example.app.undra.refresh"])
        XCTAssertEqual(r.engine.processingIdentifier, "com.example.app.undra.processing")
        XCTAssertEqual(r.engine.refreshIdentifier, "com.example.app.undra.refresh")
        // Idempotent: a reload of the app's start-up code must not register again (BGTaskScheduler raises on a duplicate).
        XCTAssertFalse(r.engine.register())
        XCTAssertEqual(r.scheduler.registered.count, 2)
    }

    func testARefusedRegistrationIsLoggedAndNotFatal() {
        let r = rig(FakeBackgroundCore())
        r.scheduler.refuseRegistrations()
        XCTAssertTrue(r.engine.register())
        XCTAssertEqual(r.log.all.count, 2)
        XCTAssertTrue(r.log.all[0].contains("BGTaskSchedulerPermittedIdentifiers"), r.log.all[0])
    }

    func testTheOSLaunchingAHandlerRunsTheWindow() async throws {
        let core = FakeBackgroundCore(pending: 1)
        let r = rig(core)
        r.engine.register()
        let task = try XCTUnwrap(r.scheduler.launch("com.example.app.undra.refresh"))
        let done = await waitUntil { !task.completions.isEmpty }
        XCTAssertTrue(done)
        XCTAssertEqual(task.completions, [true])
        XCTAssertEqual(core.runs, [25])
        XCTAssertNil(r.scheduler.launch("com.example.app.undra.unknown"))
    }

    // MARK: A window

    func testAProcessingWindowLoadsTheCoreRunsItForMinutesAndReportsSuccess() async {
        let core = FakeBackgroundCore()
        let r = rig(core)
        let task = FakeBackgroundTask(r.engine.processingIdentifier)
        await r.engine.handle(task, kind: .processing).value
        XCTAssertEqual(r.loads.withLock { $0 }, 1, "the loader runs once")
        XCTAssertEqual(core.runs, [180])
        XCTAssertEqual(task.completions, [true])
        XCTAssertEqual(r.scheduler.submitted, [], "nothing is left, so nothing is asked for")
        XCTAssertEqual(r.log.all, [])
    }

    func testARefreshWindowGetsAboutThirtySecondsLess() async {
        let core = FakeBackgroundCore()
        let r = rig(core)
        let task = FakeBackgroundTask(r.engine.refreshIdentifier)
        await r.engine.handle(task, kind: .refresh).value
        XCTAssertEqual(core.runs, [25])
        XCTAssertEqual(task.completions, [true])
    }

    func testAnUnfinishedRunReportsFailureAndAsksForAnotherWindow() async {
        let core = FakeBackgroundCore(.report(UndraBackgroundReport(finished: false, replayed: 1, refetched: 0, stillPending: 2)))
        let r = rig(core)
        let task = FakeBackgroundTask(r.engine.processingIdentifier)
        await r.engine.handle(task, kind: .processing).value
        XCTAssertEqual(task.completions, [false])
        XCTAssertEqual(r.scheduler.submitted.map { $0.identifier }, [r.engine.processingIdentifier, r.engine.refreshIdentifier])
    }

    func testAWindowNeverClaimsFinishedWhileWorkIsStillPending() async {
        // A well-behaved core reports finished only with nothing pending; the platform does not trust the pair.
        let odd = UndraBackgroundReport(finished: true, replayed: 0, refetched: 0, stillPending: 2)
        for kind in [BackgroundTaskKind.refresh, .processing] {
            let r = rig(FakeBackgroundCore(.report(odd)))
            let task = FakeBackgroundTask("t")
            await r.engine.handle(task, kind: kind).value
            XCTAssertEqual(task.completions, [false], "\(kind)")
            XCTAssertEqual(r.scheduler.submitted.count, 2, "\(kind): another window is asked for")
        }
    }

    func testExpirationCancelsTheRunAndCompletesOnceAsAFailure() async {
        let core = FakeBackgroundCore(.hang)
        let r = rig(core)
        let task = FakeBackgroundTask(r.engine.processingIdentifier)
        let work = r.engine.handle(task, kind: .processing)
        let started = await waitUntil { !core.runs.isEmpty }
        XCTAssertTrue(started)
        XCTAssertEqual(task.completions, [], "still running")
        task.expire()
        task.expire()
        await work.value
        XCTAssertEqual(task.completions, [false], "setTaskCompleted is called exactly once")
        XCTAssertEqual(r.scheduler.submitted.count, 2, "the window was cut short: ask for another")
    }

    func testACoreThatCannotBeLoadedCompletesTheWindowAsAFailure() async {
        struct NoCore: Error {}
        let r = rig(FakeBackgroundCore(), loadError: NoCore())
        let task = FakeBackgroundTask(r.engine.refreshIdentifier)
        await r.engine.handle(task, kind: .refresh).value
        XCTAssertEqual(task.completions, [false])
        XCTAssertEqual(r.scheduler.submitted, [], "a core that does not load is not retried by asking again")
        XCTAssertEqual(r.log.all.count, 1)
        XCTAssertTrue(r.log.all[0].contains(r.engine.refreshIdentifier), r.log.all[0])
    }

    func testARunThatThrowsCompletesTheWindowAsAFailureWithoutCrashing() async {
        let r = rig(FakeBackgroundCore(.fail(UndraCallError.unavailable(.closed))))
        let task = FakeBackgroundTask("t")
        await r.engine.handle(task, kind: .processing).value
        XCTAssertEqual(task.completions, [false])
        XCTAssertEqual(r.log.all.count, 1)
    }

    // MARK: The app goes to the background

    func testWithNoPendingWorkNothingIsScheduled() {
        let core = FakeBackgroundCore(pending: 0)
        let r = rig(core)
        XCTAssertNil(r.engine.lifecycleChanged(core, .background))
        XCTAssertEqual(r.scheduler.submitted, [])
        XCTAssertEqual(r.application.begun, [])
        XCTAssertEqual(core.runs, [])
    }

    func testWithPendingWorkBothWindowsAreAskedForAndTheTransitionDrains() async throws {
        let core = FakeBackgroundCore(pending: 3, .report(UndraBackgroundReport(finished: true, replayed: 3)))
        let r = rig(core)
        let drain = try XCTUnwrap(r.engine.lifecycleChanged(core, .background))
        XCTAssertEqual(r.scheduler.submitted, [
            BackgroundTaskRequest(
                identifier: "com.example.app.undra.processing",
                kind: .processing,
                earliestBeginDate: nil,
                requiresNetworkConnectivity: true
            ),
            BackgroundTaskRequest(
                identifier: "com.example.app.undra.refresh",
                kind: .refresh,
                earliestBeginDate: now.addingTimeInterval(300),
                requiresNetworkConnectivity: false
            ),
        ])
        XCTAssertEqual(r.application.begun, ["dev.undra.background"], "the transition is wrapped in a background task")
        await drain.value
        XCTAssertEqual(core.runs, [20], "it drains inside the grace the OS gives a transition")
        XCTAssertEqual(r.application.ended, [101], "and gives the background task back when it is done")
    }

    func testTheGraceExpiringCancelsTheDrainAndEndsTheTaskOnce() async throws {
        let core = FakeBackgroundCore(pending: 1, .hang)
        let r = rig(core)
        let drain = try XCTUnwrap(r.engine.lifecycleChanged(core, .background))
        let started = await waitUntil { !core.runs.isEmpty }
        XCTAssertTrue(started)
        r.application.expire(101)
        await drain.value
        XCTAssertEqual(r.application.ended, [101], "ended once, by the expiration, not again when the drain returns")
    }

    func testComingBackToTheForegroundLetsGoOfTheGrace() async throws {
        let core = FakeBackgroundCore(pending: 1, .hang)
        let r = rig(core)
        let drain = try XCTUnwrap(r.engine.lifecycleChanged(core, .background))
        _ = await waitUntil { !core.runs.isEmpty }
        XCTAssertNil(r.engine.lifecycleChanged(core, .inactive))
        XCTAssertEqual(r.application.ended, [], "inactive is not a reason to stop")
        XCTAssertNil(r.engine.lifecycleChanged(core, .active))
        await drain.value
        XCTAssertEqual(r.application.ended, [101])
    }

    func testAnOSThatRefusesTheBackgroundTaskStillGetsTheRequests() {
        let core = FakeBackgroundCore(pending: 2)
        let r = rig(core)
        r.application.refuse()
        XCTAssertNil(r.engine.lifecycleChanged(core, .background), "no grace, so no drain")
        XCTAssertEqual(r.scheduler.submitted.count, 2)
        XCTAssertEqual(core.runs, [])
        XCTAssertEqual(r.application.ended, [])
    }

    func testSchedulerErrorsAreLoggedAndSwallowed() async throws {
        let core = FakeBackgroundCore(pending: 1)
        let r = rig(core)
        r.scheduler.failSubmits()
        let drain = try XCTUnwrap(r.engine.lifecycleChanged(core, .background))
        await drain.value
        XCTAssertEqual(r.scheduler.submitted.count, 2, "both are tried even though the first failed")
        XCTAssertEqual(r.log.all.count, 2)
        XCTAssertTrue(r.log.all[0].contains("unavailable"), r.log.all[0])
    }

    // MARK: Which core

    func testAnEngineIgnoresACoreAnotherEngineClaimed() {
        let first = FakeBackgroundCore(pending: 1, .hang)
        let second = FakeBackgroundCore(pending: 1, .hang)
        let a = rig(first, taskIdentifier: "com.example.a")
        let b = rig(second, taskIdentifier: "com.example.b")
        XCTAssertNotNil(a.engine.lifecycleChanged(first, .background))
        XCTAssertNil(b.engine.lifecycleChanged(first, .background), "b does not react to a's core")
        XCTAssertEqual(b.scheduler.submitted, [])
        XCTAssertNotNil(b.engine.lifecycleChanged(second, .background), "b takes the next core")
        XCTAssertNil(a.engine.lifecycleChanged(second, .background))
        XCTAssertEqual(a.scheduler.submitted.count, 2)
        XCTAssertEqual(b.scheduler.submitted.count, 2)
        a.engine.stop()
        b.engine.stop()
    }

    func testAnEngineRebindsWhenItsCoreWasShutDown() {
        let old = FakeBackgroundCore(pending: 1)
        let fresh = FakeBackgroundCore(pending: 1)
        let r = rig(old)
        r.application.refuse()
        _ = r.engine.lifecycleChanged(old, .background)
        XCTAssertEqual(r.scheduler.submitted.count, 2)
        old.shutDown()
        _ = r.engine.lifecycleChanged(fresh, .background)
        XCTAssertEqual(r.scheduler.submitted.count, 4, "the engine follows the core that replaced its own")
    }

    // MARK: Through a real core

    func testTheLifecycleThatARealCoreIsToldReachesTheEngine() async throws {
        let transport = FakeTransport()
        transport.statsDocument = "{\"background\": {\"tasks\": 3, \"pending\": 2}}"
        let core = try makeCore(transport)
        let scheduler = FakeBackgroundScheduler()
        let application = FakeBackgroundApplication()
        let engine = BackgroundEngine(
            taskIdentifier: "com.example.real",
            scheduler: scheduler,
            application: application,
            now: { Date(timeIntervalSince1970: 0) },
            loader: { core }
        )
        engine.register()
        addTeardownBlock {
            engine.stop()
            core.shutdown()
        }
        // Whoever reports the lifecycle: the default adapter, the app's `UndraLifecycle`, or an event of its own.
        core.event(port: StandardPorts.Lifecycle.portId, method: StandardPorts.Lifecycle.changed, payload: UndraAppState.active.undraEncoded())
        XCTAssertEqual(scheduler.submitted, [], "active asks for nothing")
        UndraLifecycle(core: core).changed(.background)
        XCTAssertEqual(scheduler.submitted.map { $0.identifier }, ["com.example.real.processing", "com.example.real.refresh"])
        XCTAssertEqual(application.begun, ["dev.undra.background"])
        // The drain is a call of `run_background` the fake core never answers; coming back ends it.
        let sent = await waitUntil { !transport.calls.isEmpty }
        XCTAssertTrue(sent)
        XCTAssertEqual(transport.calls.first?.target, .freeFunction(methodId: 0x0e5b_14ff))
        UndraLifecycle(core: core).changed(.active)
        let ended = await waitUntil { !application.ended.isEmpty }
        XCTAssertTrue(ended)
    }
}

// MARK: - Finding the core

@MainActor
final class BackgroundCoreResolutionTests: XCTestCase {
    private func realCore() throws -> UndraCore {
        return try makeCore(FakeTransport())
    }

    func testTheLoadersCoreIsUsed() async throws {
        let core = try realCore()
        defer { core.shutdown() }
        let resolved = try await undraResolveBackgroundCore(loader: { core }, current: { nil })
        XCTAssertTrue(resolved === core)
    }

    func testAnAlreadyLoadedCoreIsFoundThroughCurrent() async throws {
        // The foreground case: the app's launch loaded the core, so its generated `load` says so.
        let core = try realCore()
        defer { core.shutdown() }
        let resolved = try await undraResolveBackgroundCore(
            loader: { throw UndraLoadError.alreadyLoaded },
            current: { core }
        )
        XCTAssertTrue(resolved === core)
    }

    func testALoadInProgressIsWaitedFor() async throws {
        // The background-launch case: `App.init` is loading the core while the handler starts.
        let core = try realCore()
        defer { core.shutdown() }
        let polls = Guarded<Int>(0)
        let resolved = try await undraResolveBackgroundCore(
            loader: { throw UndraLoadError.alreadyLoaded },
            current: {
                let n = polls.withLock { (count: inout Int) -> Int in
                    count += 1
                    return count
                }
                return n >= 3 ? core : nil
            },
            attempts: 10,
            pauseNanoseconds: 1_000_000
        )
        XCTAssertTrue(resolved === core)
        XCTAssertEqual(polls.withLock { $0 }, 3)
    }

    func testACoreThatNeverAppearsFailsAfterTheAttempts() async throws {
        do {
            _ = try await undraResolveBackgroundCore(
                loader: { throw UndraLoadError.alreadyLoaded },
                current: { nil },
                attempts: 3,
                pauseNanoseconds: 1_000_000
            )
            XCTFail("there is no core to run")
        } catch {
            XCTAssertEqual(error as? UndraLoadError, .alreadyLoaded)
        }
    }

    func testAShutDownCurrentCoreIsNotUsed() async throws {
        let core = try realCore()
        core.shutdown()
        do {
            _ = try await undraResolveBackgroundCore(
                loader: { throw UndraLoadError.alreadyLoaded },
                current: { core },
                attempts: 2,
                pauseNanoseconds: 1_000_000
            )
            XCTFail("a shut-down core cannot run")
        } catch {
            XCTAssertEqual(error as? UndraLoadError, .alreadyLoaded)
        }
    }

    func testOtherLoadFailuresPropagateAtOnce() async throws {
        let calls = Guarded<Int>(0)
        do {
            _ = try await undraResolveBackgroundCore(
                loader: {
                    calls.withLock { $0 += 1 }
                    throw UndraLoadError.missingCoreTable
                },
                current: { nil }
            )
            XCTFail("the load failed")
        } catch {
            XCTAssertEqual(error as? UndraLoadError, .missingCoreTable)
        }
        XCTAssertEqual(calls.withLock { $0 }, 1)
    }
}
