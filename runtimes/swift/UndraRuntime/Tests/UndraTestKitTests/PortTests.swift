import XCTest
@testable import UndraTestKit
import UndraRuntime

final class PortTests: XCTestCase {
    private func keyArgs(_ key: String) -> [UInt8] {
        var w = UndraWriter()
        w.writeString(key)
        return w.finish()
    }

    private func setArgs(_ key: String, _ value: [UInt8]) -> [UInt8] {
        var w = UndraWriter()
        w.writeString(key)
        w.writeBytes(value)
        return w.finish()
    }

    private func methods(_ impl: PortImpl?) -> [UInt32: AsyncPortMethod] {
        if case .async(let table)? = impl {
            return table
        }
        return [:]
    }

    private let kvSet = undraMethodId("Kv", "set")
    private let kvGet = undraMethodId("Kv", "get")

    /// A core to ask the adapters for their port tables (nothing is called through it).
    @MainActor private func scratchCore() throws -> RecordedCore {
        return try RecordedCore.load(Recording(schemaHash: 1, source: "test"), expectedSchemaHash: 1)
    }

    @MainActor
    func testARecorderAndAReplayerRoundTripASessionsPortTraffic() async throws {
        let core = try scratchCore()
        defer { core.close() }
        let now = Locked<UInt64>(1_000)
        let recorder = PortRecorder(schemaHash: 0x2a, now: { now.withLock { $0 } }, platform: "ios")
        let kv = MemKv()
        let wrapped = recorder.wrap(portId: undraPortId("Kv"), kv.makePortImpl(core: core.core)!)
        guard case .async(let table) = wrapped else { return XCTFail("Kv is async") }
        _ = try await table[kvSet]!(setArgs("a", [1, 2]))
        now.withLock { $0 = 1_040 }
        let got = try await table[kvGet]!(keyArgs("a"))
        XCTAssertEqual(got.hex, "01020000000102") // Some(Bytes [1, 2])

        let text = recorder.toJSON()
        let recording = try Recording(json: text)
        XCTAssertEqual(recording.platform, "ios")
        XCTAssertEqual(recording.source, "adapters")
        XCTAssertEqual(recording.events.map { $0.t }, [0, 0, 40, 40])
        XCTAssertTrue(text.contains("\"name\":\"Kv.set\""), text)

        // A fresh run answers from the recording, with no store behind it.
        let replayer = Replayer(recording)
        let replayed = methods(replayer.adapters().all.first!.makePortImpl(core: core.core))
        let setReply = try await replayed[kvSet]!(setArgs("a", [1, 2]))
        XCTAssertEqual(setReply.count, 0)
        let getReply = try await replayed[kvGet]!(keyArgs("a"))
        XCTAssertEqual(getReply.hex, got.hex)
        try replayer.finish()
    }

    @MainActor
    func testACallThatDeviatesIsATypedErrorAndConsumesNothing() async throws {
        let core = try scratchCore()
        defer { core.close() }
        let recorder = PortRecorder(schemaHash: 1, now: { 0 })
        guard case .async(let table) = recorder.wrap(portId: undraPortId("Kv"), MemKv().makePortImpl(core: core.core)!) else { return XCTFail("async") }
        _ = try await table[kvGet]!(keyArgs("a"))
        let replayer = Replayer(recorder.record())
        let kv = methods(replayer.adapters().all.first!.makePortImpl(core: core.core))
        do {
            _ = try await kv[kvGet]!(keyArgs("other"))
            XCTFail("expected a deviation")
        } catch let error as ReplayError {
            XCTAssertTrue(error.description.contains("other arguments"), error.description)
        }
        do {
            _ = try await kv[kvSet]!(keyArgs("a"))
            XCTFail("expected a deviation")
        } catch let error as ReplayError {
            XCTAssertTrue(error.description.contains("recording has Kv.get next"), error.description)
        }
        guard case .mismatch(_, let called, _, let argsDiffer, let nth) = replayer.errors()[0] else { return XCTFail("mismatch") }
        XCTAssertEqual([called, "\(argsDiffer)", "\(nth)"], ["Kv.get", "true", "0"])
        // Nothing was consumed: the right call still matches, and then the port is used up.
        let right = try await kv[kvGet]!(keyArgs("a"))
        XCTAssertFalse(right.isEmpty)
        do {
            _ = try await kv[kvGet]!(keyArgs("a"))
            XCTFail("expected exhaustion")
        } catch let error as ReplayError {
            XCTAssertTrue(error.description.contains("used up"), error.description)
        }
        XCTAssertEqual(replayer.errors().count, 3)
        XCTAssertThrowsError(try replayer.finish())
    }

