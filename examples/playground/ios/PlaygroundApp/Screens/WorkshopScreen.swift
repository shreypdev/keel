import PlaygroundCore
import SwiftUI

/// What the core says while the workshop runs a job: the app's own `Reporter` (ADR-041). The core
/// calls it on the main actor, after applying the store changes it made before each call, so the
/// screen can show it as it is.
@MainActor @Observable
final class JobReporter: Reporter {
    private(set) var done: UInt32 = 0
    private(set) var total: UInt32 = 0
    private(set) var lines: [String] = []
    /// What `confirm` answers: whether a job counts as done.
    var goOn = true

    func progress(done: UInt32, total: UInt32) {
        self.done = done
        self.total = total
    }

    func note(line: String) {
        lines.append(line)
        if lines.count > 8 {
            lines.removeFirst(lines.count - 8)
        }
    }

    func confirm(question: String) async throws(ReportError) -> Bool {
        return goOn
    }
}

/// Two shelves the workshop hands out (child stores, ADR-040) and passes back to it: stock one,
/// merge it into the other. A job reports to the app's `JobReporter` while it runs.
struct WorkshopScreen: View {
    let workshop: Workshop
    let left: Shelf
    let right: Shelf
    @State private var reporter = JobReporter()
    /// The subscription that keeps the reporter hearing `announce` while the screen is shown.
    @State private var watch: Watch?
    @State private var outcome = ""

    var body: some View {
        NavigationStack {
            List {
                Section("Shelves") {
                    shelfRow(left)
                    shelfRow(right)
                    Button("Merge \(left.label) into \(right.label)") {
                        workshop.merge(from: left, onto: right)
                    }
                    .accessibilityIdentifier("workshop-merge")
                }
                Section("Job") {
                    Toggle("Go on when asked", isOn: $reporter.goOn)
                    Button("Run a job of 5 steps") {
                        Task { await runJob() }
                    }
                    .accessibilityIdentifier("workshop-run")
                    if reporter.total > 0 {
                        ProgressView(value: Double(reporter.done), total: Double(reporter.total))
                    }
                    Text("\(workshop.jobs) jobs, \(workshop.notes) notes")
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("workshop-counts")
                    if !outcome.isEmpty {
                        Text(outcome).accessibilityIdentifier("workshop-outcome")
                    }
                }
                Section("Notes") {
                    Button("Announce") {
                        _ = try? workshop.announce(line: "hello from iOS")
                    }
                    ForEach(Array(reporter.lines.enumerated()), id: \.offset) { _, line in
                        Text(line).font(.footnote.monospaced())
                    }
                }
            }
            .navigationTitle("Workshop")
            .navigationBarTitleDisplayMode(.inline)
            .onAppear {
                watch = try? workshop.watch(reporter: reporter)
            }
            .onDisappear {
                // Closing the subscription is what lets the core drop the reporter.
                watch?.close()
                watch = nil
            }
        }
    }

    private func shelfRow(_ shelf: Shelf) -> some View {
        HStack {
            Text(shelf.label)
            Spacer()
            Text("\(shelf.items) items").monospacedDigit().foregroundStyle(.secondary)
            Button("+3") { shelf.stock(count: 3) }
                .buttonStyle(.bordered)
                .accessibilityIdentifier("workshop-stock-\(shelf.label)")
        }
    }

    private func runJob() async {
        do {
            let steps = try await workshop.run(steps: 5, reporter: reporter)
            outcome = "Ran \(steps) steps"
        } catch ReportError.declined {
            outcome = "Declined"
        } catch is CancellationError {
            outcome = ""
        } catch {
            outcome = error.localizedDescription
        }
    }
}
