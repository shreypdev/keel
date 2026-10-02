#if DEBUG
import PlaygroundCore
import SwiftUI
import UndraRuntime
import UndraTestKit

// SwiftUI previews over the testing kit (docs/TESTING.md). Two ways to give a screen its data without a running app:
//
//   * a RecordedCore plays a recorded session under the generated store: no Rust runs, so a state that is costly to reach (three items, one done)
//     is one line, and it renders the same everywhere;
//   * a PreviewCore loads the app's own core with the deterministic fakes as its ports, so the real logic runs against a server and a clock the
//     preview scripts.
//
// A process holds one core: each preview shuts the one before it down.

#Preview("Todos: a recorded session, played to the end") {
    let recorded = try! RecordedCore.load(PreviewData.recording("session-todos"), expectedSchemaHash: UndraIds.schemaHash)
    recorded.playAll()
    return TodosScreen(todos: try! Todos(ctx: recorded.core), selection: try! TodoSelection(ctx: recorded.core))
}

#Preview("Todos: the real core on scripted ports") {
    let preview = try! PreviewCore.load(UndraPlaygroundCore.load, seed: PreviewData.seed())
    let todos = try! Todos(ctx: preview.core)
    return TodosScreen(todos: todos, selection: try! TodoSelection(ctx: preview.core))
        .task {
            for title in ["Buy milk", "Walk the dog", "Write the docs"] {
                _ = try? await todos.add(title: title)
            }
        }
}

#Preview("Remote list: the seeded server answers") {
    let preview = try! PreviewCore.load(UndraPlaygroundCore.load, seed: PreviewData.seed())
    configureRemote(RemoteConfig(baseUrl: "https://api.test"), ctx: preview.core)
    return RemoteScreen(inbox: try! RemoteTodosQueryHandle(list: "inbox", ctx: preview.core))
}
#endif
