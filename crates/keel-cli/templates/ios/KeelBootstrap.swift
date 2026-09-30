import Foundation
import KeelRuntime
import @@SWIFT_MODULE@@

/// Attaches the app to its Rust core, once, before any store is created.
enum KeelBootstrap {
    /// Loads the core linked into the app (`keel build --platform ios`). In debug builds, when
    /// `KEEL_DEV_URL` is set (for example `ws://192.168.1.20:7443`), attaches to the core that
    /// `keel dev` serves instead: edit the Rust, save, relaunch the app, no rebuild of the app.
    @MainActor
    static func start() throws {
        #if DEBUG
        if let url = ProcessInfo.processInfo.environment["KEEL_DEV_URL"], !url.isEmpty {
            try KeelCore.load(.remote(url: url, expectedSchemaHash: KeelIds.schemaHash))
            return
        }
        #endif
        try KeelCore.load(.inproc(expectedSchemaHash: KeelIds.schemaHash))
    }
}
