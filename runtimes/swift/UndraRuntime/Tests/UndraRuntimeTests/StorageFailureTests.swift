import Foundation
import Security
import XCTest
@testable import UndraRuntime

// The shared failure-injection suite of ADR-049 ("storage ports have an error channel"), Swift
// column. The Kotlin and TypeScript runtimes run the same cases under the same names:
//
//   1. every StorageError, from every Kv and SecureStore method, answers port status 1 with the
//      encoded error, byte for byte, through the real bridge;
//   2. a failed write changes nothing and the next write succeeds;
//   3. Fs reports a full disk as FsError.full;
//   4. an untyped throw (an adapter bug) still answers status 2, now with an ERROR log naming the
//      port, the method and the adapter;
//   5. the platform's failures map as the ADR's table says (here: Foundation errors and Keychain
//      OSStatus values).

// MARK: - The failing backend

/// A key-value backend over memory that fails on demand: `fail(_:with:)` makes one operation (or
/// every one) throw the given `StorageError` until `recover()`.
final class FailingBackend: KeyValueBackend, @unchecked Sendable {
    /// The four operations of a key-value port.
    enum Operation: String, CaseIterable, Sendable {
        case get
        case set
        case delete
        case list
    }

    private struct State {
        var values: [String: [UInt8]] = [:]
        var failures: [Operation: StorageError] = [:]
        var calls: [Operation] = []
    }

    private let state = Guarded<State>(State())

    /// Makes `operation` throw `error` from now on (`nil` makes it succeed again).
    func fail(_ operation: Operation, with error: StorageError?) {
        state.withLock { (current: inout State) -> Void in
            current.failures[operation] = error
        }
    }

    /// Makes every operation succeed again.
    func recover() {
        state.withLock { (current: inout State) -> Void in
            current.failures = [:]
        }
    }

    /// The operations called so far, in order, failed or not.
    var calls: [Operation] {
        return state.withLock { (current: inout State) -> [Operation] in
            return current.calls
        }
    }

    private func enter(_ operation: Operation) throws(StorageError) {
        let failure = state.withLock { (current: inout State) -> StorageError? in
            current.calls.append(operation)
            return current.failures[operation]
        }
        if let failure = failure {
            throw failure
        }
    }

    func get(_ key: String) throws(StorageError) -> [UInt8]? {
        try enter(.get)
        return state.withLock { (current: inout State) -> [UInt8]? in
            return current.values[key]
        }
    }

    func set(_ key: String, _ value: [UInt8]) throws(StorageError) {
        try enter(.set)
        state.withLock { (current: inout State) -> Void in
            current.values[key] = value
        }
    }

    func delete(_ key: String) throws(StorageError) {
        try enter(.delete)
        state.withLock { (current: inout State) -> Void in
            current.values[key] = nil
        }
    }

    func list(prefix: String) throws(StorageError) -> [String] {
        try enter(.list)
        return state.withLock { (current: inout State) -> [String] in
            return current.values.keys.filter { $0.hasPrefix(prefix) }.sorted()
        }
    }
}

/// A scripted Keychain: every call answers `status` (and `item` for a successful copy).
final class ScriptedKeychain: KeychainCalls, @unchecked Sendable {
    private let state = Guarded<(status: OSStatus, item: AnyObject?)>((status: errSecSuccess, item: nil))

    /// Answers every call with `status`, and a successful copy with `item`.
    func answer(_ status: OSStatus, item: AnyObject? = nil) {
        state.withLock { (current: inout (status: OSStatus, item: AnyObject?)) -> Void in
            current = (status, item)
        }
    }

    private var current: (status: OSStatus, item: AnyObject?) {
        return state.withLock { (current: inout (status: OSStatus, item: AnyObject?)) -> (status: OSStatus, item: AnyObject?) in
            return current
        }
    }

    func copyMatching(_ query: [String: Any]) -> (OSStatus, AnyObject?) {
        let answer = current
        return (answer.status, answer.status == errSecSuccess ? answer.item : nil)
    }

    func update(_ query: [String: Any], _ attributes: [String: Any]) -> OSStatus {
        return current.status
    }

    func add(_ attributes: [String: Any]) -> OSStatus {
        return current.status
    }

