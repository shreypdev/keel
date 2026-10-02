import PlaygroundCore
import SwiftUI

/// An infinite query (`FeedQueryHandle`, ADR-043): the core fetches the feed a page of 50 rows at a time, and `data` grows by that
/// page (the store applies a keyed patch of 50 inserts, SwiftUI sees 50 new rows). Each row asks `loadMore(ifNeededFor:)` when it
/// appears, which fetches the next page once the row is within five of the end; the footer shows `fetchingNextPage`; pulling the
/// list down is `refetch()`, which finds only the rows that changed ("Touch" in the toolbar makes the even rows change). "Even rows" is another
/// parameter of the query, so another cache entry with its own pages.
struct FeedScreen: View {
    let all: FeedQueryHandle
    let even: FeedQueryHandle
    @State private var evenOnly = false
    @State private var revision: UInt32 = 0

    private var feed: FeedQueryHandle {
        return evenOnly ? even : all
    }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Picker("Rows", selection: $evenOnly) {
                        Text("All").tag(false)
                        Text("Even rows").tag(true)
                    }
                    .pickerStyle(.segmented)
                    .accessibilityIdentifier("feed-filter")
                    LabeledContent("Status") {
                        Text(statusName).accessibilityIdentifier("feed-status")
                    }
                }
                Section("Feed") {
                    ForEach(feed.data) { item in
                        FeedRow(item: item)
                            .onAppear { feed.loadMore(ifNeededFor: item) }
                    }
                    footer
                }
            }
            .accessibilityIdentifier("feed-list")
            .refreshable { feed.refetch() }
            .navigationTitle("Feed")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Touch") {
                        revision += 1
                        touchFeed(revision: revision)
                        feed.refetch()
                    }
                    .accessibilityIdentifier("feed-touch")
                }
                ToolbarItem(placement: .principal) {
                    Text("\(feed.data.count) rows")
                        .font(.headline)
                        .accessibilityIdentifier("feed-count")
                }
            }
        }
    }

    @ViewBuilder
    private var footer: some View {
        HStack {
            if feed.fetchingNextPage {
                ProgressView()
                Text("Loading the next page")
            } else if feed.hasNextPage {
                Text("Scroll for more")
            } else {
                Text("That is all")
            }
        }
        .foregroundStyle(.secondary)
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("feed-footer")
    }

    private var statusName: String {
        switch feed.status {
        case .idle: "Idle"
        case .fetching: "Fetching"
        case .success: "Success"
        case .error: "Error"
        }
    }
}

/// One row of the feed: its identity, its label and its version (the even rows show the revision "Touch" set).
private struct FeedRow: View {
    let item: Item

    var body: some View {
        HStack {
            Text("#\(item.id)")
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 64, alignment: .leading)
            Text(item.label)
            Spacer()
            Text("v\(item.version)")
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("feed-row-\(item.id)")
    }
}
