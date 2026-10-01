import Foundation
import PlaygroundA
import PlaygroundACoreFFI
import PlaygroundB
import PlaygroundBCoreFFI
import UndraFFI
import UndraRuntime
import XCTest

extension ContractScenarios {
    // MARK: S26

    /// ADR-044: the playground core under two more namespaces, `playground_a` and `playground_b`, in
    /// this process next to the playground core the other scenarios share. Each is its own image with
    /// its own table, runtime and threads; their generated packages default to their own core.
    func testS26_twoCores() async {
        await scenario("S26", "two cores") {
            let options = { () -> LoadOptions in .inproc(adapters: Fixture.shared.makeAdapters()) }
            let tableA = UnsafeRawPointer(playground_a_undra_api())!.assumingMemoryBound(to: UndraApi.self).pointee
            let tableB = UnsafeRawPointer(playground_b_undra_api())!.assumingMemoryBound(to: UndraApi.self).pointee

            // 1. Both load through their generated entries; each entry's core is its own.
            try checkEqual(tableA.abi_version, 2, "C ABI version of playground_a")
            try checkEqual(tableB.abi_version, 2, "C ABI version of playground_b")
            try checkEqual(String(cString: tableA.name_space), "playground_a", "namespace of A's table")
            try checkEqual(String(cString: tableB.name_space), "playground_b", "namespace of B's table")
            try check(UndraPlaygroundA.core.isShutDown, "A's entry is the closed placeholder before it loads")
            let a = try UndraPlaygroundA.load(options())
            let b = try UndraPlaygroundB.load(options())
            try check(a !== b, "two cores")
            try check(UndraPlaygroundA.core === a && UndraPlaygroundB.core === b, "each entry's core is its own")
            try checkEqual(PlaygroundA.UndraIds.namespace, "playground_a", "UndraIds.namespace of package A")
            try checkEqual(PlaygroundB.UndraIds.namespace, "playground_b", "UndraIds.namespace of package B")
            try checkEqual(a.schemaHash, PlaygroundA.UndraIds.schemaHash, "A's schema hash")
            try checkEqual(b.schemaHash, PlaygroundB.UndraIds.schemaHash, "B's schema hash")

            // 2. A call on each, through each package's own default core.
            let callsA = a.stat("crossings.calls")
            let callsB = b.stat("crossings.calls")
            try checkEqual(try PlaygroundA.add(a: 2, b: 3), 5, "add(2, 3) through package A")
            try checkEqual(a.stat("crossings.calls") - callsA, 1, "calls A counted")
            try checkEqual(b.stat("crossings.calls") - callsB, 0, "calls B counted for a call of package A")
            try checkEqual(try PlaygroundB.add(a: 2, b: 3), 5, "add(2, 3) through package B")
            try checkEqual(a.stat("crossings.calls") - callsA, 1, "calls A counted for a call of package B")
            try checkEqual(b.stat("crossings.calls") - callsB, 1, "calls B counted")

            // 3. An observed change on each, independent.
            let handlesA = a.stat("live_handles")
            let handlesB = b.stat("live_handles")
            let counterA = try PlaygroundA.Counter()
            let counterB = try PlaygroundB.Counter()
            let changeSetsA = a.mirror.stats().changeSetsReceived
            let changeSetsB = b.mirror.stats().changeSetsReceived
            counterA.add(amount: 2)
            try checkEqual(counterA.count, 2, "A's count right after add(2)")
            try checkEqual(a.mirror.stats().changeSetsReceived - changeSetsA, 1, "change-sets A's mirror received")
            try checkEqual(b.mirror.stats().changeSetsReceived - changeSetsB, 0, "change-sets B's mirror received for A's write")
            counterB.add(amount: 5)
            try checkEqual(counterB.count, 5, "B's count right after add(5)")
            try checkEqual(counterA.count, 2, "A's count after B's write")

            // 4. Independent statistics; each object keeps its own core.
            try checkEqual(a.stat("live_handles") - handlesA, 1, "handles A made")
            try checkEqual(b.stat("live_handles") - handlesB, 1, "handles B made")
            counterA.close()
            try checkEqual(a.stat("live_handles") - handlesA, 0, "A's handles after its counter closed")
            try checkEqual(b.stat("live_handles") - handlesB, 1, "B's handles after A's counter closed")

            // 5. One shut down while the other keeps working.
            a.shutdown()
            try check(UndraPlaygroundA.core.isShutDown, "A's entry is the closed placeholder after A's shutdown")
            let statsA = tableA.stats_json!()
            let documentA = Data(bytes: try require(statsA.ptr, "A's stats_json"), count: Int(statsA.len))
            tableA.buf_free!(statsA)
            let parsedA = try require(try JSONSerialization.jsonObject(with: documentA) as? [String: Any], "A's stats")
            try check((parsedA["initialized"] as? Bool) == false, "A's image runs no core after its shutdown: \(parsedA)")
            try await waitUntil("A's threads to exit") {
                let buffer = tableA.stats_json!()
                defer { tableA.buf_free!(buffer) }
                let bytes = Data(bytes: try require(buffer.ptr, "A's stats_json"), count: Int(buffer.len))
                let document = try require(try JSONSerialization.jsonObject(with: bytes) as? [String: Any], "A's stats")
                return (document["runtime_threads"] as? NSNumber)?.intValue == 0
            }
            counterB.add(amount: 1)
            try checkEqual(counterB.count, 6, "B's count after A's shutdown")
            try checkEqual(try PlaygroundB.add(a: 2, b: 3), 5, "add(2, 3) through B after A's shutdown")
            try checkThrows({ try PlaygroundA.add(a: 2, b: 3, ctx: a) }, UndraCallError.unavailable(.closed), "add(2, 3) on A after its shutdown")
            try checkThrows({ try PlaygroundA.add(a: 2, b: 3) }, UndraCallError.unavailable(.closed), "add(2, 3) through package A's entry after A's shutdown")

            // 6. A namespace loads once; a closed one loads again.
            do {
                _ = try UndraPlaygroundB.load(options())
                throw ScenarioFailure(description: "B loaded twice")
            } catch UndraLoadError.alreadyLoaded {}
            try check(UndraPlaygroundB.core === b, "B's entry still holds B")
            let again = try UndraPlaygroundA.load(options())
            try check(UndraPlaygroundA.core === again, "A's entry holds the reloaded A")
            try checkEqual(try PlaygroundA.add(a: 2, b: 3), 5, "add(2, 3) through the reloaded A")
            counterB.close()
            again.shutdown()
            b.shutdown()
        }
    }
}
