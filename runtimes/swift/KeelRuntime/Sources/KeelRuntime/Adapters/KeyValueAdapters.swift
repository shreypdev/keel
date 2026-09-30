// Kv and SecureStore (docs/SPEC.md section 8): the same four methods over two backends.
//
//   get(key: String) -> Option<Bytes>       set(key: String, value: Bytes)
//   delete(key: String)                     list(prefix: String) -> Vec<String>
//
// Kv keeps one file per key in Application Support; SecureStore keeps one Keychain item per key.
// Neither method can report an error through its signature, so a backend failure makes the call
// "unavailable" (port status 2), which the core sees as `PortError::Unavailable`.

import Foundation
@preconcurrency import Security

// MARK: - The shared port table

/// The storage behind a key-value port.
protocol KeyValueBackend: Sendable {
    func get(_ key: String) throws -> [UInt8]?
    func set(_ key: String, _ value: [UInt8]) throws
    func delete(_ key: String) throws
    func list(prefix: String) throws -> [String]
}

/// The four method ids of a key-value port (`Kv` and `SecureStore` differ only in these).
struct KeyValueIds: Sendable {
    let get: UInt32
    let set: UInt32
    let delete: UInt32
    let list: UInt32
}

enum KeyValuePort {
    /// The async method table of a key-value port over `backend`.
    static func makeImpl(ids: KeyValueIds, backend: any KeyValueBackend) -> PortImpl {
        return .async([
            ids.get: { args in
                var reader = KeelReader(args)
                let key = try reader.readString()
                try reader.finish()
                let value = try backend.get(key)
                return value.map(KeelBytes.init).keelEncoded()
            },
            ids.set: { args in
                var reader = KeelReader(args)
                let key = try reader.readString()
                let value = try reader.readBytes()
                try reader.finish()
                try backend.set(key, value)
                return []
            },
            ids.delete: { args in
                var reader = KeelReader(args)
                let key = try reader.readString()
                try reader.finish()
                try backend.delete(key)
                return []
            },
            ids.list: { args in
                var reader = KeelReader(args)
                let prefix = try reader.readString()
                try reader.finish()
                let keys = try backend.list(prefix: prefix)
                return keys.keelEncoded()
            },
        ])
    }
}

// MARK: - Kv

/// `Kv` over files in the app's Application Support directory.
///
/// Each key is a file named by the hash of the key; the file starts with the key itself, so
/// `list` can recover keys of any length and a hash collision can never return another key's
/// value. Writes are atomic.
public struct KvAdapter: KeelAdapter {
    private let backend: FileKeyValueBackend

    /// Creates the adapter over `<Application Support>/<bundle id>/Keel/kv`.
    public init() {
        self.backend = FileKeyValueBackend(directory: KvAdapter.defaultDirectory(named: "kv"))
    }

    /// Creates the adapter over `directory` (created on first write).
    public init(directory: URL) {
        self.backend = FileKeyValueBackend(directory: directory)
    }

    public var portId: UInt32 {
        return StandardPorts.Kv.portId
    }

    public func makePortImpl(core: KeelCore) -> PortImpl? {
        let ids = KeyValueIds(
            get: StandardPorts.Kv.get,
            set: StandardPorts.Kv.set,
            delete: StandardPorts.Kv.delete,
            list: StandardPorts.Kv.list
        )
        return KeyValuePort.makeImpl(ids: ids, backend: backend)
    }

    /// `<Application Support>/<bundle id>/Keel/<name>`.
    static func defaultDirectory(named name: String) -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
        let bundle = Bundle.main.bundleIdentifier ?? "app"
        return base
            .appendingPathComponent(bundle, isDirectory: true)
            .appendingPathComponent("Keel", isDirectory: true)
            .appendingPathComponent(name, isDirectory: true)
    }
}

/// One file per key: `u32 key length, key UTF-8, value bytes`, named `<fnv1a64>-<fnv1a32>`.
final class FileKeyValueBackend: KeyValueBackend, @unchecked Sendable {
    private let directory: URL

    init(directory: URL) {
        self.directory = directory
    }

    /// The file name of `key`: two FNV-1a hashes in hex, 16 + 1 + 8 characters.
    static func fileName(for key: String) -> String {
        return hex(fnv1a64(key), width: 16) + "-" + hex(UInt64(fnv1a32(key)), width: 8)
    }

    private static func hex(_ value: UInt64, width: Int) -> String {
        let digits = String(value, radix: 16)
        if digits.count >= width {
            return digits
        }
        return String(repeating: "0", count: width - digits.count) + digits
    }

    private func fileURL(for key: String) -> URL {
        return directory.appendingPathComponent(FileKeyValueBackend.fileName(for: key), isDirectory: false)
    }

    func get(_ key: String) throws -> [UInt8]? {
        let url = fileURL(for: key)
        guard FileManager.default.fileExists(atPath: url.path) else {
            return nil
        }
        let data = try Data(contentsOf: url)
        var reader = KeelReader([UInt8](data))
        let storedKey = try reader.readString()
        if storedKey != key {
            // A hash collision with another key: this key has no value.
            return nil
        }
        return Array(reader.readRemaining())
    }

