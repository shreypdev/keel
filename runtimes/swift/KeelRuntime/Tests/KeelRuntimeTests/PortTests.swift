import Foundation
import XCTest
@testable import KeelRuntime

/// The host side of ports: `registerPort`, sync and async tables, typed errors, and log routing
/// (docs/SPEC.md sections 5.7, 6.3 and 8).
@MainActor
final class PortTests: XCTestCase {
    private let portId: UInt32 = 0x51
    private let echo: UInt32 = 1
    private let typedFailure: UInt32 = 2
    private let otherFailure: UInt32 = 3

    private func syncPort() -> PortImpl {
        return .sync([
            1: { args in
                return args + [0xEE]
            },
            2: { _ in
                throw KeelPortError(body: [7, 7])
            },
            3: { _ in
                throw PortAdapterError.failed("no")
            },
        ])
    }

    private func asyncPort() -> PortImpl {
        return .async([
            1: { args in
                return args + [0xAA]
            },
            2: { _ in
                throw KeelPortError(body: [8, 8])
            },
            3: { _ in
                throw PortAdapterError.failed("no")
            },
            4: { _ in
                try await Task.sleep(nanoseconds: 60_000_000)
                return [1]
            },
        ])
    }

    private func decodeSync(_ outcome: PortCallOutcome, file: StaticString = #filePath, line: UInt = #line) -> Wire.PortReply? {
        guard case .sync(let bytes) = outcome else {
            XCTFail("expected a synchronous outcome, got \(outcome)", file: file, line: line)
            return nil
        }
        do {
            return try Wire.PortReply.decode(bytes)
        } catch {
            XCTFail("undecodable PortReply: \(error)", file: file, line: line)
            return nil
        }
    }

    // MARK: Registration

