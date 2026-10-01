import UndraRuntime
import PlaygroundCore
import SwiftUI

/// The playground: four small screens over one Rust core. Everything they show is state that lives
/// in the core (`Todos`, `Counter`, `BigList`, a cached server list); the views only read it and
/// call its methods.
@main
struct PlaygroundApp: App {
    /// The stores of the four screens, created once the core is loaded.
    @State private var model: PlaygroundModel

    /// Counts the cores this process has loaded after the first: the screens start over when it changes.
    @State private var epoch = 0

    init() {
        do {
            try UndraBootstrap.start()
            _model = State(initialValue: try PlaygroundModel())
        } catch {
            // Loading fails when the core was built from another schema than these bindings
            // (`undra bindgen`, `undra build`), or when it cannot be reached (`undra dev`).
            fatalError("Undra did not start: \(error)")
        }
    }

    var body: some Scene {
        WindowGroup {
            RootView(model: model)
                .id(epoch)
                .safeAreaInset(edge: .top, spacing: 0) { DevStatusBar().id(epoch) }
                .task { UndraBootstrap.coreLost = { Task { await reload() } } }
        }
    }

    /// `undra dev` restarted the core, so the stores of the old one are gone: load the new core (the dev server may
    /// still be starting) and create the stores again.
    @MainActor
    private func reload() async {
        for _ in 0..<60 {
            do {
                try UndraBootstrap.start()
                model = try PlaygroundModel()
                epoch += 1
                return
            } catch {
                try? await Task.sleep(nanoseconds: 500_000_000)
            }
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
        inbox = try RemoteTodosQueryHandle(list: UndraBootstrap.inboxList)
    }
}
