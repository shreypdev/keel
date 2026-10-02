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
        if let port = ProcessInfo.processInfo.environment["UNDRA_DIAG_PORT"], let number = Int(port) {
            print("DIAGRAW using the external server on \(number)")
            try await trials(ws: "ws://127.0.0.1:\(number)", http: "http://127.0.0.1:\(number)")
            return
        }
        if RealtimeAdapterServer.current == nil {
            RealtimeAdapterServer.current = try RealtimeServer.start()
        }
        let server = try XCTUnwrap(RealtimeAdapterServer.current)
        try await trials(ws: server.ws, http: server.http)
    }

    func trials(ws: String, http: String) async throws {
        for echo in [false, true] {
            for _ in 0 ..< 3 {
                let d = Delegate()
                let session = URLSession(configuration: .default, delegate: d, delegateQueue: OperationQueue())
                let task = session.webSocketTask(with: URL(string: "\(ws)/ws/echo")!)
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
                var request = URLRequest(url: URL(string: "\(http)/stats")!)
                request.cachePolicy = .reloadIgnoringLocalCacheData
                let (data, _) = try await URLSession.shared.data(for: request)
                let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
                let last = (json?["connections"] as? [[String: Any]])?.last { ($0["path"] as? String) == "/ws/echo" }
                print("DIAGRAW echo=\(echo): closeCode=\(String(describing: last?["closeCode"])) clientClosed=\(String(describing: last?["clientClosed"]))")
            }
        }
    }
}

/// The same trials with the test class on the main actor: the main queue is serviced while the test awaits.
@MainActor
final class DiagRawMainTests: XCTestCase {
    func testRawTrialsOnTheMainActor() async throws {
        if RealtimeAdapterServer.current == nil {
            RealtimeAdapterServer.current = try RealtimeServer.start()
        }
        let server = try XCTUnwrap(RealtimeAdapterServer.current)
        for _ in 0 ..< 4 {
            let d = DiagRawTests.Delegate()
            let session = URLSession(configuration: .default, delegate: d, delegateQueue: OperationQueue())
            let task = session.webSocketTask(with: URL(string: "\(server.ws)/ws/echo")!)
            task.maximumMessageSize = 16 * 1024 * 1024
            await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
                d.opened = c
                task.resume()
            }
            task.cancel(with: .normalClosure, reason: Data("done".utf8))
            session.finishTasksAndInvalidate()
            try? await Task.sleep(nanoseconds: 600_000_000)
            let seen = try await server.last("/ws/echo")
            print("DIAGMAIN closeCode=\(String(describing: seen?.closeCode)) clientClosed=\(String(describing: seen?.clientClosed))")
        }
    }
}
