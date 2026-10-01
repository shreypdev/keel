// Kv and SecureStore (docs/SPEC.md section 8): the same four methods over two backends, every
// one with the `StorageError` channel (ADR-049).
//
//   get(key: String) -> Result<Option<Bytes>, StorageError>
//   set(key: String, value: Bytes) -> Result<(), StorageError>
//   delete(key: String) -> Result<(), StorageError>
//   list(prefix: String) -> Result<Vec<String>, StorageError>
//
// Kv keeps one file per key in Application Support; SecureStore keeps one Keychain item per key.
// A backend failure answers port status 1 with the encoded `StorageError` (an `UndraPortError`),
// never "unavailable": the platform failure is mapped as ADR-049's table says.
//
//   out of space (NSFileWriteOutOfSpaceError, ENOSPC, EDQUOT, errSecDiskFull)        .full
//   protected data before the first unlock (errSecInteractionNotAllowed,
//     errSecAuthFailed, NSFile{Read,Write}NoPermissionError from data protection)     .locked
//   a stored entry that does not decode, a Keychain item that is not data           .corrupt
//   no Keychain in this process (errSecNotAvailable, errSecMissingEntitlement)      .unavailable
//   anything else                                                                    .io(message)
//
// Undecodable arguments (a core bug, not a storage failure) still throw the `WireError`, which the
// bridge answers with status 2 and an ERROR log.

import Foundation
@preconcurrency import Security

// MARK: - The shared port table

/// The storage behind a key-value port. Every method reports a failure as a ``StorageError``:
/// the typed `throws` makes an untyped failure (which the bridge could only answer
/// "unavailable") impossible to write.
protocol KeyValueBackend: Sendable {
    func get(_ key: String) throws(StorageError) -> [UInt8]?
    func set(_ key: String, _ value: [UInt8]) throws(StorageError)
    func delete(_ key: String) throws(StorageError)
    func list(prefix: String) throws(StorageError) -> [String]
}

/// The four method ids of a key-value port (`Kv` and `SecureStore` differ only in these).
struct KeyValueIds: Sendable {
    let get: UInt32
    let set: UInt32
    let delete: UInt32
    let list: UInt32

    static let kv = KeyValueIds(
        get: StandardPorts.Kv.get,
        set: StandardPorts.Kv.set,
        delete: StandardPorts.Kv.delete,
        list: StandardPorts.Kv.list
    )

    static let secureStore = KeyValueIds(
        get: StandardPorts.SecureStore.get,
        set: StandardPorts.SecureStore.set,
        delete: StandardPorts.SecureStore.delete,
        list: StandardPorts.SecureStore.list
    )
}

enum KeyValuePort {
    /// The async method table of a key-value port over `backend`.
    static func makeImpl(ids: KeyValueIds, backend: any KeyValueBackend) -> PortImpl {
        return .async([
            ids.get: { args in
                var reader = UndraReader(args)
                let key = try reader.readString()
                try reader.finish()
                let value = try answer { () throws(StorageError) -> [UInt8]? in
                    try backend.get(key)
                }
                return value.map(UndraBytes.init).undraEncoded()
            },
            ids.set: { args in
                var reader = UndraReader(args)
                let key = try reader.readString()
                let value = try reader.readBytes()
                try reader.finish()
                try answer { () throws(StorageError) -> Void in
                    try backend.set(key, value)
                }
                return []
            },
            ids.delete: { args in
                var reader = UndraReader(args)
                let key = try reader.readString()
                try reader.finish()
                try answer { () throws(StorageError) -> Void in
                    try backend.delete(key)
                }
                return []
            },
            ids.list: { args in
                var reader = UndraReader(args)
                let prefix = try reader.readString()
                try reader.finish()
                let keys = try answer { () throws(StorageError) -> [String] in
                    try backend.list(prefix: prefix)
                }
                return keys.undraEncoded()
            },
        ])
    }

    /// Runs a backend operation; its `StorageError` becomes the port's typed answer (status 1).
    static func answer<T>(_ body: () throws(StorageError) -> T) throws(UndraPortError) -> T {
        do {
            return try body()
        } catch {
            throw UndraPortError(body: error.undraEncoded())
        }
    }
}

