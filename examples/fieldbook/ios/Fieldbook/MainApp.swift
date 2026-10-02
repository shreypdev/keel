import SwiftUI
import UndraRuntime
import FieldbookCore

@main
struct MainApp: App {
    /// The two stores: the sign-in state and the notebook. Their state lives in the Rust core.
    @State private var auth: Auth
    @State private var notebook: Notebook

    /// Counts the cores this process has loaded after the first: the screen starts over when it changes.
    @State private var epoch = 0

    init() {
        do {
            try UndraBootstrap.start()
            let stores = try MainApp.makeStores()
            _auth = State(initialValue: stores.0)
            _notebook = State(initialValue: stores.1)
        } catch {
            // Loading fails when the core was built from another schema than these bindings
            // (`undra bindgen`, `undra build`), or when it cannot be reached (`undra dev`).
            fatalError("Undra did not start: \(error)")
        }
    }

    /// Tells the core where the server is (`server/server.mjs`: the iOS Simulator reaches the Mac at 127.0.0.1;
    /// set FIELDBOOK_SERVER in the scheme for a device) and creates the stores. Both `resume` and `load` run in the
    /// background: the screen shows what the device holds at once.
    @MainActor
    private static func makeStores() throws -> (Auth, Notebook) {
        let server = ProcessInfo.processInfo.environment["FIELDBOOK_SERVER"] ?? "http://127.0.0.1:8787"
        configureServer(ServerConfig(baseUrl: server))
        let auth = try Auth()
        let notebook = try Notebook()
        Task {
            _ = try? await notebook.load()
            try? await auth.resume()
        }
        return (auth, notebook)
    }

    var body: some Scene {
        WindowGroup {
            ContentView(auth: auth, notebook: notebook)
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
                let stores = try MainApp.makeStores()
                auth = stores.0
                notebook = stores.1
                epoch += 1
                return
            } catch {
                try? await Task.sleep(nanoseconds: 500_000_000)
            }
        }
    }
}
