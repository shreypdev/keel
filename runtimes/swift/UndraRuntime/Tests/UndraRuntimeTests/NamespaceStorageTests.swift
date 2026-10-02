import Foundation
import XCTest
@testable import UndraRuntime

/// The default `Kv`, `Fs`, `SecureStore` and `Db` locations are per core namespace (ADR-044 amendment
/// A): two cores of one app that both use the defaults never read or overwrite each other's data, and
/// an adapter the app gives a directory or a service of its own keeps it, whatever the core.
final class NamespaceStorageTests: XCTestCase {
    /// Two namespaces no other run uses, whose directories the test removes.
    private func namespaces() -> (a: String, b: String) {
        let tag = UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased().prefix(12)
        let pair = ("nsstore_a_\(tag)", "nsstore_b_\(tag)")
        addTeardownBlock {
            for namespace in [pair.0, pair.1] {
                try? FileManager.default.removeItem(at: StorageLocations.root().appendingPathComponent(namespace))
            }
        }
        return pair
    }

    private func read(_ impl: PortImpl?, _ method: UInt32, _ key: String) async throws -> [UInt8] {
        return try await PortCaller.callAsync(impl, method, keyArgs(key))
    }

    // MARK: The rule

    func testTheDefaultLocationsAreUndraNamespaceStore() {
        let root = StorageLocations.root()
        XCTAssertEqual(root.lastPathComponent, "Undra", "the casing the Swift runtime has always used on disk (the file systems of Apple platforms are case-insensitive)")
        XCTAssertEqual(root.deletingLastPathComponent().lastPathComponent, Bundle.main.bundleIdentifier ?? "app")
        for store in ["kv", "fs", "db"] {
            let directory = StorageLocations.directory(namespace: "playground_a", store: store)
            XCTAssertEqual(directory.path, root.appendingPathComponent("playground_a").appendingPathComponent(store).path)
            XCTAssertNotEqual(directory.path, StorageLocations.directory(namespace: "playground_b", store: store).path)
        }
        XCTAssertEqual(KvAdapter.defaultDirectory(namespace: "playground_a", named: "kv").path, StorageLocations.directory(namespace: "playground_a", store: "kv").path)
        XCTAssertEqual(SQLiteDbAdapter.defaultDirectory(namespace: "playground_a").path, StorageLocations.directory(namespace: "playground_a", store: "db").path)
        XCTAssertEqual(StorageLocations.keychainService(namespace: "playground_a"), "playground_a.dev.undra.securestore")
    }

    func testTheKeychainServiceOfTheDefaultCarriesTheNamespaceAndAGivenOneIsKept() throws {
        let defaults = SecureStoreAdapter()
        XCTAssertEqual((defaults.backend(forNamespace: "playground_a") as? KeychainBackend)?.service, "playground_a.dev.undra.securestore")
        XCTAssertEqual((defaults.backend(forNamespace: "playground_b") as? KeychainBackend)?.service, "playground_b.dev.undra.securestore")
        let own = SecureStoreAdapter(service: "com.example.vault")
        XCTAssertEqual((own.backend(forNamespace: "playground_a") as? KeychainBackend)?.service, "com.example.vault")
        XCTAssertEqual((own.backend(forNamespace: "playground_b") as? KeychainBackend)?.service, "com.example.vault")
    }

    func testTheCoreKnowsItsNamespace() throws {
        var options = LoadOptions.inproc(adapters: Adapters.none, expectedSchemaHash: 0x1234)
        XCTAssertNil(options.namespace)
        let unnamed = try UndraCore.connect(transport: FakeTransport(), options: options)
        XCTAssertEqual(unnamed.namespace, UndraCore.unnamedNamespace)
        XCTAssertEqual(UndraCore.unnamedNamespace, "_", "no real namespace is `_`: it starts with a lowercase letter")
        options.namespace = "playground_a"
        let named = try UndraCore.connect(transport: FakeTransport(), options: options)
        XCTAssertEqual(named.namespace, "playground_a")
    }

    // MARK: Two cores, the defaults

