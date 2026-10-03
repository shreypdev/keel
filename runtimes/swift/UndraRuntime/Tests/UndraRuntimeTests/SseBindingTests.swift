import Foundation
import XCTest
@testable import UndraRuntime

/// A scripted event stream: the test pushes events and ends it.
final class ScriptedEventStream: SseStream, @unchecked Sendable {
    private struct State {
        var inbox: [SseEvent] = []
        var end: SseError?
        var finished = false
        var closed = false
        var pulled = 0
        var waiter: CheckedContinuation<Void, Never>?
    }

    let url: String
    let lastEventId: String?
    private let state = Locked(State())

    init(url: String, lastEventId: String?) {
        self.url = url
        self.lastEventId = lastEventId
    }

    var events: AsyncThrowingStream<SseEvent, any Error> {
        return AsyncThrowingStream(unfolding: { [self] () async throws -> SseEvent? in
            while true {
                let outcome = self.state.withLock { (current: inout State) -> Result<SseEvent?, SseError>? in
                    if current.closed {
                        return .success(nil)
                    }
                    if !current.inbox.isEmpty {
                        current.pulled += 1
                        return .success(current.inbox.removeFirst())
                    }
                    if let end = current.end {
                        return .failure(end)
                    }
                    return current.finished ? .success(nil) : nil
                }
                if let outcome = outcome {
                    return try outcome.get()
                }
                await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
                    let ready = self.state.withLock { (current: inout State) -> Bool in
                        if !current.inbox.isEmpty || current.end != nil || current.finished || current.closed {
                            return true
                        }
                        current.waiter = continuation
                        return false
                    }
                    if ready {
                        continuation.resume()
                    }
                }
            }
        })
    }

    private func wake(_ change: (inout State) -> Void) {
        let waiter = state.withLock { (current: inout State) -> CheckedContinuation<Void, Never>? in
            change(&current)
            let taken = current.waiter
            current.waiter = nil
            return taken
        }
        waiter?.resume()
    }

    func push(_ events: SseEvent...) {
        wake { $0.inbox.append(contentsOf: events) }
    }

    func end(_ error: SseError) {
        wake { $0.end = error }
    }

    func finish() {
        wake { $0.finished = true }
    }

    var pulled: Int {
        return state.withLock { $0.pulled }
    }

    var isClosed: Bool {
        return state.withLock { $0.closed }
    }

    func close() async {
        wake { $0.closed = true }
    }
}

final class ScriptedSse: SseAdapter, @unchecked Sendable {
    private let state = Locked<(refusal: SseError?, streams: [ScriptedEventStream])>((nil, []))

    func refuse(with error: SseError?) {
        state.withLock { $0.refusal = error }
    }

    var streams: [ScriptedEventStream] {
        return state.withLock { $0.streams }
    }

    func open(url: String, headers: [Header], lastEventId: String?) async throws(SseError) -> any SseStream {
        if let refusal = state.withLock({ $0.refusal }) {
            throw refusal
        }
        let stream = ScriptedEventStream(url: url, lastEventId: lastEventId)
        state.withLock { $0.streams.append(stream) }
        return stream
    }
}

// MARK: - The Sse binding

final class SseBindingTests: XCTestCase {
    private func events(_ range: Range<Int>) -> [SseEvent] {
        return range.map { SseEvent(id: "\($0)", data: "e\($0)") }
    }

    func testOpenChecksTheUrlRegistersIdsAndPassesTheLastEventId() async throws {
        let adapter = ScriptedSse()
        let binding = SseBinding(adapter: adapter)
        let first = try await binding.open(url: "https://feed.test/a", headers: [], lastEventId: "7")
        let second = try await binding.open(url: "HTTP://feed.test/b", headers: [], lastEventId: nil)
        XCTAssertEqual([first, second], [1, 2])
        XCTAssertEqual(adapter.streams.map(\.lastEventId), ["7", nil])
        await expectThrows(SseError.refused(status: nil, message: "invalid URL: ws://feed.test")) { () async throws(SseError) -> UInt32 in
            try await binding.open(url: "ws://feed.test", headers: [], lastEventId: nil)
        }
        adapter.refuse(with: .refused(status: 204, message: "stop"))
        await expectThrows(SseError.refused(status: 204, message: "stop")) { () async throws(SseError) -> UInt32 in
            try await binding.open(url: "https://feed.test", headers: [], lastEventId: nil)
        }
        XCTAssertEqual(adapter.streams.count, 2)
    }

