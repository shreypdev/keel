import Foundation
import XCTest
@testable import UndraRuntime

// ADR-032: what a generated method throws, and how a command reports. Everything here is the
// runtime's half of the error channel; the generated half (`do { ... } catch { throw
// UndraCallError.mapped(error, domain: E.self) }`) is pinned by the bindgen goldens and the contract
// suite.

// MARK: - Fixtures

/// A hand-written stand-in for a generated error enum. The layout is chosen so that the ambiguity
/// of a stream's error item (ADR-032, Risks) can be pinned: `.code(UInt16)` is variant 0, so the
/// four bytes of an empty `String` read as `.code(0)`, and `.note(String)` carries a `String`.
enum WireTestError: UndraError {
    case code(UInt16)
    case note(String)
    case empty

    static func undraDecode(_ r: inout UndraReader) throws -> WireTestError {
        let at = r.position
        let index = try r.readU16()
        switch index {
        case 0:
            return .code(try r.readU16())
        case 1:
            return .note(try r.readString())
        case 2:
            return .empty
        default:
            throw WireError.invalidTag(tag: UInt32(index), at: at, type: "WireTestError")
        }
    }

    func undraEncode(_ w: inout UndraWriter) {
        switch self {
        case .code(let value):
            w.writeU16(0)
            w.writeU16(value)
        case .note(let text):
            w.writeU16(1)
            w.writeString(text)
        case .empty:
            w.writeU16(2)
        }
    }
}

/// The encoding of one `String`, which is what the core writes in a stream's error item when it ends
/// the stream itself.
private func stringBody(_ text: String) -> [UInt8] {
    var writer = UndraWriter()
    writer.writeString(text)
    return writer.finish()
}

/// The body of a panic reply: message, then backtrace.
private func panicBody(_ message: String, _ backtrace: String) -> [UInt8] {
    var writer = UndraWriter()
    writer.writeString(message)
    writer.writeString(backtrace)
    return writer.finish()
}

/// Records what `LoadOptions.onError` receives, from any thread.
private final class ErrorLog: @unchecked Sendable {
    private let items = Guarded<[UndraUnhandledError]>([])

    var all: [UndraUnhandledError] {
        return items.withLock { (current: inout [UndraUnhandledError]) -> [UndraUnhandledError] in
            return current
        }
    }

    func append(_ item: UndraUnhandledError) {
        items.withLock { (current: inout [UndraUnhandledError]) -> Void in
            current.append(item)
        }
    }
}

private func replyError(status: ReplyStatus, body: [UInt8]) -> UndraReplyError {
    return UndraReplyError(status: status, body: body)
}

// MARK: - The mapping

final class CallErrorMappingTests: XCTestCase {
    private func mappedCall(_ error: any Error, file: StaticString = #filePath, line: UInt = #line) -> UndraCallError? {
        let result = UndraCallError.mapped(error)
        guard let call = result as? UndraCallError else {
            XCTFail("expected an UndraCallError, got \(result)", file: file, line: line)
            return nil
        }
        return call
    }

    func testCancellationErrorStaysItself() {
        XCTAssertTrue(UndraCallError.mapped(CancellationError()) is CancellationError)
    }

    func testAnUndraCallErrorIsMappedToItself() {
        for original in [
            UndraCallError.cancelledByCore,
            .panicked(message: "m", backtrace: "b"),
            .refused(reason: "r"),
            .unavailable(.closed),
            .malformed("x"),
        ] {
            XCTAssertEqual(mappedCall(original), original)
            XCTAssertEqual(mappedCall(UndraCallError.mapped(original)), original, "idempotent")
        }
    }

    func testStatusCancelledIsCancelledByTheCoreAndNeverACancellationError() {
        let result = UndraCallError.mapped(replyError(status: .cancelled, body: []))
        XCTAssertFalse(result is CancellationError)
        XCTAssertEqual(result as? UndraCallError, .cancelledByCore)
    }

    func testStatusPanicCarriesMessageAndBacktrace() {
        XCTAssertEqual(
            mappedCall(replyError(status: .panic, body: panicBody("boom", "frame 0\nframe 1"))),
            .panicked(message: "boom", backtrace: "frame 0\nframe 1")
        )
    }