    func testTwoCoresWithTheDefaultKvNeverSeeEachOthersKeys() async throws {
        let (a, b) = namespaces()
        let adapters = Adapters([KvAdapter()])
        let coreA = try makeCore(FakeTransport(), adapters: adapters, namespace: a)
        let coreB = try makeCore(FakeTransport(), adapters: adapters, namespace: b)
        // One adapter value serves both cores: the namespace is the core's, not the adapter's.
        let kvA = adapters.all[0].makePortImpl(core: coreA)
        let kvB = adapters.all[0].makePortImpl(core: coreB)
        let set = StandardPorts.Kv.set
        let get = StandardPorts.Kv.get
        let list = StandardPorts.Kv.list
        let entry = { (key: String, value: [UInt8]) in encodeArgs { (w: inout UndraWriter) in w.writeString(key); w.writeBytes(value) } }
        _ = try await PortCaller.callAsync(kvA, set, entry("k", [0xA1, 0xA1]))
        _ = try await PortCaller.callAsync(kvB, set, entry("k", [0xB2, 0xB2]))
        _ = try await PortCaller.callAsync(kvA, set, entry("only-a", [0xC3]))

        let fromA = try await PortCaller.callAsync(kvA, get, keyArgs("k"))
        let fromB = try await PortCaller.callAsync(kvB, get, keyArgs("k"))
        XCTAssertNotEqual(fromA, fromB, "each core reads its own value of the same key")
        XCTAssertTrue(fromA.contains(0xA1) && !fromA.contains(0xB2))
        XCTAssertTrue(fromB.contains(0xB2) && !fromB.contains(0xA1))
        let missingInB = try await PortCaller.callAsync(kvB, get, keyArgs("only-a"))
        let presentInA = try await PortCaller.callAsync(kvA, get, keyArgs("only-a"))
        XCTAssertNotEqual(missingInB, presentInA, "a key of A is not in B")
        let listedB = try await PortCaller.callAsync(kvB, list, keyArgs(""))
        XCTAssertFalse(String(decoding: listedB, as: UTF8.self).contains("only-a"))

        let root = StorageLocations.root()
        for namespace in [a, b] {
            let directory = root.appendingPathComponent(namespace).appendingPathComponent("kv")
            var isDirectory: ObjCBool = false
            XCTAssertTrue(FileManager.default.fileExists(atPath: directory.path, isDirectory: &isDirectory) && isDirectory.boolValue, directory.path)
        }
    }

    func testTwoCoresWithTheDefaultFsNeverSeeEachOthersFiles() async throws {
        let (a, b) = namespaces()
        let adapters = Adapters([FsAdapter()])
        let coreA = try makeCore(FakeTransport(), adapters: adapters, namespace: a)
        let coreB = try makeCore(FakeTransport(), adapters: adapters, namespace: b)
        let fsA = adapters.all[0].makePortImpl(core: coreA)
        let fsB = adapters.all[0].makePortImpl(core: coreB)
        let write = encodeArgs { (w: inout UndraWriter) in w.writeString("notes/a.txt"); w.writeBytes([0x77]) }
        _ = try await PortCaller.callAsync(fsA, StandardPorts.Fs.write, write)
        let read = keyArgs("notes/a.txt")
        let fromA = try await PortCaller.callAsync(fsA, StandardPorts.Fs.read, read)
        XCTAssertTrue(fromA.contains(0x77))
        do {
            let fromB = try await PortCaller.callAsync(fsB, StandardPorts.Fs.read, read)
            XCTFail("B read A's file: \(fromB)")
        } catch {
            // `FsError::NotFound`, as an `UndraPortError`.
        }
        let file = StorageLocations.root().appendingPathComponent(a).appendingPathComponent("fs").appendingPathComponent("notes/a.txt")
        XCTAssertTrue(FileManager.default.fileExists(atPath: file.path), file.path)
    }

    func testTwoCoresWithTheDefaultDbOpenTwoFiles() async throws {
        let (a, b) = namespaces()
        let adapter = SQLiteDbAdapter()
        let coreA = try makeCore(FakeTransport(), adapters: Adapters([adapter]), namespace: a)
        let coreB = try makeCore(FakeTransport(), adapters: Adapters([adapter]), namespace: b)
        for core in [coreA, coreB] {
            let served = adapter.scoped(toNamespace: core.namespace)
            let connection = try await served.open(name: "app")
            try await connection.executeScript("CREATE TABLE t (v INTEGER); INSERT INTO t VALUES (\(core === coreA ? 1 : 2));")
            await connection.close()
        }
        let root = StorageLocations.root()
        for namespace in [a, b] {
            let file = root.appendingPathComponent(namespace).appendingPathComponent("db").appendingPathComponent("app.sqlite")
            XCTAssertTrue(FileManager.default.fileExists(atPath: file.path), file.path)
        }
        let rows = try await adapter.scoped(toNamespace: b).open(name: "app")
        let result = try await rows.query("SELECT v FROM t", [])
        XCTAssertEqual(result.rows.count, 1)
        XCTAssertEqual(result.rows.first?.first, .integer(2), "B's database holds B's row")
        await rows.close()
        // A `DbPortAdapter` the app builds over the default adapter scopes it to its core too.
        XCTAssertTrue(DbPortAdapter(adapter).makePortImpl(core: coreA) != nil)
    }

    // MARK: Adapters the app configures are untouched

