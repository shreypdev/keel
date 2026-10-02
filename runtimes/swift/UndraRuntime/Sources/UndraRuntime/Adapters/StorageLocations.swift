// Where the default storage adapters keep their data (docs/SPEC.md section 8, ADR-044 amendment A).
//
// One rule for `Kv`, `Fs`, `SecureStore` and `Db`: the location is per core namespace, so two cores
// of one app never share a store unless the app hands them one of its own.
//
//   Kv, Fs, Db   <Application Support>/<bundle id>/Undra/<namespace>/{kv,fs,db}
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

    /// `<Application Support>/<bundle id>/Undra`: the root every core's data is under. The casing is the one the Swift
    /// runtime has always used on disk: the file systems of Apple platforms are case-insensitive, and a directory that
    /// differs from an existing one only in case is "there" for `mkdir` yet cannot be created in under the simulator,
    /// so renaming it to `undra` would break every container that holds the older directory.
    static func root() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
        let bundle = Bundle.main.bundleIdentifier ?? "app"
        return base
            .appendingPathComponent(bundle, isDirectory: true)
            .appendingPathComponent("Undra", isDirectory: true)
    }

    /// `<Application Support>/<bundle id>/Undra/<namespace>/<store>`, `store` being `kv`, `fs` or `db`.
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

/// The rule a core's namespace follows (`undra.toml` `[core] namespace`, SPEC 13): the default stores are kept in
/// directories and Keychain services named after it, so what `LoadOptions.namespace` says is checked before it
/// can reach a path.
enum CoreNamespace {
    /// The longest namespace, in bytes.
    static let maxLength = 32

    /// What is wrong with `namespace`, or `nil` when it is one: a lowercase letter, then lowercase letters, digits
    /// and `_`, at most ``maxLength`` bytes. `..`, `/`, a NUL, an empty or a long one are not.
    static func problem(_ namespace: String) -> String? {
        let bytes = Array(namespace.utf8)
        if bytes.isEmpty {
            return "it is empty"
        }
        if bytes.count > maxLength {
            return "it is \(bytes.count) bytes long, and a namespace has at most \(maxLength)"
        }
        if !(bytes[0] >= 0x61 && bytes[0] <= 0x7A) {
            return "it must start with a lowercase letter"
        }
        for scalar in namespace.unicodeScalars {
            let v = scalar.value
            if !((v >= 0x61 && v <= 0x7A) || (v >= 0x30 && v <= 0x39) || v == 0x5F) {
                return "\(scalar.escaped(asASCII: true)) is not allowed (lowercase letters, digits and `_` only)"
            }
        }
        return nil
    }

    /// Throws ``UndraLoadError/invalidNamespace(_:)`` unless `namespace` is a core namespace.
    static func require(_ namespace: String?) throws {
        guard let namespace = namespace else {
            return
        }
        if let problem = problem(namespace) {
            let shown = namespace.count > 40 ? String(namespace.prefix(40)) + "..." : namespace
            throw UndraLoadError.invalidNamespace(
                "`\(shown.unicodeScalars.map { $0.escaped(asASCII: true) }.joined())`: \(problem). It is the core's namespace (`UndraIds.namespace`), which the generated `Undra<Namespace>.load` passes"
            )
        }
    }
}
