import Foundation
import UndraRuntime
import @@SWIFT_MODULE@@

/// Attaches the app to its Rust core, once, before any store is created.
enum UndraBootstrap {
    /// Loads the core linked into the app (`undra build --platform ios`). In debug builds, when
    /// `UNDRA_DEV_URL` is set (for example `ws://192.168.1.20:7443`), attaches to the core that
    /// `undra dev` serves instead: edit the Rust, save, relaunch the app, no rebuild of the app.
    @MainActor
    static func start() throws {
        #if DEBUG
        if let url = ProcessInfo.processInfo.environment["UNDRA_DEV_URL"], !url.isEmpty {
            try UndraCore.load(.remote(url: url, expectedSchemaHash: UndraIds.schemaHash))
            return
        }
        #endif
        try UndraCore.load(.inproc(expectedSchemaHash: UndraIds.schemaHash))
    }
}
