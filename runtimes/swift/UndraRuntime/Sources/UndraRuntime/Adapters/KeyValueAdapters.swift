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
                var reader = UndraReader(args)
                let key = try reader.readString()
                try reader.finish()
                let value = try backend.get(key)
                return value.map(UndraBytes.init).undraEncoded()
            },
            ids.set: { args in
                var reader = UndraReader(args)
                let key = try reader.readString()
                let value = try reader.readBytes()
                try reader.finish()
                try backend.set(key, value)
                return []
            },
            ids.delete: { args in
                var reader = UndraReader(args)
                let key = try reader.readString()
                try reader.finish()
                try backend.delete(key)
                return []
            },
            ids.list: { args in
                var reader = UndraReader(args)
                let prefix = try reader.readString()
                try reader.finish()
                let keys = try backend.list(prefix: prefix)
                return keys.undraEncoded()
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
public struct KvAdapter: UndraAdapter {
    private let backend: FileKeyValueBackend

    /// Creates the adapter over `<Application Support>/<bundle id>/Undra/kv`.
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

    public func makePortImpl(core: UndraCore) -> PortImpl? {
        let ids = KeyValueIds(
            get: StandardPorts.Kv.get,
            set: StandardPorts.Kv.set,
            delete: StandardPorts.Kv.delete,
            list: StandardPorts.Kv.list
        )
        return KeyValuePort.makeImpl(ids: ids, backend: backend)
    }

    /// `<Application Support>/<bundle id>/Undra/<name>`.
    static func defaultDirectory(named name: String) -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
        let bundle = Bundle.main.bundleIdentifier ?? "app"
        return base
            .appendingPathComponent(bundle, isDirectory: true)
            .appendingPathComponent("Undra", isDirectory: true)
            .appendingPathComponent(name, isDirectory: true)
    }
}

/// One file per key: `u32 key length, key UTF-8, value bytes`, named `<fnv1a64>-<fnv1a32>`.
///
/// The directory is shared with the React Native module's C++ store (same names, same layout), and
/// a process killed mid-write leaves a temporary file in it. Three rules keep those out of sight:
///
/// * `set` writes `<name>.<16 hex digits>.tmp`, flushes it and renames it to `<name>` (the name
///   `temporaryName(for:)` defines, and the C++ store uses), so an entry file is whole or absent;
/// * `list` considers only a *sealed entry*: a file named like `fileName(for:)` writes it, whose
///   header holds the key that name belongs to. Temporary files, dot files, files of any other
///   name and files with a damaged header are skipped (the entry format ends the value at the end
///   of the file, so a value cut short cannot be told from a shorter one; the sealing above is
///   what rules it out);
/// * `get` of a file with a damaged header is "no value" (the key is not listed either), not a
///   failure the app would meet on every launch until it overwrote the key.
final class FileKeyValueBackend: KeyValueBackend, @unchecked Sendable {
    private let directory: URL

    init(directory: URL) {
        self.directory = directory
    }

    /// The file name of `key`: two FNV-1a hashes in hex, 16 + 1 + 8 characters.
    static func fileName(for key: String) -> String {
        return hex(fnv1a64(key), width: 16) + "-" + hex(UInt64(fnv1a32(key)), width: 8)
    }

    /// The end of the name of a file `set` is still writing.
    static let temporarySuffix = ".tmp"

    /// The name of the temporary file for the entry file `name`: `<name>.<16 hex digits>.tmp`.
    static func temporaryName(for name: String) -> String {
        return name + "." + PosixFiles.randomHex() + temporarySuffix
    }

    /// Whether `name` has the shape of an entry file name (`fileName(for:)`): 16 lowercase hex
    /// digits, `-`, 8 more.
    static func isEntryName(_ name: String) -> Bool {
        let bytes = Array(name.utf8)
        guard bytes.count == 16 + 1 + 8 else {
            return false
        }
        for (index, byte) in bytes.enumerated() {
            if index == 16 {
                if byte != UInt8(ascii: "-") {
                    return false
                }
            } else if !(UInt8(ascii: "0") ... UInt8(ascii: "9")).contains(byte)
                        && !(UInt8(ascii: "a") ... UInt8(ascii: "f")).contains(byte) {
                return false
            }
        }
        return true
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
        guard let header = FileKeyValueBackend.header(of: data) else {
            // Not a sealed entry (damaged, or cut short): this key has no value.
            return nil
        }
        if header.key != key {
            // A hash collision with another key: this key has no value.
            return nil
        }
        return [UInt8](data[(data.startIndex + header.end)...])
    }

    func set(_ key: String, _ value: [UInt8]) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        var writer = UndraWriter(capacity: 4 + key.utf8.count + value.count)
        writer.writeString(key)
        writer.writeRaw(value)
        let raw = open(directory.path, O_RDONLY | O_DIRECTORY | O_CLOEXEC)
        if raw < 0 {
            throw PortAdapterError.failed("cannot open \(directory.path): \(PosixError(code: errno))")
        }
        let name = FileKeyValueBackend.fileName(for: key)
        do {
            try PosixFiles.writeAtomically(
                writer.finish(),
                named: name,
                via: FileKeyValueBackend.temporaryName(for: name),
                in: OwnedDescriptor(raw),
                mode: 0o600
            )
        } catch let error as PosixError {
            throw PortAdapterError.failed("cannot write the entry of '\(key)': \(error)")
        }
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
        var keys: [String] = []
        for name in try FileManager.default.contentsOfDirectory(atPath: directory.path) {
            guard FileKeyValueBackend.isEntryName(name) else {
                continue
            }
            // A file that vanished or cannot be read meanwhile is skipped, as the React Native
            // module's store does.
            guard let key = (try? readKey(at: directory.appendingPathComponent(name))) ?? nil,
                  FileKeyValueBackend.fileName(for: key) == name,
                  key.hasPrefix(prefix)
            else {
                continue
            }
            keys.append(key)
        }
        keys.sort()
        return keys
    }

    /// Reads only the key of an entry file, without loading the value.
    private func readKey(at url: URL) throws -> String? {
        let data = try Data(contentsOf: url, options: .mappedIfSafe)
        return FileKeyValueBackend.header(of: data)?.key
    }

    /// The key at the start of an entry file's `data` and where the value begins, or nil when the
    /// header is not whole: fewer than four bytes, a key longer than the file, or a key that is
    /// not UTF-8.
    private static func header(of data: Data) -> (key: String, end: Int)? {
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
        guard let key = String(data: keyBytes, encoding: .utf8) else {
            return nil
        }
        return (key, 4 + length)
    }
}

// MARK: - SecureStore

/// `SecureStore` over the Keychain: one generic-password item per key (service
/// `dev.undra.securestore`, account = key), readable after the first unlock and never migrated to
/// another device.
public struct SecureStoreAdapter: UndraAdapter {
    private let backend: any KeyValueBackend

    /// Creates the adapter over the Keychain, in `service`.
    public init(service: String = "dev.undra.securestore") {
        self.backend = KeychainBackend(service: service)
    }

    /// Creates the adapter over a custom backend. Tests use an in-memory one.
    init(backend: any KeyValueBackend) {
        self.backend = backend
    }

    public var portId: UInt32 {
        return StandardPorts.SecureStore.portId
    }

    public func makePortImpl(core: UndraCore) -> PortImpl? {
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
