import Foundation
import XCTest
@testable import KeelRuntime

/// Records everything a transport reports to the core.
final class RecordingInbound: KeelInbound, @unchecked Sendable {
    enum Event: Equatable {
        case reply(UInt32, [UInt8])
        case changeSet([UInt8])
        case streamItem(UInt32, [UInt8])
        case portCall(UInt32, UInt32, UInt32, [UInt8])
        case log(UInt8, String, String)
        case disconnect(String)
    }

    private let events = Guarded<[Event]>([])
    var portOutcome: PortCallOutcome = .unavailable

    var recorded: [Event] {
        return events.withLock { (list: inout [Event]) -> [Event] in
            return list
        }
    }

    private func record(_ event: Event) {
        events.withLock { (list: inout [Event]) -> Void in
            list.append(event)
        }
    }

    func onReply(callId: UInt32, payload: [UInt8]) {
        record(.reply(callId, payload))
    }

    func onChangeSet(_ payload: [UInt8]) {
        record(.changeSet(payload))
    }

    func onStreamItem(callId: UInt32, payload: [UInt8]) {
        record(.streamItem(callId, payload))
    }

    func onPortCall(portId: UInt32, methodId: UInt32, portCallId: UInt32, args: [UInt8]) -> PortCallOutcome {
        record(.portCall(portId, methodId, portCallId, args))
        return portOutcome
    }

    func onLog(level: UInt8, target: String, message: String) {
        record(.log(level, target, message))
    }

    func onDisconnect(_ error: any Error) {
        record(.disconnect(String(describing: error)))
    }
}

/// Collects the frames a transport writes.
final class FrameSink: @unchecked Sendable {
    private let frames = Guarded<[[UInt8]]>([])

    func append(_ frame: [UInt8]) {
        frames.withLock { (list: inout [[UInt8]]) -> Void in
            list.append(frame)
        }
    }

    var all: [[UInt8]] {
        return frames.withLock { (list: inout [[UInt8]]) -> [[UInt8]] in
            return list
        }
    }

    /// The decoded envelopes written so far.
    var envelopes: [Envelope.Decoded] {
        var result: [Envelope.Decoded] = []
        for frame in all {
            if let decoded = try? decodeEnvelope(frame) {
                result.append(decoded)
            }
        }
        return result
    }
}

private func makeFrame(_ kind: Envelope.Kind, seq: UInt32 = 1, schema: UInt64, _ payload: [UInt8]) -> [UInt8] {
    return encodeEnvelope(kind: kind, seq: seq, schemaHash: schema, payload: payload)
}

/// The remote transport's protocol logic, driven through a frame sink instead of a socket
/// (docs/SPEC.md sections 3.2 and 11).
@MainActor
final class RemoteTransportTests: XCTestCase {
    private let expected: UInt64 = 0x77
    private let coreHash: UInt64 = 0x77

    private func coreHello(schema: UInt64) -> [UInt8] {
        let hello = Wire.Hello(keelVersion: "1.0.0", schemaHash: schema, platform: "rust", mode: "dev")
        return makeFrame(.hello, schema: schema, hello.encode())
    }

    private func startOptions(timeout: Double = 5) -> TransportStartOptions {
        return TransportStartOptions(platform: "ios", logLevel: 2, connectTimeout: timeout, expectedSchemaHash: expected)
    }

