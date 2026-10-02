import PlaygroundCore
import SwiftUI

/// A polled query (`TickerQueryHandle`, ADR-043): a counter the core bumps on every fetch, fetched again a second after the last
/// fetch ended for as long as somebody watches it and the app is active. The screen is the observer: it creates the handle when it
/// appears and closes it when it goes, so leaving the tab stops the polling (the last observer stops it). The switches use what an
/// observer can ask for: `setPollInterval` for a faster poll than the query's own, and a failure the core makes on purpose (the
/// error shows, the polling goes on, and it clears with the next success).
struct TickerScreen: View {
    @State private var ticker: TickerQueryHandle?
    @State private var fast = false
    @State private var failing = false
    @State private var problem: String?

    var body: some View {
        NavigationStack {
            List {
                Section("Ticker") {
                    LabeledContent("Counter") {
                        Text(ticker?.data.map { "\($0)" } ?? "none yet")
                            .font(.title2.monospacedDigit())
                            .accessibilityIdentifier("ticker-value")
                    }
                    LabeledContent("Status") {
                        HStack(spacing: 8) {
                            if ticker?.fetching == true {
                                ProgressView().accessibilityIdentifier("ticker-fetching")
                            }
                            Text(statusName).accessibilityIdentifier("ticker-status")
                        }
                    }
                    LabeledContent("Updated") {
                        Text(ticker?.updatedAt?.formatted(date: .omitted, time: .standard) ?? "never")
                            .accessibilityIdentifier("ticker-updated")
                    }
                    if let error = ticker?.error {
                        Text(error.localizedDescription)
                            .foregroundStyle(.red)
                            .accessibilityIdentifier("ticker-error")
                    }
                }
                Section("What this observer asks for") {
                    Toggle("Poll every 250 ms", isOn: $fast)
                        .accessibilityIdentifier("ticker-fast")
                        .onChange(of: fast) { _, _ in applyInterval() }
                    Toggle("Fail on purpose", isOn: $failing)
                        .accessibilityIdentifier("ticker-failing")
                        .onChange(of: failing) { _, failing in setTickerFailing(failing: failing) }
                    Button("Refetch now") { ticker?.refetch() }
                        .accessibilityIdentifier("ticker-refetch")
                    if let problem {
                        Text(problem).font(.footnote).foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle("Ticker")
            .navigationBarTitleDisplayMode(.inline)
        }
        .onAppear { watch() }
        .onDisappear { stopWatching() }
    }

    private var statusName: String {
        switch ticker?.status ?? .idle {
        case .idle: "Idle"
        case .fetching: "Fetching"
        case .success: "Success"
        case .error: "Error"
        }
    }

    /// Becomes an observer of the ticker: the core fetches it now and polls it from then on.
    private func watch() {
        guard ticker == nil else {
            return
        }
        do {
            ticker = try TickerQueryHandle()
            problem = nil
            applyInterval()
        } catch {
            problem = error.localizedDescription
        }
    }

    /// Stops being an observer. With no observer left the core stops polling, and the switches go back to their defaults.
    private func stopWatching() {
        ticker?.close()
        ticker = nil
        fast = false
        if failing {
            failing = false
            setTickerFailing(failing: false)
        }
    }

    /// The observer's own interval: 250 ms while the switch is on, the query's own (one second) when it is off.
    private func applyInterval() {
        ticker?.setPollInterval(fast ? .milliseconds(250) : nil)
    }
}
