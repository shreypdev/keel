import Foundation
import KeelRuntime
import PlaygroundCore
import XCTest

extension ContractScenarios {
    // MARK: S01

    func testS01_primitivesRoundTrip() async {
        await scenario("S01", "primitives round-trip") {
            let core = try self.core

            // 1. The typical value.
            let typical = Primitives(
                flag: true, tiny: -8, small: -16_000, int: -2_000_000_000,
                long: -9_000_000_000_000_000_000, byte: 255, word: 65_535, dword: 4_000_000_000,
                qword: 18_000_000_000_000_000_000, single: 1.5, double: -2.25e100,
                text: "héllo, wörld ✓", blob: [0, 1, 2, 254, 255],
                span: .seconds(1) + .nanoseconds(500_000_123),
                at: Date(timeIntervalSince1970: 1_700_000_000.123),
                id: UUID(uuidString: "12345678-9abc-def0-0102-030405060708")!
            )
            try self.checkSame(echoPrimitives(typical, ctx: core), typical, "the typical value")

            // 2. The extremes, in both directions.
            var low = typical
            low.tiny = -128
            low.small = -32_768
            low.int = -2_147_483_648
            low.long = -9_223_372_036_854_775_808
            low.qword = 18_446_744_073_709_551_615
            low.single = -0.0
            low.double = .infinity
            low.text = ""
            low.blob = []
            try self.checkSame(echoPrimitives(low, ctx: core), low, "the low extremes")
            try check(echoPrimitives(low, ctx: core).single.sign == .minus, "-0.0 keeps its sign")

            var high = typical
            high.long = 9_223_372_036_854_775_807
            high.tiny = 127
            high.small = 32_767
            high.int = 2_147_483_647
            high.double = .nan
            high.text = String(repeating: "ü", count: 10_000)
            high.blob = (0 ..< 65_536).map { UInt8($0 % 251) }
            let echoedHigh = echoPrimitives(high, ctx: core)
            try check(echoedHigh.double.isNaN, "NaN comes back as NaN")
            try self.checkSame(echoedHigh, high, "the high extremes")
            try checkEqual(echoedHigh.text.utf8.count, 20_000, "10,000 characters of ü are 20,000 bytes")

            // 3. `ping()` has no arguments and no result.
            ping(ctx: core)

            // 4. Nothing is aliased: what the core returns is the caller's own copy.
            let sent = typical
            var returned = echoPrimitives(sent, ctx: core)
            returned.blob[0] = 99
            returned.text += "!"
            returned.id = UUID()
            try self.checkSame(sent, typical, "mutating the returned value changes what was sent")
            try self.checkSame(echoPrimitives(sent, ctx: core), typical, "a second echo of the original")
        }
    }

    /// Every field equal; floating-point fields by bit pattern (so `-0.0` differs from `0.0`),
    /// except NaN, which only has to be NaN.
    private func checkSame(_ actual: Primitives, _ expected: Primitives, _ what: String) throws {
        try checkEqual(actual.flag, expected.flag, "\(what): flag")
        try checkEqual(actual.tiny, expected.tiny, "\(what): tiny")
        try checkEqual(actual.small, expected.small, "\(what): small")
        try checkEqual(actual.int, expected.int, "\(what): int")
        try checkEqual(actual.long, expected.long, "\(what): long")
        try checkEqual(actual.byte, expected.byte, "\(what): byte")
        try checkEqual(actual.word, expected.word, "\(what): word")
        try checkEqual(actual.dword, expected.dword, "\(what): dword")
        try checkEqual(actual.qword, expected.qword, "\(what): qword")
        try checkEqual(actual.single.bitPattern, expected.single.bitPattern, "\(what): single")
        if expected.double.isNaN {
            try check(actual.double.isNaN, "\(what): double is NaN")
        } else {
            try checkEqual(actual.double.bitPattern, expected.double.bitPattern, "\(what): double")
        }
        try checkEqual(actual.text, expected.text, "\(what): text")
        try checkEqual(actual.blob, expected.blob, "\(what): blob")
        try checkEqual(actual.span, expected.span, "\(what): span")
        try checkEqual(
            Int64((actual.at.timeIntervalSince1970 * 1000).rounded()),
            Int64((expected.at.timeIntervalSince1970 * 1000).rounded()),
            "\(what): at (milliseconds)"
        )
        try checkEqual(actual.id, expected.id, "\(what): id")
    }

    // MARK: S02

