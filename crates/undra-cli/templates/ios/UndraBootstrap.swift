import Foundation
import UndraRuntime
import @@SWIFT_MODULE@@

/// Attaches the app to its Rust core, once, before any store is created.
enum UndraBootstrap {
    /// The dev server this process uses (`UNDRA_DEV_URL`, debug builds), or `nil` for the in-process core.
    @MainActor static var devURL: String?

    /// The core `start()` loaded, so a view can show what its connection is doing (`core.connection`).
    @MainActor static var core: UndraCore?

    /// Called when `undra dev` restarted the core and the objects of this app's core are gone: the app loads the new
    /// core and starts over on it (`MainApp.reload`).
    @MainActor static var coreLost: (() -> Void)?

    /// Loads the core linked into the app (`undra build --platform ios`). In debug builds, when
    /// `UNDRA_DEV_URL` is set (for example `ws://192.168.1.20:7443`), attaches to the core that
    /// `undra dev` serves instead: edit the Rust, save, and the app is on the rebuilt core within a second, no rebuild
    /// of the app.
    @MainActor
    static func start() throws {
        #if DEBUG
        if let url = ProcessInfo.processInfo.environment["UNDRA_DEV_URL"], !url.isEmpty {
            devURL = url
            core = try UndraCore.load(.remote(
                url: url,
                expectedSchemaHash: UndraIds.schemaHash,
                // The runtime reconnects by itself; when it finds a new core instead of its own, it says so.
                onConnectionChange: { state in
                    if case .closed(.sessionLost) = state {
                        Task { @MainActor in coreLost?() }
                    }
                }
            ))
            return
        }
        #endif
        core = try UndraCore.load(.inproc(expectedSchemaHash: UndraIds.schemaHash))
    }
}
