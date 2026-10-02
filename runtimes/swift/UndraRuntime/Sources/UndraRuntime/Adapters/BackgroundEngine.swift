// Background scheduling (ADR-046, decision 3.4): when and how the app asks the core to use a background
// window, written against four small seams so that every decision runs in tests on macOS. The iOS
// binding of the seams (BackgroundTasks, UIApplication) and the public entry, `UndraBackground`, are in
// UndraBackground.swift.
//
//   The OS grants a window  ->  `handle(_:kind:)`: load the core if it is not loaded, run it with the window's
//                               deadline, cancel it when the window is taken back, report the outcome.
//   The app goes to the background with work waiting  ->  `lifecycleChanged(_:_:)`: ask the OS for windows
//                               and use the grace the OS gives a transition to drain what can be drained.

import Foundation

// MARK: - The seams

/// What the engine needs of a `BGTask`.
protocol BackgroundTaskHandle: AnyObject, Sendable {
    /// The task's identifier.
    var identifier: String { get }
    /// Called by the OS when the window is about to be taken back.
    var expirationHandler: (@Sendable () -> Void)? { get set }
    /// Tells the OS the task is done; calling it twice is a bug the engine never commits.
    func setTaskCompleted(success: Bool)
}

/// The two kinds of window.
enum BackgroundTaskKind: Sendable, Equatable {
    /// A `BGProcessingTask`: minutes, needs the network, often while charging.
    case processing
    /// A `BGAppRefreshTask`: about 30 seconds.
    case refresh
}

/// A request for a window (a `BGTaskRequest`).
struct BackgroundTaskRequest: Sendable, Equatable {
    var identifier: String
    var kind: BackgroundTaskKind
    var earliestBeginDate: Date?
    var requiresNetworkConnectivity: Bool
}

/// `BGTaskScheduler`.
protocol BackgroundScheduler: Sendable {
    /// Registers the launch handler of `identifier`; `false` if the OS refused.
    func register(identifier: String, handler: @escaping @Sendable (any BackgroundTaskHandle) -> Void) -> Bool
    /// Asks for a window. Throws what the OS throws (the simulator, an identifier missing from the Info.plist).
    func submit(_ request: BackgroundTaskRequest) throws
}

/// The part of `UIApplication` that extends the app's life across a transition.
protocol BackgroundApplication: Sendable {
    /// `beginBackgroundTask`: a token while the OS grants the grace, `nil` when it refuses (or the call is not on the main thread).
    func beginBackgroundTask(named name: String, expiration: @escaping @Sendable () -> Void) -> Int?
    /// `endBackgroundTask`.
    func endBackgroundTask(_ token: Int)
}

/// The core, as far as scheduling needs it: `UndraCore` and the fakes of the tests.
protocol BackgroundCore: AnyObject, Sendable {
    /// Whether the core was shut down: a window cannot use it, and an engine may bind to another.
    var isShutDown: Bool { get }
    func runInBackground(deadline: TimeInterval) async throws -> UndraBackgroundReport
    /// `stats().background.pending`: work a window would drain.
    func pendingBackgroundWork() -> Int
}

extension UndraCore: BackgroundCore {
    func pendingBackgroundWork() -> Int {
        return stats().background.pending
    }
}

/// The deadlines and delays of the engine; ``UndraBackground/Options`` is its public face.
struct BackgroundOptions: Sendable, Equatable {
    var processingDeadline: TimeInterval = 180
    var refreshDeadline: TimeInterval = 25
    var refreshDelay: TimeInterval = 300
    var transitionDeadline: TimeInterval = 20
}

// MARK: - Resolving the core

/// The core a background window runs on: what `loader` returns, and, when `loader` says the core is
/// already loaded (it was loaded at launch, as in a SwiftUI app whose `App.init` loads it, or it is being
/// loaded right now by the launch that woke the app), the core `UndraCore.current` holds, waiting up to
/// `attempts * pause` for a load in progress to finish.
func undraResolveBackgroundCore(
    loader: @Sendable () async throws -> UndraCore,
    current: @Sendable () -> UndraCore? = { UndraCore.current },
    attempts: Int = 50,
    pauseNanoseconds: UInt64 = 100_000_000
) async throws -> UndraCore {
    var tries = 0
    while true {
        do {
            return try await loader()
        } catch UndraLoadError.alreadyLoaded {
            if let core = current(), !core.isShutDown {
                return core
            }
            tries += 1
            if tries >= attempts {
                throw UndraLoadError.alreadyLoaded
            }
            try await Task.sleep(nanoseconds: pauseNanoseconds)
        }
    }
}

// MARK: - The engine

