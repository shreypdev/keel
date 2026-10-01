import Foundation
import XCTest
@testable import UndraRuntime

/// Streams: ordering, the credit window (docs/SPEC.md section 3.7), errors and cancellation.
@MainActor
final class CoreStreamTests: XCTestCase {
    private func numbered(_ count: Int) -> [[UInt8]] {
        return (0 ..< count).map { (index: Int) -> [UInt8] in
            return [UInt8(index)]
        }
    }

    private func makeStreamingCore(_ transport: FakeTransport, items: [[UInt8]], holdOpen: Bool = false) throws -> UndraCore {
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
            fake.openStream(call.callId, items: [[1], [2]], ending: .error([9, 9]))
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
        XCTAssertEqual(error as? UndraReplyError, UndraReplyError(status: .error, body: [9, 9]))
        XCTAssertEqual(core.stats().hostOpenStreams, 0)
    }

    // MARK: Failed items (flag 3, ADR-036)

    /// A core whose streams send `items` and then `ending`.
    private func makeEndingCore(_ transport: FakeTransport, items: [[UInt8]], ending: FakeTransport.Ending) throws -> UndraCore {
        transport.onCall = { call, fake in
            fake.openStream(call.callId, items: items, ending: ending)
            return true
        }
        return try makeCore(transport)
    }

    /// `texts` encoded one after the other: the body of a panic reply (message, backtrace) or of a
    /// refusal (reason), docs/SPEC.md section 3.4.
    private func strings(_ texts: String...) -> [UInt8] {
        var writer = UndraWriter()
        for text in texts {
            writer.writeString(text)
        }
        return writer.finish()
    }

    /// What a generated stream method returns: the items decoded, the failure mapped with
    /// `mapped(streamFailure:)`, or `mapped(streamFailure:domain:)` when the method has an error type.
    private func generatedStream(_ core: UndraCore, domain: Bool) -> AsyncThrowingStream<UInt8, any Error> {
        return core.stream(
            .freeFunction(methodId: 8),
            method: 8,
            args: [],
            decode: { (body: [UInt8]) throws -> UInt8 in
                return body[0]
            },
            mapError: { (error: any Error) -> any Error in
                if domain {
                    return UndraCallError.mapped(streamFailure: error, domain: WireTestError.self)
                }
                return UndraCallError.mapped(streamFailure: error)
            }
        )
    }

    /// Runs a generated stream to its end; returns the items it delivered and the error it threw.
    private func drain(_ stream: AsyncThrowingStream<UInt8, any Error>) async -> ([UInt8], (any Error)?) {
        var received: [UInt8] = []
        let error = await captureError {
            for try await item in stream {
                received.append(item)
            }
        }
        return (received, error)
    }

    func testAFailedItemEndsTheRawStreamWithTheReplyErrorItStandsFor() async throws {
        let cases: [(Wire.StreamFailure, UndraReplyError)] = [
            (
                Wire.StreamFailure(status: .cancelled, message: "the runtime shut down"),
                UndraReplyError(status: .cancelled, body: [])
            ),
            (
                Wire.StreamFailure(status: .panic, message: "boom", detail: "frame 0\nframe 1"),
                UndraReplyError(status: .panic, body: strings("boom", "frame 0\nframe 1"))
            ),
            (
                Wire.StreamFailure(status: .badRequest, message: "stale handle"),
                UndraReplyError(status: .badRequest, body: strings("stale handle"))
            ),
        ]
        for (failure, expected) in cases {
            let transport = FakeTransport()
            let core = try makeEndingCore(transport, items: [[1], [2]], ending: .failure(failure))
            var received: [[UInt8]] = []
            let error = await captureError {
                for try await item in core.stream(.freeFunction(methodId: 8), method: 8, args: []) {
                    received.append(item)
                }
            }
            XCTAssertEqual(received, [[1], [2]], "\(failure.status): the items before the failure arrive")
            XCTAssertEqual(error as? UndraReplyError, expected, "\(failure.status)")
            XCTAssertTrue(transport.cancels.isEmpty, "\(failure.status): a failed item is the stream's last; nothing to cancel")
            XCTAssertEqual(core.stats().hostOpenStreams, 0)
            XCTAssertEqual(core.stats().hostPendingCalls, 0)
        }
    }

