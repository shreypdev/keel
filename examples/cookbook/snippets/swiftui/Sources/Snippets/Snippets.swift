// The Swift lines the cookbook pages show. The `docs:begin` / `docs:end` markers delimit what a page
// quotes (site/scripts/build-cookbook.mjs copies it); everything outside them is scaffolding that
// makes the lines compile (`../../check.sh`).
import CookbookCore
import SwiftUI

// MARK: scaffolding

struct LoginView: View { let auth: Auth; var body: some View { EmptyView() } }
struct HomeView: View { let user: String; var body: some View { EmptyView() } }

// MARK: auth

// docs:begin auth-swift
struct Root: View {
    let auth: Auth

    var body: some View {
        switch auth.session {
        case .signedOut: LoginView(auth: auth)
        case .signedIn(let user): HomeView(user: user)
        }
    }
}

@MainActor func logIn(_ auth: Auth, email: String, password: String) async -> String? {
    do {
        try await auth.signIn(email: email, password: password)
        return nil
    } catch AuthError.badCredentials {
        return "Wrong email or password"
    } catch {
        return error.localizedDescription
    }
}
// docs:end

// MARK: paging

// docs:begin paging-swift
struct FeedView: View {
    let feed: Feed

    var body: some View {
        List(feed.visible, id: \.id) { post in
            Text(post.title)
                .task {
                    // The core ignores a call while a page is in flight or after the last page.
                    if post.id == feed.visible.last?.id { _ = try? await feed.loadMore() }
                }
        }
        .searchable(text: Binding(get: { feed.topic }, set: { feed.setTopic(topic: $0) }))
        .refreshable { _ = try? await feed.refresh() }
    }
}
// docs:end

// MARK: forms

// docs:begin forms-swift
struct SignUpView: View {
    let form: SignUp

    var body: some View {
        Form {
            TextField("Email", text: Binding(get: { form.email }, set: { form.setEmail(email: $0) }))
                .onSubmit { form.blur(.email) }
            ForEach(form.errors, id: \.message) { error in
                Text(error.message).foregroundStyle(.red)
            }
            Button("Create account") {
                Task {
                    do { _ = try await form.submit() }
                    catch SubmitError.emailTaken { /* point at the email field */ }
                    catch { /* Invalid: the errors above already say why */ }
                }
            }
            .disabled(!form.valid)
        }
    }
}
// docs:end

// MARK: upload

// docs:begin upload-swift
struct UploadsView: View {
    let uploads: Uploads

    var body: some View {
        ForEach(uploads.uploads, id: \.id) { row in
            ProgressView(value: Double(row.sent), total: Double(max(row.total, 1)))
            if case .failed(let reason) = row.state {
                Button("Retry (\(reason))") { Task { try? await uploads.retry(id: row.id) } }
            }
        }
    }
}
// docs:end

// MARK: offline

// docs:begin offline-swift
@MainActor func notesScreen() async throws {
    let notes = try NotesQueryHandle(list: "inbox")       // cached on disk: shown before the network answers
    _ = notes.data

    // Shows at once; returns when the server has it, even if that is after the train leaves the tunnel.
    _ = try await createNote(list: "inbox", text: "Buy milk", pinned: false)

    let waiting = try outbox().pending                    // "2 changes waiting to sync"
    _ = waiting
}
// docs:end