/// One registration of ``UndraBackground``: the identifiers, the handlers, the lifecycle reaction.
final class BackgroundEngine: @unchecked Sendable {
    /// A drain that rides the grace of a transition to the background.
    private struct Transition {
        var token: Int
        var work: Task<Void, Never>
    }

    private struct State {
        var registered = false
        var core: WeakCore?
        var transition: Transition?
        var lifecycleToken: Int?
    }

    private struct WeakCore {
        weak var core: (any BackgroundCore)?
    }

    /// Which engine owns which core, so that two registrations (two cores in one process) do not both react
    /// to the same core's lifecycle.
    private static let claims = Guarded<[(core: WeakCore, engine: ObjectIdentifier)]>([])

    let taskIdentifier: String
    private let scheduler: any BackgroundScheduler
    private let application: any BackgroundApplication
    private let loader: @Sendable () async throws -> any BackgroundCore
    private let options: BackgroundOptions
    private let now: @Sendable () -> Date
    private let log: @Sendable (String) -> Void
    private let state = Guarded<State>(State())

    init(
        taskIdentifier: String,
        scheduler: any BackgroundScheduler,
        application: any BackgroundApplication,
        options: BackgroundOptions = BackgroundOptions(),
        now: @escaping @Sendable () -> Date = { Date() },
        log: @escaping @Sendable (String) -> Void = { UndraLog.warning($0) },
        loader: @escaping @Sendable () async throws -> any BackgroundCore
    ) {
        self.taskIdentifier = taskIdentifier
        self.scheduler = scheduler
        self.application = application
        self.options = options
        self.now = now
        self.log = log
        self.loader = loader
    }

    /// The identifier of the processing window: `<taskIdentifier>.processing`.
    var processingIdentifier: String {
        return taskIdentifier + ".processing"
    }

    /// The identifier of the refresh window: `<taskIdentifier>.refresh`.
    var refreshIdentifier: String {
        return taskIdentifier + ".refresh"
    }

    // MARK: Registering

    /// Registers both launch handlers with the scheduler and starts listening to the lifecycle. Once per
    /// engine: a second call does nothing and returns `false`.
    @discardableResult
    func register() -> Bool {
        let first = state.withLock { (current: inout State) -> Bool in
            if current.registered {
                return false
            }
            current.registered = true
            return true
        }
        if !first {
            return false
        }
        for (identifier, kind) in [(processingIdentifier, BackgroundTaskKind.processing), (refreshIdentifier, .refresh)] {
            let accepted = scheduler.register(identifier: identifier) { [weak self] task in
                _ = self?.handle(task, kind: kind)
            }
            if !accepted {
                log("BGTaskScheduler did not register \(identifier): is it listed in BGTaskSchedulerPermittedIdentifiers (Info.plist), and is this called before the app finished launching?")
            }
        }
        let token = UndraCore.addLifecycleObserver { [weak self] core, reported in
            _ = self?.lifecycleChanged(core, reported)
        }
        state.withLock { (current: inout State) -> Void in
            current.lifecycleToken = token
        }
        return true
    }

    /// Stops listening to the lifecycle (tests; an app registers for the life of the process).
    func stop() {
        let token = state.withLock { (current: inout State) -> Int? in
            let old = current.lifecycleToken
            current.lifecycleToken = nil
            return old
        }
        if let token = token {
            UndraCore.removeLifecycleObserver(token)
        }
        endTransition()
    }

    // MARK: The OS grants a window

    /// Runs the core in the window of `task`: loads it if needed, runs it for the kind's deadline, cancels
    /// the run when the OS takes the window back, and reports the outcome. `setTaskCompleted` is called
    /// exactly once, whatever happens, and nothing here throws or traps (R6). The returned task finishes
    /// when the window has been handled (tests await it).
    @discardableResult
    func handle(_ task: any BackgroundTaskHandle, kind: BackgroundTaskKind) -> Task<Void, Never> {
        let completion = Completion(task)
        let deadline = kind == .processing ? options.processingDeadline : options.refreshDeadline
        let work = Task { [self] in
            do {
                let core = try await loader()
                adopt(core)
                let report = try await core.runInBackground(deadline: deadline)
                // `finished` already means nothing is pending; a refresh window must not claim more.
                let done = report.finished && report.stillPending == 0
                if !done {
                    scheduleRequests()
                }
                completion.complete(success: done)
            } catch is CancellationError {
                // The OS took the window back mid-run: what was done is kept, ask for another window.
                scheduleRequests()
                completion.complete(success: false)
            } catch {
                log("the background window \(task.identifier) failed: \(error)")
                completion.complete(success: false)
            }
        }
        task.expirationHandler = {
            work.cancel()
        }
        return work
    }