    func testAnUndecodablePanicReportStillMapsToPanicked() {
        XCTAssertEqual(
            mappedCall(replyError(status: .panic, body: [0xFF, 0xFF])),
            .panicked(message: "<undecodable panic report>", backtrace: "")
        )
        XCTAssertEqual(
            mappedCall(replyError(status: .panic, body: [])),
            .panicked(message: "<undecodable panic report>", backtrace: "")
        )
        // A message without a backtrace keeps the message.
        XCTAssertEqual(
            mappedCall(replyError(status: .panic, body: stringBody("only a message"))),
            .panicked(message: "only a message", backtrace: "")
        )
    }

    func testStatusBadRequestIsRefusedWithTheCoresText() {
        XCTAssertEqual(
            mappedCall(replyError(status: .badRequest, body: stringBody("E_REENTRANT: the core is calling out"))),
            .refused(reason: "E_REENTRANT: the core is calling out")
        )
        XCTAssertEqual(
            mappedCall(replyError(status: .badRequest, body: [0xFF])),
            .refused(reason: "<undecodable reason>")
        )
    }

    func testATypedErrorOnAMethodWithoutOneIsMalformed() {
        XCTAssertEqual(
            mappedCall(replyError(status: .error, body: [1, 2, 3])),
            .malformed("the core answered with a typed error, but this method has none (3 bytes)")
        )
    }

    func testAStreamWhereAReplyWasExpectedIsMalformed() {
        XCTAssertEqual(
            mappedCall(replyError(status: .streamOpened, body: [])),
            .malformed("the core opened a stream where a single reply was expected")
        )
    }

    func testStatusOkAsAFailureIsMalformedAndTotal() {
        XCTAssertEqual(
            mappedCall(replyError(status: .ok, body: [])),
            .malformed("the core answered ok as a failure")
        )
    }

    func testTransportErrorsAreUnavailable() {
        for error in [
            UndraTransportError.closed,
            .timedOut(operation: "callSync"),
            .connectionLost(reason: "reset"),
        ] {
            XCTAssertEqual(mappedCall(error), .unavailable(error))
        }
    }

    func testProtocolErrorsAreMalformed() {
        XCTAssertEqual(
            mappedCall(UndraProtocolError.nullHandle),
            .malformed(UndraProtocolError.nullHandle.description)
        )
        XCTAssertEqual(
            mappedCall(UndraProtocolError.notAStream(callId: 4)),
            .malformed(UndraProtocolError.notAStream(callId: 4).description)
        )
    }

    func testWireErrorsAreMalformed() {
        let wire = WireError.unexpectedEOF(needed: 4, at: 0)
        XCTAssertEqual(mappedCall(wire), .malformed("the reply does not decode: \(wire)"))
    }

    func testAModeErrorIsRefused() {
        let mode = UndraModeError(operation: "snapshot", mode: .remote)
        XCTAssertEqual(mappedCall(mode), .refused(reason: mode.description))
    }

    func testAnyOtherErrorIsReturnedUnchanged() {
        struct Foreign: Error, Equatable {}
        XCTAssertEqual(UndraCallError.mapped(Foreign()) as? Foreign, Foreign())
    }

    // MARK: Domain

    func testATypedReplyBecomesTheDomainError() {
        let body = WireTestError.note("no good").undraEncoded()
        let result = UndraCallError.mapped(replyError(status: .error, body: body), domain: WireTestError.self)
        XCTAssertEqual(result as? WireTestError, .note("no good"))
    }

    func testATypedReplyThatDoesNotDecodeIsMalformed() {
        let result = UndraCallError.mapped(replyError(status: .error, body: [0x63, 0x00]), domain: WireTestError.self)
        XCTAssertEqual(
            result as? UndraCallError,
            .malformed("a WireTestError that does not decode (2 bytes)")
        )
        // Trailing bytes after a valid value do not decode either.
        let trailing = WireTestError.empty.undraEncoded() + [0]
        XCTAssertTrue(
            UndraCallError.mapped(replyError(status: .error, body: trailing), domain: WireTestError.self)
                is UndraCallError
        )
    }

    func testNonTypedStatusesFallThroughWithADomain() {
        XCTAssertTrue(UndraCallError.mapped(CancellationError(), domain: WireTestError.self) is CancellationError)
        XCTAssertEqual(
            UndraCallError.mapped(replyError(status: .cancelled, body: []), domain: WireTestError.self) as? UndraCallError,
            .cancelledByCore
        )
        XCTAssertEqual(
            UndraCallError.mapped(UndraTransportError.closed, domain: WireTestError.self) as? UndraCallError,
            .unavailable(.closed)
        )
        XCTAssertEqual(
            UndraCallError.mapped(replyError(status: .panic, body: panicBody("p", "")), domain: WireTestError.self)
                as? UndraCallError,
            .panicked(message: "p", backtrace: "")
        )
    }

