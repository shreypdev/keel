import OSLog
import PlaygroundA
import PlaygroundB
import SwiftUI
import UndraRuntime

/// The two-core test app on iOS (ADR-044): two copies of the playground core, built under the
/// namespaces `playground_a` and `playground_b` and linked into this one binary as two prelinked
/// static libraries, loaded side by side through their generated entries. At launch it gives each a
/// call and an observed change, compares their statistics, closes one and checks the other keeps
/// working. Both cores use the default adapters, whose `Kv` is per core namespace (ADR-044 amendment A): each
/// writes the same key and reads its own value back, and the files are in two directories. Every check is a
/// `two-cores ios:` line in the log and a row on screen.
@main
struct TwoCoresApp: App {
    @State private var checks: [TwoCoreCheck] = []

    var body: some Scene {
        WindowGroup {
            NavigationStack {
                List(checks) { check in
                    Label(check.what, systemImage: check.passed ? "checkmark.circle.fill" : "xmark.octagon.fill")
                        .foregroundStyle(check.passed ? Color.green : Color.red)
                        .accessibilityIdentifier(check.passed ? "pass" : "fail")
                }
                .navigationTitle(checks.isEmpty ? "Two cores: running" : checks.allSatisfy(\.passed) ? "Two cores: passed" : "Two cores: FAILED")
            }
            .task {
                if checks.isEmpty {
                    checks = await TwoCoreChecks.run()
                }
            }
        }
    }
}

/// One line of the run.
struct TwoCoreCheck: Identifiable {
    let id = UUID()
    let what: String
    let passed: Bool
}

@MainActor
enum TwoCoreChecks {
    private static let log = Logger(subsystem: "dev.undra.twocores", category: "two-cores")

    static func run() async -> [TwoCoreCheck] {
        var checks: [TwoCoreCheck] = []
        func check(_ what: String, _ passed: Bool) {
            log.notice("two-cores ios: \(passed ? "ok  " : "FAIL", privacy: .public) \(what, privacy: .public)")
            checks.append(TwoCoreCheck(what: what, passed: passed))
        }
        do {
            // The default adapters: `Kv`, `Fs`, `SecureStore` and `Db` are per core namespace.
            let a = try UndraPlaygroundA.load(.inproc())
            let b = try UndraPlaygroundB.load(.inproc())
            check(
                "both loaded: \(UndraPlaygroundA.namespace) and \(UndraPlaygroundB.namespace), two cores",
                a !== b && UndraPlaygroundA.core === a && UndraPlaygroundB.core === b
            )
            check(
                "schema hash of each: \(hex(a.schemaHash)), \(hex(b.schemaHash))",
                a.schemaHash == PlaygroundA.UndraIds.schemaHash && b.schemaHash == PlaygroundB.UndraIds.schemaHash
            )
            let sumA = try PlaygroundA.add(a: 2, b: 3)
            let sumB = try PlaygroundB.add(a: 2, b: 3)
            check("a call on each: add(2, 3) is \(sumA) through A and \(sumB) through B", sumA == 5 && sumB == 5)
            // R6 through a prelinked object: the panic unwinds to the table entry's guard in each
            // core's own image and comes back as a reply, not an abort.
            let panicA = panicMessage { _ = try PlaygroundA.explode(reason: "boom in A") }
            let panicB = panicMessage { _ = try PlaygroundB.explode(reason: "boom in B") }
            check(
                "a panic in each is a reply: \(panicA ?? "none"), \(panicB ?? "none")",
                panicA == "boom in A" && panicB == "boom in B" && (try? PlaygroundA.add(a: 1, b: 1)) == 2
            )
            let handlesA = a.stats().coreLiveHandles
            let handlesB = b.stats().coreLiveHandles
            let counterA = try PlaygroundA.Counter()
            let counterB = try PlaygroundB.Counter()
            counterA.add(amount: 2)
            counterB.add(amount: 5)
            check("an observed change on each: A's count is \(counterA.count), B's is \(counterB.count)", counterA.count == 2 && counterB.count == 5)
            let newA = a.stats().coreLiveHandles - handlesA
            let newB = b.stats().coreLiveHandles - handlesB
            check("independent statistics: A has \(newA) new handle, B has \(newB)", newA == 1 && newB == 1)
            await checkStorage(check)
            a.shutdown()
            counterB.add(amount: 1)
            var unavailable = false
            do {
                _ = try PlaygroundA.add(a: 2, b: 3)
            } catch UndraCallError.unavailable {
                unavailable = true
            }
            let sumAfter = try PlaygroundB.add(a: 2, b: 3)
            check(
                "one shut down while the other keeps working: B's count is \(counterB.count), a call on A is \(unavailable ? "unavailable" : "answered")",
                counterB.count == 6 && unavailable && sumAfter == 5
            )
            counterB.close()
            b.shutdown()
        } catch {
            check("the run failed: \(error)", false)
        }
        log.notice("two-cores ios: \(checks.allSatisfy(\.passed) ? "passed" : "FAILED", privacy: .public)")
        return checks
    }

