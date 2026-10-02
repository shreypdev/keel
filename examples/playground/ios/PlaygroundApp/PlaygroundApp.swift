import UndraRuntime
import PlaygroundCore
import SwiftUI

/// The playground: five small screens over one Rust core. Everything they show is state that lives
/// in the core (`Todos`, `Counter`, `BigList`, a cached server list, notes in SQLite); the views only
/// read it and call its methods.
@main
struct PlaygroundApp: App {
    /// The stores of the five screens, created once the core is loaded; none in benchmark mode (`-bench`, see
    /// `BenchLaunch`), where the benchmark loads the core itself.
    @State private var model: PlaygroundModel?

    /// Counts the cores this process has loaded after the first: the screens start over when it changes.
    @State private var epoch = 0

    init() {
        if BenchLaunch.mode != nil {
            _model = State(initialValue: nil)
            return
        }
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
            if let bench = BenchLaunch.mode {
                // A benchmark run gets neither the dev status bar nor the reload wrapper: it loads the core itself.
                BenchScreen(mode: bench, quick: BenchLaunch.quick)
            } else if let model {
                RootView(model: model)
                    .id(epoch)
                    .safeAreaInset(edge: .top, spacing: 0) { DevStatusBar().id(epoch) }
                    .task { UndraBootstrap.coreLost = { Task { await reload() } } }
            }
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
    /// Opened by its screen (`NotesScreen`): the database is the platform's `SQLiteDbAdapter`.
    let notes: Notes
    let workshop: Workshop
    /// Two shelves the workshop hands out: child stores, one wrapper each however often asked for.
    let leftShelf: Shelf
    let rightShelf: Shelf

    init() throws {
        todos = try Todos()
        counter = try Counter()
        bigList = try BigList()
        inbox = try RemoteTodosQueryHandle(list: UndraBootstrap.inboxList)
        notes = try Notes()
        workshop = try Workshop()
        leftShelf = try workshop.shelf(name: "left")
        rightShelf = try workshop.shelf(name: "right")
    }
}