    // MARK: Streams

    private func streamError(_ body: [UInt8]) -> UndraReplyError {
        return UndraReplyError(status: .error, body: body)
    }

    func testAStreamErrorItemWithADomainReadsAsTheDomainError() {
        let result = UndraCallError.mapped(
            streamFailure: streamError(WireTestError.code(7).undraEncoded()),
            domain: WireTestError.self
        )
        XCTAssertEqual(result as? WireTestError, .code(7))
    }

    func testACancelledStringIsCancelledByTheCoreWithAndWithoutADomain() {
        let body = stringBody("cancelled: the runtime shut down")
        XCTAssertEqual(UndraCallError.mapped(streamFailure: streamError(body)) as? UndraCallError, .cancelledByCore)
        XCTAssertEqual(
            UndraCallError.mapped(streamFailure: streamError(body), domain: WireTestError.self) as? UndraCallError,
            .cancelledByCore
        )
    }

    func testAnyOtherStringIsAPanicWithoutABacktrace() {
        let body = stringBody("index out of bounds")
        let expected = UndraCallError.panicked(message: "index out of bounds", backtrace: "")
        XCTAssertEqual(UndraCallError.mapped(streamFailure: streamError(body)) as? UndraCallError, expected)
        XCTAssertEqual(
            UndraCallError.mapped(streamFailure: streamError(body), domain: WireTestError.self) as? UndraCallError,
            expected
        )
    }

    func testAGarbageStreamErrorItemIsMalformed() {
        let garbage: [UInt8] = [0xFF, 0xFF, 0xFF]
        let expected = UndraCallError.malformed("a stream error item that does not decode (3 bytes)")
        XCTAssertEqual(UndraCallError.mapped(streamFailure: streamError(garbage)) as? UndraCallError, expected)
        XCTAssertEqual(
            UndraCallError.mapped(streamFailure: streamError(garbage), domain: WireTestError.self) as? UndraCallError,
            expected
        )
        // A String followed by more bytes is not "exactly one String".
        let trailing = stringBody("cancelled: x") + [0]
        XCTAssertEqual(
            UndraCallError.mapped(streamFailure: streamError(trailing)) as? UndraCallError,
            .malformed("a stream error item that does not decode (17 bytes)")
        )
    }

    func testAStreamFailureThatIsNotAnErrorItemUsesTheOrdinaryMapping() {
        XCTAssertTrue(UndraCallError.mapped(streamFailure: CancellationError()) is CancellationError)
        XCTAssertEqual(
            UndraCallError.mapped(streamFailure: UndraTransportError.closed, domain: WireTestError.self)
                as? UndraCallError,
            .unavailable(.closed)
        )
        XCTAssertEqual(
            UndraCallError.mapped(streamFailure: replyError(status: .panic, body: panicBody("p", "b"))) as? UndraCallError,
            .panicked(message: "p", backtrace: "b")
        )
        XCTAssertEqual(
            UndraCallError.mapped(streamFailure: replyError(status: .badRequest, body: stringBody("stale")))
                as? UndraCallError,
            .refused(reason: "stale")
        )
    }

    /// ADR-032, Risks: a stream's error item is either the encoded `E` or the core's `String`, and
    /// nothing on the wire says which. With a domain, `E` is tried first. These two tests pin that.
    func testAnEncodedErrorWithAStringPayloadResolvesAsTheErrorNotAsAString() {
        // `.note("cancelled: x")` encodes to u16 1, then the String; it does not read as one String
        // (its first u32 is huge), but even a payload that spells the core's prefix is the error.
        let body = WireTestError.note("cancelled: the runtime shut down").undraEncoded()
        XCTAssertEqual(
            UndraCallError.mapped(streamFailure: streamError(body), domain: WireTestError.self) as? WireTestError,
            .note("cancelled: the runtime shut down")
        )
        // Without a domain the same bytes are not a String, so they are malformed, not a panic.
        guard case .malformed? = UndraCallError.mapped(streamFailure: streamError(body)) as? UndraCallError else {
            return XCTFail("expected .malformed")
        }
    }

