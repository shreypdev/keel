import SwiftUI
import UndraRuntime

/// What `undra dev` said about its last reload ("Reloaded, state kept", ADR-053): `DevStatusBar` shows it for a few
/// seconds. Feed it from `LoadOptions.onDevNotice` (see `UndraBootstrap`); the callback only ever fires for a core that
/// `undra dev` serves.
@MainActor
final class DevNotice: ObservableObject {
    static let shared = DevNotice()

    /// The message to show, or `nil` once its few seconds are over.
    @Published private(set) var message: String?
    private var shown = 0

    func show(_ text: String) {
        message = text
        shown += 1
        let mine = shown
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 4_000_000_000)
            if mine == shown {
                message = nil
            }
        }
    }
}

/// The connection to `undra dev` as a thin bar above the screens: green while connected, orange while the runtime
/// reconnects, red when the connection is over, and for a few seconds what the dev server says about a reload.
/// Nothing at all for the in-process core.
struct DevStatusBar: View {
    var body: some View {
        if let url = UndraBootstrap.devURL, let core = UndraBootstrap.core {
            DevStatusLine(url: url, connection: core.connectionObject)
        }
    }

    /// What a user of `undra dev` reads about the connection, in one line.
    static func describe(_ url: String, _ state: UndraConnectionState) -> String {
        switch state {
        case .connecting:
            return "Connecting to \(url)"
        case .connected:
            return "Dev server: \(url)"
        case .reconnecting(let attempt):
            return "Reconnecting to \(url) (attempt \(attempt))"
        case .closed(.sessionLost):
            return "The core was rebuilt: loading the new one"
        case .closed(.schemaMismatch):
            return "The schema changed, state reset: run undra bindgen and rebuild the app"
        case .closed(.requested):
            return "Disconnected"
        case .closed(.failed(let reason)):
            return "Connection failed: \(reason)"
        }
    }

    fileprivate static func color(_ state: UndraConnectionState) -> Color {
        switch state {
        case .connected:
            return Color(red: 0.18, green: 0.49, blue: 0.2)
        case .closed:
            return Color(red: 0.78, green: 0.16, blue: 0.16)
        default:
            return Color(red: 0.94, green: 0.42, blue: 0.0)
        }
    }
}

/// The bar itself. `core.connectionObject` is an `ObservableObject` (the observation-based `core.connection` needs
/// iOS 17), so `@ObservedObject` is all the code it takes.
private struct DevStatusLine: View {
    let url: String
    @ObservedObject var connection: UndraConnectionObject
    @ObservedObject private var notice = DevNotice.shared

    var body: some View {
        Text(notice.message ?? DevStatusBar.describe(url, connection.state))
            .font(.caption)
            .foregroundStyle(.white)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 12)
            .padding(.vertical, 3)
            .background(DevStatusBar.color(connection.state))
            .accessibilityIdentifier("dev-status")
    }
}
