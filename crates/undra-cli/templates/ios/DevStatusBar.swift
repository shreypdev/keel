import SwiftUI
import UndraRuntime

/// The connection to `undra dev` as a thin bar above the screens: green while connected, orange while the runtime
/// reconnects, red when the connection is over. Nothing at all for the in-process core.
///
/// `core.connection` is `@Observable`, so reading its `state` here is all the code it takes.
struct DevStatusBar: View {
    var body: some View {
        if let url = UndraBootstrap.devURL, let core = UndraBootstrap.core {
            let state = core.connection.state
            Text(Self.describe(url, state))
                .font(.caption)
                .foregroundStyle(.white)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 12)
                .padding(.vertical, 3)
                .background(Self.color(state))
                .accessibilityIdentifier("dev-status")
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
            return "The schema changed: run undra bindgen and rebuild the app"
        case .closed(.requested):
            return "Disconnected"
        case .closed(.failed(let reason)):
            return "Connection failed: \(reason)"
        }
    }

    private static func color(_ state: UndraConnectionState) -> Color {
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