    func testAStreamCancelledByTheCoreThrowsCancelledByCoreWithAndWithoutADomain() async throws {
        for reason in ["the runtime shut down", "a restore replaced the receiver"] {
            for domain in [false, true] {
                let transport = FakeTransport()
                let failure = Wire.StreamFailure(status: .cancelled, message: reason)
                let core = try makeEndingCore(transport, items: [[4]], ending: .failure(failure))
                let (received, error) = await drain(generatedStream(core, domain: domain))
                XCTAssertEqual(received, [4])
                XCTAssertEqual(error as? UndraCallError, .cancelledByCore, "\(reason), domain: \(domain)")
                XCTAssertFalse(error is CancellationError, "the consumer's task was not cancelled")
            }
        }
    }

    func testAPanickedStreamThrowsPanickedWithItsMessageAndBacktrace() async throws {
        for domain in [false, true] {
            let transport = FakeTransport()
            let failure = Wire.StreamFailure(status: .panic, message: "index out of bounds", detail: "0: core::panicking\n1: app::tick")
            let core = try makeEndingCore(transport, items: [[1], [2], [3]], ending: .failure(failure))
            let (received, error) = await drain(generatedStream(core, domain: domain))
            XCTAssertEqual(received, [1, 2, 3])
            XCTAssertEqual(
                error as? UndraCallError,
                .panicked(message: "index out of bounds", backtrace: "0: core::panicking\n1: app::tick"),
                "domain: \(domain)"
            )
        }
    }

    func testARefusedStreamThrowsRefusedWithTheReason() async throws {
        for domain in [false, true] {
            let transport = FakeTransport()
            let failure = Wire.StreamFailure(status: .badRequest, message: "the object was closed", detail: "ignored")
            let core = try makeEndingCore(transport, items: [], ending: .failure(failure))
            let (received, error) = await drain(generatedStream(core, domain: domain))
            XCTAssertEqual(received, [])
            XCTAssertEqual(error as? UndraCallError, .refused(reason: "the object was closed"), "domain: \(domain)")
        }
    }

    func testAFailedItemNeedsNoCreditAndComesAfterEveryItemInTheWindow() async throws {
        // Sixteen items use the whole initial credit; the failed item still follows, without credit.
        let transport = FakeTransport()
        let failure = Wire.StreamFailure(status: .cancelled, message: "the runtime shut down")
        let core = try makeEndingCore(transport, items: numbered(16), ending: .failure(failure))
        let stream = generatedStream(core, domain: false)
        let callId = transport.calls[0].callId
        XCTAssertEqual(transport.credits(for: callId), [16])
        XCTAssertEqual(transport.deliveredCount(callId), 16)
        XCTAssertEqual(core.stats().hostOpenStreams, 0, "the failed item already ended the stream")
        let (received, error) = await drain(stream)
        XCTAssertEqual(received, (0 ..< 16).map { UInt8($0) })
        XCTAssertEqual(error as? UndraCallError, .cancelledByCore)
        XCTAssertTrue(transport.cancels.isEmpty)
    }

