import Foundation
import KeelRuntime
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
final class PlaygroundNetwork: KeelAdapter, @unchecked Sendable {
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
            KeelBootstrap.inboxList: [
                ServerTodo(id: 1, title: "Buy milk", done: false),
                ServerTodo(id: 2, title: "Walk the dog", done: false),
                ServerTodo(id: 3, title: "Write Keel", done: false),
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
    func setOffline(_ offline: Bool, core: KeelCore = .shared) {
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

    func makePortImpl(core: KeelCore) -> PortImpl? {
        return .async([
            fnv1a32("Http.request"): { [self] arguments in
                let request = try WireHttpRequest.decode(arguments)
                if isOffline {
                    throw KeelPortError(body: HttpError.network("The Internet connection appears to be offline.").keelEncoded())
                }
                try await Task.sleep(for: PlaygroundNetwork.latency)
                return respond(to: request).encoded()
            },
        ])
    }

    /// Routes one request. The URL is `https://playground.keel.test/lists/{list}/todos[/{id}]`.
    private func respond(to request: WireHttpRequest) -> WireHttpResponse {
        guard let url = URL(string: request.url), url.host == URL(string: KeelBootstrap.serverURL)?.host else {
            return WireHttpResponse(status: 404, body: Data())
        }
        let path = url.pathComponents.filter { $0 != "/" }
        guard path.count >= 3, path[0] == "lists", path[2] == "todos" else {
            return WireHttpResponse(status: 404, body: Data())
        }
        let list = path[1]
        switch (request.method, path.count) {
        case ("GET", 3):
            return json(200, state.withLock { $0.lists[list] ?? [] })
        case ("POST", 3):
            return create(in: list, request)
        case ("PATCH", 4):
            return change(in: list, id: UInt32(path[3]), request)
        default:
            return WireHttpResponse(status: 404, body: Data())
        }
    }

    private func create(in list: String, _ request: WireHttpRequest) -> WireHttpResponse {
        struct NewTodo: Decodable {
            let title: String
        }
        guard let body = request.body, let new = try? JSONDecoder().decode(NewTodo.self, from: body) else {
            return WireHttpResponse(status: 400, body: Data())
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

    private func change(in list: String, id: UInt32?, _ request: WireHttpRequest) -> WireHttpResponse {
        struct Change: Decodable {
            let done: Bool
        }
        guard let id, let body = request.body, let change = try? JSONDecoder().decode(Change.self, from: body) else {
            return WireHttpResponse(status: 400, body: Data())
        }
        let changed = state.withLock { (state: inout State) -> ServerTodo? in
            guard let index = state.lists[list]?.firstIndex(where: { $0.id == id }) else {
                return nil
            }
            state.lists[list]?[index].done = change.done
            return state.lists[list]?[index]
        }
        guard let changed else {
            return WireHttpResponse(status: 404, body: Data())
        }
        return json(200, changed)
    }

    private func json<Value: Encodable>(_ status: UInt16, _ value: Value) -> WireHttpResponse {
        // Encoding plain Codable structs cannot fail.
        return WireHttpResponse(status: status, body: (try? JSONEncoder().encode(value)) ?? Data())
    }
}
