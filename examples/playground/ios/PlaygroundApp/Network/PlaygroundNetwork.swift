import Foundation
import UndraRuntime
import os
import PlaygroundCore

/// The playground's network: an in-memory server that answers the core's `Http` port, and the
/// Offline switch that makes it unreachable.
///
/// It speaks what `examples/playground/core` expects of a server (`remote.rs`): `GET
/// /lists/{list}/todos` answers a JSON array, `POST` creates an item from `{"title": ...}`
/// (answering 201, and answering a repeated `Idempotency-Key` with the item it already made), and
/// `PATCH /lists/{list}/todos/{id}` changes `{"done": ...}`. Every answer takes about 300 ms, so the
/// screen has time to show that it is fetching and the optimistic row is visible.
///
/// While offline every request fails with `HttpError.network`, and the core is told through the
/// `Connectivity` port so that it queues idempotent mutations and replays them when the network
/// returns.
final class PlaygroundNetwork: UndraAdapter, @unchecked Sendable {
    /// The one network of the app.
    static let shared = PlaygroundNetwork()

    /// How long the server takes to answer.
    private static let latency: Duration = .milliseconds(300)

    /// An item as the server stores and sends it.
    private struct ServerTodo: Codable {
        let id: UInt32
        var title: String
        var done: Bool
    }

    private struct State {
        var offline = false
        var nextId: UInt32 = 4
        var lists: [String: [ServerTodo]] = [
            UndraBootstrap.inboxList: [
                ServerTodo(id: 1, title: "Buy milk", done: false),
                ServerTodo(id: 2, title: "Walk the dog", done: false),
                ServerTodo(id: 3, title: "Write Undra", done: false),
            ],
        ]
        /// The item each `Idempotency-Key` already created.
        var created: [String: ServerTodo] = [:]
    }

    private let state = OSAllocatedUnfairLock(initialState: State())

    private init() {}

    // MARK: Offline

    /// Whether the server is unreachable.
    var isOffline: Bool {
        return state.withLock { $0.offline }
    }

    /// Makes the server unreachable (or reachable again) and tells the core, which replays what it
    /// queued when the network comes back.
    func setOffline(_ offline: Bool, core: UndraCore = .shared) {
        state.withLock { $0.offline = offline }
        core.event(
            port: WireConnectivity.portId,
            method: WireConnectivity.changedMethod,
            payload: WireConnectivity.changed(online: !offline)
        )
    }

    // MARK: The Http port

    var portId: UInt32 {
        return fnv1a32("port.Http")
    }

    func makePortImpl(core: UndraCore) -> PortImpl? {
        return .async([
            fnv1a32("Http.request"): { [self] arguments in
                let request = try HttpRequest.undraDecoded(from: arguments)
                if isOffline {
                    throw UndraPortError(body: HttpError.network("The Internet connection appears to be offline.").undraEncoded())
                }
                try await Task.sleep(for: PlaygroundNetwork.latency)
                return respond(to: request).undraEncoded()
            },
        ])
    }

    /// Routes one request. The URL is `https://playground.undra.test/lists/{list}/todos[/{id}]`.
    private func respond(to request: HttpRequest) -> HttpResponse {
        guard let url = URL(string: request.url), url.host == URL(string: UndraBootstrap.serverURL)?.host else {
            return HttpResponse(status: 404)
        }
        let path = url.pathComponents.filter { $0 != "/" }
        guard path.count >= 3, path[0] == "lists", path[2] == "todos" else {
            return HttpResponse(status: 404)
        }
        let list = path[1]
        switch (request.method, path.count) {
        case (.get, 3):
            return json(200, state.withLock { $0.lists[list] ?? [] })
        case (.post, 3):
            return create(in: list, request)
        case (.patch, 4):
            return change(in: list, id: UInt32(path[3]), request)
        default:
            return HttpResponse(status: 404)
        }
    }

    private func create(in list: String, _ request: HttpRequest) -> HttpResponse {
        struct NewTodo: Decodable {
            let title: String
        }
        guard let body = request.body, let new = try? JSONDecoder().decode(NewTodo.self, from: Data(body)) else {
            return HttpResponse(status: 400)
        }
        let key = request.header("Idempotency-Key")
        let todo = state.withLock { (state: inout State) -> ServerTodo in
            if let key, let existing = state.created[key] {
                return existing
            }
            let todo = ServerTodo(id: state.nextId, title: new.title, done: false)
            state.nextId += 1
            state.lists[list, default: []].append(todo)
            if let key {
                state.created[key] = todo
            }
            return todo
        }
        return json(201, todo)
    }

    private func change(in list: String, id: UInt32?, _ request: HttpRequest) -> HttpResponse {
        struct Change: Decodable {
            let done: Bool
        }
        guard let id, let body = request.body, let change = try? JSONDecoder().decode(Change.self, from: Data(body)) else {
            return HttpResponse(status: 400)
        }
        let changed = state.withLock { (state: inout State) -> ServerTodo? in
            guard let index = state.lists[list]?.firstIndex(where: { $0.id == id }) else {
                return nil
            }
            state.lists[list]?[index].done = change.done
            return state.lists[list]?[index]
        }
        guard let changed else {
            return HttpResponse(status: 404)
        }
        return json(200, changed)
    }

    private func json<Value: Encodable>(_ status: UInt16, _ value: Value) -> HttpResponse {
        // Encoding plain Codable structs cannot fail.
        return HttpResponse(
            status: status,
            headers: [Header(name: "Content-Type", value: "application/json")],
            body: [UInt8]((try? JSONEncoder().encode(value)) ?? Data())
        )
    }
}

private extension HttpRequest {
    /// The value of the header called `name`, compared without regard to case.
    func header(_ name: String) -> String? {
        return headers.first { $0.name.caseInsensitiveCompare(name) == .orderedSame }?.value
    }
}
