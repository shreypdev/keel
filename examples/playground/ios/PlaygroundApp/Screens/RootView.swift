import SwiftUI

/// The four screens, one per tab. Launch with `-tab remote` (or `todos`, `counter`, `biglist`) to
/// start on another one: `xcrun simctl launch booted dev.keel.playground -tab remote`.
struct RootView: View {
    /// A tab of the app; the raw value is what `-tab` takes.
    enum Tab: String {
        case todos, counter, biglist, remote
    }

    let model: PlaygroundModel
    @State private var selection: Tab = Tab(rawValue: UserDefaults.standard.string(forKey: "tab") ?? "") ?? .todos

    var body: some View {
        TabView(selection: $selection) {
            TodosScreen(todos: model.todos)
                .tabItem { Label("Todos", systemImage: "checklist").accessibilityIdentifier("tab-todos") }
                .tag(Tab.todos)
            CounterScreen(counter: model.counter)
                .tabItem { Label("Counter", systemImage: "plusminus.circle").accessibilityIdentifier("tab-counter") }
                .tag(Tab.counter)
            BigListScreen(list: model.bigList)
                .tabItem { Label("10k list", systemImage: "list.number").accessibilityIdentifier("tab-biglist") }
                .tag(Tab.biglist)
            RemoteScreen(inbox: model.inbox)
                .tabItem { Label("Remote", systemImage: "icloud").accessibilityIdentifier("tab-remote") }
                .tag(Tab.remote)
        }
    }
}