    func testAStringThatAlsoDecodesAsTheErrorResolvesAsTheError() {
        // An empty message is four zero bytes, which is also `WireTestError.code(0)` (variant 0 and a
        // zero `UInt16`). The documented tie-break: the domain error wins.
        let body = stringBody("")
        XCTAssertEqual(body, [0, 0, 0, 0])
        XCTAssertEqual(
            UndraCallError.mapped(streamFailure: streamError(body), domain: WireTestError.self) as? WireTestError,
            .code(0)
        )
        // A stream without an error type has no `E` to prefer: it is a panic with no message.
        XCTAssertEqual(
            UndraCallError.mapped(streamFailure: streamError(body)) as? UndraCallError,
            .panicked(message: "", backtrace: "")
        )
    }

    // MARK: Descriptions

    func testDescriptionsAreTheDocumentedTexts() {
        let cases: [(UndraCallError, String)] = [
            (.cancelledByCore, "the Undra core cancelled the call (a restore replaced its object, or the core shut down)"),
            (.panicked(message: "boom", backtrace: "frame"), "the Undra core panicked: boom"),
            (.refused(reason: "stale handle"), "the Undra core refused the call: stale handle"),
            (.unavailable(.closed), "the Undra core is unavailable: the Undra core is shut down or disconnected"),
            (.malformed("short read"), "the Undra core sent a reply the bindings cannot read: short read"),
        ]
        for (error, text) in cases {
            XCTAssertEqual(error.description, text)
            XCTAssertEqual(error.errorDescription, text)
            XCTAssertEqual(error.localizedDescription, text)
        }
    }

    func testAnUnhandledErrorNamesTheOperationAndTheReason() {
        let unhandled = UndraUnhandledError(operation: "Todos.toggle", error: .refused(reason: "closed"))
        XCTAssertEqual(unhandled.description, "Todos.toggle failed: the Undra core refused the call: closed")
        XCTAssertEqual(unhandled, UndraUnhandledError(operation: "Todos.toggle", error: .refused(reason: "closed")))
        XCTAssertEqual(unhandled.operation, "Todos.toggle")
        XCTAssertEqual(unhandled.error, .refused(reason: "closed"))
    }
}

// MARK: - Through UndraCore

@MainActor
final class CallErrorCoreTests: XCTestCase {
    private let target = CallTarget.freeFunction(methodId: 4)

    private func replyWith(_ status: ReplyStatus, _ body: [UInt8]) -> @Sendable (Wire.Call) -> [UInt8] {
        return { call in
            return Wire.Reply(callId: call.callId, status: status, body: ArraySlice(body)).encode()
        }
    }

    /// What a generated sync method without an error type does, written out once.
    private func syncCall(_ core: UndraCore) throws -> [UInt8] {
        do {
            return try core.callSync(target, method: 4, args: [])
        } catch {
            throw UndraCallError.mapped(error)
        }
    }

    /// What a generated async method with an error type does.
    private func asyncCall(_ core: UndraCore) async throws -> [UInt8] {
        do {
            return try await core.call(target, method: 4, args: [])
        } catch {
            throw UndraCallError.mapped(error, domain: WireTestError.self)
        }
    }

    func testCallSyncStatusesMapThroughTheGeneratedPattern() throws {
        let expectations: [(ReplyStatus, [UInt8], UndraCallError)] = [
            (.panic, panicBody("boom", "bt"), .panicked(message: "boom", backtrace: "bt")),
            (.cancelled, [], .cancelledByCore),
            (.badRequest, stringBody("closed handle"), .refused(reason: "closed handle")),
        ]
        for (status, body, expected) in expectations {
            let transport = FakeTransport()
            transport.onCallSync = replyWith(status, body)
            let core = try makeCore(transport)
            XCTAssertThrowsError(try syncCall(core), "\(status)") { error in
                XCTAssertEqual(error as? UndraCallError, expected, "\(status)")
            }
            XCTAssertEqual(core.stats().hostPendingCalls, 0)
        }
    }

    func testCallSyncWithAGarbageReplyIsMalformed() throws {
        let transport = FakeTransport()
        transport.onCallSync = { _ in return [0xFF] }
        let core = try makeCore(transport)
        XCTAssertThrowsError(try syncCall(core)) { error in
            guard case .malformed(let text)? = error as? UndraCallError else {
                return XCTFail("expected .malformed, got \(error)")
            }
            XCTAssertTrue(text.contains("malformed reply"), text)
        }
    }

