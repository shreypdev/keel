import KeelRuntime
import PlaygroundCore
import SwiftUI

/// The playground: four small screens over one Rust core. Everything they show is state that lives
/// in the core (`Todos`, `Counter`, `BigList`, a cached server list); the views only read it and
/// call its methods.
@main
struct PlaygroundApp: App {
    /// The stores of the four screens, created once the core is loaded.
    @State private var model: PlaygroundModel

    init() {
        do {
            try KeelBootstrap.start()
            _model = State(initialValue: try PlaygroundModel())
        } catch {
            // Loading fails when the core was built from another schema than these bindings
            // (`keel bindgen`, `keel build`), or when it cannot be reached (`keel dev`).
            fatalError("Keel did not start: \(error)")
        }
    }

    var body: some Scene {
        WindowGroup {
            RootView(model: model)
        }
    }
}

/// The stores the screens share for the life of the app.
@MainActor
final class PlaygroundModel {
    let todos: Todos
    let counter: Counter
    let bigList: BigList
    let inbox: RemoteTodosQueryHandle

    init() throws {
        todos = try Todos()
        counter = try Counter()
        bigList = try BigList()
        inbox = try RemoteTodosQueryHandle(list: KeelBootstrap.inboxList)
    }
}