// MARK: - Mapping platform failures

/// Maps a platform failure of a storage backend onto ``StorageError`` (ADR-049's table).
enum StorageFailure {
    /// The `StorageError` for a Foundation (or POSIX, or wire) error a file operation threw.
    static func classify(_ error: any Error) -> StorageError {
        if let storage = error as? StorageError {
            return storage
        }
        if let wire = error as? WireError {
            return .corrupt("the stored entry does not decode: \(wire)")
        }
        if isOutOfSpace(error) {
            return .full
        }
        let ns = error as NSError
        if ns.domain == NSCocoaErrorDomain {
            switch ns.code {
            case NSFileReadNoPermissionError, NSFileWriteNoPermissionError:
                // Data protection refuses a protected file before the first unlock with EPERM
                // (or no detail); a permission bit that forbids it is EACCES, a real failure.
                if posixCode(of: error) == EACCES {
                    return .io(error.localizedDescription)
                }
                return .locked
            case NSFileReadCorruptFileError:
                return .corrupt(error.localizedDescription)
            default:
                break
            }
        }
        return .io(error.localizedDescription)
    }

    /// Whether `error` (or the error under it) says the disk or the quota is exhausted:
    /// `NSFileWriteOutOfSpaceError`, `ENOSPC` or `EDQUOT`.
    static func isOutOfSpace(_ error: any Error) -> Bool {
        let ns = error as NSError
        if ns.domain == NSCocoaErrorDomain && ns.code == NSFileWriteOutOfSpaceError {
            return true
        }
        guard let code = posixCode(of: error) else {
            return false
        }
        return code == ENOSPC || code == EDQUOT
    }

    /// The POSIX code of `error`, or of the error under it (`NSUnderlyingErrorKey`), if any.
    static func posixCode(of error: any Error) -> Int32? {
        var current: NSError? = error as NSError
        var depth = 0
        while let ns = current, depth < 8 {
            if ns.domain == NSPOSIXErrorDomain {
                return Int32(truncatingIfNeeded: ns.code)
            }
            current = ns.userInfo[NSUnderlyingErrorKey] as? NSError
            depth += 1
        }
        return nil
    }

    /// Runs a file operation, mapping whatever it throws with ``classify(_:)``.
    static func run<T>(_ body: () throws -> T) throws(StorageError) -> T {
        do {
            return try body()
        } catch {
            throw classify(error)
        }
    }
}

// MARK: - Kv

/// `Kv` over files in the app's Application Support directory.
///
/// Each key is a file named by the hash of the key; the file starts with the key itself, so
/// `list` can recover keys of any length and a hash collision can never return another key's
/// value. Writes are atomic.
///
/// Failures are typed (``StorageError``, ADR-049): a full disk is `.full`, a file that data
/// protection keeps unreadable until the first unlock is `.locked`, an entry file that does not
/// decode is `.corrupt` (its key keeps it until it is overwritten or deleted), anything else is
/// `.io` with the platform's message.
public struct KvAdapter: UndraAdapter {
    private let backend: any KeyValueBackend

    /// Creates the adapter over `<Application Support>/<bundle id>/Undra/kv`.
    public init() {
        self.backend = FileKeyValueBackend(directory: KvAdapter.defaultDirectory(named: "kv"))
    }

    /// Creates the adapter over `directory` (created on first write).
    public init(directory: URL) {
        self.backend = FileKeyValueBackend(directory: directory)
    }

    /// Creates the adapter over a custom backend. Tests use in-memory and failing ones.
    init(backend: any KeyValueBackend) {
        self.backend = backend
    }

    /// `fnv1a32("port.Kv")`.
    public var portId: UInt32 {
        return StandardPorts.Kv.portId
    }

    /// The asynchronous `Kv` method table over the files.
    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return KeyValuePort.makeImpl(ids: .kv, backend: backend)
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

    /// Whether `error` says the file is not there (it was never written, or deleted meanwhile).
    private static func isMissing(_ error: any Error) -> Bool {
        let ns = error as NSError
        if ns.domain == NSCocoaErrorDomain {
            return ns.code == NSFileReadNoSuchFileError || ns.code == NSFileNoSuchFileError
        }
        return StorageFailure.posixCode(of: error) == ENOENT
    }