    /// The default stores of two cores are two stores (ADR-044 amendment A): the same `Kv` key written through each core
    /// reads back that core's own value, a key one core wrote is not the other's, and the files are in
    /// `<Application Support>/<bundle id>/Undra/<namespace>/kv`.
    private static func checkStorage(_ check: (String, Bool) -> Void) async {
        let nonce = UUID().uuidString.prefix(8).lowercased()
        let key = "two-cores.key"
        let onlyInA = "two-cores.only-a"
        do {
            try await PlaygroundA.kvPut(key: key, value: Array("value-of-a-\(nonce)".utf8))
            try await PlaygroundB.kvPut(key: key, value: Array("value-of-b-\(nonce)".utf8))
            try await PlaygroundA.kvPut(key: onlyInA, value: [1])
            let readA = try await PlaygroundA.kvGet(key: key).map { String(decoding: $0, as: UTF8.self) }
            let readB = try await PlaygroundB.kvGet(key: key).map { String(decoding: $0, as: UTF8.self) }
            check(
                "Kv: both wrote \(key) and read their own value back: A has \(readA ?? "nothing"), B has \(readB ?? "nothing")",
                readA == "value-of-a-\(nonce)" && readB == "value-of-b-\(nonce)"
            )
            let keysA = try await PlaygroundA.kvKeys(prefix: "two-cores.")
            let keysB = try await PlaygroundB.kvKeys(prefix: "two-cores.")
            check(
                "Kv: a key only A wrote is not B's: A lists \(keysA), B lists \(keysB)",
                keysA.contains(onlyInA) && !keysB.contains(onlyInA) && keysB.contains(key)
            )
            let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
                .appendingPathComponent(Bundle.main.bundleIdentifier ?? "app", isDirectory: true)
                .appendingPathComponent("Undra", isDirectory: true)
            let directories = [UndraPlaygroundA.namespace, UndraPlaygroundB.namespace].map {
                base.appendingPathComponent($0, isDirectory: true).appendingPathComponent("kv", isDirectory: true)
            }
            var isDirectory: ObjCBool = false
            let present = directories.allSatisfy { FileManager.default.fileExists(atPath: $0.path, isDirectory: &isDirectory) && isDirectory.boolValue }
            check(
                "Kv files: \(directories.map { $0.path.replacingOccurrences(of: base.deletingLastPathComponent().path, with: "…") }.joined(separator: " and "))",
                present
            )
            try await PlaygroundA.kvRemove(key: key)
            try await PlaygroundA.kvRemove(key: onlyInA)
            try await PlaygroundB.kvRemove(key: key)
        } catch {
            check("Kv: the check failed: \(error)", false)
        }
    }

    /// The message of the panic `body` reports as `UndraCallError.panicked`, or `nil`.
    private static func panicMessage(_ body: () throws -> Void) -> String? {
        do {
            try body()
        } catch UndraCallError.panicked(let message, _) {
            return message
        } catch {
            return nil
        }
        return nil
    }

    private static func hex(_ value: UInt64) -> String {
        "0x" + String(value, radix: 16)
    }
}
