// UndraBackground: iOS background execution with BackgroundTasks (ADR-046, decision 3.4). The decisions
// are in BackgroundEngine.swift and run in tests on macOS; this file binds them to BGTaskScheduler and
// UIApplication.

#if os(iOS)
import BackgroundTasks
import Foundation
import UIKit

/// Gives the core the background windows the OS grants: offline mutations replay, stale queries refetch
/// and persistence flushes while the app is not on screen (ADR-046).
///
/// Register once at launch, before the app finishes launching (`App.init`, or
/// `application(_:didFinishLaunchingWithOptions:)`), with the generated loader of your core:
///
/// ```swift
/// @main struct MyApp: App {
///     init() {
///         UndraBackground.register(taskIdentifier: "com.example.app.undra") { try UndraMyCore.load() }
///         // ... load the core, create the stores
///     }
/// }
/// ```
///
/// Registering does three things.
///
/// 1. **Two windows.** It registers a `BGProcessingTask` (needs the network; minutes) under
///    `<taskIdentifier>.processing` and a `BGAppRefreshTask` (about 30 seconds) under
///    `<taskIdentifier>.refresh`. Both identifiers must be in the Info.plist's
///    `BGTaskSchedulerPermittedIdentifiers`, and `UIBackgroundModes` must list `processing` and `fetch`
///    (`undra init` writes all of it for `<bundle id>.undra`).
/// 2. **The window's work.** When the OS grants a window the runtime loads the core, if the app was
///    launched in the background and has not loaded it (`loader`: it is called, and may return the core
///    that is already loaded or throw `UndraLoadError.alreadyLoaded`, in which case the loaded core is
///    used), runs ``UndraCore/runInBackground(deadline:)`` with the window's deadline, cancels the run from the
///    task's expiration handler (the core keeps what it did), and calls `setTaskCompleted(success:)` with
///    whether everything finished. A window that did not finish asks for another.
/// 3. **Asking for windows.** When the app enters the background and the core has work waiting
///    (`stats().background.pending > 0` after it processed `Lifecycle.Background`), the runtime submits both
///    requests (the processing one with `requiresNetworkConnectivity`, the refresh one a few minutes out)
///    and wraps the transition in `UIApplication.beginBackgroundTask`, draining in its grace (about 30
///    seconds) so that a mutation in flight finishes. With nothing pending it schedules nothing. Errors from
///    `BGTaskScheduler` (the simulator, an identifier missing from the Info.plist, too many requests) are
///    logged and swallowed.
///
/// It listens to the lifecycle the core is told (``LifecycleAdapter`` does it by default, or the app's own
/// ``UndraLifecycle`` calls), so there is nothing else to wire. A process with several cores registers
/// once per core, each with its own `taskIdentifier` and a loader that returns the core of its namespace
/// (`{ Undra<Ns>.core.isShutDown ? try Undra<Ns>.load() : Undra<Ns>.core }`).
///
/// ## Trying it in the simulator
///
/// The simulator does not run background tasks by itself. Run the app from Xcode, pause it in the debugger
/// and evaluate (Apple's documented way, one line each):
///
///     e -l objc -- (void)[[BGTaskScheduler sharedScheduler] _simulateLaunchForTaskWithIdentifier:@"com.example.app.undra.refresh"]
///     e -l objc -- (void)[[BGTaskScheduler sharedScheduler] _simulateExpirationForTaskWithIdentifier:@"com.example.app.undra.refresh"]
///
/// The first runs the handler (the core runs in the window and the task completes), the second calls the
/// expiration handler while it runs. Use the `.processing` identifier for the processing window.
@available(iOSApplicationExtension, unavailable)
public enum UndraBackground {
    /// How long the windows are used and when the next one is asked for.
    public struct Options: Sendable, Equatable {
        /// The deadline of a processing window, seconds. Default 180: processing windows last minutes, and
        /// the expiration handler cuts a run short if the OS is quicker.
        public var processingDeadline: TimeInterval
        /// The deadline of a refresh window, seconds. Default 25: the OS gives about 30.
        public var refreshDeadline: TimeInterval
        /// How far out the refresh request's earliest begin date is, seconds. Default 300; the OS decides
        /// the real time (usually much later).
        public var refreshDelay: TimeInterval
        /// The deadline of the drain that rides the grace of the transition to the background, seconds.
        /// Default 20: the OS grants about 30.
        public var transitionDeadline: TimeInterval

        /// Creates options; the defaults are the documented ones.
        public init(
            processingDeadline: TimeInterval = 180,
            refreshDeadline: TimeInterval = 25,
            refreshDelay: TimeInterval = 300,
            transitionDeadline: TimeInterval = 20
        ) {
            self.processingDeadline = processingDeadline
            self.refreshDeadline = refreshDeadline
            self.refreshDelay = refreshDelay
            self.transitionDeadline = transitionDeadline
        }
    }