    func testNextHandsBatchesWithinTheWindowThenTheEnd() async throws {
        let adapter = ScriptedSse()
        let binding = SseBinding(adapter: adapter)
        let stream = try await binding.open(url: "https://feed.test", headers: [], lastEventId: nil)
        let scripted = adapter.streams[0]
        for event in events(0 ..< 40) {
            scripted.push(event)
        }
        await eventually("the read-ahead") { scripted.pulled == 16 }
        try await Task.sleep(nanoseconds: 50_000_000)
        XCTAssertEqual(scripted.pulled, 16, "16 before the first next, and no more")
        let three = try await binding.next(stream: stream, max: 3)
        XCTAssertEqual(three, events(0 ..< 3))
        await expectThrows(SseError.network("no event stream 5")) { () async throws(SseError) -> [SseEvent] in
            try await binding.next(stream: 5, max: 1)
        }
        scripted.end(.ended)
        var all = three
        while true {
            do {
                all += try await binding.next(stream: stream, max: 16)
            } catch {
                XCTAssertEqual(error, .ended)
                break
            }
        }
        XCTAssertEqual(all, events(0 ..< 40), "every event before the end, in order")
        await expectThrows(SseError.ended) { () async throws(SseError) -> [SseEvent] in
            try await binding.next(stream: stream, max: 16)
        }
    }

    func testAFinishedStreamEndedAndCloseIsClean() async throws {
        let adapter = ScriptedSse()
        let binding = SseBinding(adapter: adapter)
        let finished = try await binding.open(url: "https://feed.test", headers: [], lastEventId: nil)
        adapter.streams[0].finish()
        await expectThrows(SseError.ended) { () async throws(SseError) -> [SseEvent] in
            try await binding.next(stream: finished, max: 16)
        }
        let open = try await binding.open(url: "https://feed.test", headers: [], lastEventId: nil)
        let (refused, waiting) = await firstOfTwo { await capture { () async throws(SseError) -> [SseEvent] in try await binding.next(stream: open, max: 16) } }
        XCTAssertEqual(refused, .failure(.protocol("a next is already pending on stream \(open)")), "the second next is refused at once")
        try await binding.close(stream: open)
        let answered = try await waiting.value.get()
        XCTAssertEqual(answered, [], "close answers the pending next with nothing")
        XCTAssertTrue(adapter.streams[1].isClosed)
        try await binding.close(stream: open)
        let after = try await binding.next(stream: open, max: 1)
        XCTAssertEqual(after, [])
        await expectThrows(SseError.network("no event stream 9")) { () async throws(SseError) -> Void in
            try await binding.close(stream: 9)
        }
    }

    func testDetachClosesTheOpenStreams() async throws {
        let adapter = ScriptedSse()
        let port = SsePortAdapter(adapter)
        XCTAssertEqual(port.portId, 0x75d2_ef19)
        let core = try makeCore(FakeTransport())
        let impl = port.makePortImpl(core: core)
        let openArgs = encodeArgs { (w: inout UndraWriter) -> Void in
            w.writeString("https://feed.test")
            [Header]().undraEncode(&w)
            Optional<String>.some("3").undraEncode(&w)
        }
        let openReply = try await PortCaller.callAsync(impl, StandardPorts.Sse.open, openArgs)
        XCTAssertEqual(try UInt32.undraDecoded(from: openReply), 1)
        adapter.streams[0].push(SseEvent(data: "x"))
        let nextArgs = encodeArgs { (w: inout UndraWriter) -> Void in
            w.writeU32(1)
            w.writeU32(16)
        }
        let next = try [SseEvent].undraDecoded(from: await PortCaller.callAsync(impl, StandardPorts.Sse.next, nextArgs))
        XCTAssertEqual(next, [SseEvent(data: "x")])
        port.detach()
        await eventually("the shutdown close") { adapter.streams[0].isClosed }
        core.shutdown()
    }
}

// MARK: - The parser

final class SseParserTests: XCTestCase {
    /// The feed of contract-tests/servers/realtime-server.mjs (`/sse/feed`).
    static let feed = ": a comment\nretry: 1500\nid: 1\ndata: one\n\n"
        + "event: tick\nid: 2\ndata: two\ndata: lines\n\n"
        + "data: three\n\n"
        + "id: 4\r\ndata: four\r\n\r\n"
        + "event: ignored\n\n"

