import Foundation
@testable import UndraRuntime

/// The `Http` port of the harness: an in-memory server.
///
/// Routes are matched by method and exact URL. The server records every request (method, URL,
/// headers, body), can hold a reply back for a number of milliseconds and can answer with a
/// network error (`HttpError.network("offline")`). An unknown route answers 404.
final class FakeServer: UndraAdapter, @unchecked Sendable {
    /// Where the harness tells the core the server is (`configureRemote`).
    static let baseURL = "https://playground.test"

    /// A request as the core sent it.
    struct Request: Sendable {
        let method: String
        let url: String
        let headers: [(name: String, value: String)]
        let body: [UInt8]?

        /// The value of the header called `name` (case-insensitive), if any.
        func header(_ name: String) -> String? {
            return headers.first { $0.name.lowercased() == name.lowercased() }?.value
        }

        /// The body as text.
        var bodyText: String? {
            return body.map { String(decoding: $0, as: UTF8.self) }
        }
    }

    /// What a route does with a request.
    enum Outcome: Sendable {
        case response(status: UInt16, body: [UInt8])
        case networkError(String)
    }

    private struct Route {
        let outcome: Outcome
        let delayMs: UInt32
    }

    private struct State {
        var routes: [String: Route] = [:]
        var requests: [Request] = []
    }

    private let state = Locked<State>(State())

    var portId: UInt32 {
        return StandardPorts.Http.portId
    }

    // MARK: Scripting

    /// Answers `method` `path` with `status` and `body`, after `delayMs` milliseconds.
    func respond(_ method: String, _ path: String, status: UInt16 = 200, body: String, delayMs: UInt32 = 0) {
        install(method, path, Route(outcome: .response(status: status, body: Array(body.utf8)), delayMs: delayMs))
    }

    /// Answers `method` `path` with `status` and `json` encoded as JSON.
    func respond<Body: Encodable>(_ method: String, _ path: String, status: UInt16 = 200, json: Body, delayMs: UInt32 = 0) {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        // Encoding a plain `Codable` struct cannot fail.
        let body = (try? encoder.encode(json)) ?? Data()
        install(method, path, Route(outcome: .response(status: status, body: [UInt8](body)), delayMs: delayMs))
    }

    /// Fails `method` `path` with `HttpError.network(reason)`, after `delayMs` milliseconds.
    func failNetwork(_ method: String, _ path: String, reason: String = "offline", delayMs: UInt32 = 0) {
        install(method, path, Route(outcome: .networkError(reason), delayMs: delayMs))
    }

    private func install(_ method: String, _ path: String, _ route: Route) {
        state.withLock { (current: inout State) -> Void in
            current.routes[FakeServer.key(method, FakeServer.baseURL + path)] = route
        }
    }

    private static func key(_ method: String, _ url: String) -> String {
        return method + " " + url
    }

    // MARK: Recording

    /// Every request so far, oldest first.
    var requests: [Request] {
        return state.withLock { (current: inout State) -> [Request] in current.requests }
    }

    /// The requests to `method` `path`, oldest first.
    func requests(_ method: String, _ path: String) -> [Request] {
        let url = FakeServer.baseURL + path
        return requests.filter { $0.method == method && $0.url == url }
    }

    // MARK: The port

    func makePortImpl(core: UndraCore) -> PortImpl? {
        let state = self.state
        return .async([
            StandardPorts.Http.request: { args in
                var reader = UndraReader(args)
                let request = try HttpRequest.undraDecode(&reader)
                try reader.finish()
                let route = state.withLock { (current: inout State) -> Route? in
                    current.requests.append(Request(
                        method: request.method.name,
                        url: request.url,
                        headers: request.headers.map { (name: $0.name, value: $0.value) },
                        body: request.body
                    ))
                    return current.routes[FakeServer.key(request.method.name, request.url)]
                }
                if let route = route, route.delayMs > 0 {
                    try await Task.sleep(nanoseconds: UInt64(route.delayMs) * 1_000_000)
                }
                switch route?.outcome {
                case .response(let status, let body)?:
                    return HttpResponse(status: status, headers: [], body: body).undraEncoded()
                case .networkError(let reason)?:
                    throw UndraPortError(body: HttpError.network(reason).undraEncoded())
                case nil:
                    return HttpResponse(status: 404, headers: [], body: []).undraEncoded()
                }
            },
        ])
    }
}

/// A to-do as the server speaks it: `{"id":1,"title":"Buy milk","done":false}`.
struct ServerTodo: Codable, Equatable, Sendable {
    let id: UInt32
    let title: String
    let done: Bool
}
