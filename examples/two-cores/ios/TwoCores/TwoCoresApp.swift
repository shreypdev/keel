import OSLog
import PlaygroundA
import PlaygroundB
import SwiftUI
import UndraRuntime

/// The two-core test app on iOS (ADR-044): two copies of the playground core, built under the
/// namespaces `playground_a` and `playground_b` and linked into this one binary as two prelinked
/// static libraries, loaded side by side through their generated entries. At launch it gives each a
/// call and an observed change, compares their statistics, closes one and checks the other keeps
/// working. Every check is a `two-cores ios:` line in the log and a row on screen.
@main
struct TwoCoresApp: App {
    @State private var checks = TwoCoreChecks.run()

    var body: some Scene {
        WindowGroup {
            NavigationStack {
                List(checks) { check in
                    Label(check.what, systemImage: check.passed ? "checkmark.circle.fill" : "xmark.octagon.fill")
                        .foregroundStyle(check.passed ? Color.green : Color.red)
                        .accessibilityIdentifier(check.passed ? "pass" : "fail")
                }
                .navigationTitle(checks.allSatisfy(\.passed) ? "Two cores: passed" : "Two cores: FAILED")
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

    static func run() -> [TwoCoreCheck] {
        var checks: [TwoCoreCheck] = []
        func check(_ what: String, _ passed: Bool) {
            log.notice("two-cores ios: \(passed ? "ok  " : "FAIL", privacy: .public) \(what, privacy: .public)")
            checks.append(TwoCoreCheck(what: what, passed: passed))
        }
        do {
            let a = try UndraPlaygroundA.load(.inproc(adapters: .none))
            let b = try UndraPlaygroundB.load(.inproc(adapters: .none))
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

    private static func hex(_ value: UInt64) -> String {
        "0x" + String(value, radix: 16)
    }
}
