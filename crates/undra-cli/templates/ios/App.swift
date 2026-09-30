import SwiftUI
import UndraRuntime
import @@SWIFT_MODULE@@

@main
struct MainApp: App {
    /// The to-do list, a store whose state lives in the Rust core.
    @State private var todos: Todos

    init() {
        do {
            try UndraBootstrap.start()
            _todos = State(initialValue: try Todos())
        } catch {
            // Loading fails when the core was built from another schema than these bindings
            // (`undra bindgen`, `undra build`), or when it cannot be reached (`undra dev`).
            fatalError("Undra did not start: \(error)")
        }
    }

    var body: some Scene {
        WindowGroup {
            ContentView(todos: todos)
        }
    }
}