    func get(_ key: String) throws(StorageError) -> [UInt8]? {
        let data: Data
        do {
            data = try Data(contentsOf: fileURL(for: key))
        } catch {
            if FileKeyValueBackend.isMissing(error) {
                return nil
            }
            throw StorageFailure.classify(error)
        }
        var reader = UndraReader([UInt8](data))
        let storedKey: String
        do {
            storedKey = try reader.readString()
        } catch {
            throw .corrupt("the Kv entry for key \"\(key)\" does not decode: \(error)")
        }
        if storedKey != key {
            // A hash collision with another key: this key has no value.
            return nil
        }
        return Array(reader.readRemaining())
    }

    func set(_ key: String, _ value: [UInt8]) throws(StorageError) {
        var writer = UndraWriter(capacity: 4 + key.utf8.count + value.count)
        writer.writeString(key)
        writer.writeRaw(value)
        let bytes = writer.finish()
        let directory = self.directory
        let url = fileURL(for: key)
        try StorageFailure.run {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            try Data(bytes).write(to: url, options: .atomic)
        }
    }

    func delete(_ key: String) throws(StorageError) {
        let url = fileURL(for: key)
        guard FileManager.default.fileExists(atPath: url.path) else {
            return
        }
        // Deleting the file of a colliding key would lose someone else's value. A file that is
        // not an entry at all (no key to compare) is this key's, damaged: deleting it is right.
        if let stored = try readKey(at: url), stored != key {
            return
        }
        do {
            try FileManager.default.removeItem(at: url)
        } catch {
            if FileKeyValueBackend.isMissing(error) {
                return
            }
            throw StorageFailure.classify(error)
        }
    }

    func list(prefix: String) throws(StorageError) -> [String] {
        guard FileManager.default.fileExists(atPath: directory.path) else {
            return []
        }
        let directory = self.directory
        let entries = try StorageFailure.run {
            try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
        }
        var keys: [String] = []
        for url in entries {
            // A file that is not an entry (too short, a bad length, a key that is not UTF-8) is
            // skipped: it names no key.
            if let key = try readKey(at: url), key.hasPrefix(prefix) {
                keys.append(key)
            }
        }
        keys.sort()
        return keys
    }

    /// Reads only the key of an entry file, without loading the value; `nil` for a file that is
    /// not an entry or has gone.
    private func readKey(at url: URL) throws(StorageError) -> String? {
        let data: Data
        do {
            data = try Data(contentsOf: url, options: .mappedIfSafe)
        } catch {
            if FileKeyValueBackend.isMissing(error) {
                return nil
            }
            throw StorageFailure.classify(error)
        }
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
/// `dev.undra.securestore`, account = key), readable after the first unlock and never migrated to
/// another device.
///
/// Failures are typed (``StorageError``, ADR-049): `errSecInteractionNotAllowed` (the Keychain
/// before the first unlock) and `errSecAuthFailed` are `.locked`, an item that is not data or does
/// not decode is `.corrupt`, a process without a Keychain (`errSecNotAvailable`,
/// `errSecMissingEntitlement`) is `.unavailable`, `errSecDiskFull` is `.full`, and anything else
/// is `.io` with the Security framework's message and the `OSStatus`.
public struct SecureStoreAdapter: UndraAdapter {
    private let backend: any KeyValueBackend

    /// Creates the adapter over the Keychain, in `service`.
    public init(service: String = "dev.undra.securestore") {
        self.backend = KeychainBackend(service: service)
    }

    /// Creates the adapter over a custom backend. Tests use in-memory and failing ones.
    init(backend: any KeyValueBackend) {
        self.backend = backend
    }

    /// `fnv1a32("port.SecureStore")`.
    public var portId: UInt32 {
        return StandardPorts.SecureStore.portId
    }

    /// The asynchronous `SecureStore` method table over the Keychain.
    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return KeyValuePort.makeImpl(ids: .secureStore, backend: backend)
    }
}

/// The four Keychain calls `KeychainBackend` makes, so that tests can answer them with any
/// `OSStatus`.
protocol KeychainCalls: Sendable {
    /// `SecItemCopyMatching`.
    func copyMatching(_ query: [String: Any]) -> (OSStatus, AnyObject?)
    /// `SecItemUpdate`.
    func update(_ query: [String: Any], _ attributes: [String: Any]) -> OSStatus
    /// `SecItemAdd`.
    func add(_ attributes: [String: Any]) -> OSStatus
    /// `SecItemDelete`.
    func delete(_ query: [String: Any]) -> OSStatus
}

/// The system Keychain.
struct SystemKeychain: KeychainCalls {
    func copyMatching(_ query: [String: Any]) -> (OSStatus, AnyObject?) {
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        return (status, item)
    }