    func testS02_recordsEnumsAndErrors() async {
        await scenario("S02", "records, enums and errors") {
            let core = try self.core

            // 1. Every variant of the figure comes back as the same variant.
            let figures: [Figure] = [.circle(radius: 2.5), .rect(width: 2, height: 3.5), .label(""), .label("héllo"), .empty]
            for figure in figures {
                try checkEqual(echoFigure(figure, ctx: core), figure, "echo_figure \(figure)")
            }

            // 2. A record that nests the other kinds, full and empty.
            let everything = Composite(
                name: "everything", tags: ["a", "", "ü"], figure: .rect(width: 2, height: 3.5),
                history: [.circle(radius: 1), .label("x"), .empty],
                scores: ["high": 99, "low": -3], names: [7: "seven", 1: "one"], limit: 7
            )
            try checkEqual(echoComposite(everything, ctx: core), everything, "a full composite")
            let nothing = Composite(name: "everything", tags: [], figure: nil, history: [], scores: [:], names: [:], limit: nil)
            try checkEqual(echoComposite(nothing, ctx: core), nothing, "an empty composite")

            // 3. Areas.
            try checkEqual(try area(.rect(width: 2, height: 4), ctx: core), 8.0, "area of a 2 x 4 rectangle")
            let circle = try area(.circle(radius: 1), ctx: core)
            try check(abs(circle - Double.pi) < 1e-12, "area of the unit circle is pi, got \(circle)")

            // 4. Errors arrive as the typed error of the binding.
            try checkFailure(outcome { () throws(LabError) -> Double in try area(.label("hat"), ctx: core) },
                             .rejected(code: 1, reason: "`hat` has no area"), "area(Label)")
            try checkFailure(outcome { () throws(LabError) -> Double in try area(.empty, ctx: core) },
                             .empty, "area(Empty)")
            try checkFailure(outcome { () throws(LabError) -> UInt32 in try parseCount(text: "", ctx: core) },
                             .empty, "parse_count(\"\")")
            try checkFailure(outcome { () throws(LabError) -> UInt32 in try parseCount(text: "1234567890", ctx: core) },
                             .tooLong(max: 9), "parse_count of ten digits")
            try checkFailure(outcome { () throws(LabError) -> UInt32 in try parseCount(text: "4x2", ctx: core) },
                             .notANumber("4x2"), "parse_count(\"4x2\")")
            try checkEqual(try parseCount(text: " 42 ", ctx: core), 42, "parse_count of \" 42 \"")

            // 5. The message is the core's `Display`.
            try checkEqual(LabError.tooLong(max: 9).description, "longer than 9 characters", "the Display of TooLong")
        }
    }

    // MARK: S03

    func testS03_syncCall() async {
        await scenario("S03", "sync call") {
            let core = try self.core

            // 1 and 2. The sync path is a plain function call: `plainSyncCalls` is not `async`.
            let (forty, wrapped, greeting) = ContractScenarios.plainSyncCalls(core)
            try checkEqual(forty, 42, "add(40, 2)")
            try checkEqual(wrapped, -2_147_483_648, "add(2147483647, 1) wraps")
            try checkEqual(greeting, "Hello, Ada, from the playground core", "greet(\"Ada\")")

            // 3. Ten thousand calls, every sum right, `crossings.calls` up by exactly 10,000.
            let before = core.stat("crossings.calls")
            let started = ContinuousClock.now
            var wrong = 0
            for n in 0 ..< 10_000 {
                if PlaygroundCore.add(a: Int32(n), b: 1, ctx: core) != Int32(n) + 1 {
                    wrong += 1
                }
            }
            let elapsed = ContinuousClock.now - started
            try checkEqual(wrong, 0, "wrong sums out of 10,000")
            try checkEqual(core.stat("crossings.calls") - before, 10_000, "crossings.calls delta")
            let nanoseconds = Double(elapsed.components.seconds) * 1e9 + Double(elapsed.components.attoseconds) / 1e9
            ScenarioCase.report("S03 mean \(Int(nanoseconds / 10_000)) ns per sync call (no budget asserted)")

            // 4. A sync call of an async method is refused, and the core is unharmed.
            do {
                _ = try core.callSync(
                    .freeFunction(methodId: KeelIds.Functions.addLater),
                    method: KeelIds.Functions.addLater,
                    args: encoded { (w: inout KeelWriter) in
                        w.writeI32(1)
                        w.writeI32(1)
                        w.writeU32(1)
                    }
                )
                throw ScenarioFailure(description: "a sync call of add_later was accepted")
            } catch let error as KeelReplyError {
                try checkEqual(error.status, .badRequest, "status of a sync call of an async method")
            }
            try checkEqual(PlaygroundCore.add(a: 1, b: 1, ctx: core), 2, "add(1, 1) after the refusal")
        }
    }

    /// The sync path from a plain, non-async function: it compiles only because `add`, `greet` do not suspend.
    nonisolated static func plainSyncCalls(_ core: KeelCore) -> (Int32, Int32, String) {
        return (
            PlaygroundCore.add(a: 40, b: 2, ctx: core),
            PlaygroundCore.add(a: 2_147_483_647, b: 1, ctx: core),
            greet(name: "Ada", ctx: core)
        )
    }
}