    func delete(_ query: [String: Any]) -> OSStatus {
        return current.status
    }
}

/// Collects the runtime's own log records while a test runs.
final class LogRecorder: @unchecked Sendable {
    private let records = Guarded<[(UndraLog.Level, String)]>([])

    /// Starts recording; stops when the test ends.
    init(_ test: XCTestCase) {
        let records = self.records
        let token = UndraLog.addObserver { (level: UndraLog.Level, message: String) -> Void in
            records.withLock { (all: inout [(UndraLog.Level, String)]) -> Void in
                all.append((level, message))
            }
        }
        test.addTeardownBlock {
            UndraLog.removeObserver(token)
        }
    }

    /// The ERROR records that contain every one of `fragments`.
    func errors(containing fragments: String...) -> [String] {
        return records.withLock { (all: inout [(UndraLog.Level, String)]) -> [String] in
            return all.filter { (record: (UndraLog.Level, String)) -> Bool in
                record.0 == .error && fragments.allSatisfy { record.1.contains($0) }
            }.map { $0.1 }
        }
    }
}

// MARK: - The suite

@MainActor
final class StorageFailureTests: XCTestCase {
    /// Every `StorageError` variant.
    static let everyError: [StorageError] = [
        .unavailable("needs a backend"), .full, .locked, .corrupt("the entry does not decode"), .io("EIO"),
    ]

    /// A storage port under test: its id, its method ids and an adapter over a backend.
    private struct StoragePort {
        let name: String
        let ids: KeyValueIds
        let portId: UInt32
        let adapter: @Sendable (any KeyValueBackend) -> any UndraAdapter
    }

    private static let ports: [StoragePort] = [
        StoragePort(name: "Kv", ids: .kv, portId: StandardPorts.Kv.portId) { KvAdapter(backend: $0) },
        StoragePort(name: "SecureStore", ids: .secureStore, portId: StandardPorts.SecureStore.portId) {
            SecureStoreAdapter(backend: $0)
        },
    ]

    private func methodId(_ operation: FailingBackend.Operation, _ ids: KeyValueIds) -> UInt32 {
        switch operation {
        case .get: return ids.get
        case .set: return ids.set
        case .delete: return ids.delete
        case .list: return ids.list
        }
    }

    /// The encoded arguments of `operation`.
    private func args(_ operation: FailingBackend.Operation, key: String = "k", value: [UInt8] = [1, 2]) -> [UInt8] {
        return encodeArgs { (writer: inout UndraWriter) -> Void in
            writer.writeString(key)
            if operation == .set {
                writer.writeBytes(value)
            }
        }
    }

    /// Calls a port method through the bridge (the core's side of a port call) and returns the
    /// reply; throws if none comes.
    private func callThroughBridge(
        _ transport: FakeTransport,
        portId: UInt32,
        methodId: UInt32,
        args: [UInt8],
        file: StaticString = #filePath,
        line: UInt = #line
    ) async throws -> Wire.PortReply {
        let before = transport.portReplies.count
        let portCallId = UInt32(before + 1)
        let outcome = transport.callPort(portId: portId, methodId: methodId, portCallId: portCallId, args: args)
        XCTAssertEqual(outcome, .async, "the storage ports are asynchronous", file: file, line: line)
        let answered = await waitUntil { transport.portReplies.count == before + 1 }
        guard answered else {
            throw TestFailure(description: "no port reply to \(StandardPorts.describe(methodId: methodId))")
        }
        let reply = transport.portReplies[before]
        XCTAssertEqual(reply.portCallId, portCallId, file: file, line: line)
        return reply
    }

    // MARK: 1. Typed failures

