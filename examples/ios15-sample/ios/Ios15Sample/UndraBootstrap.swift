import Foundation
import UndraRuntime
import Ios15SampleCore

/// Attaches the app to its Rust core, once, before any store is created.
enum UndraBootstrap {
    /// The dev server this process uses (`UNDRA_DEV_URL`, debug builds), or `nil` for the in-process core.
    @MainActor static var devURL: String?

    /// The core `start()` loaded, so a view can show what its connection is doing (`core.connection`, or `core.connectionObject` below iOS 17).
    @MainActor static var core: UndraCore?

    /// Called when `undra dev` restarted the core and could not carry its state over (a schema change, a state over the
    /// limit), so the objects of this app's core are gone: the app loads the new core and starts over on it
    /// (`MainApp.reload`).
    @MainActor static var coreLost: (() -> Void)?

    /// Loads the core linked into the app (`undra build --platform ios`) through the bindings' entry,
    /// `UndraIos15SampleCore`, which checks it was built from their schema. In debug builds, when
    /// `UNDRA_DEV_URL` is set (for example `ws://192.168.1.20:7443`), attaches to the core that
    /// `undra dev` serves instead: edit the Rust, save, and the app is on the rebuilt core within a second, with its state
    /// and no rebuild of the app (ADR-053).
    @MainActor
    static func start() throws {
        #if DEBUG
        if let url = ProcessInfo.processInfo.environment["UNDRA_DEV_URL"], !url.isEmpty {
            devURL = url
            core = try UndraIos15SampleCore.load(.remote(
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
        core = try UndraIos15SampleCore.load()
    }
}