    func update(_ query: [String: Any], _ attributes: [String: Any]) -> OSStatus {
        return SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
    }

    func add(_ attributes: [String: Any]) -> OSStatus {
        return SecItemAdd(attributes as CFDictionary, nil)
    }

    func delete(_ query: [String: Any]) -> OSStatus {
        return SecItemDelete(query as CFDictionary)
    }
}

/// Keychain items, one per key.
struct KeychainBackend: KeyValueBackend {
    let service: String
    let keychain: any KeychainCalls

    init(service: String, keychain: any KeychainCalls = SystemKeychain()) {
        self.service = service
        self.keychain = keychain
    }

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

    /// The `StorageError` for a Keychain `status` other than success (ADR-049's table).
    static func storageError(_ status: OSStatus, operation: String) -> StorageError {
        let reason = SecCopyErrorMessageString(status, nil) as String? ?? "unknown error"
        let message = "Keychain \(operation) failed: \(reason) (OSStatus \(status))"
        switch status {
        case errSecInteractionNotAllowed, errSecAuthFailed:
            return .locked
        case errSecDecode:
            return .corrupt(message)
        case errSecNotAvailable, errSecMissingEntitlement:
            return .unavailable(message)
        case errSecDiskFull:
            return .full
        default:
            return .io(message)
        }
    }

    func get(_ key: String) throws(StorageError) -> [UInt8]? {
        var request = query(account: key)
        request[kSecReturnData as String] = true
        request[kSecMatchLimit as String] = kSecMatchLimitOne
        let (status, item) = keychain.copyMatching(request)
        if status == errSecItemNotFound {
            return nil
        }
        guard status == errSecSuccess else {
            throw KeychainBackend.storageError(status, operation: "read")
        }
        guard let data = item as? Data else {
            let found = item.map { String(describing: type(of: $0)) } ?? "nothing"
            throw .corrupt("the Keychain item for key \"\(key)\" is not data (found \(found))")
        }
        return [UInt8](data)
    }

    func set(_ key: String, _ value: [UInt8]) throws(StorageError) {
        let data = Data(value)
        let status = keychain.update(query(account: key), [kSecValueData as String: data])
        if status == errSecSuccess {
            return
        }
        if status != errSecItemNotFound {
            throw KeychainBackend.storageError(status, operation: "update")
        }
        var add = query(account: key)
        add[kSecValueData as String] = data
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let addStatus = keychain.add(add)
        if addStatus != errSecSuccess {
            throw KeychainBackend.storageError(addStatus, operation: "add")
        }
    }

    func delete(_ key: String) throws(StorageError) {
        let status = keychain.delete(query(account: key))
        if status != errSecSuccess && status != errSecItemNotFound {
            throw KeychainBackend.storageError(status, operation: "delete")
        }
    }

    func list(prefix: String) throws(StorageError) -> [String] {
        var request = query(account: nil)
        request[kSecReturnAttributes as String] = true
        request[kSecMatchLimit as String] = kSecMatchLimitAll
        let (status, items) = keychain.copyMatching(request)
        if status == errSecItemNotFound {
            return []
        }
        guard status == errSecSuccess else {
            throw KeychainBackend.storageError(status, operation: "list")
        }
        guard let entries = items as? [[String: Any]] else {
            let found = items.map { String(describing: type(of: $0)) } ?? "nothing"
            throw .corrupt("the Keychain listing is not a list of attribute dictionaries (found \(found))")
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
