import Foundation
import PlaygroundCore
import UndraRuntime
import UndraTestKit
import XCTest

// The Swift testing kit (docs/TESTING.md) against the real playground core through the C ABI: PreviewCore loads the app's own core with the
// deterministic fakes as its ports and a manual clock; RecordedCore plays a recorded session under the generated store.

private func fixture(_ name: String) throws -> String {
    var url = URL(fileURLWithPath: #filePath)
    for _ in 0..<5 {
        url.deleteLastPathComponent()
    }
    return try String(contentsOf: url.appendingPathComponent("testkit/\(name)"), encoding: .utf8)
}

@MainActor
private func waitUntil(_ what: @autoclosure () -> String, timeout: Duration = .seconds(5), _ condition: @MainActor () -> Bool) async throws {
    let deadline = ContinuousClock.now + timeout
    while !condition() {
        if ContinuousClock.now >= deadline {
            XCTFail("timed out waiting for \(what())")
            return
        }
        try await Task.sleep(for: .milliseconds(10))
    }
}

@MainActor
final class PreviewAndRecordedTests: XCTestCase {
    func testT1APreviewedStoreRunsTheRealLogicOnTheFakes() async throws {
        let preview = try PreviewCore.load(UndraPlaygroundCore.load, seed: try Seed(json: try fixture("fixtures/seed.json")))
        defer { preview.close() }
        let todos = try Todos(ctx: preview.core)
        _ = try await todos.add(title: "Buy milk")
        let walk = try await todos.add(title: "Walk the dog")
        todos.toggle(id: walk.id)
        await preview.settle()
        XCTAssertEqual(todos.visible.map { $0.title }, ["Buy milk", "Walk the dog"])
        XCTAssertEqual(todos.remaining, 1)
    }

    func testT2TheSeedAnswersTheQueryAndTheManualClockMakesItStale() async throws {
        let preview = try PreviewCore.load(UndraPlaygroundCore.load, seed: try Seed(json: try fixture("fixtures/seed.json")))
        defer { preview.close() }
        configureRemote(RemoteConfig(baseUrl: "https://api.test"), ctx: preview.core)
        let first = try RemoteTodosQueryHandle(list: "inbox", ctx: preview.core)
        await preview.settle()
        XCTAssertEqual(first.status, .success)
        XCTAssertEqual(first.data?.map { $0.title }, ["Buy milk", "Walk the dog"])
        XCTAssertEqual(preview.fakes.http.calls.map { $0.url }, ["https://api.test/lists/inbox/todos"])

        // Fresh for 30 s: a second observer is served from the cache.
        try await preview.advance(ms: 10_000)
        _ = try RemoteTodosQueryHandle(list: "inbox", ctx: preview.core)
        XCTAssertEqual(preview.fakes.http.calls.count, 1, "a fresh entry was fetched again")

        // Past it, the next observer fetches again, and everyone sees the new list.
        preview.fakes.http.reset()
        preview.fakes.http.respond(url: "https://api.test/lists/inbox/todos", httpResponse(status: 200, body: "[{\"id\":3,\"title\":\"Third\",\"done\":false}]"))
        try await preview.advance(ms: 31_000)
        _ = try RemoteTodosQueryHandle(list: "inbox", ctx: preview.core)
        await preview.settle()
        try await waitUntil("the refreshed list") { first.data?.map { $0.title } == ["Third"] }
        XCTAssertEqual(preview.fakes.http.calls.count, 1)
        XCTAssertEqual(preview.clock.nowMs, 1_700_000_041_000)
    }

    func testT3TheSeededPortsAreWhatTheCoreReadsAndWhatItWritesLandsInTheFakes() async throws {
        let preview = try PreviewCore.load(UndraPlaygroundCore.load, seed: try Seed(json: try fixture("fixtures/seed.json")))
        defer { preview.close() }
        let core = preview.core
        let greeting = try await kvGet(key: "greeting", ctx: core)
        XCTAssertEqual(greeting.map { String(decoding: $0, as: UTF8.self) }, "hello")
        let absent = try await kvGet(key: "absent", ctx: core)
        XCTAssertNil(absent)
        try await kvPut(key: "saved", value: [1, 2, 3], ctx: core)
        XCTAssertEqual(preview.fakes.kv.value("saved"), [1, 2, 3])
        let token = try await secretGet(key: "token", ctx: core)
        XCTAssertEqual(token.map { String(decoding: $0, as: UTF8.self) }, "t-123")
        let note = try await fileRead(path: "notes/a.txt", ctx: core)
        XCTAssertEqual(String(decoding: note, as: UTF8.self), "hello")
        try await fileWrite(path: "out/b.txt", data: Array("written".utf8), ctx: core)
        XCTAssertEqual(preview.fakes.fs.contents("out/b.txt").map { String(decoding: $0, as: UTF8.self) }, "written")
        do {
            _ = try await fileRead(path: "nope", ctx: core)
            XCTFail("a missing file is a typed error")
        } catch let error as FsError {
            XCTAssertEqual(error, .notFound)
        }
        XCTAssertEqual(preview.fakes.kv.ops.map { $0.op }.filter { $0 == "set" }.count, 1)
    }

    func testT4ARecordedSessionPlaysUnderTheGeneratedStore() async throws {
        let recorded = try RecordedCore.load(try Recording(json: try fixture("fixtures/session-todos.json")), expectedSchemaHash: UndraIds.schemaHash)
        defer { recorded.close() }
        let todos = try Todos(ctx: recorded.core)
        XCTAssertTrue(todos.todos.isEmpty && todos.remaining == 0, "the first state is the empty list")
        recorded.advance(ms: 100)
        XCTAssertEqual(todos.todos.map { $0.title }, ["Buy milk"])
        recorded.advance(ms: 200)
        XCTAssertEqual(todos.todos.map { $0.title }, ["Buy milk", "Walk the dog", "Write the docs"])
        XCTAssertEqual(todos.remaining, 3)
        recorded.playAll()
        XCTAssertEqual(todos.todos.map { $0.done }, [true, false, false])
        XCTAssertEqual(todos.remaining, 2)
    }
}
