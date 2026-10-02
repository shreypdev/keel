import Foundation
import UndraRuntime
import @@SWIFT_MODULE@@

/// Attaches the app to its Rust core, once, before any store is created.
enum UndraBootstrap {
    /// The dev server this process uses (`UNDRA_DEV_URL`, debug builds), or `nil` for the in-process core.
    @MainActor static var devURL: String?

    /// The core `start()` loaded, so a view can show what its connection is doing (`core.connection`).
    @MainActor static var core: UndraCore?

    /// Called when `undra dev` restarted the core and could not carry its state over (a schema change, a state over the
    /// limit), so the objects of this app's core are gone: the app loads the new core and starts over on it
    /// (`MainApp.reload`).
    @MainActor static var coreLost: (() -> Void)?

    /// Loads the core linked into the app (`undra build --platform ios`) through the bindings' entry,
    /// `@@CORE_ENTRY@@`, which checks it was built from their schema. In debug builds, when
    /// `UNDRA_DEV_URL` is set (for example `ws://192.168.1.20:7443`), attaches to the core that
    /// `undra dev` serves instead: edit the Rust, save, and the app is on the rebuilt core within a second, with its state
    /// and no rebuild of the app (ADR-053).
    @MainActor
    static func start() throws {
        registerBackground()
        #if DEBUG
        if let url = ProcessInfo.processInfo.environment["UNDRA_DEV_URL"], !url.isEmpty {
            devURL = url
            core = try @@CORE_ENTRY@@.load(.remote(
                url: url,
                // The runtime reconnects by itself; when it finds a new core instead of its own, it says so.
                onConnectionChange: { state in
                    if case .closed(.sessionLost) = state {
                        Task { @MainActor in coreLost?() }
                    }
                },
                // What the dev server says about a reload ("Reloaded, state kept"), for the status bar.
                onDevNotice: { message in
                    Task { @MainActor in DevNotice.shared.show(message) }
                }
            ))
            return
        }
        #endif
        core = try @@CORE_ENTRY@@.load()
    }

    /// Lets the OS give the app background windows (ADR-046): the runtime then replays offline mutations that
    /// are still queued and refetches stale queries while the app is not on screen, and asks for the next window
    /// when the app goes to the background with work waiting. It must run before the app finishes launching (this
    /// is called from `MainApp.init`) and once: a second call, after `undra dev` reloaded the core, does nothing.
    /// The identifiers (`<bundle id>.undra.processing` and `.refresh`) are listed in `Config/Info.plist`. The
    /// loader is only used when the OS launches the app in the background and the core is not loaded yet.
    ///
    /// To try it on a device (the simulator has no background scheduler), background the app with work waiting, pause it in the debugger and evaluate
    /// `e -l objc -- (void)[[BGTaskScheduler sharedScheduler] _simulateLaunchForTaskWithIdentifier:@"<bundle id>.undra.refresh"]`.
    private static func registerBackground() {
        guard let bundleId = Bundle.main.bundleIdentifier else {
            return
        }
        UndraBackground.register(taskIdentifier: bundleId + ".undra") {
            try @@CORE_ENTRY@@.load()
        }
    }
}