    private static let engines = Guarded<[String: BackgroundEngine]>([:])

    /// Registers the two background windows of `taskIdentifier` and starts scheduling them.
    ///
    /// Call it once at launch, before the app finishes launching: `BGTaskScheduler` refuses registrations
    /// made later. Registering the same `taskIdentifier` again does nothing (a reload of the dev core calls
    /// the app's start-up code again).
    ///
    /// - Parameters:
    ///   - taskIdentifier: the prefix of the two identifiers, `<taskIdentifier>.processing` and
    ///     `<taskIdentifier>.refresh`, which the Info.plist lists in `BGTaskSchedulerPermittedIdentifiers`.
    ///   - options: the deadlines.
    ///   - loader: returns the core, loading it if it is not loaded: the generated `Undra<Ns>.load`.
    public static func register(
        taskIdentifier: String,
        options: Options = Options(),
        loader: @escaping @Sendable () async throws -> UndraCore
    ) {
        let engine = BackgroundEngine(
            taskIdentifier: taskIdentifier,
            scheduler: SystemBackgroundScheduler(),
            application: SystemBackgroundApplication(),
            options: BackgroundOptions(
                processingDeadline: options.processingDeadline,
                refreshDeadline: options.refreshDeadline,
                refreshDelay: options.refreshDelay,
                transitionDeadline: options.transitionDeadline
            ),
            loader: {
                return try await undraResolveBackgroundCore(loader: loader)
            }
        )
        let fresh = engines.withLock { (all: inout [String: BackgroundEngine]) -> Bool in
            if all[taskIdentifier] != nil {
                return false
            }
            all[taskIdentifier] = engine
            return true
        }
        if fresh {
            engine.register()
        }
    }
}

// MARK: - The system's side of the seams

/// A `BGTask`.
private final class SystemBackgroundTask: BackgroundTaskHandle, @unchecked Sendable {
    private let task: BGTask
    private let handler = Guarded<(@Sendable () -> Void)?>(nil)

    init(_ task: BGTask) {
        self.task = task
    }

    var identifier: String {
        return task.identifier
    }

    var expirationHandler: (@Sendable () -> Void)? {
        get {
            return handler.withLock { (current: inout (@Sendable () -> Void)?) -> (@Sendable () -> Void)? in
                return current
            }
        }
        set {
            handler.withLock { (current: inout (@Sendable () -> Void)?) -> Void in
                current = newValue
            }
            task.expirationHandler = newValue
        }
    }

    func setTaskCompleted(success: Bool) {
        task.setTaskCompleted(success: success)
    }
}

/// `BGTaskScheduler.shared`.
private struct SystemBackgroundScheduler: BackgroundScheduler {
    func register(identifier: String, handler: @escaping @Sendable (any BackgroundTaskHandle) -> Void) -> Bool {
        return BGTaskScheduler.shared.register(forTaskWithIdentifier: identifier, using: nil) { task in
            handler(SystemBackgroundTask(task))
        }
    }

    func submit(_ request: BackgroundTaskRequest) throws {
        switch request.kind {
        case .processing:
            let processing = BGProcessingTaskRequest(identifier: request.identifier)
            processing.requiresNetworkConnectivity = request.requiresNetworkConnectivity
            processing.requiresExternalPower = false
            processing.earliestBeginDate = request.earliestBeginDate
            try BGTaskScheduler.shared.submit(processing)
        case .refresh:
            let refresh = BGAppRefreshTaskRequest(identifier: request.identifier)
            refresh.earliestBeginDate = request.earliestBeginDate
            try BGTaskScheduler.shared.submit(refresh)
        }
    }
}

/// `UIApplication.shared`, which is main-thread only: a call from another thread is declined (begin) or
/// hopped (end) rather than waited for.
@available(iOSApplicationExtension, unavailable)
private struct SystemBackgroundApplication: BackgroundApplication {
    func beginBackgroundTask(named name: String, expiration: @escaping @Sendable () -> Void) -> Int? {
        guard Thread.isMainThread else {
            UndraLog.warning("the lifecycle was reported off the main thread: the transition to the background is not wrapped in a background task")
            return nil
        }
        return MainActor.assumeIsolated {
            let identifier = UIApplication.shared.beginBackgroundTask(withName: name) {
                expiration()
            }
            return identifier == .invalid ? nil : identifier.rawValue
        }
    }

    func endBackgroundTask(_ token: Int) {
        if Thread.isMainThread {
            MainActor.assumeIsolated {
                UIApplication.shared.endBackgroundTask(UIBackgroundTaskIdentifier(rawValue: token))
            }
        } else {
            DispatchQueue.main.async {
                UIApplication.shared.endBackgroundTask(UIBackgroundTaskIdentifier(rawValue: token))
            }
        }
    }
}
#endif