    func testRegisteringAPortTellsTheTransport() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        core.registerPort(portId, syncPort())
        XCTAssertEqual(transport.sent, [.registerPort(portId)])
        XCTAssertEqual(core.stats().hostRegisteredPorts, 1)
    }

    func testAnUnregisteredPortIsUnavailable() throws {
        let transport = FakeTransport()
        _ = try makeCore(transport)
        XCTAssertEqual(transport.callPort(portId: 0x99, methodId: 1, portCallId: 1, args: []), .unavailable)
    }

    func testRegisteringAgainReplacesThePort() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        core.registerPort(portId, syncPort())
        core.registerPort(portId, .sync([1: { _ in return [0x42] }]))
        let reply = decodeSync(transport.callPort(portId: portId, methodId: echo, portCallId: 1, args: [1]))
        XCTAssertEqual(reply.map { Array($0.body) }, [0x42])
    }

    // MARK: Sync ports

    func testASyncPortAnswersInlineWithTheEncodedResult() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        core.registerPort(portId, syncPort())
        let outcome = transport.callPort(portId: portId, methodId: echo, portCallId: 5, args: [1, 2])
        let reply = try XCTUnwrap(decodeSync(outcome))
        XCTAssertEqual(reply.portCallId, 5)
        XCTAssertEqual(reply.status, .ok)
        XCTAssertEqual(Array(reply.body), [1, 2, 0xEE])
    }

    func testASyncPortThatThrowsAKeelPortErrorAnswersWithStatusError() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        core.registerPort(portId, syncPort())
        let outcome = transport.callPort(portId: portId, methodId: typedFailure, portCallId: 6, args: [])
        let reply = try XCTUnwrap(decodeSync(outcome))
        XCTAssertEqual(reply.portCallId, 6)
        XCTAssertEqual(reply.status, .error)
        XCTAssertEqual(Array(reply.body), [7, 7])
    }

    func testASyncPortThatThrowsAnythingElseAnswersUnavailable() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        core.registerPort(portId, syncPort())
        let outcome = transport.callPort(portId: portId, methodId: otherFailure, portCallId: 7, args: [])
        let reply = try XCTUnwrap(decodeSync(outcome))
        XCTAssertEqual(reply.portCallId, 7)
        XCTAssertEqual(reply.status, .unavailable)
        XCTAssertEqual(Array(reply.body), [])
    }

    func testAnUnknownMethodIsUnavailable() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        core.registerPort(portId, syncPort())
        core.registerPort(portId + 1, asyncPort())
        XCTAssertEqual(transport.callPort(portId: portId, methodId: 0x77, portCallId: 1, args: []), .unavailable)
        XCTAssertEqual(transport.callPort(portId: portId + 1, methodId: 0x77, portCallId: 1, args: []), .unavailable)
    }

    // MARK: Async ports

    func testAnAsyncPortAnswersLaterThroughPortReply() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        core.registerPort(portId, asyncPort())
        let outcome = transport.callPort(portId: portId, methodId: echo, portCallId: 11, args: [3])
        XCTAssertEqual(outcome, .async)
        let answered = await waitUntil { transport.portReplies.count == 1 }
        XCTAssertTrue(answered)
        let reply = transport.portReplies[0]
        XCTAssertEqual(reply.portCallId, 11)
        XCTAssertEqual(reply.status, .ok)
        XCTAssertEqual(Array(reply.body), [3, 0xAA])
    }

    func testAnAsyncPortThatThrowsATypedErrorAnswersWithStatusError() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        core.registerPort(portId, asyncPort())
        XCTAssertEqual(transport.callPort(portId: portId, methodId: typedFailure, portCallId: 12, args: []), .async)
        let answered = await waitUntil { transport.portReplies.count == 1 }
        XCTAssertTrue(answered)
        XCTAssertEqual(transport.portReplies[0], Wire.PortReply(portCallId: 12, status: .error, body: [8, 8]))
    }

    func testAnAsyncPortThatThrowsAnythingElseAnswersUnavailable() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        core.registerPort(portId, asyncPort())
        XCTAssertEqual(transport.callPort(portId: portId, methodId: otherFailure, portCallId: 13, args: []), .async)
        let answered = await waitUntil { transport.portReplies.count == 1 }
        XCTAssertTrue(answered)
        XCTAssertEqual(transport.portReplies[0], Wire.PortReply(portCallId: 13, status: .unavailable))
    }

    func testAnAsyncPortDoesNotBlockTheCallerAndSeveralCallsOverlap() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        core.registerPort(portId, asyncPort())
        let started = Date()
        for id in 20 ..< 26 {
            XCTAssertEqual(transport.callPort(portId: portId, methodId: 4, portCallId: UInt32(id), args: []), .async)
        }
        XCTAssertLessThan(Date().timeIntervalSince(started), 0.05, "the callback returned without waiting")
        let answered = await waitUntil { transport.portReplies.count == 6 }
        XCTAssertTrue(answered)
        let ids = Set(transport.portReplies.map { $0.portCallId })
        XCTAssertEqual(ids, Set((20 ..< 26).map { UInt32($0) }))
    }

    func testNoAsyncReplyIsSentAfterShutdown() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        core.registerPort(portId, asyncPort())
        XCTAssertEqual(transport.callPort(portId: portId, methodId: 4, portCallId: 30, args: []), .async)
        core.shutdown()
        try await Task.sleep(nanoseconds: 150_000_000)
        XCTAssertEqual(transport.portReplies.count, 0)
    }

    // MARK: Logs

    func testCoreLogsAreRoutedToTheLogPortWhenOneIsRegistered() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let captured = Guarded<[[UInt8]]>([])
        core.registerPort(StandardPorts.Log.portId, .sync([
            StandardPorts.Log.log: { args in
                captured.withLock { (list: inout [[UInt8]]) -> Void in
                    list.append(args)
                }
                return []
            },
        ]))
        transport.deliverLog(level: 3, target: "keel::net", message: "slow \u{1F30A}")
        let arguments = captured.withLock { (list: inout [[UInt8]]) -> [[UInt8]] in
            return list
        }
        XCTAssertEqual(arguments.count, 1)
        var reader = KeelReader(arguments[0])
        XCTAssertEqual(try reader.readU8(), 3)
        XCTAssertEqual(try reader.readString(), "keel::net")
        XCTAssertEqual(try reader.readString(), "slow \u{1F30A}")
        XCTAssertNoThrow(try reader.finish())
    }

    func testCoreLogsWithoutALogPortAreForwardedWithoutFailing() throws {
        let transport = FakeTransport()
        _ = try makeCore(transport)
        for level in 0 ... 6 {
            transport.deliverLog(level: UInt8(level), target: "t", message: "m")
        }
    }

    // MARK: Errors

    func testAPortErrorCarriesItsBody() {
        XCTAssertEqual(KeelPortError(body: [1]).body, [1])
        XCTAssertEqual(KeelPortError(body: [1]), KeelPortError(body: [1]))
        XCTAssertNotEqual(KeelPortError(body: [1]), KeelPortError(body: [2]))
    }
}
