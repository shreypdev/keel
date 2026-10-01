import UndraRuntime
import PlaygroundCore
import SwiftUI

/// The playground: four small screens over one Rust core. Everything they show is state that lives
/// in the core (`Todos`, `Counter`, `BigList`, a cached server list); the views only read it and
/// call its methods.
@main
struct PlaygroundApp: App {
    /// The stores of the four screens, created once the core is loaded; none in benchmark mode (`-bench`, see
    /// `BenchLaunch`), where the benchmark loads the core itself.
    @State private var model: PlaygroundModel?

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
                BenchScreen(mode: bench, quick: BenchLaunch.quick)
            } else if let model {
                RootView(model: model)
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
