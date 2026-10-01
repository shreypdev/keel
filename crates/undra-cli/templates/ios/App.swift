import SwiftUI
import UndraRuntime
import @@SWIFT_MODULE@@

@main
struct MainApp: App {
    /// The to-do list, a store whose state lives in the Rust core.
    @State private var todos: Todos

    /// Counts the cores this process has loaded after the first: the screen starts over when it changes.
    @State private var epoch = 0

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
                .id(epoch)
                .safeAreaInset(edge: .top, spacing: 0) { DevStatusBar().id(epoch) }
                .task { UndraBootstrap.coreLost = { Task { await reload() } } }
        }
    }

    /// `undra dev` restarted the core, so the store of the old one is gone: load the new core (the dev server may
    /// still be starting) and create the store again.
    @MainActor
    private func reload() async {
        for _ in 0..<60 {
            do {
                try UndraBootstrap.start()
                todos = try Todos()
                epoch += 1
                return
            } catch {
                try? await Task.sleep(nanoseconds: 500_000_000)
            }
        }
    }
}
