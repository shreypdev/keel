import PlaygroundCore
import SwiftUI

/// The data layer: a server list cached by the core. `inbox` is the query handle; its status,
/// fetching flag, data, error and update time are signals of the core. Adding an item shows it at
/// once (the core's optimistic update) and takes it back if the server refuses; while the Offline
/// switch is on, the addition waits in the core's queue and is sent when the network returns.
struct RemoteScreen: View {
    let inbox: RemoteTodosQueryHandle
    @State private var draft = ""
    @State private var problem: String?
    @State private var offline = PlaygroundNetwork.shared.isOffline

    var body: some View {
        NavigationStack {
            List {
                Section("Query") {
                    LabeledContent("Status") {
                        HStack(spacing: 8) {
                            if inbox.fetching {
                                ProgressView().accessibilityIdentifier("remote-fetching")
                            }
                            Text(statusName).accessibilityIdentifier("remote-status")
                        }
                    }
                    LabeledContent("Updated") {
                        Text(inbox.updatedAt?.formatted(date: .omitted, time: .standard) ?? "never")
                            .accessibilityIdentifier("remote-updated")
                    }
                    if let error = inbox.error {
                        Text(error.localizedDescription)
                            .foregroundStyle(.red)
                            .accessibilityIdentifier("remote-error")
                    }
                    Toggle("Offline", isOn: $offline)
                        .accessibilityIdentifier("remote-offline")
                        .onChange(of: offline) { _, offline in
                            PlaygroundNetwork.shared.setOffline(offline)
                        }
                }
                Section("Inbox") {
                    ForEach(inbox.data ?? [], id: \.id) { todo in
                        row(todo)
                    }
                    HStack {
                        TextField("New item", text: $draft)
                            .submitLabel(.done)
                            .onSubmit(add)
                            .accessibilityIdentifier("remote-input")
                        Button("Add", action: add)
                            .disabled(draft.trimmingCharacters(in: .whitespaces).isEmpty)
                            .accessibilityIdentifier("remote-add")
                    }
                    if let problem {
                        Text(problem)
                            .font(.footnote)
                            .foregroundStyle(.red)
                            .accessibilityIdentifier("remote-problem")
                    }
                }
                Section {
                    Button("Refresh") { inbox.refetch() }
                        .accessibilityIdentifier("remote-refresh")
                }
            }
            .refreshable { inbox.refetch() }
            .navigationTitle("Remote")
            .navigationBarTitleDisplayMode(.inline)
        }
    }

    private var statusName: String {
        switch inbox.status {
        case .idle: "Idle"
        case .fetching: "Fetching"
        case .success: "Success"
        case .error: "Error"
        }
    }

    /// An item the server has not answered for yet has an identity counting down from `u32::MAX`.
    private func isPlaceholder(_ todo: RemoteTodo) -> Bool {
        return todo.id > UInt32.max - 1_000
    }

    @ViewBuilder
    private func row(_ todo: RemoteTodo) -> some View {
        if isPlaceholder(todo) {
            HStack {
                Label(todo.title, systemImage: "circle.dotted")
                Spacer()
                Text(offline ? "waiting for network" : "sending")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .foregroundStyle(.secondary)
            .accessibilityIdentifier("remote-row-pending")
        } else {
            Button {
                toggle(todo)
            } label: {
                Label(todo.title, systemImage: todo.done ? "checkmark.circle.fill" : "circle")
                    .foregroundStyle(todo.done ? .secondary : .primary)
            }
            .accessibilityIdentifier("remote-row")
        }
    }

    /// Creates the item: shown at once, sent to the server, taken back if the server refuses.
    private func add() {
        let title = draft.trimmingCharacters(in: .whitespaces)
        guard !title.isEmpty else {
            return
        }
        draft = ""
        Task {
            do {
                _ = try await createRemoteTodo(list: KeelBootstrap.inboxList, title: title)
                problem = nil
            } catch {
                problem = error.localizedDescription
            }
        }
    }

    /// Flips `done`: shown at once, sent to the server, flipped back if the server refuses.
    private func toggle(_ todo: RemoteTodo) {
        Task {
            do {
                _ = try await setRemoteDone(list: KeelBootstrap.inboxList, id: todo.id, done: !todo.done)
                problem = nil
            } catch {
                problem = error.localizedDescription
            }
        }
    }
}
