import Foundation
import XCTest
@testable import UndraRuntime

/// `UndraCore` against the scripted fake core: loading, schema check, sync and async calls,
/// cancellation, shutdown, constructors and statistics.
@MainActor
final class CoreCallTests: XCTestCase {
    // MARK: Loading

    func testSchemaMismatchThrowsAndShutsTheTransportDown() throws {
        let transport = FakeTransport(schemaHash: 1)
        XCTAssertThrowsError(try makeCore(transport, expectedSchemaHash: 2)) { error in
            XCTAssertEqual(error as? UndraSchemaMismatchError, UndraSchemaMismatchError(expected: 2, got: 1))
        }
        XCTAssertTrue(transport.wasShutDown)
    }

    func testSchemaMismatchMessageNamesBothHashesAndTheFix() {
        let text = UndraSchemaMismatchError(expected: 0x691e_ee07_33e4_a44f, got: 0xab).description
        XCTAssertTrue(text.contains("0x691eee0733e4a44f"))
        XCTAssertTrue(text.contains("0x00000000000000ab"))
        XCTAssertTrue(text.contains("undra bindgen"))
    }

    func testConnectReportsTheCoresSchemaHashAndMode() throws {
        let inproc = try makeCore(FakeTransport(schemaHash: 0x1234))
        XCTAssertEqual(inproc.schemaHash, 0x1234)
        XCTAssertEqual(inproc.mode, .inproc)
        let remote = try makeCore(FakeTransport(schemaHash: 0x1234, directSync: false))
        XCTAssertEqual(remote.mode, .remote)
    }

    func testAdaptersAreRegisteredAndAttached() throws {
        let transport = FakeTransport()
        let recorder = AttachRecorder(portId: 0xAB)
        let adapters = Adapters([recorder])
        let core = try makeCore(transport, adapters: adapters)
        XCTAssertEqual(transport.sent, [.registerPort(0xAB)])
        XCTAssertEqual(recorder.events(), ["attach"])
        core.shutdown()
        XCTAssertEqual(recorder.events(), ["attach", "detach"])
    }

    // MARK: callSync

    func testCallSyncReturnsTheReplyBody() throws {
        let transport = FakeTransport()
        transport.onCallSync = { call in
            return Wire.Reply(callId: call.callId, status: .ok, body: [1, 2, 3]).encode()
        }
        let core = try makeCore(transport)
        let body = try core.callSync(.freeFunction(methodId: 7), method: 7, args: [9])
        XCTAssertEqual(body, [1, 2, 3])
        let call = try XCTUnwrap(transport.calls.first)
        XCTAssertEqual(call.target, .freeFunction(methodId: 7))
        XCTAssertEqual(Array(call.args), [9])
        XCTAssertNotEqual(call.callId, 0)
        XCTAssertEqual(core.stats().hostPendingCalls, 0)
    }

