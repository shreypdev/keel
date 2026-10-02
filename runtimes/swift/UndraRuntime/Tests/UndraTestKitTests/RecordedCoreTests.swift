import XCTest
@testable import UndraTestKit
import UndraRuntime

/// Opens the Todos store of the recording the way a generated store does: construct, register with the mirror, observe every signal.
@MainActor
private final class RawTodos {
    let handle: UndraHandle
    var remaining = [UInt32]()

    init(core: UndraCore) throws {
        handle = try core.construct(type: fnv1a32("Todos"), method: fnv1a32("Todos.new"), args: [])
        core.mirror.register(handle) { [weak self] signal, op, reader in
            if signal == 3 && op == .fullValue, let value = try? reader.readU32() {
                self?.remaining.append(value)
            }
        }
        core.observe(handle, signal: Observe.allSignals, on: true)
    }

    var lastRemaining: UInt32? { remaining.last }
}

@MainActor
final class RecordedCoreTests: XCTestCase {
    private func load(_ options: ReplayOptions = ReplayOptions(), hash: UInt64? = nil) throws -> RecordedCore {
        let recording = try Recording(json: try fixture("fixtures/session-todos.json"))
        return try RecordedCore.load(recording, expectedSchemaHash: hash ?? recording.schemaHash, options: options)
    }

    func testShowsTheStateTheRecordingFirstSawThenFollowsThePlayhead() throws {
        let recorded = try load()
        defer { recorded.close() }
        XCTAssertEqual(recorded.playhead, 100)
        XCTAssertEqual(recorded.durationMs, 500)
        let todos = try RawTodos(core: recorded.core)
        XCTAssertEqual(todos.lastRemaining, 0)
        recorded.advance(ms: 100)
        XCTAssertEqual(todos.lastRemaining, 1)
        recorded.advance(ms: 200)
        XCTAssertEqual(todos.lastRemaining, 3)
        recorded.playAll()
        XCTAssertEqual(todos.lastRemaining, 2)
        XCTAssertEqual(recorded.playhead, 500)
    }

    func testStartsAnywhereAStoreObservedLateIsBroughtUpToThePlayhead() throws {
        let recorded = try load(ReplayOptions(startAtMs: 400))
        defer { recorded.close() }
        XCTAssertEqual(try RawTodos(core: recorded.core).lastRemaining, 3)
    }

    func testACallItHasNoRecordingForIsRefusedWithAFailureThatNamesIt() throws {
        let recorded = try load()
        defer { recorded.close() }
        _ = try RawTodos(core: recorded.core)
        // The recording holds one constructor reply; a second one has none left.
        XCTAssertThrowsError(try recorded.core.construct(type: fnv1a32("Todos"), method: fnv1a32("Todos.new"), args: [])) { error in
            XCTAssertTrue("\(error)".contains("no reply for constructor") || "\(UndraCallError.mapped(error))".contains("no reply for constructor"), "\(error)")
        }
    }

    func testAMethodCallIsAnsweredFromTheRecordedReplyWhateverTheArgumentsInOrder() async throws {
        let recorded = try load()
        defer { recorded.close() }
        let todos = try RawTodos(core: recorded.core)
        let target = CallTarget.objectMethod(handle: todos.handle, methodId: fnv1a32("Todos.add"))
        var w = UndraWriter()
        w.writeString("anything at all")
        let args = w.finish()
        let first = try await recorded.core.call(target, method: fnv1a32("Todos.add"), args: args)
        XCTAssertTrue(first.hex.hasSuffix("427579206d696c6b00"), first.hex) // "Buy milk", not done
        let second = try await recorded.core.call(target, method: fnv1a32("Todos.add"), args: args)
        XCTAssertTrue(second.hex.contains("57616c6b2074686520646f67"), second.hex) // "Walk the dog"
    }

    func testCanRepeatTheLastReplyInsteadOfRefusing() async throws {
        let recorded = try load(ReplayOptions(exhausted: .repeatLast))
        defer { recorded.close() }
        let todos = try RawTodos(core: recorded.core)
        let target = CallTarget.objectMethod(handle: todos.handle, methodId: fnv1a32("Todos.add"))
        var bodies = [String]()
        for _ in 0..<5 {
            bodies.append(try await recorded.core.call(target, method: fnv1a32("Todos.add"), args: []).hex)
        }
        XCTAssertEqual(bodies[2], bodies[3])
        XCTAssertEqual(bodies[2], bodies[4])
    }

    func testARecordingOfAnotherSchemaIsRefused() throws {
        XCTAssertThrowsError(try load(hash: 1)) { error in
            XCTAssertTrue(error is UndraSchemaMismatchError, "\(error)")
        }
        // The error carries both hashes, so a stale recording says which schema it belongs to.
        let recorded = try Recording(json: try fixture("fixtures/session-todos.json")).schemaHash
        XCTAssertThrowsError(try load(hash: 1)) { error in
            XCTAssertEqual(error as? UndraSchemaMismatchError, UndraSchemaMismatchError(expected: 1, got: recorded))
        }
    }

    func testReplaysAStreamsItemsWithTheCallIdOfTheLiveCall() async throws {
        let hash: UInt64 = 0x77
        let recording = Recording(schemaHash: hash, source: "hand", events: [
            RecordedEvent(t: 0, kind: .call(target: .function(method: 5), call: 9, args: [])),
            RecordedEvent(t: 0, kind: .reply(call: 9, status: .streamOpened, body: [])),
            RecordedEvent(t: 1, kind: .streamItem(call: 9, flag: .item, body: [1, 0, 0, 0])),
            RecordedEvent(t: 2, kind: .streamItem(call: 9, flag: .item, body: [2, 0, 0, 0])),
            RecordedEvent(t: 3, kind: .streamItem(call: 9, flag: .end, body: [])),
        ])
        let recorded = try RecordedCore.load(recording, expectedSchemaHash: hash)
        defer { recorded.close() }
        var seen = [UInt32]()
        for try await item in recorded.core.stream(.freeFunction(methodId: 5), method: 5, args: []) {
            var reader = UndraReader(item)
            seen.append(try reader.readU32())
        }
        XCTAssertEqual(seen, [1, 2])
    }
}
