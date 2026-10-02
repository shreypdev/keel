// Where the default storage adapters keep their data (docs/SPEC.md section 8, ADR-044 amendment A).
//
// One rule for `Kv`, `Fs`, `SecureStore` and `Db`: the location is per core namespace, so two cores
// of one app never share a store unless the app hands them one of its own.
//
//   Kv, Fs, Db   <Application Support>/<bundle id>/undra/<namespace>/{kv,fs,db}
//   SecureStore  Keychain service "<namespace>.dev.undra.securestore"
//
// The namespace is the core's (`UndraCore.namespace`), known when the adapter's port is made for a
// core (`makePortImpl(core:)`), so a default adapter value can be created before any core exists and
// shared by several. The React Native module's native stores use the same directories and service.

import Foundation

/// The default locations of the Apple adapters.
enum StorageLocations {
    /// The Keychain service of the default `SecureStore`, before the namespace is put in front.
    static let secureStoreService = "dev.undra.securestore"

    /// `<Application Support>/<bundle id>/undra`: the root every core's data is under.
    static func root() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
        let bundle = Bundle.main.bundleIdentifier ?? "app"
        return base
            .appendingPathComponent(bundle, isDirectory: true)
            .appendingPathComponent("undra", isDirectory: true)
    }

    /// `<Application Support>/<bundle id>/undra/<namespace>/<store>`, `store` being `kv`, `fs` or `db`.
    static func directory(namespace: String, store: String) -> URL {
        return root()
            .appendingPathComponent(namespace, isDirectory: true)
            .appendingPathComponent(store, isDirectory: true)
    }

    /// `<namespace>.dev.undra.securestore`: the Keychain service of the default `SecureStore`.
    static func keychainService(namespace: String) -> String {
        return namespace + "." + secureStoreService
    }
}