    /// Starts `transport` on a background thread (`start` blocks), plays the core's `Hello`, and
    /// returns the transport's result.
    private func handshake(
        _ transport: WebSocketTransport,
        sink: FrameSink,
        inbound: RecordingInbound,
        answer: [UInt8]?,
        waitForResult: Bool = true
    ) async -> Result<TransportInfo, any Error>? {
        let options = startOptions()
        let result = Guarded<Result<TransportInfo, any Error>?>(nil)
        DispatchQueue.global().async {
            do {
                let info = try transport.start(inbound: inbound, options: options)
                result.withLock { (value: inout Result<TransportInfo, any Error>?) -> Void in
                    value = .success(info)
                }
            } catch {
                result.withLock { (value: inout Result<TransportInfo, any Error>?) -> Void in
                    value = .failure(error)
                }
            }
        }
        let sentHello = await waitUntil { sink.all.count >= 1 }
        XCTAssertTrue(sentHello, "the transport must open with a Hello")
        if let answer = answer {
            transport.handleFrame(answer)
        }
        if !waitForResult {
            return nil
        }
        _ = await waitUntil {
            return result.withLock { (value: inout Result<TransportInfo, any Error>?) -> Bool in
                return value != nil
            }
        }
        return result.withLock { (value: inout Result<TransportInfo, any Error>?) -> Result<TransportInfo, any Error>? in
            return value
        }
    }

    private func startedTransport() async throws -> (WebSocketTransport, FrameSink, RecordingInbound) {
        let sink = FrameSink()
        let transport = WebSocketTransport(frameSink: { (frame: [UInt8]) -> Void in
            sink.append(frame)
        })
        let inbound = RecordingInbound()
        let result = await handshake(transport, sink: sink, inbound: inbound, answer: coreHello(schema: coreHash))
        _ = try XCTUnwrap(result).get()
        return (transport, sink, inbound)
    }

    // MARK: Handshake

    func testTheHandshakeSendsHelloAndReturnsTheCoresHash() async throws {
        let sink = FrameSink()
        let transport = WebSocketTransport(frameSink: { (frame: [UInt8]) -> Void in
            sink.append(frame)
        })
        let result = await handshake(transport, sink: sink, inbound: RecordingInbound(), answer: coreHello(schema: 0x99))
        let info = try XCTUnwrap(result).get()
        XCTAssertEqual(info, TransportInfo(schemaHash: 0x99))

        let first = try XCTUnwrap(sink.envelopes.first)
        XCTAssertEqual(first.kind, .hello)
        XCTAssertEqual(first.seq, 1)
        XCTAssertEqual(first.schemaHash, expected)
        let hello = try Wire.Hello.decode(slice: first.payload)
        XCTAssertEqual(hello, Wire.Hello(keelVersion: "1.0.0", schemaHash: expected, platform: "ios", mode: "dev"))
    }

    func testAHelloTimeoutFailsTheStart() throws {
        let sink = FrameSink()
        let transport = WebSocketTransport(frameSink: { (frame: [UInt8]) -> Void in
            sink.append(frame)
        })
        XCTAssertThrowsError(try transport.start(inbound: RecordingInbound(), options: startOptions(timeout: 0.05))) { error in
            XCTAssertEqual(error as? KeelLoadError, KeelLoadError.handshakeTimedOut(seconds: 0.05))
        }
        XCTAssertEqual(sink.all.count, 1)
        XCTAssertFalse(transport.send(call: [1]), "a transport that failed to start is shut down")
    }

    func testAMalformedHelloIsRejected() async throws {
        let sink = FrameSink()
        let transport = WebSocketTransport(frameSink: { (frame: [UInt8]) -> Void in
            sink.append(frame)
        })
        let result = await handshake(
            transport,
            sink: sink,
            inbound: RecordingInbound(),
            answer: makeFrame(.hello, schema: coreHash, [1, 2])
        )
        guard case .failure(let error)? = result else {
            return XCTFail("a malformed Hello must fail the start, got \(String(describing: result))")
        }
        guard case KeelLoadError.handshakeRejected? = error as? KeelLoadError else {
            return XCTFail("expected handshakeRejected, got \(error)")
        }
    }

    func testFramesBeforeTheHelloAreDropped() async throws {
        let sink = FrameSink()
        let transport = WebSocketTransport(frameSink: { (frame: [UInt8]) -> Void in
            sink.append(frame)
        })
        let inbound = RecordingInbound()
        let early = makeFrame(.reply, schema: coreHash, Wire.Reply(callId: 1, status: .ok).encode())
        let result = await handshake(transport, sink: sink, inbound: inbound, answer: nil, waitForResult: false)
        XCTAssertNil(result, "no Hello yet, so start is still waiting")
        transport.handleFrame(early)
        XCTAssertEqual(inbound.recorded, [])
        transport.handleFrame(coreHello(schema: coreHash))
    }