    func testEveryStorageErrorOfEveryMethodAnswersStatus1WithTheEncodedError() async throws {
        for port in StorageFailureTests.ports {
            let backend = FailingBackend()
            let transport = FakeTransport()
            let core = try makeCore(transport, adapters: Adapters([port.adapter(backend)]))
            defer { core.shutdown() }
            for operation in FailingBackend.Operation.allCases {
                for error in StorageFailureTests.everyError {
                    backend.recover()
                    backend.fail(operation, with: error)
                    let label = "\(port.name).\(operation.rawValue) failing with \(error)"
                    let reply = try await callThroughBridge(
                        transport,
                        portId: port.portId,
                        methodId: methodId(operation, port.ids),
                        args: args(operation)
                    )
                    XCTAssertEqual(reply.status, .error, label)
                    XCTAssertEqual(try StorageError.undraDecoded(from: Array(reply.body)), error, label)
                    // The PortReply bytes: `port_call_id u32, status u8 = 1, StorageError`.
                    var expected = UndraWriter()
                    expected.writeU32(reply.portCallId)
                    expected.writeU8(1)
                    error.undraEncode(&expected)
                    XCTAssertEqual(bytesToHex(reply.encode()), bytesToHex(expected.finish()), label)
                }
            }
            XCTAssertEqual(backend.calls.count, FailingBackend.Operation.allCases.count * StorageFailureTests.everyError.count)
        }
    }

    func testTheExactBytesOfAFullAndALockedReply() async throws {
        let backend = FailingBackend()
        let transport = FakeTransport()
        let core = try makeCore(transport, adapters: Adapters([KvAdapter(backend: backend)]))
        defer { core.shutdown() }
        backend.fail(.set, with: .full)
        let full = try await callThroughBridge(transport, portId: StandardPorts.Kv.portId, methodId: StandardPorts.Kv.set, args: args(.set))
        XCTAssertEqual(bytesToHex(full.encode()), "01000000" + "01" + "0100")
        backend.fail(.get, with: .locked)
        let locked = try await callThroughBridge(transport, portId: StandardPorts.Kv.portId, methodId: StandardPorts.Kv.get, args: args(.get))
        XCTAssertEqual(bytesToHex(locked.encode()), "02000000" + "01" + "0200")
        backend.fail(.list, with: .corrupt("x"))
        let corrupt = try await callThroughBridge(transport, portId: StandardPorts.Kv.portId, methodId: StandardPorts.Kv.list, args: args(.list))
        XCTAssertEqual(bytesToHex(corrupt.encode()), "03000000" + "01" + "0300" + "01000000" + "78")
    }

    // MARK: 2. Recovery

    func testAFailedWriteChangesNothingAndTheNextWriteSucceeds() async throws {
        for port in StorageFailureTests.ports {
            let backend = FailingBackend()
            let transport = FakeTransport()
            let core = try makeCore(transport, adapters: Adapters([port.adapter(backend)]))
            defer { core.shutdown() }
            let set = methodId(.set, port.ids)
            let get = methodId(.get, port.ids)

            let first = try await callThroughBridge(transport, portId: port.portId, methodId: set, args: args(.set, value: [1]))
            XCTAssertEqual(first.status, .ok, port.name)
            XCTAssertEqual(Array(first.body), [], port.name)

            backend.fail(.set, with: .full)
            let refused = try await callThroughBridge(transport, portId: port.portId, methodId: set, args: args(.set, value: [2]))
            XCTAssertEqual(refused.status, .error, port.name)
            let kept = try await callThroughBridge(transport, portId: port.portId, methodId: get, args: args(.get))
            XCTAssertEqual(kept.status, .ok, port.name)
            XCTAssertEqual(try Optional<UndraBytes>.undraDecoded(from: Array(kept.body))?.bytes, [1], "\(port.name): the failed write changed nothing")

            backend.recover()
            let second = try await callThroughBridge(transport, portId: port.portId, methodId: set, args: args(.set, value: [3]))
            XCTAssertEqual(second.status, .ok, port.name)
            let now = try await callThroughBridge(transport, portId: port.portId, methodId: get, args: args(.get))
            XCTAssertEqual(try Optional<UndraBytes>.undraDecoded(from: Array(now.body))?.bytes, [3], port.name)
            let listed = try await callThroughBridge(transport, portId: port.portId, methodId: methodId(.list, port.ids), args: args(.list, key: ""))
            XCTAssertEqual(try [String].undraDecoded(from: Array(listed.body)), ["k"], port.name)
            let deleted = try await callThroughBridge(transport, portId: port.portId, methodId: methodId(.delete, port.ids), args: args(.delete))
            XCTAssertEqual(deleted.status, .ok, port.name)
        }
    }