    static let feedEvents = [
        SseEvent(id: "1", event: "message", data: "one", retryMs: 1500),
        SseEvent(id: "2", event: "tick", data: "two\nlines", retryMs: nil),
        SseEvent(id: "2", event: "message", data: "three", retryMs: nil),
        SseEvent(id: "4", event: "message", data: "four", retryMs: nil),
    ]

    func testTheServersFeedParsesAsTheStandardSays() throws {
        var parser = SseParser()
        XCTAssertEqual(try parser.push(Array(SseParserTests.feed.utf8)), SseParserTests.feedEvents)
        XCTAssertEqual(parser.lastId, "4")
    }

    func testAResumedParserStartsFromTheLastEventId() throws {
        var parser = SseParser(lastEventId: "2")
        XCTAssertEqual(parser.lastId, "2")
        XCTAssertEqual(try parser.push(Array("data: three\n\nid: 4\ndata: four\n\n".utf8)), Array(SseParserTests.feedEvents.suffix(2)))
    }

    func testEveryChunkingGivesTheSameEvents() throws {
        let bytes = Array(SseParserTests.feed.utf8)
        for size in [1, 2, 3, 5, 7, 64] {
            var parser = SseParser()
            var events: [SseEvent] = []
            var start = 0
            while start < bytes.count {
                let end = min(start + size, bytes.count)
                events += try parser.push(bytes[start ..< end])
                start = end
            }
            XCTAssertEqual(events, SseParserTests.feedEvents, "chunks of \(size)")
        }
    }

    func testLineEndsFieldsAndBuffers() throws {
        var parser = SseParser()
        // CR alone ends lines; a CR then (in the next chunk) an LF is one line end.
        XCTAssertEqual(try parser.push(Array("data: a\rdata: b\r".utf8)), [])
        XCTAssertEqual(try parser.push(Array("\n\r\n".utf8)), [SseEvent(data: "a\nb")])
        // No space after the colon, one space dropped, a second kept; a field without a colon.
        XCTAssertEqual(try parser.push(Array("data:x\ndata:  y\ndata\n\n".utf8)), [SseEvent(data: "x\n y\n")])
        // An event with no data dispatches nothing and forgets its type.
        XCTAssertEqual(try parser.push(Array("event: lost\n\ndata: z\n\n".utf8)), [SseEvent(data: "z")])
        // An id with NUL is ignored; an empty id clears the last id.
        XCTAssertEqual(try parser.push(Array("id: 5\n\ndata: a\n\nid: 6\u{0}\ndata: b\n\nid\ndata: c\n\n".utf8)), [
            SseEvent(id: "5", data: "a"), SseEvent(id: "5", data: "b"), SseEvent(id: nil, data: "c"),
        ])
        // retry only with ASCII digits, and only for the next event.
        XCTAssertEqual(try parser.push(Array("retry: 12a\ndata: a\n\nretry: 30\ndata: b\n\ndata: c\n\n".utf8)), [
            SseEvent(data: "a"), SseEvent(data: "b", retryMs: 30), SseEvent(data: "c"),
        ])
        // Unknown fields and comments are ignored; an empty data line is an empty event.
        XCTAssertEqual(try parser.push(Array(":hi\nfoo: bar\ndata:\n\n".utf8)), [SseEvent(data: "")])
        // An event the body ends before its blank line is not dispatched.
        XCTAssertEqual(try parser.push(Array("data: cut".utf8)), [])
    }

    /// The standard splits a line at the code point U+003A, drops one U+0020 after it and refuses an id with a U+0000 in it, as
    /// the Kotlin and TypeScript parsers do (on UTF-16 code units). A combining mark after the colon, the space or the NUL belongs
    /// to the value: Swift's `Character`s would join it to them (`":\u{301}"` is one grapheme, not `":"`).
    func testAColonSpaceOrNulBeforeACombiningMarkIsStillOne() throws {
        var parser = SseParser()
        XCTAssertEqual(try parser.push(Array("data:\u{301}x\n\n".utf8)), [SseEvent(data: "\u{301}x")])
        XCTAssertEqual(try parser.push(Array("event: \u{301}\ndata: \u{301}y\n\n".utf8)), [SseEvent(event: "\u{301}", data: "\u{301}y")])
        XCTAssertEqual(try parser.push(Array("id:\u{301}\ndata: z\n\n".utf8)), [SseEvent(id: "\u{301}", data: "z")])
        XCTAssertEqual(try parser.push(Array("id: 7\u{0}\u{301}\ndata: w\n\n".utf8)), [SseEvent(id: "\u{301}", data: "w")], "an id with a NUL is ignored")
        XCTAssertEqual(try parser.push(Array(":\u{301} a comment\ndata: v\n\n".utf8)), [SseEvent(id: "\u{301}", data: "v")])
    }

