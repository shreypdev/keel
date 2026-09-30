import Foundation
import KeelRuntime
import PlaygroundCore

/// Attaches the app to its Rust core, once, before any store is created.
enum KeelBootstrap {
    /// The server the remote screen talks to. The address is made up: the app answers the `Http`
    /// port itself (`PlaygroundNetwork`), so nothing leaves the device.
    static let serverURL = "https://playground.keel.test"

    /// The server list the remote screen shows.
    static let inboxList = "inbox"

    /// Loads the core linked into the app (`keel build --platform ios`) with the default adapters,
    /// except that `Http` is the in-memory server and `Connectivity` is the one the Offline switch
    /// drives: the app's network is simulated, so its connectivity is too. In debug builds, when
    /// `KEEL_DEV_URL` is set (for example `ws://192.168.1.20:7443`), attaches to the core that
    /// `keel dev` serves instead: edit the Rust, save, relaunch the app, no rebuild of the app.
    @MainActor
    static func start() throws {
        let network = PlaygroundNetwork.shared
        let adapters = Adapters.platformDefault
            .removing(portId: fnv1a32("port.Connectivity"))
            .replacing(network)
        #if DEBUG
        if let url = ProcessInfo.processInfo.environment["KEEL_DEV_URL"], !url.isEmpty {
            try KeelCore.load(.remote(url: url, adapters: adapters, expectedSchemaHash: KeelIds.schemaHash))
            configureRemote(RemoteConfig(baseUrl: serverURL))
            return
        }
        #endif
        try KeelCore.load(.inproc(adapters: adapters, expectedSchemaHash: KeelIds.schemaHash))
        // Tell the core where the server is, before anything observes the remote list.
        configureRemote(RemoteConfig(baseUrl: serverURL))
    }
}
