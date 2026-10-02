import Foundation
import XCTest

// TEMPORARY DIAGNOSTIC (ci-green): raw URLSession close trials against the harness's server, inside XCTest.
final class DiagRawTests: XCTestCase {
    final class Delegate: NSObject, URLSessionWebSocketDelegate, @unchecked Sendable {
        var opened: CheckedContinuation<Void, Never>?
        func urlSession(_ session: URLSession, webSocketTask: URLSessionWebSocketTask, didOpenWithProtocol p: String?) {
            opened?.resume()
            opened = nil
        }
    }

    func testRawTrialsInsideXCTest() async throws {
        if RealtimeAdapterServer.current == nil {
            RealtimeAdapterServer.current = try RealtimeServer.start()
        }
        let server = try XCTUnwrap(RealtimeAdapterServer.current)
        for echo in [false, true] {
            for _ in 0 ..< 3 {
                let d = Delegate()
                let session = URLSession(configuration: .default, delegate: d, delegateQueue: OperationQueue())
                let task = session.webSocketTask(with: URL(string: "\(server.ws)/ws/echo")!)
                task.maximumMessageSize = 16 * 1024 * 1024
                await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
                    d.opened = c
                    task.resume()
                }
                if echo {
                    try? await task.send(.string("hello"))
                    _ = try? await task.receive()
                }
                task.cancel(with: .normalClosure, reason: Data("done".utf8))
                session.finishTasksAndInvalidate()
                try? await Task.sleep(nanoseconds: 600_000_000)
                let seen = try await server.last("/ws/echo")
                print("DIAGRAW echo=\(echo): closeCode=\(String(describing: seen?.closeCode)) clientClosed=\(String(describing: seen?.clientClosed))")
            }
        }
    }
}
