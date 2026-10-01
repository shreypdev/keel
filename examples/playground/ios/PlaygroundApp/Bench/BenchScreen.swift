import SwiftUI

/// What `-bench` asked for. The playground starts in benchmark mode, instead of showing its screens, when it is launched with
/// `-bench full` (every row and the drain experiment) or `-bench cold` (one cold start: the first load of the process and
/// the restore of the snapshot a full run left behind); `-benchQuick YES` shortens a run to check the plumbing. This is
/// what `PlaygroundBenchTests` and `scripts/bench-device.sh --device ios` do; nothing else reads it.
enum BenchLaunch {
    enum Mode: String {
        case full, cold
    }

    /// The requested mode, or `nil` for the app itself.
    static var mode: Mode? {
        return UserDefaults.standard.string(forKey: "bench").flatMap(Mode.init(rawValue:))
    }

    /// Whether the run is the short one.
    static var quick: Bool {
        return UserDefaults.standard.bool(forKey: "benchQuick")
    }
}

/// The benchmark's only screen: a status line and the JSON the run produced, for the UI test to read (`bench-status` is
/// `running`, then `done` or `failed: ...`; `bench-result` is the JSON, in pieces of 400 characters `bench-result-0`,
/// `bench-result-1`, ... because an accessibility label is not the place for 10 KB).
struct BenchScreen: View {
    let mode: BenchLaunch.Mode
    let quick: Bool

    @State private var status = "running"
    @State private var pieces: [String] = []

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Undra device benchmark: \(mode.rawValue)\(quick ? " (quick)" : "")").font(.headline)
            Text(status).accessibilityIdentifier("bench-status")
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(pieces.enumerated()), id: \.offset) { index, piece in
                        Text(piece).font(.system(size: 9, design: .monospaced)).accessibilityIdentifier("bench-result-\(index)")
                    }
                }
            }
        }
        .padding()
        .task { await run() }
    }

    @MainActor
    private func run() async {
        do {
            let runner = BenchRunner(config: quick ? .quick : .full)
            let raw: [String: Any]
            switch mode {
            case .full: raw = try await runner.runFull()
            case .cold: raw = try runner.runCold()
            }
            let json = try benchJSON(raw)
            // The pieces first, then `done`: the test reads them once the status says so.
            pieces = stride(from: 0, to: json.count, by: 400).map { start in
                let from = json.index(json.startIndex, offsetBy: start)
                let to = json.index(from, offsetBy: min(400, json.count - start))
                return String(json[from ..< to])
            }
            status = "done"
        } catch {
            status = "failed: \(error)"
        }
    }
}
