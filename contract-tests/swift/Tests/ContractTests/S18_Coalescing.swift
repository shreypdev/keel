import UndraRuntime
import PlaygroundCore
import XCTest
#if UNDRA_FLOOR
import Combine
#endif

extension ContractScenarios {
    // MARK: S18

    /// The core commits one change-set per transaction, and the mirror merges what arrived before it
    /// drains (ADR-031). The calls are made on the main actor, where a synchronous call drains the
    /// mirror before it returns, as a UI's would be.
    func testS18_coalescedBurst() async {
        await scenario("S18", "coalesced burst") {
            let core = try self.core
            let ids = UndraIds.Objects.Stress.self
            @MainActor func burst(_ mode: StressMode, _ transactions: UInt32) -> [UInt8] {
                return encoded { (w: inout UndraWriter) in
                    mode.undraEncode(&w)
                    w.writeU32(transactions)
                }
            }

            // 1. Raw: burst(.firehose, 1000) reaches the raw mirror callback once, with the final value.
            let raw = try RawStore(core: core, type: ids.typeId, method: ids.new)
            defer { raw.close() }
            raw.observe()
            raw.clear()
            let transactions = core.stat("transactions")
            let before = core.mirror.stats()
            try raw.callSync(ids.burst, burst(.firehose, 1000))
            try checkEqual(raw.entries.count, 1, "raw mirror callbacks when burst(.firehose, 1000) returned")
            try checkEqual(raw.entries[0].signal, 0, "the entry's signal")
            try check(raw.entries[0].op == .fullValue, "the entry is a full value")
            try checkEqual(try raw.entries[0].decode(UInt64.self), 1000, "the final value")
            try checkEqual(core.stat("transactions") - transactions, 1000, "transactions for the burst")
            let after = core.mirror.stats()
            try checkEqual(after.changeSetsReceived - before.changeSetsReceived, 1000, "change-sets the mirror received")
            try checkEqual(after.entriesApplied - before.entriesApplied, 1, "entries the mirror applied")

            // 2. Generated: the final value is there when burst returns (read-your-writes).
            let stress = try Stress(ctx: core)
            defer { stress.close() }
            #if UNDRA_FLOOR
            // At the iOS 15 floor the store is an `ObservableObject` whose properties are `@Published`, which fires on every
            // set (ADR-045): what the mirror merged must reach it merged, one `objectWillChange` per applied entry.
            var willChange = 0
            let willChangeSink = stress.objectWillChange.sink { willChange += 1 }
            defer { willChangeSink.cancel() }
            #endif
            stress.burst(mode: .firehose, transactions: 1000)
            try checkEqual(stress.value, 1000, "value right after burst(.firehose, 1000)")
            #if UNDRA_FLOOR
            try checkEqual(willChange, 1, "objectWillChange events for 1000 merged change-sets")
            willChange = 0
            #endif

            // 3. Generated, no_coalesce: every progress entry is applied (SwiftUI shows what it renders).
            let applied = core.mirror.stats().entriesApplied
            stress.burst(mode: .progress, transactions: 10)
            try checkEqual(stress.progress, 10, "progress right after burst(.progress, 10)")
            try checkEqual(core.mirror.stats().entriesApplied - applied, 10, "progress entries the mirror applied")
            #if UNDRA_FLOOR
            try checkEqual(willChange, 10, "objectWillChange events for 10 no_coalesce entries")
            #endif
        }
    }
}