    func testCallSyncAfterShutdownIsUnavailable() throws {
        let core = try makeCore(FakeTransport())
        core.shutdown()
        XCTAssertThrowsError(try syncCall(core)) { error in
            XCTAssertEqual(error as? UndraCallError, .unavailable(.closed))
        }
    }

    func testCallSyncOverARemoteTransportThatTimesOutIsUnavailable() throws {
        let transport = FakeTransport(directSync: false)
        let core = try makeCore(transport, blockingCallTimeout: 0.05)
        XCTAssertThrowsError(try syncCall(core)) { error in
            XCTAssertEqual(error as? UndraCallError, .unavailable(.timedOut(operation: "callSync")))
        }
    }

    func testCallStatusesMapThroughTheGeneratedPattern() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let statuses: [(ReplyStatus, [UInt8])] = [
            (.panic, panicBody("boom", "bt")),
            (.cancelled, []),
            (.badRequest, stringBody("stale")),
            (.error, WireTestError.note("typed").undraEncoded()),
        ]
        var seen: [any Error] = []
        for (status, body) in statuses {
            transport.onCall = { call, fake in
                fake.deliver(Wire.Reply(callId: call.callId, status: status, body: ArraySlice(body)))
                return true
            }
            if let error = await captureError({ _ = try await self.asyncCall(core) }) {
                seen.append(error)
            }
        }
        XCTAssertEqual(seen.count, 4)
        XCTAssertEqual(seen[0] as? UndraCallError, .panicked(message: "boom", backtrace: "bt"))
        XCTAssertEqual(seen[1] as? UndraCallError, .cancelledByCore)
        XCTAssertEqual(seen[2] as? UndraCallError, .refused(reason: "stale"))
        XCTAssertEqual(seen[3] as? WireTestError, .note("typed"))
    }

    func testTheCoreRefusingACallWithoutAReplyIsRefused() async throws {
        let transport = FakeTransport()
        transport.onCall = { _, _ in return false }
        let core = try makeCore(transport)
        let error = await captureError { _ = try await self.asyncCall(core) }
        guard case .refused? = error as? UndraCallError else {
            return XCTFail("expected .refused, got \(String(describing: error))")
        }
    }

    func testACallCancelledWhileWaitingThrowsCancellationErrorAndNotCancelledByCore() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let task = Task { () -> [UInt8] in
            return try await self.asyncCall(core)
        }
        let sent = await waitUntil { !transport.calls.isEmpty }
        XCTAssertTrue(sent)
        task.cancel()
        let result = await task.result
        guard case .failure(let error) = result else {
            return XCTFail("a cancelled call must not succeed")
        }
        XCTAssertTrue(error is CancellationError, "\(error)")
        XCTAssertNil(error as? UndraCallError)
        // The core's status 3 for the same call arrives late and finds nobody waiting.
        transport.deliver(Wire.Reply(callId: transport.calls[0].callId, status: .cancelled))
    }

    func testACallCancelledBeforeItIsSentThrowsCancellationError() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let task = Task { () -> [UInt8] in
            withUnsafeCurrentTask { (current: UnsafeCurrentTask?) -> Void in
                current?.cancel()
            }
            return try await self.asyncCall(core)
        }
        let result = await task.result
        guard case .failure(let error) = result else {
            return XCTFail("a cancelled call must not succeed")
        }
        XCTAssertTrue(error is CancellationError, "\(error)")
        XCTAssertEqual(transport.calls.count, 0)
    }

    func testACallFailedByShutdownIsUnavailable() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let task = Task { () -> [UInt8] in
            return try await self.asyncCall(core)
        }
        let sent = await waitUntil { !transport.calls.isEmpty }
        XCTAssertTrue(sent)
        core.shutdown()
        let result = await task.result
        guard case .failure(let error) = result else {
            return XCTFail("a call in flight at shutdown must fail")
        }
        XCTAssertEqual(error as? UndraCallError, .unavailable(.closed))
        let later = await captureError { _ = try await self.asyncCall(core) }
        XCTAssertEqual(later as? UndraCallError, .unavailable(.closed))
    }

    func testAnUndecodableConstructorHandleIsMalformed() throws {
        let transport = FakeTransport()
        transport.onCallSync = { call in
            return Wire.Reply(callId: call.callId, status: .ok, body: [1, 2]).encode()
        }
        let core = try makeCore(transport)
        do {
            _ = try core.construct(type: 1, method: 2, args: [])
            XCTFail("a short handle must not construct")
        } catch {
            guard case .malformed? = UndraCallError.mapped(error) as? UndraCallError else {
                return XCTFail("expected .malformed, got \(error)")
            }
        }
    }

    // MARK: Streams end to end

    private func failedStream(_ core: UndraCore, domain: Bool) async -> (any Error)? {
        let stream = core.stream(
            .freeFunction(methodId: 8),
            method: 8,
            args: [],
            decode: { (body: [UInt8]) throws -> UInt8 in return body[0] },
            mapError: { (error: any Error) -> any Error in
                if domain {
                    return UndraCallError.mapped(streamFailure: error, domain: WireTestError.self)
                }
                return UndraCallError.mapped(streamFailure: error)
            }
        )
        return await captureError {
            for try await _ in stream {}
        }
    }

    func testStreamsEndWithTheMappedErrorForEveryKindOfFailure() async throws {
        let bodies: [(String, [UInt8], Bool, (any Error) -> Bool)] = [
            ("typed", WireTestError.code(9).undraEncoded(), true, { ($0 as? WireTestError) == .code(9) }),
            ("restore", stringBody("cancelled: restore replaced the stores"), true, { ($0 as? UndraCallError) == .cancelledByCore }),
            ("shutdown", stringBody("cancelled: the runtime shut down"), false, { ($0 as? UndraCallError) == .cancelledByCore }),
            ("panic", stringBody("stream blew up"), false, { ($0 as? UndraCallError) == .panicked(message: "stream blew up", backtrace: "") }),
        ]
        for (name, body, domain, check) in bodies {
            let transport = FakeTransport()
            transport.onCall = { call, fake in
                fake.openStream(call.callId, items: [[1]], failureBody: body)
                return true
            }
            let core = try makeCore(transport)
            let error = await failedStream(core, domain: domain)
            XCTAssertNotNil(error, name)
            XCTAssertTrue(error.map(check) ?? false, "\(name): \(String(describing: error))")
        }
    }

    func testAStreamOnAShutDownCoreEndsUnavailable() async throws {
        let core = try makeCore(FakeTransport())
        core.shutdown()
        let error = await failedStream(core, domain: true)
        XCTAssertEqual(error as? UndraCallError, .unavailable(.closed))
    }

    func testCancellingAStreamConsumerStillEndsQuietly() async throws {
        let transport = FakeTransport()
        transport.onCall = { call, fake in
            fake.openStream(call.callId, items: [[1]], holdOpen: true)
            return true
        }
        let core = try makeCore(transport)
        let stream = core.stream(
            .freeFunction(methodId: 8),
            method: 8,
            args: [],
            decode: { (body: [UInt8]) throws -> UInt8 in return body[0] },
            mapError: { UndraCallError.mapped(streamFailure: $0) }
        )
        let consumer = Task { () -> Int in
            var count = 0
            for try await _ in stream {
                count += 1
            }
            return count
        }
        let started = await waitUntil { !transport.calls.isEmpty }
        XCTAssertTrue(started)
        consumer.cancel()
        let result = await consumer.result
        switch result {
        case .success:
            break
        case .failure(let error):
            XCTFail("consumer cancellation must end the loop quietly, got \(error)")
        }
    }
}

