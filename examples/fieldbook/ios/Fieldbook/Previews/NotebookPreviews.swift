#if DEBUG
import FieldbookCore
import SwiftUI
import UndraRuntime
import UndraTestKit

// SwiftUI previews over the testing kit (docs/TESTING.md): the app's own core loaded in the preview, with the
// deterministic fakes as its ports. The notes below are what the device holds (`Kv`), the member is signed in
// (`SecureStore`), and the server is a fake that takes every note, so a screen in any state is a few lines and
// looks the same on every machine. A process holds one core: each preview shuts the one before it down.

private func seed(offline: Bool) -> String {
    func note(_ id: Int, _ title: String, _ body: String, _ tag: String, pinned: Bool = false) -> String {
        let key = "fieldbook.note." + String(format: "%08d", id)
        return "\"\(key)\": \"{\\\"id\\\":\(id),\\\"title\\\":\\\"\(title)\\\",\\\"body\\\":\\\"\(body)\\\",\\\"tag\\\":\\\"\(tag)\\\",\\\"pinned\\\":\(pinned),\\\"created_ms\\\":\(1_700_000_000_000 + id * 3_600_000),\\\"photos\\\":[]}\""
    }
    return """
    {
      "version": 1,
      "kv": {
        \(note(1, "Heron at the weir", "Grey, still, one leg up.", "birds", pinned: true)),
        \(note(2, "Gate 3 hinge", "Rusted through.", "repairs")),
        \(note(3, "Owl pellet", "Under the oak by the north fence.", "birds"))
      },
      "secure_store": { "fieldbook.access": "access-1", "fieldbook.refresh": "refresh-1" },
      "http": [{ "url_prefix": "https://fieldbook.test/notes", \(offline ? "\"error\": \"timeout\"" : "\"status\": 204") }],
      "connectivity": { "online": \(!offline), "kind": "\(offline ? "none" : "wifi")" }
    }
    """
}

@MainActor private func notebookScreen(offline: Bool) -> some View {
    let preview = try! PreviewCore.load(UndraFieldbookCore.load, seed: Seed(json: seed(offline: offline)))
    configureServer(ServerConfig(baseUrl: "https://fieldbook.test"), ctx: preview.core)
    let auth = try! Auth(ctx: preview.core)
    let notebook = try! Notebook(ctx: preview.core)
    return NotebookView(auth: auth, notebook: notebook, user: "Ada")
        .task { _ = try? await notebook.load() }
}

#Preview("Notebook: three notes, the real core on scripted ports") {
    notebookScreen(offline: false)
}

#Preview("Notebook: no signal, writes wait in the queue") {
    notebookScreen(offline: true)
}
#endif
