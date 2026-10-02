// Kv and SecureStore (docs/SPEC.md section 8): the same four methods over two backends, every
// one with the `StorageError` channel (ADR-049).
//
//   get(key: String) -> Result<Option<Bytes>, StorageError>
//   set(key: String, value: Bytes) -> Result<(), StorageError>
//   delete(key: String) -> Result<(), StorageError>
//   list(prefix: String) -> Result<Vec<String>, StorageError>
//
// Kv keeps one file per key in Application Support (`undra/<namespace>/kv`); SecureStore keeps one
// Keychain item per key (service `<namespace>.dev.undra.securestore`): the defaults are per core
// namespace (ADR-044 amendment A, `StorageLocations`).
// A backend failure answers port status 1 with the encoded `StorageError` (an `UndraPortError`),
// never "unavailable": the platform failure is mapped as ADR-049's table says.
//
//   out of space (NSFileWriteOutOfSpaceError, ENOSPC, EDQUOT, errSecDiskFull)        .full
//   protected data before the first unlock (errSecInteractionNotAllowed,
//     errSecAuthFailed, EPERM, NSFile{Read,Write}NoPermissionError over EPERM)        .locked
//   a stored entry whose header does not decode, a Keychain item that is not data   .corrupt
//   no Keychain in this process (errSecNotAvailable, errSecMissingEntitlement)      .unavailable
//   anything else (EACCES included)                                                  .io(message)
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
    /// The `StorageError` for a Foundation, POSIX (`PosixError`, `POSIXError`) or wire error a file
    /// operation threw. `context` (what was being done) prefixes the message of `.io` and `.corrupt`.
    static func classify(_ error: any Error, context: String? = nil) -> StorageError {
        if let storage = error as? StorageError {
            return storage
        }
        func message(_ text: String) -> String {
            return context.map { "\($0): \(text)" } ?? text
        }
        if let wire = error as? WireError {
            return .corrupt(message("the stored entry does not decode: \(wire)"))
        }
        if isOutOfSpace(error) {
            return .full
        }
        if let posix = error as? PosixError {
            return classify(errno: posix.code, message: message(posix.description))
        }
        let ns = error as NSError
        if ns.domain == NSPOSIXErrorDomain {
            return classify(errno: Int32(truncatingIfNeeded: ns.code), message: message(error.localizedDescription))
        }
        if ns.domain == NSCocoaErrorDomain {
            switch ns.code {
            case NSFileReadNoPermissionError, NSFileWriteNoPermissionError:
                // Data protection refuses a protected file before the first unlock with EPERM
                // (or no detail); a permission bit that forbids it is EACCES, a real failure.
                if posixCode(of: error) == EACCES {
                    return .io(message(error.localizedDescription))
                }
                return .locked
            case NSFileReadCorruptFileError:
                return .corrupt(message(error.localizedDescription))
            default:
                break
            }
        }
        return .io(message(error.localizedDescription))
    }

    /// The `StorageError` of an `errno` a POSIX call left: `ENOSPC` and `EDQUOT` are `.full`,
    /// `EPERM` (what data protection answers before the first unlock) is `.locked`, anything else
    /// (`EACCES` included: a permission bit, not a lock) is `.io(message)`.
    static func classify(errno code: Int32, message: String) -> StorageError {
        switch code {
        case ENOSPC, EDQUOT:
            return .full
        case EPERM:
            return .locked
        default:
            return .io(message)
        }
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

    /// The POSIX code of `error` (a `PosixError`, or an `NSError` in `NSPOSIXErrorDomain`), or of the
    /// error under it (`NSUnderlyingErrorKey`), if any.
    static func posixCode(of error: any Error) -> Int32? {
        if let posix = error as? PosixError {
            return posix.code
        }
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

    /// Runs a file operation, mapping whatever it throws with ``classify(_:context:)``.
    static func run<T>(context: String? = nil, _ body: () throws -> T) throws(StorageError) -> T {
        do {
            return try body()
        } catch {
            throw classify(error, context: context)
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
/// The default adapter keeps its files in `<Application Support>/<bundle id>/undra/<namespace>/kv`,
/// the namespace being the core's it is registered with (ADR-044 amendment A): two cores of one app
/// never see each other's keys. ``init(directory:)`` keeps them in a directory of the app's choice
/// instead, whatever the core.
///
/// Failures are typed (``StorageError``, ADR-049): a full disk is `.full`, a file that data
/// protection keeps unreadable until the first unlock is `.locked`, an entry file whose header
/// does not decode is `.corrupt` (its key keeps it until it is overwritten or deleted), anything
/// else is `.io` with the platform's message.
public struct KvAdapter: UndraAdapter {
    /// The backend for the core with the namespace given.
    private let backendFor: @Sendable (String) -> any KeyValueBackend

    /// Creates the adapter over `<Application Support>/<bundle id>/undra/<namespace>/kv`, the
    /// namespace being that of the core it is registered with.
    public init() {
        self.backendFor = { namespace in
            FileKeyValueBackend(directory: KvAdapter.defaultDirectory(namespace: namespace, named: "kv"))
        }
    }

    /// Creates the adapter over `directory` (created on first write), for every core it serves.
    public init(directory: URL) {
        let backend = FileKeyValueBackend(directory: directory)
        self.backendFor = { _ in backend }
    }

    /// Creates the adapter over a custom backend. Tests use in-memory and failing ones.
    init(backend: any KeyValueBackend) {
        self.backendFor = { _ in backend }
    }

    /// `fnv1a32("port.Kv")`.
    public var portId: UInt32 {
        return StandardPorts.Kv.portId
    }

    /// The asynchronous `Kv` method table over the files of `core`'s namespace.
    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return KeyValuePort.makeImpl(ids: .kv, backend: backend(forNamespace: core.namespace))
    }

    /// The storage for the core with `namespace`.
    func backend(forNamespace namespace: String) -> any KeyValueBackend {
        return backendFor(namespace)
    }

    /// `<Application Support>/<bundle id>/undra/<namespace>/<name>`.
    static func defaultDirectory(namespace: String, named name: String) -> URL {
        return StorageLocations.directory(namespace: namespace, store: name)
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
///   what rules it out), and so is a file that vanished or cannot be read meanwhile, as the C++
///   store does; a store data protection keeps locked is `.locked`, not empty;
/// * `get` of a file with a damaged header is `StorageError.corrupt` (ADR-049): since entries are
///   sealed, such a file is damage, not a write cut short, and "no value" would let the query
///   client take a stored queue for an empty one and overwrite it. The client handles `.corrupt`
///   (a cache entry is dropped and refetched, a queue is moved to the dead letters), and `set` or
///   `delete` of the key replaces or removes the file. The key is not listed.
///
/// Every failure is a ``StorageError`` (``StorageFailure``): a full disk or quota is `.full`,
/// `EPERM` (data protection before the first unlock) is `.locked`, anything else is `.io`.
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
            throw StorageFailure.classify(error, context: "cannot read the entry of '\(key)'")
        }
        guard let header = FileKeyValueBackend.header(of: data) else {
            throw .corrupt("the Kv entry for key \"\(key)\" is damaged: its header does not decode")
        }
        if header.key != key {
            // A hash collision with another key: this key has no value.
            return nil
        }
        return [UInt8](data[(data.startIndex + header.end)...])
    }

    func set(_ key: String, _ value: [UInt8]) throws(StorageError) {
        let directory = self.directory
        try StorageFailure.run(context: "cannot create \(directory.path)") {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        }
        var writer = UndraWriter(capacity: 4 + key.utf8.count + value.count)
        writer.writeString(key)
        writer.writeRaw(value)
        let raw = open(directory.path, O_RDONLY | O_DIRECTORY | O_CLOEXEC)
        if raw < 0 {
            throw StorageFailure.classify(PosixError(code: errno), context: "cannot open \(directory.path)")
        }
        let name = FileKeyValueBackend.fileName(for: key)
        let bytes = writer.finish()
        try StorageFailure.run(context: "cannot write the entry of '\(key)'") {
            try PosixFiles.writeAtomically(
                bytes,
                named: name,
                via: FileKeyValueBackend.temporaryName(for: name),
                in: OwnedDescriptor(raw),
                mode: 0o600
            )
        }
    }

    func delete(_ key: String) throws(StorageError) {
        let url = fileURL(for: key)
        guard FileManager.default.fileExists(atPath: url.path) else {
            return
        }
        // Deleting the file of a colliding key would lose someone else's value. A file whose
        // header is damaged names no key: it is this key's, and deleting it is right.
        if let stored = try readKey(at: url), stored != key {
            return
        }
        do {
            try FileManager.default.removeItem(at: url)
        } catch {
            if FileKeyValueBackend.isMissing(error) {
                return
            }
            throw StorageFailure.classify(error, context: "cannot delete the entry of '\(key)'")
        }
    }

    func list(prefix: String) throws(StorageError) -> [String] {
        guard FileManager.default.fileExists(atPath: directory.path) else {
            return []
        }
        let directory = self.directory
        let names = try StorageFailure.run(context: "cannot list \(directory.path)") {
            try FileManager.default.contentsOfDirectory(atPath: directory.path)
        }
        var keys: [String] = []
        for name in names {
            guard FileKeyValueBackend.isEntryName(name) else {
                continue
            }
            // A file that vanished or cannot be read meanwhile is skipped, as the React Native
            // module's store does; a locked store is not taken for an empty one.
            let stored: String?
            do {
                stored = try readKey(at: directory.appendingPathComponent(name))
            } catch {
                if error == .locked {
                    throw error
                }
                continue
            }
            guard let key = stored,
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

    /// Reads only the key of an entry file, without loading the value; `nil` for a file that has
    /// gone or whose header is damaged.
    private func readKey(at url: URL) throws(StorageError) -> String? {
        let data: Data
        do {
            data = try Data(contentsOf: url, options: .mappedIfSafe)
        } catch {
            if FileKeyValueBackend.isMissing(error) {
                return nil
            }
            throw StorageFailure.classify(error, context: "cannot read \(url.lastPathComponent)")
        }
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
/// `<namespace>.dev.undra.securestore`, account = key), readable after the first unlock and never
/// migrated to another device.
///
/// The default service carries the namespace of the core the adapter is registered with (ADR-044
/// amendment A), so two cores of one app never read each other's secrets; ``init(service:)`` uses
/// the service the app names, whatever the core.
///
/// Failures are typed (``StorageError``, ADR-049): `errSecInteractionNotAllowed` (the Keychain
/// before the first unlock) and `errSecAuthFailed` are `.locked`, an item that is not data or does
/// not decode is `.corrupt`, a process without a Keychain (`errSecNotAvailable`,
/// `errSecMissingEntitlement`) is `.unavailable`, `errSecDiskFull` is `.full`, and anything else
/// is `.io` with the Security framework's message and the `OSStatus`.
public struct SecureStoreAdapter: UndraAdapter {
    /// The backend for the core with the namespace given.
    private let backendFor: @Sendable (String) -> any KeyValueBackend

    /// Creates the adapter over the Keychain, in the service `<namespace>.dev.undra.securestore` of
    /// the core it is registered with, or in `service` when one is given.
    public init(service: String? = nil) {
        if let service = service {
            self.backendFor = { _ in KeychainBackend(service: service) }
        } else {
            self.backendFor = { namespace in
                KeychainBackend(service: StorageLocations.keychainService(namespace: namespace))
            }
        }
    }

    /// Creates the adapter over a custom backend. Tests use in-memory and failing ones.
    init(backend: any KeyValueBackend) {
        self.backendFor = { _ in backend }
    }

    /// `fnv1a32("port.SecureStore")`.
    public var portId: UInt32 {
        return StandardPorts.SecureStore.portId
    }

    /// The asynchronous `SecureStore` method table over the Keychain items of `core`'s namespace.
    public func makePortImpl(core: UndraCore) -> PortImpl? {
        return KeyValuePort.makeImpl(ids: .secureStore, backend: backend(forNamespace: core.namespace))
    }

    /// The storage for the core with `namespace`.
    func backend(forNamespace namespace: String) -> any KeyValueBackend {
        return backendFor(namespace)
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
