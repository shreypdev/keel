import SwiftUI
import KeelRuntime
import @@SWIFT_MODULE@@

@main
struct MainApp: App {
    /// The to-do list, a store whose state lives in the Rust core.
    @State private var todos: Todos

    init() {
        do {
            try KeelBootstrap.start()
            _todos = State(initialValue: try Todos())
        } catch {
            // Loading fails when the core was built from another schema than these bindings
            // (`keel bindgen`, `keel build`), or when it cannot be reached (`keel dev`).
            fatalError("Keel did not start: \(error)")
        }
    }

    var body: some Scene {
        WindowGroup {
            ContentView(todos: todos)
        }
    }
}