    // MARK: Incoming

    func testIncomingFramesAreRoutedToTheInbound() async throws {
        let (transport, _, inbound) = try await startedTransport()
        let reply = Wire.Reply(callId: 7, status: .ok, body: [1]).encode()
        transport.handleFrame(makeFrame(.reply, seq: 2, schema: coreHash, reply))
        let changeSet = makeChangeSet([(KeelHandle(rawValue: 5), 0, [1, 2])]).encode()
        transport.handleFrame(makeFrame(.changeSet, seq: 3, schema: coreHash, changeSet))
        let item = Wire.StreamItem(callId: 9, flag: .item, body: [4]).encode()
        transport.handleFrame(makeFrame(.streamItem, seq: 4, schema: coreHash, item))
        transport.handleFrame(makeFrame(.log, seq: 5, schema: coreHash, Wire.Log(level: 3, target: "t", message: "m").encode()))
        transport.handleFrame(makeFrame(.snapshot, seq: 6, schema: coreHash, Wire.Snapshot(stores: []).encode()))
        XCTAssertEqual(inbound.recorded, [
            .reply(7, reply),
            .changeSet(changeSet),
            .streamItem(9, item),
            .log(3, "t", "m"),
        ])
    }

    func testMalformedAndMisdirectedFramesAreDroppedWithoutDisconnecting() async throws {
        let (transport, _, inbound) = try await startedTransport()
        transport.handleFrame([1, 2, 3])
        transport.handleFrame([UInt8](repeating: 0, count: 40))
        transport.handleFrame(makeFrame(.call, schema: coreHash, [1]))
        transport.handleFrame(makeFrame(.reply, schema: coreHash, [1]))
        transport.handleFrame(makeFrame(.streamItem, schema: coreHash, [1, 2]))
        transport.handleFrame(makeFrame(.log, schema: coreHash, [9]))
        transport.handleFrame(makeFrame(.portCall, schema: coreHash, [9]))
        XCTAssertEqual(inbound.recorded, [])
        let good = Wire.Reply(callId: 1, status: .ok).encode()
        transport.handleFrame(makeFrame(.reply, schema: coreHash, good))
        XCTAssertEqual(inbound.recorded, [.reply(1, good)])
    }

    func testAFrameWithAnotherSchemaHashDisconnects() async throws {
        let (transport, _, inbound) = try await startedTransport()
        transport.handleFrame(makeFrame(.reply, schema: 0xBAD, Wire.Reply(callId: 1, status: .ok).encode()))
        let events = inbound.recorded
        XCTAssertEqual(events.count, 1)
        guard case .disconnect(let text)? = events.first else {
            return XCTFail("expected a disconnect, got \(events)")
        }
        XCTAssertTrue(text.contains("0x0000000000000bad"), text)
        XCTAssertFalse(transport.send(call: [1]))
    }

    // MARK: Port calls

    func testPortCallsAreAnsweredWithPortReplyFrames() async throws {
        let (transport, sink, inbound) = try await startedTransport()
        let call = Wire.PortCall(portId: 3, methodId: 4, portCallId: 11, args: [5, 6]).encode()

        let inline = Wire.PortReply(portCallId: 11, status: .ok, body: [8]).encode()
        inbound.portOutcome = .sync(reply: inline)
        transport.handleFrame(makeFrame(.portCall, seq: 2, schema: coreHash, call))
        inbound.portOutcome = .unavailable
        transport.handleFrame(makeFrame(.portCall, seq: 3, schema: coreHash, call))
        inbound.portOutcome = .async
        transport.handleFrame(makeFrame(.portCall, seq: 4, schema: coreHash, call))

        let replies = sink.envelopes.filter { $0.kind == .portReply }
        XCTAssertEqual(replies.count, 2, "an async port answers later, not now")
        XCTAssertEqual(Array(replies[0].payload), inline)
        XCTAssertEqual(Array(replies[1].payload), Wire.PortReply(portCallId: 11, status: .unavailable).encode())
        XCTAssertEqual(inbound.recorded.count, 3)
        XCTAssertEqual(inbound.recorded[0], .portCall(3, 4, 11, [5, 6]))
    }