    func testAnAdapterGivenADirectoryUsesItForEveryCore() async throws {
        let (a, b) = namespaces()
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("undra-ns-\(UUID().uuidString)")
        addTeardownBlock { try? FileManager.default.removeItem(at: directory) }
        let adapters = Adapters([KvAdapter(directory: directory)])
        let coreA = try makeCore(FakeTransport(), adapters: adapters, namespace: a)
        let coreB = try makeCore(FakeTransport(), adapters: adapters, namespace: b)
        let entry = encodeArgs { (w: inout UndraWriter) in w.writeString("shared"); w.writeBytes([0x99]) }
        _ = try await PortCaller.callAsync(adapters.all[0].makePortImpl(core: coreA), StandardPorts.Kv.set, entry)
        let seenByB = try await PortCaller.callAsync(adapters.all[0].makePortImpl(core: coreB), StandardPorts.Kv.get, keyArgs("shared"))
        XCTAssertTrue(seenByB.contains(0x99), "the app chose one directory for both")
        XCTAssertFalse(FileManager.default.fileExists(atPath: StorageLocations.root().appendingPathComponent(a).path))

        let db = SQLiteDbAdapter(directory: directory)
        XCTAssertEqual(db.fileURL(forDatabase: "x", namespace: a).path, directory.appendingPathComponent("x.sqlite").path)
        XCTAssertTrue(db.scoped(toNamespace: b) as AnyObject === db)
    }

    // MARK: A namespace from outside the table is a path component

    /// What an app can hand `LoadOptions.namespace`, and no core has: each would become a directory name.
    private let badNamespaces: [(String, String)] = [
        ("..", "dot dot"), (".", "dot"), ("a/b", "slash"), ("a\\b", "backslash"), ("../escape", "traversal"), ("", "empty"),
        (String(repeating: "a", count: 33), "33 bytes"), ("Upper", "uppercase"), ("1abc", "starts with a digit"), ("_", "an explicit underscore"),
        ("caf\u{e9}", "unicode"), ("a\u{0}b", "a NUL byte"), ("with space", "a space"), ("with-dash", "a dash"), ("a.b", "a dot"),
    ]

    func testAGoodNamespaceIsAccepted() {
        for namespace in ["a", "playground_core", String(repeating: "a", count: 32), "a1_b2"] {
            XCTAssertNil(CoreNamespace.problem(namespace), namespace)
        }
    }

    func testABadNamespaceIsRefusedTypedBeforeAnythingIsStarted() {
        for (namespace, why) in badNamespaces {
            var options = LoadOptions.inproc(adapters: Adapters.none, expectedSchemaHash: 0x1234)
            options.namespace = namespace
            let transport = FakeTransport()
            XCTAssertThrowsError(try UndraCore.connect(transport: transport, options: options), why) { error in
                guard case .invalidNamespace(let reason)? = error as? UndraLoadError else {
                    return XCTFail("\(why): \(error)")
                }
                XCTAssertTrue(reason.contains("namespace"), reason)
                XCTAssertLessThan(reason.count, 400, "a long one is shown shortened")
            }
            XCTAssertFalse(transport.wasStarted, "\(why): the transport was not started")
            XCTAssertThrowsError(try UndraCore.load(options), why) { error in
                XCTAssertTrue(error is UndraLoadError, "\(why): \(error)")
            }
        }
    }

    func testATablesBadNamespaceIsRefusedBeforeInitAndTheClaim() throws {
        FakeCore.reset()
        for (namespace, why) in badNamespaces where !namespace.contains("\u{0}") && !namespace.isEmpty {
            let table = FakeCoreTable(namespace: namespace)
            XCTAssertThrowsError(try UndraCore.load(.inproc(api: table.pointer, adapters: Adapters.none, expectedSchemaHash: 0xFA4E_C0DE_0000_0001)), why) { error in
                guard case .invalidNamespace? = error as? UndraLoadError else {
                    return XCTFail("\(why): \(error)")
                }
            }
            XCTAssertFalse(FakeCore.initialised, "\(why): init did not run")
            XCTAssertFalse(InprocTransport.isClaimed(namespace), "\(why): nothing was claimed")
        }
    }

    // MARK: The layout one namespace has on Apple, as the React Native module spells it

    func testTheLayoutOfOneNamespaceIsTheLiteralTheReactNativeModuleChecks() {
        // `cpp/test/apple_platform_test.mm` asserts these same suffixes of the React Native module's paths: the two shells of an app
        // keep a core's data in one place, whichever of them wrote it.
        let tail = { (store: String) in StorageLocations.directory(namespace: "playground_a", store: store).path }
        XCTAssertTrue(tail("kv").hasSuffix("/Undra/playground_a/kv"), tail("kv"))
        XCTAssertTrue(tail("fs").hasSuffix("/Undra/playground_a/fs"), tail("fs"))
        XCTAssertTrue(tail("db").hasSuffix("/Undra/playground_a/db"), tail("db"))
        XCTAssertEqual(StorageLocations.keychainService(namespace: "playground_a"), "playground_a.dev.undra.securestore")
        XCTAssertTrue(KvAdapter.defaultDirectory(namespace: "playground_a", named: "kv").path.hasSuffix("/Undra/playground_a/kv"))
        XCTAssertTrue(SQLiteDbAdapter.defaultDirectory(namespace: "playground_a").path.hasSuffix("/Undra/playground_a/db"))
    }
}