    func testAFailedItemThatDoesNotDecodeIsMalformedAndNotCancelled() async throws {
        let cases: [(String, [UInt8], WireError)] = [
            ("empty", [], .unexpectedEOF(needed: 1, at: 0)),
            ("truncated message", [3, 1, 0], .unexpectedEOF(needed: 4, at: 1)),
            ("missing detail", [5, 0, 0, 0, 0], .unexpectedEOF(needed: 4, at: 5)),
            (
                "the typed-error status",
                Wire.StreamFailure(status: .error, message: "x").encode(),
                .invalidTag(tag: 1, at: 0, type: "StreamFailure.status")
            ),
            ("an unknown status", [6, 0, 0, 0, 0, 0, 0, 0, 0], .invalidTag(tag: 6, at: 0, type: "StreamFailure.status")),
            (
                "trailing bytes",
                Wire.StreamFailure(status: .cancelled, message: "").encode() + [0],
                .trailingBytes(count: 1)
            ),
        ]
        for (name, body, wire) in cases {
            let expected = UndraProtocolError.malformedMessage(context: "stream failure", error: wire)
            // The raw stream.
            let rawTransport = FakeTransport()
            let rawCore = try makeEndingCore(rawTransport, items: [[1]], ending: .failed(body))
            var received: [[UInt8]] = []
            let rawError = await captureError {
                for try await item in rawCore.stream(.freeFunction(methodId: 8), method: 8, args: []) {
                    received.append(item)
                }
            }
            XCTAssertEqual(received, [[1]], name)
            XCTAssertEqual(rawError as? UndraProtocolError, expected, name)
            XCTAssertTrue(rawTransport.cancels.isEmpty, "\(name): the core ended the stream with its flag")
            XCTAssertEqual(rawCore.stats().hostOpenStreams, 0, name)
            // A generated stream, with and without a domain.
            for domain in [false, true] {
                let transport = FakeTransport()
                let core = try makeEndingCore(transport, items: [[1]], ending: .failed(body))
                let (_, error) = await drain(generatedStream(core, domain: domain))
                XCTAssertEqual(error as? UndraCallError, .malformed(expected.description), "\(name), domain: \(domain)")
            }
        }
    }

    func testAnItemWithAnUnknownFlagFailsTheStreamAndCancelsIt() async throws {
        let transport = FakeTransport()
        let core = try makeStreamingCore(transport, items: [], holdOpen: true)
        let stream = generatedStream(core, domain: true)
        let callId = transport.calls[0].callId
        var payload = UndraWriter()
        payload.writeU32(callId)
        payload.writeU8(4)
        let bytes = payload.finish()
        DispatchQueue.global().asyncAfter(deadline: .now() + 0.01) {
            transport.deliverRawStreamItem(callId, bytes)
        }
        let (_, error) = await drain(stream)
        let expected = UndraProtocolError.malformedMessage(
            context: "stream item",
            error: .invalidTag(tag: 4, at: 4, type: "StreamFlag")
        )
        XCTAssertEqual(error as? UndraCallError, .malformed(expected.description))
        let cancelled = await waitUntil { transport.cancels == [callId] }
        XCTAssertTrue(cancelled, "an item the host cannot read may not be the last one: the core is told to stop")
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
        XCTAssertEqual(error as? UndraReplyError, UndraReplyError(status: .error, body: [4]))
        XCTAssertEqual(core.stats().hostOpenStreams, 0)
    }

    func testAStreamRejectedByTheCoreThrowsBadRequest() async throws {
        let transport = FakeTransport()
        transport.onCall = { _, _ in return false }
        let core = try makeCore(transport)
        let error = await captureError {
            for try await _ in core.stream(.freeFunction(methodId: 8), method: 8, args: []) {}
        }
        XCTAssertEqual((error as? UndraReplyError)?.status, .badRequest)
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
        XCTAssertEqual(error as? UndraProtocolError, UndraProtocolError.notAStream(callId: callId))
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
        XCTAssertTrue(error is UndraProtocolError, "\(String(describing: error))")
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
            XCTAssertEqual(error as? UndraTransportError, UndraTransportError.closed)
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

    private func decodedStream(_ core: UndraCore) -> AsyncThrowingStream<Int, any Error> {
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
            fake.openStream(call.callId, items: [[1], [2]], ending: .error([7]))
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
                if let reply = error as? UndraReplyError, reply.status == .error {
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