    // MARK: Outgoing

    func testOutgoingMessagesAreEnvelopedWithIncreasingSequenceNumbersAndTheCoresHash() async throws {
        let (transport, sink, _) = try await startedTransport()
        XCTAssertTrue(transport.send(call: [1, 2, 3]))
        transport.cancel(callId: 4)
        transport.streamCredit(callId: 5, credit: 16)
        transport.observe(handle: KeelHandle(rawValue: 6), signal: UInt32.max, on: true)
        transport.release(handle: KeelHandle(rawValue: 7))
        transport.event(portId: 8, methodId: 9, payload: [1])
        transport.timerFired(10)
        transport.portReply([0xAA])
        transport.registerPort(11)

        let sent = Array(sink.envelopes.dropFirst())
        XCTAssertEqual(sent.map { $0.kind }, [.call, .cancel, .streamCredit, .observe, .release, .event, .timerFired, .portReply])
        XCTAssertEqual(sent.map { $0.seq }, [2, 3, 4, 5, 6, 7, 8, 9])
        XCTAssertTrue(sent.allSatisfy { $0.schemaHash == coreHash })
        XCTAssertEqual(Array(sent[0].payload), [1, 2, 3])
        XCTAssertEqual(try Wire.Cancel.decode(slice: sent[1].payload), Wire.Cancel(callId: 4))
        XCTAssertEqual(try Wire.StreamCredit.decode(slice: sent[2].payload), Wire.StreamCredit(callId: 5, credit: 16))
        XCTAssertEqual(try Wire.Observe.decode(slice: sent[3].payload), Wire.Observe(handle: KeelHandle(rawValue: 6), signalId: UInt32.max, on: true))
        XCTAssertEqual(try Wire.Release.decode(slice: sent[4].payload), Wire.Release(handle: KeelHandle(rawValue: 7)))
        XCTAssertEqual(try Wire.Event.decode(slice: sent[5].payload), Wire.Event(portId: 8, methodId: 9, payload: [1]))
        XCTAssertEqual(try Wire.TimerFired.decode(slice: sent[6].payload), Wire.TimerFired(timerId: 10))
        XCTAssertEqual(Array(sent[7].payload), [0xAA])
    }

    func testAShutDownTransportSendsNothing() async throws {
        let (transport, sink, _) = try await startedTransport()
        transport.shutdown()
        let before = sink.all.count
        XCTAssertFalse(transport.send(call: [1]))
        transport.cancel(callId: 1)
        transport.timerFired(1)
        XCTAssertEqual(sink.all.count, before)
        transport.shutdown()
    }

    // MARK: Capabilities

    func testTheRemoteTransportHasNoInlineCallsSnapshotsOrStats() throws {
        let transport = WebSocketTransport(frameSink: { (_: [UInt8]) -> Void in })
        XCTAssertEqual(transport.mode, .remote)
        XCTAssertFalse(transport.supportsDirectSync)
        XCTAssertNil(transport.statsJSON())
        XCTAssertThrowsError(try transport.callSync([1])) { error in
            XCTAssertEqual((error as? KeelModeError)?.mode, .remote)
        }
        XCTAssertThrowsError(try transport.snapshot()) { error in
            XCTAssertEqual(error as? KeelModeError, KeelModeError(operation: "snapshot", mode: .remote))
        }
        XCTAssertThrowsError(try transport.restore([1])) { error in
            XCTAssertEqual(error as? KeelModeError, KeelModeError(operation: "restore", mode: .remote))
        }
        XCTAssertTrue(KeelModeError(operation: "snapshot", mode: .remote).description.contains("remote"))
    }

