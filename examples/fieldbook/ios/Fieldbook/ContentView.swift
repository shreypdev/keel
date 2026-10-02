import PhotosUI
import SwiftUI
import UIKit
import FieldbookCore

/// Fieldbook: sign in, then the notebook. Reading a signal (`auth.session`, `notebook.visible`) is a plain property
/// read: the core pushes changes, SwiftUI observes them.
struct ContentView: View {
    let auth: Auth
    let notebook: Notebook

    var body: some View {
        switch auth.session {
        case .signedOut: SignInView(auth: auth)
        case .signedIn(let user): NotebookView(auth: auth, notebook: notebook, user: user)
        }
    }
}

struct SignInView: View {
    let auth: Auth
    @State private var name = ""
    @State private var code = ""
    @State private var problem: String?

    var body: some View {
        Form {
            Section("Fieldbook") {
                TextField("Your name", text: $name)
                SecureField("Team code", text: $code)
                Button("Sign in") {
                    Task {
                        do { try await auth.signIn(name: name, code: code); problem = nil }
                        catch AuthError.badCredentials { problem = "Wrong name or team code (the demo's is “fieldbook”)." }
                        catch { problem = error.localizedDescription }
                    }
                }
                .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || code.isEmpty || auth.busy)
                if let problem { Text(problem).foregroundStyle(.red).font(.footnote) }
            }
        }
    }
}

struct NotebookView: View {
    let auth: Auth
    let notebook: Notebook
    let user: String
    @State private var title = ""
    @State private var body_ = ""
    @State private var tag = ""
    @State private var problem: String?

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Text(notebook.pending == 0 ? "Everything is sent" : "\(notebook.pending) change(s) waiting to send")
                        .font(.footnote).foregroundStyle(.secondary)
                    if let problem { Text(problem).foregroundStyle(.red).font(.footnote) }
                }
                Section("New note") {
                    TextField("Title", text: $title)
                    TextField("Tag", text: $tag).textInputAutocapitalization(.never)
                    TextField("What did you see?", text: $body_, axis: .vertical)
                    Button("Add note", action: add).disabled(title.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                if !notebook.tags.isEmpty {
                    Section {
                        ScrollView(.horizontal, showsIndicators: false) {
                            HStack {
                                tagButton("all", tag: "")
                                ForEach(notebook.tags, id: \.self) { tagButton($0, tag: $0) }
                            }
                        }
                    }
                }
                Section {
                    ForEach(notebook.visible, id: \.id) { note in
                        NoteRow(note: note, notebook: notebook, onProblem: { problem = $0 })
                    }
                }
            }
            .searchable(text: Binding(get: { notebook.filter.query }, set: { notebook.setQuery(query: $0) }))
            .navigationTitle("Fieldbook")
            .toolbar {
                Button(user) {
                    Task {
                        do { try await auth.signOut() }
                        catch AuthError.pendingWrites(let count) { problem = "\(count) change(s) have not been sent yet." }
                        catch { problem = error.localizedDescription }
                    }
                }
            }
        }
    }

    private func tagButton(_ label: String, tag: String) -> some View {
        Button(label) { notebook.setTag(tag: tag) }
            .buttonStyle(.bordered)
            .tint(notebook.filter.tag == tag ? .accentColor : .secondary)
    }

    private func add() {
        Task {
            do {
                _ = try await notebook.add(title: title, body: body_, tag: tag)
                title = ""; body_ = ""; problem = nil
            } catch {
                problem = error.localizedDescription
            }
        }
    }
}

struct NoteRow: View {
    let note: Note
    let notebook: Notebook
    let onProblem: (String?) -> Void
    @State private var picked: PhotosPickerItem?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(note.title).font(.headline)
                if !note.tag.isEmpty { Text(note.tag).font(.caption).padding(.horizontal, 6).overlay(Capsule().stroke()) }
                Spacer()
                Button(note.pinned ? "Unpin" : "Pin") { run { try await notebook.togglePin(id: note.id) } }
                    .buttonStyle(.borderless)
            }
            if !note.body.isEmpty { Text(note.body) }
            Text(note.created.formatted()).font(.caption).foregroundStyle(.secondary)
            HStack {
                ForEach(note.photos, id: \.self) { PhotoThumb(path: $0) }
                PhotosPicker("+ photo", selection: $picked, matching: .images)
            }
        }
        .swipeActions { Button("Delete", role: .destructive) { run { try await notebook.remove(id: note.id) } } }
        .onChange(of: picked) { _, item in
            guard let item else { return }
            run {
                if let data = try await item.loadTransferable(type: Data.self) {
                    _ = try await notebook.attachPhoto(id: note.id, photo: [UInt8](data))
                }
            }
            picked = nil
        }
    }

    private func run(_ work: @escaping () async throws -> Void) {
        Task {
            do { try await work(); onProblem(nil) } catch { onProblem(error.localizedDescription) }
        }
    }
}

/// A photo of a note: the core reads the bytes from its `Fs` port, the view draws them.
struct PhotoThumb: View {
    let path: String
    @State private var image: UIImage?

    var body: some View {
        Group {
            if let image { Image(uiImage: image).resizable().scaledToFill() } else { Color.secondary.opacity(0.2) }
        }
        .frame(width: 56, height: 56)
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .task { image = (try? await readPhoto(path: path)).flatMap { UIImage(data: Data($0)) } }
    }
}
