import PlaygroundCore
import SwiftUI

/// A counter whose value, tally of changes and parity label all live in the core. `parity` is a
/// computed value: the core derives it from `count` and sends it with the same change-set, so the
/// screen never shows a count and a parity that disagree.
struct CounterScreen: View {
    let counter: Counter

    var body: some View {
        NavigationStack {
            VStack(spacing: 24) {
                Spacer()
                Text("\(counter.count)")
                    .font(.system(size: 88, weight: .semibold, design: .rounded))
                    .monospacedDigit()
                    .contentTransition(.numericText(value: Double(counter.count)))
                    .animation(.snappy, value: counter.count)
                    .accessibilityIdentifier("counter-value")
                Text(counter.parity == .even ? "even" : "odd")
                    .font(.title3)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("counter-parity")
                HStack(spacing: 32) {
                    Button {
                        counter.decrement()
                    } label: {
                        Image(systemName: "minus.circle.fill").font(.system(size: 56))
                    }
                    .accessibilityLabel("Decrement")
                    .accessibilityIdentifier("counter-dec")
                    Button {
                        counter.increment()
                    } label: {
                        Image(systemName: "plus.circle.fill").font(.system(size: 56))
                    }
                    .accessibilityLabel("Increment")
                    .accessibilityIdentifier("counter-inc")
                }
                Text("\(counter.changes) changes")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("counter-changes")
                Spacer()
                Button("Reset", role: .destructive) { counter.reset() }
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("counter-reset")
            }
            .padding()
            .navigationTitle("Counter")
            .navigationBarTitleDisplayMode(.inline)
        }
    }
}