    func testOnlyWebSocketURLsAreAccepted() {
        for url in ["http://127.0.0.1:1", "ftp://x", "127.0.0.1:7878", "", "ws://"] {
            XCTAssertThrowsError(try WebSocketTransport(urlString: url), url) { error in
                XCTAssertEqual(error as? KeelLoadError, KeelLoadError.invalidURL(url))
            }
        }
        XCTAssertNoThrow(try WebSocketTransport(urlString: "ws://127.0.0.1:7878"))
        XCTAssertNoThrow(try WebSocketTransport(urlString: "WSS://example.com/keel"))
    }

    // MARK: A whole core over the remote transport

    func testACoreOverTheRemoteTransportAnswersBlockingCallsAndAsyncCalls() async throws {
        let sink = FrameSink()
        let transport = WebSocketTransport(frameSink: { (frame: [UInt8]) -> Void in
            sink.append(frame)
        })
        let box = Guarded<KeelCore?>(nil)
        let expectedHash = expected
        DispatchQueue.global().async {
            let options = LoadOptions.remote(url: "ws://unused", adapters: Adapters.none, expectedSchemaHash: expectedHash)
            let core = try? KeelCore.connect(transport: transport, options: options)
            box.withLock { (value: inout KeelCore?) -> Void in
                value = core
            }
        }
        let helloSent = await waitUntil { sink.all.count >= 1 }
        XCTAssertTrue(helloSent)
        transport.handleFrame(coreHello(schema: coreHash))
        let connected = await waitUntil {
            return box.withLock { (value: inout KeelCore?) -> Bool in
                return value != nil
            }
        }
        XCTAssertTrue(connected)
        let core = try XCTUnwrap(box.withLock { (value: inout KeelCore?) -> KeelCore? in
            return value
        })
        XCTAssertEqual(core.mode, .remote)

        // A blocking call: the reply arrives from "the network" while the caller waits.
        let body = Guarded<[UInt8]?>(nil)
        DispatchQueue.global().async {
            let reply = try? core.callSync(.freeFunction(methodId: 5), method: 5, args: [1])
            body.withLock { (value: inout [UInt8]?) -> Void in
                value = reply
            }
        }
        func lastCallId() -> UInt32? {
            for envelope in sink.envelopes.reversed() where envelope.kind == .call {
                return (try? Wire.Call.decode(slice: envelope.payload))?.callId
            }
            return nil
        }
        let sentCall = await waitUntil { lastCallId() != nil }
        XCTAssertTrue(sentCall)
        let firstId = try XCTUnwrap(lastCallId())
        transport.handleFrame(makeFrame(.reply, seq: 2, schema: coreHash, Wire.Reply(callId: firstId, status: .ok, body: [42]).encode()))
        let answered = await waitUntil {
            return body.withLock { (value: inout [UInt8]?) -> Bool in
                return value != nil
            }
        }
        XCTAssertTrue(answered)
        XCTAssertEqual(body.withLock { (value: inout [UInt8]?) -> [UInt8]? in
            return value
        }, [42])

        // An async call over the same connection.
        let task = Task {
            return try await core.call(.freeFunction(methodId: 6), method: 6, args: [2])
        }
        let sentSecond = await waitUntil { (lastCallId() ?? firstId) != firstId }
        XCTAssertTrue(sentSecond)
        let secondId = try XCTUnwrap(lastCallId())
        transport.handleFrame(makeFrame(.reply, seq: 3, schema: coreHash, Wire.Reply(callId: secondId, status: .ok, body: [43]).encode()))
        let result = try await task.value
        XCTAssertEqual(result, [43])

        // A disconnect fails what is in flight.
        core.shutdown()
        XCTAssertTrue(core.isShutDown)
    }
}