    func set(_ key: String, _ value: [UInt8]) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        var writer = KeelWriter(capacity: 4 + key.utf8.count + value.count)
        writer.writeString(key)
        writer.writeRaw(value)
        try Data(writer.finish()).write(to: fileURL(for: key), options: .atomic)
    }

    func delete(_ key: String) throws {
        let url = fileURL(for: key)
        guard FileManager.default.fileExists(atPath: url.path) else {
            return
        }
        // Deleting the file of a colliding key would lose someone else's value.
        if let stored = try readKey(at: url), stored != key {
            return
        }
        try FileManager.default.removeItem(at: url)
    }

    func list(prefix: String) throws -> [String] {
        guard FileManager.default.fileExists(atPath: directory.path) else {
            return []
        }
        let entries = try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
        var keys: [String] = []
        for url in entries {
            if let key = try readKey(at: url), key.hasPrefix(prefix) {
                keys.append(key)
            }
        }
        keys.sort()
        return keys
    }

    /// Reads only the key of an entry file, without loading the value.
    private func readKey(at url: URL) throws -> String? {
        let data = try Data(contentsOf: url, options: .mappedIfSafe)
        guard data.count >= 4 else {
            return nil
        }
        let start = data.startIndex
        var length = 0
        var shift = 0
        var offset = 0
        while offset < 4 {
            length |= Int(data[start + offset]) << shift
            shift += 8
            offset += 1
        }
        guard data.count >= 4 + length else {
            return nil
        }
        let keyBytes = data.subdata(in: (start + 4) ..< (start + 4 + length))
        return String(data: keyBytes, encoding: .utf8)
    }
}

// MARK: - SecureStore

/// `SecureStore` over the Keychain: one generic-password item per key (service
/// `dev.keel.securestore`, account = key), readable after the first unlock and never migrated to
/// another device.
public struct SecureStoreAdapter: KeelAdapter {
    private let backend: any KeyValueBackend

    /// Creates the adapter over the Keychain, in `service`.
    public init(service: String = "dev.keel.securestore") {
        self.backend = KeychainBackend(service: service)
    }

    /// Creates the adapter over a custom backend. Tests use an in-memory one.
    init(backend: any KeyValueBackend) {
        self.backend = backend
    }

    public var portId: UInt32 {
        return StandardPorts.SecureStore.portId
    }

    public func makePortImpl(core: KeelCore) -> PortImpl? {
        let ids = KeyValueIds(
            get: StandardPorts.SecureStore.get,
            set: StandardPorts.SecureStore.set,
            delete: StandardPorts.SecureStore.delete,
            list: StandardPorts.SecureStore.list
        )
        return KeyValuePort.makeImpl(ids: ids, backend: backend)
    }
}

/// Keychain items, one per key.
struct KeychainBackend: KeyValueBackend {
    let service: String

    private func query(account: String?) -> [String: Any] {
        var result: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
        ]
        if let account = account {
            result[kSecAttrAccount as String] = account
        }
        return result
    }

    func get(_ key: String) throws -> [UInt8]? {
        var request = query(account: key)
        request[kSecReturnData as String] = true
        request[kSecMatchLimit as String] = kSecMatchLimitOne
        var item: CFTypeRef?
        let status = SecItemCopyMatching(request as CFDictionary, &item)
        if status == errSecItemNotFound {
            return nil
        }
        guard status == errSecSuccess, let data = item as? Data else {
            throw PortAdapterError.failed("Keychain read failed (OSStatus \(status))")
        }
        return [UInt8](data)
    }

    func set(_ key: String, _ value: [UInt8]) throws {
        let data = Data(value)
        let update: [String: Any] = [kSecValueData as String: data]
        let status = SecItemUpdate(query(account: key) as CFDictionary, update as CFDictionary)
        if status == errSecSuccess {
            return
        }
        if status != errSecItemNotFound {
            throw PortAdapterError.failed("Keychain update failed (OSStatus \(status))")
        }
        var add = query(account: key)
        add[kSecValueData as String] = data
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let addStatus = SecItemAdd(add as CFDictionary, nil)
        if addStatus != errSecSuccess {
            throw PortAdapterError.failed("Keychain add failed (OSStatus \(addStatus))")
        }
    }

    func delete(_ key: String) throws {
        let status = SecItemDelete(query(account: key) as CFDictionary)
        if status != errSecSuccess && status != errSecItemNotFound {
            throw PortAdapterError.failed("Keychain delete failed (OSStatus \(status))")
        }
    }

    func list(prefix: String) throws -> [String] {
        var request = query(account: nil)
        request[kSecReturnAttributes as String] = true
        request[kSecMatchLimit as String] = kSecMatchLimitAll
        var items: CFTypeRef?
        let status = SecItemCopyMatching(request as CFDictionary, &items)
        if status == errSecItemNotFound {
            return []
        }
        guard status == errSecSuccess, let entries = items as? [[String: Any]] else {
            throw PortAdapterError.failed("Keychain list failed (OSStatus \(status))")
        }
        var keys: [String] = []
        for entry in entries {
            if let account = entry[kSecAttrAccount as String] as? String, account.hasPrefix(prefix) {
                keys.append(account)
            }
        }
        keys.sort()
        return keys
    }
}
