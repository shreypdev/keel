import Foundation
import OSLog
import UndraRuntime
import PlaygroundCore

/// Attaches the app to its Rust core, once, before any store is created.
enum UndraBootstrap {
    /// The server the remote screen talks to. The address is made up: the app answers the `Http`
    /// port itself (`PlaygroundNetwork`), so nothing leaves the device.
    static let serverURL = "https://playground.undra.test"

    /// The server list the remote screen shows.
    static let inboxList = "inbox"

    /// Where failures that nobody could catch end up: a command (`todos.toggle(id:)`) that the core
    /// refused, a change a store could not apply. Undra has already logged each one; this puts it
    /// in the app's own log too. The handler runs on the thread that made the call and must not
    /// call back into Undra. In a debug build you could stop at the failing line instead:
    /// `{ assertionFailure("\($0)") }`.
    private static let onError: @Sendable (UndraUnhandledError) -> Void = { unhandled in
        Logger(subsystem: "dev.undra.playground", category: "undra")
            .error("\(unhandled.description, privacy: .public)")
    }

    /// Loads the core linked into the app (`undra build --platform ios`) with the default adapters,
    /// except that `Http` is the in-memory server, `Connectivity` is the one the Offline switch
    /// drives (the app's network is simulated, so its connectivity is too) and `Kv` is emptied at launch. In debug builds, when
    /// `UNDRA_DEV_URL` is set (for example `ws://192.168.1.20:7443`), attaches to the core that
    /// `undra dev` serves instead: edit the Rust, save, relaunch the app, no rebuild of the app.
    @MainActor
    static func start() throws {
        // The server the app carries forgets everything when the app quits, so the core's cache of
        // it must too: the key-value store (the query cache, the offline queue) lives in a temporary
        // directory that every launch starts empty.
        let store = FileManager.default.temporaryDirectory.appendingPathComponent("playground-kv", isDirectory: true)
        try? FileManager.default.removeItem(at: store)
        let adapters = Adapters.platformDefault
            .removing(portId: fnv1a32("port.Connectivity"))
            .replacing(PlaygroundNetwork.shared)
            .replacing(KvAdapter(directory: store))
        #if DEBUG
        if let url = ProcessInfo.processInfo.environment["UNDRA_DEV_URL"], !url.isEmpty {
            try UndraCore.load(.remote(url: url, adapters: adapters, expectedSchemaHash: UndraIds.schemaHash, onError: onError))
            configureRemote(RemoteConfig(baseUrl: serverURL))
            return
        }
        #endif
        try UndraCore.load(.inproc(adapters: adapters, expectedSchemaHash: UndraIds.schemaHash, onError: onError))
        // Tell the core where the server is, before anything observes the remote list.
        configureRemote(RemoteConfig(baseUrl: serverURL))
    }
}