    /// Bytes in contiguous storage (an array, a slice, a chunk of `Data`) are parsed a run at a time; any other sequence a byte at
    /// a time. Seeded bodies of awkward pieces (every line end, a byte order mark, characters of two to four bytes, combining
    /// marks, NUL, bytes that are not UTF-8), cut into seeded chunks, give the same events, the same error and the same last id
    /// both ways.
    func testRunsAndSingleBytesParseTheSame() throws {
        struct OneByteAtATime: Sequence {
            let bytes: ArraySlice<UInt8>
            func makeIterator() -> IndexingIterator<ArraySlice<UInt8>> {
                return bytes.makeIterator()
            }
        }
        let pieces: [[UInt8]] = [
            "data: ", "data:", "id: ", "id:", "event: ", "retry: ", "1500", "x", " ", ":", ": comment", "foo", "\u{E9}", "\u{65E5}\u{672C}",
            "\u{1F600}", "\u{301}", "\u{0}", "\n", "\r", "\r\n", "\n\n", "\r\r", "\r\n\r\n",
        ].map { Array($0.utf8) } + [[0xFF], [0xE6, 0x97], [0xEF, 0xBB, 0xBF]]
        var seed: UInt64 = 0x00C0_FFEE_2026_1002
        func next(_ bound: Int) -> Int {
            seed = seed &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
            return Int((seed >> 33) % UInt64(bound + 1))
        }
        var errors = 0
        for round in 0 ..< 400 {
            var body: [UInt8] = next(3) == 0 ? [0xEF, 0xBB, 0xBF] : []
            for _ in 0 ..< 60 {
                // Bytes that are not UTF-8 are rare, so that most bodies parse to their end.
                var piece = pieces[next(pieces.count - 1)]
                while piece.first.map({ $0 == 0xFF || $0 == 0xE6 }) == true && next(20) != 0 {
                    piece = pieces[next(pieces.count - 4)]
                }
                body += piece
            }
            var runs = SseParser(lastEventId: "start")
            var single = SseParser(lastEventId: "start")
            var byRuns: [SseEvent] = []
            var bySingle: [SseEvent] = []
            var runsError: SseError?
            var singleError: SseError?
            var start = 0
            while start < body.count {
                let end = min(body.count, start + 1 + next(40))
                if runsError == nil {
                    do throws(SseError) {
                        try runs.push(body[start ..< end], into: &byRuns)
                    } catch {
                        runsError = error
                    }
                }
                if singleError == nil {
                    do throws(SseError) {
                        try single.push(OneByteAtATime(bytes: body[start ..< end]), into: &bySingle)
                    } catch {
                        singleError = error
                    }
                }
                start = end
            }
            XCTAssertEqual(byRuns, bySingle, "round \(round)")
            XCTAssertEqual(runsError, singleError, "round \(round)")
            XCTAssertEqual(runs.lastId, single.lastId, "round \(round)")
            errors += runsError == nil ? 0 : 1
        }
        XCTAssertGreaterThan(errors, 0, "some bodies were not UTF-8")
        XCTAssertLessThan(errors, 400, "some bodies parsed to their end")
    }

    func testAByteOrderMarkAtTheStartIsSkippedAndInvalidUtf8IsAProtocolError() throws {
        var parser = SseParser()
        XCTAssertEqual(try parser.push([0xEF, 0xBB]), [])
        XCTAssertEqual(try parser.push([0xBF] + Array("data: é\n\n".utf8)), [SseEvent(data: "é")])
        var notABom = SseParser()
        XCTAssertThrowsError(try notABom.push([0xEF] + Array("data: x\n\n".utf8)), "EF then 'd' is not a BOM, and not UTF-8")
        var broken = SseParser()
        XCTAssertThrowsError(try broken.push([0x64, 0x61, 0x74, 0x61, 0x3A, 0xFF, 0x0A])) { error in
            XCTAssertEqual(error as? SseError, .protocol("the event stream is not UTF-8"))
        }
    }
}