    func testCallSyncSendsObjectMethodsAndConstructorsAndPagesInTheWireLayout() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let handle = UndraHandle(index: 3, generation: 2)
        _ = try core.callSync(.objectMethod(handle: handle, methodId: 11), method: 11, args: [1])
        _ = try core.callSync(.constructor(typeId: 5, methodId: 6), method: 6, args: [2])
        _ = try core.callSync(.lazyListPage(handle: handle, offset: 10, limit: 20), method: 0, args: [3])
        let calls = transport.calls
        XCTAssertEqual(calls[0].target, .objectMethod(handle: handle, methodId: 11))
        XCTAssertEqual(calls[1].target, .constructor(typeId: 5, methodId: 6))
        XCTAssertEqual(calls[2].target, .lazyListPage(handle: handle, offset: 10, limit: 20))
        XCTAssertEqual(Array(calls[2].args), [], "a page request carries no arguments")
    }

    func testCallSyncThrowsUndraReplyErrorForEveryNonOkStatus() throws {
        let statuses: [ReplyStatus] = [.error, .panic, .cancelled, .streamOpened, .badRequest]
        for status in statuses {
            let transport = FakeTransport()
            transport.onCallSync = { call in
                return Wire.Reply(callId: call.callId, status: status, body: [7, 8]).encode()
            }
            let core = try makeCore(transport)
            XCTAssertThrowsError(try core.callSync(.freeFunction(methodId: 1), method: 1, args: [])) { error in
                XCTAssertEqual(error as? UndraReplyError, UndraReplyError(status: status, body: [7, 8]), "\(status)")
            }
            XCTAssertEqual(core.stats().hostPendingCalls, 0)
        }
    }

    func testPanicAndBadRequestRepliesExposeTheirMessages() {
        var panic = UndraWriter()
        panic.writeString("boom")
        panic.writeString("backtrace")
        XCTAssertEqual(UndraReplyError(status: .panic, body: panic.finish()).message, "boom")
        var reason = UndraWriter()
        reason.writeString("unknown method")
        let bad = UndraReplyError(status: .badRequest, body: reason.finish())
        XCTAssertEqual(bad.message, "unknown method")
        XCTAssertTrue(bad.description.contains("unknown method"))
        XCTAssertNil(UndraReplyError(status: .error, body: [1]).message)
        XCTAssertNil(UndraReplyError(status: .panic, body: [0xFF]).message, "an undecodable body has no message")
    }

    func testCallSyncWithAMalformedReplyThrowsAProtocolError() throws {
        let transport = FakeTransport()
        transport.onCallSync = { _ in return [0xFF] }
        let core = try makeCore(transport)
        XCTAssertThrowsError(try core.callSync(.freeFunction(methodId: 1), method: 1, args: [])) { error in
            guard case UndraProtocolError.malformedMessage(let context, _)? = error as? UndraProtocolError else {
                return XCTFail("expected a protocol error, got \(error)")
            }
            XCTAssertEqual(context, "reply")
        }
    }

    func testCallSyncBlocksOnAnOrdinaryCallWhenThereIsNoInlinePath() throws {
        let transport = FakeTransport(directSync: false)
        transport.onCall = { call, fake in
            DispatchQueue.global().asyncAfter(deadline: .now() + 0.01) {
                fake.replyOk(call.callId, [4, 5])
            }
            return true
        }
        let core = try makeCore(transport)
        let body = try core.callSync(.freeFunction(methodId: 3), method: 3, args: [])
        XCTAssertEqual(body, [4, 5])
        XCTAssertEqual(core.stats().hostPendingCalls, 0)
    }

    func testBlockingCallTimesOutAndCancelsTheCall() throws {
        let transport = FakeTransport(directSync: false)
        let core = try makeCore(transport, blockingCallTimeout: 0.05)
        XCTAssertThrowsError(try core.callSync(.freeFunction(methodId: 3), method: 3, args: [])) { error in
            XCTAssertEqual(error as? UndraTransportError, UndraTransportError.timedOut(operation: "callSync"))
        }
        XCTAssertEqual(transport.cancels, [transport.calls[0].callId])
        XCTAssertEqual(core.stats().hostPendingCalls, 0)
    }

    func testBlockingCallTheRemoteTransportCannotSendThrowsClosed() throws {
        // The blocking path is the remote one, whose transport refuses to send only once its
        // connection is closed (ADR-032: that is `.unavailable(.closed)`, not a refusal by the core).
        let transport = FakeTransport(directSync: false)
        transport.onCall = { _, _ in return false }
        let core = try makeCore(transport)
        XCTAssertThrowsError(try core.callSync(.freeFunction(methodId: 3), method: 3, args: [])) { error in
            XCTAssertEqual(error as? UndraTransportError, .closed)
        }
        XCTAssertEqual(core.stats().hostPendingCalls, 0)
    }

    // MARK: call

    func testCallReturnsTheReplyBody() async throws {
        let transport = FakeTransport()
        transport.onCall = { call, fake in
            fake.replyOk(call.callId, [1])
            return true
        }
        let core = try makeCore(transport)
        let handle = UndraHandle(rawValue: 5)
        let body = try await core.call(.objectMethod(handle: handle, methodId: 9), method: 9, args: [2])
        XCTAssertEqual(body, [1])
        XCTAssertEqual(transport.calls[0].target, .objectMethod(handle: handle, methodId: 9))
        XCTAssertEqual(Array(transport.calls[0].args), [2])
    }

    func testCallResumesWhenTheReplyArrivesLaterOnAnotherThread() async throws {
        let transport = FakeTransport()
        transport.onCall = { call, fake in
            DispatchQueue.global().asyncAfter(deadline: .now() + 0.01) {
                fake.replyOk(call.callId, [6])
            }
            return true
        }
        let core = try makeCore(transport)
        let body = try await core.call(.freeFunction(methodId: 2), method: 2, args: [])
        XCTAssertEqual(body, [6])
    }

    func testEveryCallGetsADistinctNonZeroId() async throws {
        let transport = FakeTransport()
        transport.onCall = { call, fake in
            fake.replyOk(call.callId)
            return true
        }
        let core = try makeCore(transport)
        for _ in 0 ..< 5 {
            _ = try await core.call(.freeFunction(methodId: 2), method: 2, args: [])
        }
        _ = try core.callSync(.freeFunction(methodId: 2), method: 2, args: [])
        let ids = transport.calls.map { $0.callId }
        XCTAssertEqual(Set(ids).count, ids.count)
        XCTAssertFalse(ids.contains(0))
    }

    func testCallThrowsTheReplyErrorWithItsTypedBody() async throws {
        let transport = FakeTransport()
        transport.onCall = { call, fake in
            fake.replyError(call.callId, [9, 9])
            return true
        }
        let core = try makeCore(transport)
        let error = await captureError {
            _ = try await core.call(.freeFunction(methodId: 2), method: 2, args: [])
        }
        XCTAssertEqual(error as? UndraReplyError, UndraReplyError(status: .error, body: [9, 9]))
    }

    func testCallRejectedByTheCoreThrowsBadRequest() async throws {
        let transport = FakeTransport()
        transport.onCall = { _, _ in return false }
        let core = try makeCore(transport)
        let error = await captureError {
            _ = try await core.call(.freeFunction(methodId: 2), method: 2, args: [])
        }
        let reply = try XCTUnwrap(error as? UndraReplyError)
        XCTAssertEqual(reply.status, .badRequest)
        XCTAssertNotNil(reply.message)
        XCTAssertEqual(core.stats().hostPendingCalls, 0)
    }

    func testCallWithAMalformedReplyThrowsAProtocolError() async throws {
        let transport = FakeTransport()
        transport.onCall = { call, fake in
            fake.deliverRawReply(call.callId, [0xFF])
            return true
        }
        let core = try makeCore(transport)
        let error = await captureError {
            _ = try await core.call(.freeFunction(methodId: 2), method: 2, args: [])
        }
        XCTAssertTrue(error is UndraProtocolError, "\(String(describing: error))")
        XCTAssertEqual(core.stats().hostPendingCalls, 0)
    }

    func testRepliesForUnknownCallsAreIgnored() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        transport.replyOk(999, [1])
        transport.deliverRawReply(998, [0xFF])
        XCTAssertEqual(core.stats().hostPendingCalls, 0)
    }

    // MARK: Cancellation

    func testCancellingTheTaskCancelsTheCallInTheCore() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let task = Task {
            return try await core.call(.freeFunction(methodId: 4), method: 4, args: [])
        }
        let sent = await waitUntil { !transport.calls.isEmpty }
        XCTAssertTrue(sent)
        XCTAssertEqual(core.stats().hostPendingCalls, 1)
        task.cancel()
        let result = await task.result
        switch result {
        case .success:
            XCTFail("a cancelled call must not succeed")
        case .failure(let error):
            XCTAssertTrue(error is CancellationError, "\(error)")
        }
        let callId = transport.calls[0].callId
        XCTAssertEqual(transport.cancels, [callId])
        XCTAssertEqual(core.stats().hostPendingCalls, 0)
        // The core's status-3 reply, or any late reply, finds nobody waiting and is dropped.
        transport.deliver(Wire.Reply(callId: callId, status: .cancelled))
        transport.replyOk(callId, [1])
    }

    func testAnAlreadyCancelledTaskNeverSendsTheCall() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let task = Task { () -> [UInt8] in
            withUnsafeCurrentTask { (current: UnsafeCurrentTask?) -> Void in
                current?.cancel()
            }
            return try await core.call(.freeFunction(methodId: 4), method: 4, args: [])
        }
        let result = await task.result
        switch result {
        case .success:
            XCTFail("a cancelled call must not succeed")
        case .failure(let error):
            XCTAssertTrue(error is CancellationError, "\(error)")
        }
        XCTAssertEqual(transport.calls.count, 0)
        XCTAssertEqual(transport.cancels, [])
        XCTAssertEqual(core.stats().hostPendingCalls, 0)
    }

    func testAReplyThatWinsTheRaceIsNotCancelled() async throws {
        let transport = FakeTransport()
        transport.onCall = { call, fake in
            fake.replyOk(call.callId, [3])
            return true
        }
        let core = try makeCore(transport)
        let body = try await core.call(.freeFunction(methodId: 4), method: 4, args: [])
        XCTAssertEqual(body, [3])
        XCTAssertEqual(transport.cancels, [])
    }

    // MARK: Shutdown

    func testShutdownFailsCallsInFlightAndRefusesNewOnes() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let task = Task {
            return try await core.call(.freeFunction(methodId: 4), method: 4, args: [])
        }
        let sent = await waitUntil { !transport.calls.isEmpty }
        XCTAssertTrue(sent)
        core.shutdown()
        let result = await task.result
        switch result {
        case .success:
            XCTFail("a call in flight at shutdown must fail")
        case .failure(let error):
            XCTAssertEqual(error as? UndraTransportError, UndraTransportError.closed)
        }
        XCTAssertTrue(transport.wasShutDown)
        XCTAssertTrue(core.isShutDown)
        let later = await captureError {
            _ = try await core.call(.freeFunction(methodId: 4), method: 4, args: [])
        }
        XCTAssertEqual(later as? UndraTransportError, UndraTransportError.closed)
        XCTAssertThrowsError(try core.callSync(.freeFunction(methodId: 4), method: 4, args: []))
        core.shutdown()
    }

    func testDisconnectFailsEverythingInFlight() async throws {
        let transport = FakeTransport(directSync: false)
        let core = try makeCore(transport)
        let task = Task {
            return try await core.call(.freeFunction(methodId: 4), method: 4, args: [])
        }
        let sent = await waitUntil { !transport.calls.isEmpty }
        XCTAssertTrue(sent)
        transport.disconnect(UndraTransportError.connectionLost(reason: "reset"))
        let result = await task.result
        switch result {
        case .success:
            XCTFail("a call in flight at a disconnect must fail")
        case .failure(let error):
            XCTAssertEqual(error as? UndraTransportError, UndraTransportError.connectionLost(reason: "reset"))
        }
    }

    // MARK: construct

    func testConstructReturnsTheHandleFromTheReply() throws {
        let transport = FakeTransport()
        let handle = UndraHandle(index: 4, generation: 1)
        transport.onCallSync = { call in
            return Wire.Reply(callId: call.callId, status: .ok, body: ArraySlice(handle.undraEncoded())).encode()
        }
        let core = try makeCore(transport)
        let result = try core.construct(type: 0xAA, method: 0xBB, args: [1, 2])
        XCTAssertEqual(result, handle)
        XCTAssertEqual(transport.calls[0].target, .constructor(typeId: 0xAA, methodId: 0xBB))
        XCTAssertEqual(Array(transport.calls[0].args), [1, 2])
    }

    func testConstructRejectsTheNullHandleAndMalformedBodies() throws {
        let nullTransport = FakeTransport()
        nullTransport.onCallSync = { call in
            return Wire.Reply(callId: call.callId, status: .ok, body: ArraySlice(UndraHandle.null.undraEncoded())).encode()
        }
        let nullCore = try makeCore(nullTransport)
        XCTAssertThrowsError(try nullCore.construct(type: 1, method: 2, args: [])) { error in
            XCTAssertEqual(error as? UndraProtocolError, UndraProtocolError.nullHandle)
        }
        let shortTransport = FakeTransport()
        shortTransport.onCallSync = { call in
            return Wire.Reply(callId: call.callId, status: .ok, body: [1, 2]).encode()
        }
        let shortCore = try makeCore(shortTransport)
        XCTAssertThrowsError(try shortCore.construct(type: 1, method: 2, args: [])) { error in
            XCTAssertTrue(error is UndraProtocolError, "\(error)")
        }
    }

    func testConstructThrowsTheConstructorsError() throws {
        let transport = FakeTransport()
        transport.onCallSync = { call in
            return Wire.Reply(callId: call.callId, status: .error, body: [5]).encode()
        }
        let core = try makeCore(transport)
        XCTAssertThrowsError(try core.construct(type: 1, method: 2, args: [])) { error in
            XCTAssertEqual(error as? UndraReplyError, UndraReplyError(status: .error, body: [5]))
        }
    }

    // MARK: Events, timers, snapshots, statistics

    func testEventsAndTimersReachTheTransport() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        core.event(port: 1, method: 2, payload: [3, 4])
        core.timerFired(9)
        XCTAssertEqual(transport.sent, [.event(1, 2, [3, 4]), .timerFired(9)])
        core.shutdown()
        core.event(port: 1, method: 2, payload: [5])
        core.timerFired(10)
        XCTAssertEqual(transport.sent.count, 2, "nothing is sent after shutdown")
    }

    func testSnapshotAndRestoreGoThroughTheTransport() throws {
        let transport = FakeTransport()
        transport.snapshotBytes = [1, 2, 3]
        let core = try makeCore(transport)
        XCTAssertEqual(try core.snapshot(), [1, 2, 3])
        XCTAssertNoThrow(try core.restore([1, 2, 3]))
        transport.restoreCode = 5
        XCTAssertThrowsError(try core.restore([9])) { error in
            XCTAssertEqual(error as? UndraRestoreError, UndraRestoreError(code: 5))
        }
    }

    func testARestoreTheCoreCannotMigrateIsRefusedAsIncompatible() throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        transport.restoreCode = 7
        XCTAssertThrowsError(try core.restore([1])) { error in
            guard let refusal = error as? UndraRestoreError else {
                return XCTFail("expected UndraRestoreError, got \(error)")
            }
            XCTAssertEqual(refusal, .incompatible)
            XCTAssertEqual(refusal.code, 7)
            XCTAssertTrue(refusal.description.contains("code 7"), refusal.description)
            XCTAssertTrue(refusal.description.contains("cannot become this build's types"), refusal.description)
        }
    }

    func testTheRestoreCodesHaveNames() {
        // `restore_code` in crates/undra-ffi/src/api.rs.
        XCTAssertEqual(UndraRestoreError.panicked.code, 2)
        XCTAssertEqual(UndraRestoreError.badSnapshot.code, 5)
        XCTAssertEqual(UndraRestoreError.unavailable.code, 6)
        XCTAssertEqual(UndraRestoreError.incompatible.code, 7)
        XCTAssertEqual(UndraRestoreError(code: 7), .incompatible)
        XCTAssertNotEqual(UndraRestoreError(code: 5), .incompatible)
        XCTAssertTrue(UndraRestoreError.badSnapshot.description.contains("malformed"))
        XCTAssertTrue(UndraRestoreError.unavailable.description.contains("not running"))
        XCTAssertTrue(UndraRestoreError.panicked.description.contains("panicked"))
        XCTAssertTrue(UndraRestoreError(code: 99).description.contains("code 99: unknown code"))
        // Every description says the core is unchanged.
        for refusal in [UndraRestoreError.panicked, .badSnapshot, .unavailable, .incompatible, UndraRestoreError(code: 42)] {
            XCTAssertTrue(refusal.description.hasSuffix("a rejected restore leaves the core unchanged"))
        }
    }

    func testStatsCombineTheCoresDocumentWithHostCounters() throws {
        let transport = FakeTransport()
        transport.statsDocument = "{\"live_handles\":3,\"live_stores\":1,\"tasks\":2,\"transactions\":7,\"panics\":0,\"crossings\":{\"calls\":11}}"
        let core = try makeCore(transport)
        let object = UndraObject(core: core, handle: UndraHandle(rawValue: 77))
        var stats = core.stats()
        XCTAssertEqual(stats.coreLiveHandles, 3)
        XCTAssertEqual(stats.coreLiveStores, 1)
        XCTAssertEqual(stats.coreTasks, 2)
        XCTAssertEqual(stats.coreTransactions, 7)
        XCTAssertEqual(stats.values["crossings.calls"], 11)
        XCTAssertEqual(stats.hostLiveHandles, 1)
        object.close()
        stats = core.stats()
        XCTAssertEqual(stats.hostLiveHandles, 0)
    }

    func testStatsWithoutACoreDocumentAreHostOnly() throws {
        let core = try makeCore(FakeTransport())
        let stats = core.stats()
        XCTAssertEqual(stats.json, "")
        XCTAssertEqual(stats.coreLiveHandles, 0)
        XCTAssertEqual(stats.hostRegisteredPorts, 0)
    }

    // MARK: The shared core

    func testSharedIsEmptyUntilACoreIsLoaded() {
        // Every test in this target that loads a core (over a fake table) shuts it down again.
        XCTAssertNil(UndraCore.current)
    }

    func testLoadingInProcessWithoutACoreTableFails() throws {
        // The runtime links no core of its own (ADR-044): an in-process load needs the table.
        XCTAssertThrowsError(try UndraCore.load(.inproc(adapters: Adapters.none, expectedSchemaHash: 1))) { error in
            XCTAssertEqual(error as? UndraLoadError, UndraLoadError.missingCoreTable)
        }
        XCTAssertNil(UndraCore.current)
    }

    func testLoadingARemoteCoreWithAnInvalidURLFails() {
        for url in ["", "http://localhost:1", "not a url"] {
            XCTAssertThrowsError(try UndraCore.load(.remote(url: url, adapters: Adapters.none, expectedSchemaHash: 1)), url) { error in
                XCTAssertEqual(error as? UndraLoadError, UndraLoadError.invalidURL(url))
            }
        }
    }
}

/// An adapter that records its life cycle.
final class AttachRecorder: UndraAdapter, @unchecked Sendable {
    let portId: UInt32
    private let log = Guarded<[String]>([])

    init(portId: UInt32) {
        self.portId = portId
    }

    func makePortImpl(core: UndraCore) -> PortImpl? {
        return .sync([:])
    }

    func attach(to core: UndraCore) {
        log.withLock { (events: inout [String]) -> Void in
            events.append("attach")
        }
    }

    func detach() {
        log.withLock { (events: inout [String]) -> Void in
            events.append("detach")
        }
    }

    func events() -> [String] {
        return log.withLock { (events: inout [String]) -> [String] in
            return events
        }
    }
}
