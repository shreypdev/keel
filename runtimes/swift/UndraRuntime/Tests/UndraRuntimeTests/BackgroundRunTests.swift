import Foundation
import XCTest
@testable import UndraRuntime

/// `UndraCore.runInBackground(deadline:)` over the scripted fake core (ADR-046, decision 3.1 and 3.5):
/// the standard function's call, its reply, cancellation, and the failures it can have.
@MainActor
final class BackgroundRunTests: XCTestCase {
    private let report = UndraBackgroundReport(finished: true, replayed: 2, refetched: 1, stillPending: 0)

    private func args(milliseconds: UInt64) -> [UInt8] {
        var writer = UndraWriter()
        writer.writeU64(milliseconds)
        return writer.finish()
    }

    // MARK: The call

    func testTheRunIsTheStandardFunctionCalledWithTheDeadlineInMilliseconds() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let task = Task { try await core.runInBackground(deadline: 2.5) }
        let sent = await waitUntil { !transport.calls.isEmpty }
        XCTAssertTrue(sent)
        let call = try XCTUnwrap(transport.calls.first)
        XCTAssertEqual(call.target, .freeFunction(methodId: 0x0e5b_14ff))
        XCTAssertEqual(Array(call.args), args(milliseconds: 2_500))
        XCTAssertEqual(Array(call.args), hexToBytes("c409000000000000"), "u64 little-endian")
        transport.replyOk(call.callId, report.undraEncoded())
        let result = try await task.value
        XCTAssertEqual(result, report)
        XCTAssertEqual(core.stats().hostPendingCalls, 0)
        core.shutdown()
    }

    func testTheDeadlineIsRoundedUpAndClamped() {
        XCTAssertEqual(UndraCore.milliseconds(of: 0), 0)
        XCTAssertEqual(UndraCore.milliseconds(of: -3), 0)
        XCTAssertEqual(UndraCore.milliseconds(of: .nan), 0)
        XCTAssertEqual(UndraCore.milliseconds(of: 0.0004), 1, "a fraction of a millisecond is not zero")
        XCTAssertEqual(UndraCore.milliseconds(of: 1.2345), 1_235)
        XCTAssertEqual(UndraCore.milliseconds(of: 25), 25_000)
        XCTAssertEqual(UndraCore.milliseconds(of: 180), 180_000)
        XCTAssertEqual(UndraCore.milliseconds(of: .infinity), UInt64.max)
        XCTAssertEqual(UndraCore.milliseconds(of: 1e300), UInt64.max)
        XCTAssertEqual(UndraCore.milliseconds(of: 1e16), 10_000_000_000_000_000_000)
    }

    func testAnUnfinishedReportIsAValueNotAnError() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let task = Task { try await core.runInBackground(deadline: 1) }
        _ = await waitUntil { !transport.calls.isEmpty }
        let unfinished = UndraBackgroundReport(finished: false, replayed: 0, refetched: 0, stillPending: 3)
        transport.replyOk(transport.calls[0].callId, unfinished.undraEncoded())
        let result = try await task.value
        XCTAssertEqual(result, unfinished)
        core.shutdown()
    }

    // MARK: Cancellation

    func testCancellingTheTaskCancelsTheCallAndThrowsCancellationError() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let task = Task { try await core.runInBackground(deadline: 30) }
        let sent = await waitUntil { !transport.calls.isEmpty }
        XCTAssertTrue(sent)
        let callId = transport.calls[0].callId
        task.cancel()
        switch await task.result {
        case .success:
            XCTFail("a cancelled run must not succeed")
        case .failure(let error):
            XCTAssertTrue(error is CancellationError, "\(error)")
        }
        XCTAssertEqual(transport.cancels, [callId], "the core is told (undra_cancel)")
        XCTAssertEqual(core.stats().hostPendingCalls, 0)
        // The core's status 3 reply to the cancellation finds nobody waiting.
        transport.deliver(Wire.Reply(callId: callId, status: .cancelled))
        // And the core is still usable.
        let again = Task { try await core.runInBackground(deadline: 1) }
        _ = await waitUntil { transport.calls.count == 2 }
        transport.replyOk(transport.calls[1].callId, report.undraEncoded())
        let second = try await again.value
        XCTAssertEqual(second, report)
        core.shutdown()
    }

    func testAnAlreadyCancelledTaskSendsNothing() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let task = Task { () -> UndraBackgroundReport in
            withUnsafeCurrentTask { (current: UnsafeCurrentTask?) -> Void in
                current?.cancel()
            }
            return try await core.runInBackground(deadline: 5)
        }
        switch await task.result {
        case .success:
            XCTFail("a cancelled run must not succeed")
        case .failure(let error):
            XCTAssertTrue(error is CancellationError, "\(error)")
        }
        XCTAssertEqual(transport.calls.count, 0)
        core.shutdown()
    }

    func testStatusThreeFromTheCoreIsCancelledByCoreUnlessTheTaskWasCancelled() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let task = Task { try await core.runInBackground(deadline: 5) }
        _ = await waitUntil { !transport.calls.isEmpty }
        transport.deliver(Wire.Reply(callId: transport.calls[0].callId, status: .cancelled))
        do {
            _ = try await task.value
            XCTFail("a run the core cancelled must not succeed")
        } catch {
            XCTAssertEqual(error as? UndraCallError, .cancelledByCore)
        }
        core.shutdown()
    }

    // MARK: Failures

    func testAClosedCoreThrowsUnavailable() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        core.shutdown()
        do {
            _ = try await core.runInBackground(deadline: 5)
            XCTFail("a closed core must not run")
        } catch {
            XCTAssertEqual(error as? UndraCallError, .unavailable(.closed))
        }
        XCTAssertEqual(transport.calls.count, 0)
        // The placeholder `shared` returns before a core is loaded behaves the same.
        do {
            _ = try await UndraCore.unloaded.runInBackground(deadline: 5)
            XCTFail("the placeholder must not run")
        } catch {
            XCTAssertEqual(error as? UndraCallError, .unavailable(.closed))
        }
    }

    func testAShutdownDuringTheRunFailsItAsUnavailable() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        let task = Task { try await core.runInBackground(deadline: 30) }
        _ = await waitUntil { !transport.calls.isEmpty }
        core.shutdown()
        do {
            _ = try await task.value
            XCTFail("a shut-down core must not answer")
        } catch {
            XCTAssertEqual(error as? UndraCallError, .unavailable(.closed))
        }
    }

    func testAnUnreadableReplyIsMalformedNotACrash() async throws {
        let transport = FakeTransport()
        let core = try makeCore(transport)
        for body: [UInt8] in [[], [1], [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], Array(report.undraEncoded().dropLast())] {
            let task = Task { try await core.runInBackground(deadline: 1) }
            _ = await waitUntil { !transport.calls.isEmpty }
            let callId = try XCTUnwrap(transport.calls.last?.callId)
            transport.replyOk(callId, body)
            do {
                _ = try await task.value
                XCTFail("an undecodable body must not succeed")
            } catch {
                guard case .malformed? = error as? UndraCallError else {
                    return XCTFail("expected .malformed, got \(error)")
                }
            }
            transport.forgetSent()
        }
        core.shutdown()
    }

    func testTheCoreRefusingTheCallIsRefused() async throws {
        let transport = FakeTransport()
        transport.onCall = { _, _ in false }
        let core = try makeCore(transport)
        do {
            _ = try await core.runInBackground(deadline: 1)
            XCTFail("a refused call must not succeed")
        } catch {
            guard case .refused? = error as? UndraCallError else {
                return XCTFail("expected .refused, got \(error)")
            }
        }
        core.shutdown()
    }
}