// MARK: - report

final class ReportTests: XCTestCase {
    private func core(onError: (@Sendable (UndraUnhandledError) -> Void)?) throws -> UndraCore {
        var options = LoadOptions.inproc(adapters: Adapters.none, expectedSchemaHash: 0x1234)
        options.onError = onError
        return try UndraCore.connect(transport: FakeTransport(), options: options)
    }

    func testReportCallsTheHandlerOnceWithTheOperationAndTheMappedError() throws {
        let log = ErrorLog()
        let core = try core(onError: { log.append($0) })
        core.report(replyError(status: .badRequest, body: stringBody("closed")), operation: "Todos.toggle")
        XCTAssertEqual(log.all, [UndraUnhandledError(operation: "Todos.toggle", error: .refused(reason: "closed"))])
    }

    func testReportMapsEveryRuntimeError() throws {
        let log = ErrorLog()
        let core = try core(onError: { log.append($0) })
        core.report(UndraTransportError.closed, operation: "a")
        core.report(replyError(status: .panic, body: panicBody("p", "b")), operation: "b")
        core.report(replyError(status: .cancelled, body: []), operation: "c")
        core.report(WireError.badMagic, operation: "d")
        XCTAssertEqual(log.all.map { $0.operation }, ["a", "b", "c", "d"])
        XCTAssertEqual(log.all[0].error, .unavailable(.closed))
        XCTAssertEqual(log.all[1].error, .panicked(message: "p", backtrace: "b"))
        XCTAssertEqual(log.all[2].error, .cancelledByCore)
        guard case .malformed = log.all[3].error else {
            return XCTFail("expected .malformed")
        }
    }