    func testRecordedCallsThatWereNeverMadeAreUnconsumed() throws {
        let replayer = Replayer(try Recording(json: try fixture("fixtures/ports-remote-todos.json")))
        XCTAssertEqual(replayer.remaining, 3)
        let problems = replayer.problems()
        var next = Set<String>()
        for problem in problems {
            guard case .unconsumed(_, let name, _) = problem else { return XCTFail("expected unconsumed") }
            next.insert(name)
        }
        XCTAssertEqual(next, ["Clock.now_ms", "Rng.fill", "Http.request"])
        XCTAssertThrowsError(try replayer.finish()) { XCTAssertTrue("\($0)".contains("never made")) }
    }

    @MainActor
    func testACallTheRecordingHoldsNoReplyForIsReportedNamingThePortAndTheMethod() async throws {
        let core = try scratchCore()
        defer { core.close() }
        let recording = Recording(
            schemaHash: 1, source: "hand",
            events: [RecordedEvent(t: 0, kind: .portCall(port: undraPortId("Kv"), method: kvGet, call: 1, args: keyArgs("a")))]
        )
        let replayer = Replayer(recording)
        let kv = methods(replayer.adapters().all.first!.makePortImpl(core: core.core))
        do {
            _ = try await kv[kvGet]!(keyArgs("a"))
            XCTFail("expected unavailable")
        } catch {}
        XCTAssertEqual(replayer.errors(), [.unanswered(port: undraPortId("Kv"), called: "Kv.get", nth: 0)])
        XCTAssertThrowsError(try replayer.finish()) { error in
            XCTAssertTrue("\(error)".contains("Kv.get"), "\(error)")
            XCTAssertTrue("\(error)".contains("no reply in the recording"), "\(error)")
        }
    }

    @MainActor
    func testArgumentsThatChangeFromRunToRunCanBeIgnored() async throws {
        let core = try scratchCore()
        defer { core.close() }
        let recorder = PortRecorder(schemaHash: 1, now: { 0 })
        guard case .async(let table) = recorder.wrap(portId: undraPortId("Kv"), MemKv().makePortImpl(core: core.core)!) else { return XCTFail("async") }
        _ = try await table[kvGet]!(keyArgs("a"))
        let replayer = Replayer(recorder.record(), policy: .ignore)
        let kv = methods(replayer.adapters().all.first!.makePortImpl(core: core.core))
        _ = try await kv[kvGet]!(keyArgs("generated-id-7"))
        XCTAssertEqual(replayer.errors(), [])
    }

    @MainActor
    func testARecordedTypedErrorAndAnUnavailableAnswerReplayAsThemselves() async throws {
        let core = try scratchCore()
        defer { core.close() }
        struct NoSuchPort: Error {}
        let failing = PortImpl.async([
            1: { _ in throw UndraPortError(body: [7]) },
            2: { _ in throw NoSuchPort() },
        ])
        let recorder = PortRecorder(schemaHash: 1, now: { 0 })
        guard case .async(let table) = recorder.wrap(portId: 99, failing) else { return XCTFail("async") }
        do { _ = try await table[1]!([]); XCTFail("expected a typed error") } catch let error as UndraPortError { XCTAssertEqual(error.body, [7]) }
        do { _ = try await table[2]!([]); XCTFail("expected a failure") } catch is NoSuchPort {}
        let replayer = Replayer(try Recording(json: recorder.toJSON()))
        let methods = methods(replayer.adapters().all.first!.makePortImpl(core: core.core))
        do { _ = try await methods[1]!([]); XCTFail("expected a typed error") } catch let error as UndraPortError { XCTAssertEqual(error.body, [7]) }
        do { _ = try await methods[2]!([]); XCTFail("expected unavailable") } catch {}
        try replayer.finish()
    }
}
