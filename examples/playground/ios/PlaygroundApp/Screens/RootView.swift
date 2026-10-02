import SwiftUI

/// The nine screens, one per tab. Launch with `-tab remote` (or `todos`, `counter`, `biglist`,
/// `notes`, `workshop`, `library`, `feed`, `ticker`) to start on another one: `xcrun simctl launch booted dev.undra.playground -tab remote`.
struct RootView: View {
    /// A tab of the app; the raw value is what `-tab` takes.
    enum Tab: String {
        case todos, counter, biglist, remote, notes, workshop, library, feed, ticker
    }

    let model: PlaygroundModel
    @State private var selection: Tab = Tab(rawValue: UserDefaults.standard.string(forKey: "tab") ?? "") ?? .todos

    var body: some View {
        TabView(selection: $selection) {
            TodosScreen(todos: model.todos, selection: model.todoSelection)
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
            NotesScreen(notes: model.notes, selection: model.noteSelection)
                .tabItem { Label("Notes", systemImage: "note.text").accessibilityIdentifier("tab-notes") }
                .tag(Tab.notes)
            WorkshopScreen(workshop: model.workshop, left: model.leftShelf, right: model.rightShelf)
                .tabItem { Label("Workshop", systemImage: "hammer").accessibilityIdentifier("tab-workshop") }
                .tag(Tab.workshop)
            LibraryScreen(library: model.library)
                .tabItem { Label("Library", systemImage: "books.vertical").accessibilityIdentifier("tab-library") }
                .tag(Tab.library)
            FeedScreen(all: model.feed, even: model.evenFeed)
                .tabItem { Label("Feed", systemImage: "text.append").accessibilityIdentifier("tab-feed") }
                .tag(Tab.feed)
            TickerScreen()
                .tabItem { Label("Ticker", systemImage: "timer").accessibilityIdentifier("tab-ticker") }
                .tag(Tab.ticker)
        }
    }
}