    func testAnErrorThatIsNotOneOfTheRuntimesIsReportedAsMalformedNotTrapped() throws {
        struct Foreign: Error {}
        let log = ErrorLog()
        let core = try core(onError: { log.append($0) })
        core.report(Foreign(), operation: "x")
        core.report(CancellationError(), operation: "y")
        XCTAssertEqual(log.all.count, 2)
        for item in log.all {
            guard case .malformed = item.error else {
                return XCTFail("expected .malformed, got \(item.error)")
            }
        }
    }

    func testNoHandlerIsFine() throws {
        let core = try core(onError: nil)
        core.report(UndraTransportError.closed, operation: "Todos.toggle")
    }

    func testAHandlerThatReportsAgainDoesNotRecurse() throws {
        let count = Guarded<Int>(0)
        let holder = Guarded<UndraCore?>(nil)
        let core = try core(onError: { _ in
            count.withLock { (value: inout Int) -> Void in
                value += 1
            }
            let inner = holder.withLock { (value: inout UndraCore?) -> UndraCore? in
                return value
            }
            inner?.report(UndraTransportError.closed, operation: "from the handler")
        })
        holder.withLock { (value: inout UndraCore?) -> Void in
            value = core
        }
        core.report(UndraTransportError.closed, operation: "outer")
        XCTAssertEqual(count.withLock { (value: inout Int) -> Int in return value }, 1)
        // The flag is cleared again afterwards.
        core.report(UndraTransportError.closed, operation: "outer again")
        XCTAssertEqual(count.withLock { (value: inout Int) -> Int in return value }, 2)
    }

    func testTheHandlerRunsOnTheCallingThread() throws {
        let seen = Guarded<[Bool]>([])
        let core = try core(onError: { _ in
            seen.withLock { (value: inout [Bool]) -> Void in
                value.append(Thread.isMainThread)
            }
        })
        core.report(UndraTransportError.closed, operation: "here")
        let done = expectation(description: "background report")
        Thread.detachNewThread {
            core.report(UndraTransportError.closed, operation: "there")
            done.fulfill()
        }
        wait(for: [done], timeout: 5)
        XCTAssertEqual(seen.withLock { (value: inout [Bool]) -> [Bool] in return value }, [Thread.isMainThread, false])
    }

    func testReportsFromManyThreadsAllArrive() throws {
        let log = ErrorLog()
        let core = try core(onError: { log.append($0) })
        let group = DispatchGroup()
        for index in 0 ..< 8 {
            group.enter()
            DispatchQueue.global().async {
                for _ in 0 ..< 25 {
                    core.report(UndraTransportError.closed, operation: "thread \(index)")
                }
                group.leave()
            }
        }
        XCTAssertEqual(group.wait(timeout: .now() + 10), .success)
        XCTAssertEqual(log.all.count, 200)
    }

    func testLoadOptionsCarryTheHandlerThroughEveryInitializer() {
        let handler: @Sendable (UndraUnhandledError) -> Void = { _ in }
        XCTAssertNil(LoadOptions.inproc(expectedSchemaHash: 1).onError)
        XCTAssertNil(LoadOptions.remote(url: "ws://127.0.0.1:1", expectedSchemaHash: 1).onError)
        XCTAssertNotNil(LoadOptions.inproc(expectedSchemaHash: 1, onError: handler).onError)
        XCTAssertNotNil(LoadOptions.remote(url: "ws://127.0.0.1:1", expectedSchemaHash: 1, onError: handler).onError)
        XCTAssertNotNil(LoadOptions(mode: .inproc, expectedSchemaHash: 1, onError: handler).onError)
    }
}