    // MARK: 3. Fs

    func testFsAnswersAFullDiskWithFsErrorFull() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("undra-fs-full-" + UUID().uuidString, isDirectory: true)
        addTeardownBlock {
            try? FileManager.default.removeItem(at: root)
        }
        // The errno the atomic write leaves on a full disk or quota, and the Foundation spellings of it.
        let failures: [any Error] = [
            PosixError(code: ENOSPC),
            PosixError(code: EDQUOT),
            CocoaError(.fileWriteOutOfSpace),
            NSError(domain: NSCocoaErrorDomain, code: NSFileWriteUnknownError, userInfo: [NSUnderlyingErrorKey: POSIXError(.EDQUOT)]),
        ]
        for failure in failures {
            let adapter = FsAdapter(root: root) { (_: [UInt8], _: String, _: String, _: OwnedDescriptor, _: mode_t) throws -> Void in
                throw failure
            }
            let transport = FakeTransport()
            let core = try makeCore(transport, adapters: Adapters([adapter]))
            defer { core.shutdown() }
            let writeArgs = encodeArgs { (writer: inout UndraWriter) -> Void in
                writer.writeString("notes/a.txt")
                writer.writeBytes([1, 2, 3])
            }
            let reply = try await callThroughBridge(transport, portId: StandardPorts.Fs.portId, methodId: StandardPorts.Fs.write, args: writeArgs)
            XCTAssertEqual(reply.status, .error, "\(failure)")
            XCTAssertEqual(try FsError.undraDecoded(from: Array(reply.body)), .full, "\(failure)")
            XCTAssertEqual(bytesToHex(Array(reply.body)), "0300")
        }
    }

    // MARK: 4. An untyped throw is an adapter bug

    /// A `Kv` adapter with a bug: it throws a Foundation error instead of a `StorageError`.
    private struct UntypedKvAdapter: UndraAdapter {
        var portId: UInt32 {
            return StandardPorts.Kv.portId
        }

        func makePortImpl(core: UndraCore) -> PortImpl? {
            return .async([
                StandardPorts.Kv.get: { _ in
                    throw CocoaError(.fileReadUnknown)
                },
            ])
        }
    }

    func testAnUntypedThrowAnswersUnavailableAndLogsAnErrorNamingTheAdapter() async throws {
        let log = LogRecorder(self)
        let transport = FakeTransport()
        let core = try makeCore(transport, adapters: Adapters([UntypedKvAdapter()]))
        defer { core.shutdown() }
        let reply = try await callThroughBridge(transport, portId: StandardPorts.Kv.portId, methodId: StandardPorts.Kv.get, args: args(.get))
        XCTAssertEqual(reply, Wire.PortReply(portCallId: reply.portCallId, status: .unavailable))
        let errors = log.errors(containing: "UntypedKvAdapter", "Kv.get", "port status 2")
        XCTAssertEqual(errors.count, 1, "one ERROR record naming the adapter and the method")
    }

    func testAnUntypedThrowFromAPortRegisteredDirectlyIsLoggedToo() async throws {
        let log = LogRecorder(self)
        let transport = FakeTransport()
        let core = try makeCore(transport)
        defer { core.shutdown() }
        core.registerPort(StandardPorts.SecureStore.portId, .async([
            StandardPorts.SecureStore.delete: { _ in
                throw PortAdapterError.failed("bug")
            },
        ]))
        let reply = try await callThroughBridge(transport, portId: StandardPorts.SecureStore.portId, methodId: StandardPorts.SecureStore.delete, args: args(.delete))
        XCTAssertEqual(reply.status, .unavailable)
        XCTAssertEqual(log.errors(containing: "SecureStore implementation registered with registerPort", "SecureStore.delete").count, 1)

        // A sync port of an app (unknown ids) is named by its ids.
        core.registerPort(0x0000_00AB, .sync([0x0000_00CD: { _ in throw PortAdapterError.failed("bug") }]))
        let outcome = transport.callPort(portId: 0xAB, methodId: 0xCD, portCallId: 99, args: [])
        guard case .sync(let bytes) = outcome else {
            return XCTFail("expected a synchronous answer, got \(outcome)")
        }
        XCTAssertEqual(try Wire.PortReply.decode(bytes).status, .unavailable)
        XCTAssertEqual(log.errors(containing: "port 0x000000ab", "method 0x000000cd").count, 1)
    }

    func testUndecodableArgumentsAreAnUntypedFailureNotAStorageError() async throws {
        let log = LogRecorder(self)
        let transport = FakeTransport()
        let core = try makeCore(transport, adapters: Adapters([KvAdapter(backend: FailingBackend())]))
        defer { core.shutdown() }
        let reply = try await callThroughBridge(transport, portId: StandardPorts.Kv.portId, methodId: StandardPorts.Kv.set, args: [1])
        XCTAssertEqual(reply.status, .unavailable, "malformed arguments are the core's bug, not the storage's")
        XCTAssertEqual(log.errors(containing: "KvAdapter", "Kv.set").count, 1)
    }

    func testATypedFailureIsNotLogged() async throws {
        let log = LogRecorder(self)
        let backend = FailingBackend()
        backend.fail(.get, with: .locked)
        let transport = FakeTransport()
        let core = try makeCore(transport, adapters: Adapters([KvAdapter(backend: backend)]))
        defer { core.shutdown() }
        let reply = try await callThroughBridge(transport, portId: StandardPorts.Kv.portId, methodId: StandardPorts.Kv.get, args: args(.get))
        XCTAssertEqual(reply.status, .error)
        XCTAssertEqual(log.errors(containing: "Kv.get"), [], "a typed failure is the port's answer, not an adapter bug")
    }

    // MARK: 5. The platform's failures, mapped as ADR-049's table says

    func testFoundationFailuresMapToStorageErrors() {
        func underlying(_ code: Int, _ posix: POSIXErrorCode?) -> NSError {
            var info: [String: Any] = [:]
            if let posix = posix {
                info[NSUnderlyingErrorKey] = POSIXError(posix)
            }
            return NSError(domain: NSCocoaErrorDomain, code: code, userInfo: info)
        }
        // Quota or disk.
        XCTAssertEqual(StorageFailure.classify(CocoaError(.fileWriteOutOfSpace)), .full)
        XCTAssertEqual(StorageFailure.classify(POSIXError(.ENOSPC)), .full)
        XCTAssertEqual(StorageFailure.classify(underlying(NSFileWriteUnknownError, .ENOSPC)), .full)
        XCTAssertEqual(StorageFailure.classify(underlying(NSFileWriteUnknownError, .EDQUOT)), .full)
        // Data protection before the first unlock: EPERM, or no detail.
        XCTAssertEqual(StorageFailure.classify(underlying(NSFileReadNoPermissionError, .EPERM)), .locked)
        XCTAssertEqual(StorageFailure.classify(underlying(NSFileWriteNoPermissionError, .EPERM)), .locked)
        XCTAssertEqual(StorageFailure.classify(underlying(NSFileReadNoPermissionError, nil)), .locked)
        // A permission bit (EACCES) is a real failure, not a lock.
        guard case .io? = Optional(StorageFailure.classify(underlying(NSFileReadNoPermissionError, .EACCES))) else {
            return XCTFail("EACCES is an Io error")
        }
        // Unreadable data.
        guard case .corrupt? = Optional(StorageFailure.classify(CocoaError(.fileReadCorruptFile))) else {
            return XCTFail("a corrupt file is Corrupt")
        }
        guard case .corrupt(let reason)? = Optional(StorageFailure.classify(WireError.unexpectedEOF(needed: 4, at: 0))) else {
            return XCTFail("bytes that do not decode are Corrupt")
        }
        XCTAssertTrue(reason.contains("does not decode"), reason)
        // Anything else is Io with the platform's message; a StorageError is itself.
        XCTAssertEqual(StorageFailure.classify(CocoaError(.fileWriteUnknown)), .io(CocoaError(.fileWriteUnknown).localizedDescription))
        XCTAssertEqual(StorageFailure.classify(TestFailure(description: "x")), .io(TestFailure(description: "x").localizedDescription))
        XCTAssertEqual(StorageFailure.classify(StorageError.locked), .locked)
        // The errno of a POSIX call (`PosixError`, what the atomic write and `open` leave).
        XCTAssertEqual(StorageFailure.classify(PosixError(code: ENOSPC)), .full)
        XCTAssertEqual(StorageFailure.classify(PosixError(code: EDQUOT)), .full)
        XCTAssertEqual(StorageFailure.classify(PosixError(code: EPERM)), .locked, "data protection before the first unlock")
        XCTAssertEqual(StorageFailure.classify(POSIXError(.EPERM)), .locked)
        XCTAssertEqual(StorageFailure.classify(PosixError(code: EACCES)), .io(PosixError(code: EACCES).description))
        XCTAssertEqual(
            StorageFailure.classify(PosixError(code: EIO), context: "cannot write the entry of 'k'"),
            .io("cannot write the entry of 'k': \(PosixError(code: EIO).description)")
        )
        XCTAssertEqual(StorageFailure.posixCode(of: PosixError(code: EDQUOT)), EDQUOT)
    }

    func testKeychainStatusesMapToStorageErrors() {
        XCTAssertEqual(KeychainBackend.storageError(errSecInteractionNotAllowed, operation: "read"), .locked)
        XCTAssertEqual(KeychainBackend.storageError(errSecAuthFailed, operation: "read"), .locked)
        XCTAssertEqual(KeychainBackend.storageError(errSecDiskFull, operation: "add"), .full)
        guard case .corrupt(let decode)? = Optional(KeychainBackend.storageError(errSecDecode, operation: "read")) else {
            return XCTFail("errSecDecode is Corrupt")
        }
        XCTAssertTrue(decode.contains("OSStatus \(errSecDecode)"), decode)
        guard case .unavailable? = Optional(KeychainBackend.storageError(errSecMissingEntitlement, operation: "read")) else {
            return XCTFail("a process without a Keychain entitlement has no backend")
        }
        guard case .unavailable? = Optional(KeychainBackend.storageError(errSecNotAvailable, operation: "read")) else {
            return XCTFail("no Keychain is no backend")
        }
        guard case .io(let other)? = Optional(KeychainBackend.storageError(errSecParam, operation: "update")) else {
            return XCTFail("other statuses are Io")
        }
        XCTAssertTrue(other.hasPrefix("Keychain update failed: "), other)
        XCTAssertTrue(other.hasSuffix("(OSStatus \(errSecParam))"), other)
    }

    func testEveryKeychainOperationReportsALockedKeychainAsLocked() async throws {
        let keychain = ScriptedKeychain()
        keychain.answer(errSecInteractionNotAllowed)
        let transport = FakeTransport()
        let backend = KeychainBackend(service: "dev.undra.tests", keychain: keychain)
        let core = try makeCore(transport, adapters: Adapters([SecureStoreAdapter(backend: backend)]))
        defer { core.shutdown() }
        for operation in FailingBackend.Operation.allCases {
            let reply = try await callThroughBridge(
                transport,
                portId: StandardPorts.SecureStore.portId,
                methodId: methodId(operation, .secureStore),
                args: args(operation)
            )
            XCTAssertEqual(reply.status, .error, operation.rawValue)
            XCTAssertEqual(try StorageError.undraDecoded(from: Array(reply.body)), .locked, operation.rawValue)
        }
    }

    func testAKeychainItemThatIsNotDataIsCorrupt() throws {
        let keychain = ScriptedKeychain()
        let backend = KeychainBackend(service: "dev.undra.tests", keychain: keychain)
        keychain.answer(errSecSuccess, item: "a string, not data" as NSString)
        XCTAssertThrowsError(try backend.get("token")) { error in
            guard case .corrupt(let reason)? = error as? StorageError else {
                return XCTFail("expected Corrupt, got \(error)")
            }
            XCTAssertTrue(reason.contains("\"token\""), reason)
        }
        keychain.answer(errSecSuccess, item: NSNumber(value: 1))
        XCTAssertThrowsError(try backend.list(prefix: "")) { error in
            guard case .corrupt? = error as? StorageError else {
                return XCTFail("expected Corrupt, got \(error)")
            }
        }
        // The happy paths of the scripted Keychain: a data item, a missing item, a listing.
        keychain.answer(errSecSuccess, item: Data([7, 7]) as NSData)
        XCTAssertEqual(try backend.get("token"), [7, 7])
        keychain.answer(errSecItemNotFound)
        XCTAssertNil(try backend.get("token"))
        XCTAssertEqual(try backend.list(prefix: ""), [])
        XCTAssertNoThrow(try backend.delete("token"))
        keychain.answer(errSecSuccess, item: [[kSecAttrAccount as String: "b"], [kSecAttrAccount as String: "a"], ["other": 1]] as NSArray)
        XCTAssertEqual(try backend.list(prefix: ""), ["a", "b"])
        XCTAssertNoThrow(try backend.set("token", [1]))
    }

    func testAKvEntryThatDoesNotDecodeIsCorruptAndCanBeDeleted() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("undra-kv-corrupt-" + UUID().uuidString, isDirectory: true)
        addTeardownBlock {
            try? FileManager.default.removeItem(at: directory)
        }
        let backend = FileKeyValueBackend(directory: directory)
        try backend.set("queue", [1, 2, 3])
        let file = directory.appendingPathComponent(FileKeyValueBackend.fileName(for: "queue"))
        // A key length that runs past the end of the file.
        try Data([0xFF, 0, 0, 0, 0x71]).write(to: file)
        XCTAssertThrowsError(try backend.get("queue")) { error in
            guard case .corrupt(let reason)? = error as? StorageError else {
                return XCTFail("expected Corrupt, got \(error)")
            }
            XCTAssertTrue(reason.contains("\"queue\""), reason)
        }
        // Through the port: status 1 with Corrupt.
        let transport = FakeTransport()
        let core = try makeCore(transport, adapters: Adapters([KvAdapter(directory: directory)]))
        defer { core.shutdown() }
        let reply = try await callThroughBridge(transport, portId: StandardPorts.Kv.portId, methodId: StandardPorts.Kv.get, args: args(.get, key: "queue"))
        XCTAssertEqual(reply.status, .error)
        guard case .corrupt = try StorageError.undraDecoded(from: Array(reply.body)) else {
            return XCTFail("expected Corrupt")
        }
        // The damaged entry is still this key's: deleting it works, and the key is then missing.
        try backend.delete("queue")
        XCTAssertNil(try backend.get("queue"))
    }

    /// `get` answers `.io`; `list` skips the file it cannot read, as the React Native module's store
    /// does (only a locked store fails the listing).
    func testAKvFileThePermissionBitsForbidIsAnIoError() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("undra-kv-eacces-" + UUID().uuidString, isDirectory: true)
        let backend = FileKeyValueBackend(directory: directory)
        try backend.set("k", [1])
        let file = directory.appendingPathComponent(FileKeyValueBackend.fileName(for: "k"))
        XCTAssertEqual(chmod(file.path, 0), 0)
        addTeardownBlock {
            chmod(file.path, 0o600)
            try? FileManager.default.removeItem(at: directory)
        }
        if getuid() == 0 {
            throw XCTSkip("root reads files whatever their mode")
        }
        XCTAssertThrowsError(try backend.get("k")) { error in
            guard case .io? = error as? StorageError else {
                return XCTFail("EACCES is an Io error, got \(error)")
            }
        }
        try backend.set("other", [2])
        XCTAssertEqual(try backend.list(prefix: ""), ["other"], "the unreadable entry is skipped")
    }

    func testAKvEntryDeletedBetweenTheCheckAndTheReadIsMissing() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("undra-kv-missing-" + UUID().uuidString, isDirectory: true)
        addTeardownBlock {
            try? FileManager.default.removeItem(at: directory)
        }
        let backend = FileKeyValueBackend(directory: directory)
        XCTAssertNil(try backend.get("never"), "no directory yet")
        XCTAssertEqual(try backend.list(prefix: ""), [])
        XCTAssertNoThrow(try backend.delete("never"))
        try backend.set("k", [1])
        try FileManager.default.removeItem(at: directory.appendingPathComponent(FileKeyValueBackend.fileName(for: "k")))
        XCTAssertNil(try backend.get("k"))
    }
}