    /// Calls `setTaskCompleted` once.
    private final class Completion: @unchecked Sendable {
        private let task: any BackgroundTaskHandle
        private let done = Guarded<Bool>(false)

        init(_ task: any BackgroundTaskHandle) {
            self.task = task
        }

        func complete(success: Bool) {
            let first = done.withLock { (finished: inout Bool) -> Bool in
                if finished {
                    return false
                }
                finished = true
                return true
            }
            if first {
                task.setTaskCompleted(success: success)
            }
        }
    }

    // MARK: The app changes phase

    /// Reacts to `core` being told the app is in `reported`: on `.background`, with work waiting, asks for
    /// windows and drains inside the transition's grace; on `.active`, lets go of the grace. Returns the
    /// drain, for tests to await.
    @discardableResult
    func lifecycleChanged(_ core: any BackgroundCore, _ reported: UndraAppState) -> Task<Void, Never>? {
        guard adopt(core) else {
            return nil
        }
        switch reported {
        case .active:
            endTransition()
            return nil
        case .inactive:
            return nil
        case .background:
            // The core has processed the event by now (`undra_event` returns after its subscribers ran).
            if core.pendingBackgroundWork() <= 0 {
                return nil
            }
            scheduleRequests()
            return beginTransition(core)
        }
    }

    /// Wraps the transition in a background task and spends it draining.
    private func beginTransition(_ core: any BackgroundCore) -> Task<Void, Never>? {
        endTransition()
        guard let token = application.beginBackgroundTask(named: "dev.undra.background", expiration: { [weak self] in
            self?.endTransition()
        }) else {
            return nil
        }
        let deadline = options.transitionDeadline
        let work = Task { [weak self] in
            _ = try? await core.runInBackground(deadline: deadline)
            self?.finishTransition(token)
        }
        let kept = state.withLock { (current: inout State) -> Bool in
            if current.transition != nil {
                return false
            }
            current.transition = Transition(token: token, work: work)
            return true
        }
        if !kept {
            work.cancel()
            application.endBackgroundTask(token)
        }
        return work
    }

    /// The drain returned: end the background task if it is still the current one.
    private func finishTransition(_ token: Int) {
        let ended = state.withLock { (current: inout State) -> Bool in
            if current.transition?.token == token {
                current.transition = nil
                return true
            }
            return false
        }
        if ended {
            application.endBackgroundTask(token)
        }
    }

    /// Cancels the drain and ends the background task: the OS took the grace back, or the app is active again.
    private func endTransition() {
        let transition = state.withLock { (current: inout State) -> Transition? in
            let old = current.transition
            current.transition = nil
            return old
        }
        if let transition = transition {
            transition.work.cancel()
            application.endBackgroundTask(transition.token)
        }
    }

    // MARK: Asking for windows

    /// Submits a processing request (network required) and a refresh request (a few minutes out). The OS
    /// decides whether and when; errors (the simulator has no scheduler, a missing Info.plist entry, too
    /// many requests) are logged and swallowed.
    func scheduleRequests() {
        let requests = [
            BackgroundTaskRequest(
                identifier: processingIdentifier,
                kind: .processing,
                earliestBeginDate: nil,
                requiresNetworkConnectivity: true
            ),
            BackgroundTaskRequest(
                identifier: refreshIdentifier,
                kind: .refresh,
                earliestBeginDate: now().addingTimeInterval(options.refreshDelay),
                requiresNetworkConnectivity: false
            ),
        ]
        for request in requests {
            do {
                try scheduler.submit(request)
            } catch {
                log("could not schedule the background task \(request.identifier): \(error)")
            }
        }
    }

    // MARK: Which core

    /// Binds this engine to `core` if it is not bound to a live core yet and no other engine claimed `core`.
    /// Returns whether `core` is this engine's.
    @discardableResult
    private func adopt(_ core: any BackgroundCore) -> Bool {
        let me = ObjectIdentifier(self)
        let mine = state.withLock { (current: inout State) -> Bool? in
            if let bound = current.core?.core, !bound.isShutDown {
                return bound === core
            }
            return nil
        }
        if let mine = mine {
            return mine
        }
        let claimed = BackgroundEngine.claims.withLock { (all: inout [(core: WeakCore, engine: ObjectIdentifier)]) -> Bool in
            all.removeAll { $0.core.core == nil || $0.core.core?.isShutDown == true }
            if let existing = all.first(where: { $0.core.core === core }) {
                return existing.engine == me
            }
            all.append((core: WeakCore(core: core), engine: me))
            return true
        }
        if claimed {
            state.withLock { (current: inout State) -> Void in
                current.core = WeakCore(core: core)
            }
        }
        return claimed
    }
}
