import Foundation
import XCTest
@testable import KeelRuntime

/// Streams: ordering, the credit window (docs/SPEC.md section 3.7), errors and cancellation.
@MainActor
final class CoreStreamTests: XCTestCase {
    private func numbered(_ count: Int) -> [[UInt8]] {
        return (0 ..< count).map { (index: Int) -> [UInt8] in
            return [UInt8(index)]
        }
    }

    private func makeStreamingCore(_ transport: FakeTransport, items: [[UInt8]], holdOpen: Bool = false) throws -> KeelCore {
        transport.onCall = { call, fake in
            fake.openStream(call.callId, items: items, holdOpen: holdOpen)
            return true
        }
        return try makeCore(transport)
    }

    // MARK: Delivery

    func testStreamDeliversItemsInOrderThenEnds() async throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: numbered(40))
        var received: [[UInt8]] = []
        for try await item in core.stream(.freeFunction(methodId: 8), method: 8, args: [1]) {
            received.append(item)
        }
        XCTAssertEqual(received, numbered(40))
        XCTAssertEqual(Array(transport.calls[0].args), [1])
        XCTAssertTrue(transport.cancels.isEmpty, "a stream that ended by itself is not cancelled")
        XCTAssertEqual(core.stats().hostOpenStreams, 0)
        XCTAssertEqual(core.stats().hostPendingCalls, 0)
    }

    func testStreamGrantsTheInitialCreditAsSoonAsItIsOpen() throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: numbered(50), holdOpen: true)
        let stream = core.stream(.freeFunction(methodId: 8), method: 8, args: [])
        let callId = transport.calls[0].callId
        XCTAssertEqual(transport.credits(for: callId), [16])
        XCTAssertEqual(transport.deliveredCount(callId), 16, "the core sends exactly what was credited")
        XCTAssertEqual(core.stats().hostOpenStreams, 1)
        _ = stream
    }

    func testAnEmptyStreamEndsImmediately() async throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: [])
        var count = 0
        for try await _ in core.stream(.freeFunction(methodId: 8), method: 8, args: []) {
            count += 1
        }
        XCTAssertEqual(count, 0)
    }

    func testItemsArrivingFromAnotherThreadWhileTheConsumerWaits() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let stream = core.stream(.freeFunction(methodId: 8), method: 8, args: [])
        let callId = transport.calls[0].callId
        DispatchQueue.global().async {
            transport.deliver(Wire.Reply(callId: callId, status: .streamOpened))
            for index in 0 ..< 10 {
                Thread.sleep(forTimeInterval: 0.002)
                transport.deliverStreamItem(Wire.StreamItem(callId: callId, flag: .item, body: [UInt8(index)]))
            }
            transport.deliverStreamItem(Wire.StreamItem(callId: callId, flag: .end))
        }
        var received: [[UInt8]] = []
        for try await item in stream {
            received.append(item)
        }
        XCTAssertEqual(received, numbered(10))
    }

    func testTwoStreamsDoNotInterfere() async throws {
        let transport = FakeTransport()
        transport.onCall = { call, fake in
            let base = UInt8(call.callId & 0x7F) &* 2
            let items: [[UInt8]] = [[base], [base &+ 1]]
            fake.openStream(call.callId, items: items)
            return true
        }
        let core = try makeCore(transport)
        let first = core.stream(.freeFunction(methodId: 8), method: 8, args: [])
        let second = core.stream(.freeFunction(methodId: 9), method: 9, args: [])
        var a: [[UInt8]] = []
        var b: [[UInt8]] = []
        for try await item in second {
            b.append(item)
        }
        for try await item in first {
            a.append(item)
        }
        let ids = transport.calls.map { $0.callId }
        XCTAssertEqual(a, [[UInt8(ids[0] & 0x7F) &* 2], [UInt8(ids[0] & 0x7F) &* 2 &+ 1]])
        XCTAssertEqual(b, [[UInt8(ids[1] & 0x7F) &* 2], [UInt8(ids[1] & 0x7F) &* 2 &+ 1]])
    }

    // MARK: Flow control

    func testTheCoreNeverRunsMoreThanTheCreditWindowAheadOfTheConsumer() async throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: numbered(100), holdOpen: true)
        let stream = core.stream(.freeFunction(methodId: 8), method: 8, args: [])
        let callId = transport.calls[0].callId
        var iterator = stream.makeAsyncIterator()

        XCTAssertEqual(transport.deliveredCount(callId), 16)
        // Three items consumed: the window (13) is still above the low-water mark, no top-up.
        for expected in 0 ..< 3 {
            let item = try await iterator.next()
            XCTAssertEqual(item, [UInt8(expected)])
        }
        XCTAssertEqual(transport.credits(for: callId), [16])
        XCTAssertEqual(transport.deliveredCount(callId), 16)
        // The ninth item leaves a window of 7, below 8: the core is topped back up to 16.
        for expected in 3 ..< 9 {
            let item = try await iterator.next()
            XCTAssertEqual(item, [UInt8(expected)])
        }
        XCTAssertEqual(transport.credits(for: callId), [16, 9])
        XCTAssertEqual(transport.deliveredCount(callId), 25)
    }

    func testTheWindowNeverExceedsSixteenWhateverTheConsumptionPattern() async throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: numbered(200))
        let stream = core.stream(.freeFunction(methodId: 8), method: 8, args: [])
        let callId = transport.calls[0].callId
        var consumed = 0
        for try await item in stream {
            XCTAssertEqual(item, [UInt8(consumed)])
            consumed += 1
            let ahead = transport.deliveredCount(callId) - consumed
            XCTAssertLessThanOrEqual(ahead, 16, "at item \(consumed)")
            XCTAssertLessThanOrEqual(Int(transport.totalCredit(callId)) - consumed, 16, "at item \(consumed)")
        }
        XCTAssertEqual(consumed, 200)
        XCTAssertGreaterThanOrEqual(Int(transport.totalCredit(callId)), 200, "every item was sent against credit")
    }

    // MARK: Failures

    func testAStreamThatFailsDeliversItsItemsThenThrowsTheTypedBody() async throws {
        let transport = FakeTransport()
        transport.onCall = { call, fake in
            fake.openStream(call.callId, items: [[1], [2]], failureBody: [9, 9])
            return true
        }
        let core = try makeCore(transport)
        let received = Guarded<[[UInt8]]>([])
        let error = await captureError {
            for try await item in core.stream(.freeFunction(methodId: 8), method: 8, args: []) {
                received.withLock { (items: inout [[UInt8]]) -> Void in
                    items.append(item)
                }
            }
        }
        let seen = received.withLock { (items: inout [[UInt8]]) -> [[UInt8]] in
            return items
        }
        XCTAssertEqual(seen, [[1], [2]])
        XCTAssertEqual(error as? KeelReplyError, KeelReplyError(status: .error, body: [9, 9]))
        XCTAssertEqual(core.stats().hostOpenStreams, 0)
    }

    func testAStreamThatCannotBeOpenedThrowsTheReplyError() async throws {
        let transport = FakeTransport()
        transport.onCall = { call, fake in
            fake.replyError(call.callId, [4])
            return true
        }
        let core = try makeCore(transport)
        let error = await captureError {
            for try await _ in core.stream(.freeFunction(methodId: 8), method: 8, args: []) {}
        }
        XCTAssertEqual(error as? KeelReplyError, KeelReplyError(status: .error, body: [4]))
        XCTAssertEqual(core.stats().hostOpenStreams, 0)
    }

    func testAStreamRejectedByTheCoreThrowsBadRequest() async throws {
        let transport = FakeTransport()
        transport.onCall = { _, _ in return false }
        let core = try makeCore(transport)
        let error = await captureError {
            for try await _ in core.stream(.freeFunction(methodId: 8), method: 8, args: []) {}
        }
        XCTAssertEqual((error as? KeelReplyError)?.status, .badRequest)
        XCTAssertEqual(core.stats().hostOpenStreams, 0)
    }

    func testCallingAPlainMethodAsAStreamIsAProtocolError() async throws {
        let transport = FakeTransport()
        transport.onCall = { call, fake in
            fake.replyOk(call.callId, [1])
            return true
        }
        let core = try makeCore(transport)
        let error = await captureError {
            for try await _ in core.stream(.freeFunction(methodId: 8), method: 8, args: []) {}
        }
        let callId = transport.calls[0].callId
        XCTAssertEqual(error as? KeelProtocolError, KeelProtocolError.notAStream(callId: callId))
    }

    func testAMalformedStreamItemFailsTheStreamAndCancelsIt() async throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: [], holdOpen: true)
        let stream = core.stream(.freeFunction(methodId: 8), method: 8, args: [])
        let callId = transport.calls[0].callId
        // A StreamItem needs five bytes; three cannot be decoded.
        DispatchQueue.global().asyncAfter(deadline: .now() + 0.01) {
            transport.deliverRawStreamItem(callId, [1, 2, 3])
        }
        let error = await captureError {
            for try await _ in stream {}
        }
        XCTAssertTrue(error is KeelProtocolError, "\(String(describing: error))")
        let cancelled = await waitUntil { transport.cancels == [callId] }
        XCTAssertTrue(cancelled, "the core is told to drop a stream it cannot be understood on")
    }

    func testShutdownFailsOpenStreams() async throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: [[1]], holdOpen: true)
        let stream = core.stream(.freeFunction(methodId: 8), method: 8, args: [])
        var iterator = stream.makeAsyncIterator()
        let first = try await iterator.next()
        XCTAssertEqual(first, [1])
        core.shutdown()
        do {
            _ = try await iterator.next()
            XCTFail("the stream must fail at shutdown")
        } catch {
            XCTAssertEqual(error as? KeelTransportError, KeelTransportError.closed)
        }
    }

    // MARK: Ending early

    func testCancellingTheConsumerTaskCancelsTheStreamInTheCore() async throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: [[1]], holdOpen: true)
        let stream = core.stream(.freeFunction(methodId: 8), method: 8, args: [])
        let callId = transport.calls[0].callId
        let received = Guarded<[[UInt8]]>([])
        let task = Task { () -> Void in
            for try await item in stream {
                received.withLock { (items: inout [[UInt8]]) -> Void in
                    items.append(item)
                }
            }
        }
        let gotItem = await waitUntil {
            return received.withLock { (items: inout [[UInt8]]) -> Bool in
                return items.count == 1
            }
        }
        XCTAssertTrue(gotItem)
        task.cancel()
        let result = await task.result
        // Cancellation ends the iteration quietly, like AsyncStream.
        XCTAssertNoThrow(try result.get())
        XCTAssertEqual(transport.cancels, [callId])
        XCTAssertEqual(core.stats().hostOpenStreams, 0)
    }

    func testDroppingAStreamWithoutFinishingItCancelsItInTheCore() throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: numbered(50), holdOpen: true)
        let callId: UInt32
        do {
            let stream = core.stream(.freeFunction(methodId: 8), method: 8, args: [])
            callId = transport.calls[0].callId
            _ = stream
        }
        XCTAssertEqual(transport.cancels, [callId])
        XCTAssertEqual(core.stats().hostOpenStreams, 0)
    }

    func testBreakingOutOfTheLoopCancelsTheStream() async throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: numbered(50), holdOpen: true)
        var taken = 0
        do {
            let stream = core.stream(.freeFunction(methodId: 8), method: 8, args: [])
            for try await _ in stream {
                taken += 1
                if taken == 2 {
                    break
                }
            }
        }
        XCTAssertEqual(taken, 2)
        XCTAssertEqual(transport.cancels, [transport.calls[0].callId])
    }

    // MARK: Decoded streams (what generated stream methods call)

    private struct Doubled: Error, Equatable {
        var code: UInt8
    }

    private func decodedStream(_ core: KeelCore) -> AsyncThrowingStream<Int, any Error> {
        return core.stream(
            .freeFunction(methodId: 8),
            method: 8,
            args: [],
            decode: { (body: [UInt8]) throws -> Int in
                return Int(body[0]) * 2
            }
        )
    }

    func testADecodedStreamDeliversDecodedItemsInOrderThenEnds() async throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: numbered(40))
        var received: [Int] = []
        for try await item in decodedStream(core) {
            received.append(item)
        }
        XCTAssertEqual(received, (0 ..< 40).map { $0 * 2 })
        XCTAssertTrue(transport.cancels.isEmpty, "a stream that ended by itself is not cancelled")
        XCTAssertEqual(core.stats().hostOpenStreams, 0)
    }

    func testADecodedStreamGrantsCreditAsItsConsumerReads() async throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: numbered(100), holdOpen: true)
        let stream = decodedStream(core)
        let callId = transport.calls[0].callId
        var iterator = stream.makeAsyncIterator()

        XCTAssertEqual(transport.deliveredCount(callId), 16, "the initial credit, before anything is read")
        for expected in 0 ..< 3 {
            let item = try await iterator.next()
            XCTAssertEqual(item, expected * 2)
        }
        XCTAssertEqual(transport.credits(for: callId), [16], "three of sixteen read: no top-up yet")
        for expected in 3 ..< 9 {
            let item = try await iterator.next()
            XCTAssertEqual(item, expected * 2)
        }
        XCTAssertEqual(transport.credits(for: callId), [16, 9], "a window of seven is topped back up to sixteen")
        XCTAssertEqual(transport.deliveredCount(callId), 25)
    }

    func testADecodedStreamDoesNotRunAheadOfAConsumerThatStoppedReading() async throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: numbered(250), holdOpen: true)
        let stream = decodedStream(core)
        let callId = transport.calls[0].callId
        var iterator = stream.makeAsyncIterator()
        for _ in 0 ..< 5 {
            _ = try await iterator.next()
        }
        // Give a copying wrapper every chance to pull the rest of the stream in the background.
        try await Task.sleep(nanoseconds: 100_000_000)
        XCTAssertLessThanOrEqual(transport.deliveredCount(callId), 5 + 16, "the core ran ahead of a consumer that read 5")
        XCTAssertEqual(transport.credits(for: callId), [16])
    }

    func testADecodedStreamNeverHasMoreThanTheWindowInFlight() async throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: numbered(200))
        let stream = decodedStream(core)
        let callId = transport.calls[0].callId
        var consumed = 0
        for try await item in stream {
            XCTAssertEqual(item, (consumed & 0xFF) * 2)
            consumed += 1
            XCTAssertLessThanOrEqual(transport.deliveredCount(callId) - consumed, 16, "at item \(consumed)")
        }
        XCTAssertEqual(consumed, 200)
    }

    func testMapErrorTurnsAStreamFailureIntoTheConsumersError() async throws {
        let transport = FakeTransport()
        transport.onCall = { call, fake in
            fake.openStream(call.callId, items: [[1], [2]], failureBody: [7])
            return true
        }
        let core = try makeCore(transport)
        let stream = core.stream(
            .freeFunction(methodId: 8),
            method: 8,
            args: [],
            decode: { (body: [UInt8]) throws -> UInt8 in
                return body[0]
            },
            mapError: { (error: any Error) -> any Error in
                if let reply = error as? KeelReplyError, reply.status == .error {
                    return Doubled(code: reply.body[0])
                }
                return error
            }
        )
        var received: [UInt8] = []
        let error = await captureError {
            for try await item in stream {
                received.append(item)
            }
        }
        XCTAssertEqual(received, [1, 2])
        XCTAssertEqual(error as? Doubled, Doubled(code: 7))
    }

    func testAnItemThatCannotBeDecodedEndsTheStreamAndCancelsItInTheCore() async throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: numbered(50), holdOpen: true)
        let stream = core.stream(
            .freeFunction(methodId: 8),
            method: 8,
            args: [],
            decode: { (body: [UInt8]) throws -> UInt8 in
                if body[0] == 2 {
                    throw Doubled(code: body[0])
                }
                return body[0]
            }
        )
        let callId = transport.calls[0].callId
        var iterator = stream.makeAsyncIterator()
        var items: [UInt8] = []
        var failure: (any Error)?
        do {
            while let item = try await iterator.next() {
                items.append(item)
            }
        } catch {
            failure = error
        }
        XCTAssertEqual(items, [0, 1])
        XCTAssertEqual(failure as? Doubled, Doubled(code: 2))
        XCTAssertEqual(transport.cancels, [callId], "the core stops producing for a consumer that cannot read it")
        let after = try await iterator.next()
        XCTAssertNil(after, "a stream that failed is over")
        XCTAssertEqual(core.stats().hostOpenStreams, 0)
    }

    func testBreakingOutOfADecodedStreamCancelsItInTheCore() async throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: numbered(50), holdOpen: true)
        var taken = 0
        do {
            for try await _ in decodedStream(core) {
                taken += 1
                if taken == 2 {
                    break
                }
            }
        }
        XCTAssertEqual(taken, 2)
        XCTAssertEqual(transport.cancels, [transport.calls[0].callId])
        XCTAssertEqual(core.stats().hostOpenStreams, 0)
    }

    func testCancellingTheTaskOfADecodedStreamCancelsItInTheCore() async throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: [[1]], holdOpen: true)
        let stream = decodedStream(core)
        let callId = transport.calls[0].callId
        let received = Guarded<Int>(0)
        let task = Task { () -> Void in
            for try await _ in stream {
                received.withLock { (count: inout Int) -> Void in
                    count += 1
                }
            }
        }
        let gotItem = await waitUntil {
            return received.withLock { (count: inout Int) -> Bool in
                return count == 1
            }
        }
        XCTAssertTrue(gotItem)
        task.cancel()
        let result = await task.result
        XCTAssertNoThrow(try result.get())
        XCTAssertEqual(transport.cancels, [callId])
    }

    func testADecodedStreamOnAShutDownCoreFails() async throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: [[1]])
        core.shutdown()
        let error = await captureError {
            for try await _ in decodedStream(core) {}
        }
        XCTAssertNotNil(error)
    }
}
