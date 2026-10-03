# undra-compose

The optional Compose helpers of the Kotlin runtime (ADR-043): the only Gradle module that depends on Compose.
`:runtime` stays Kotlin stdlib + kotlinx-coroutines, and an app that draws its lists another way never pulls this in.
Coordinates `dev.undra:undra-compose:0.1.0-SNAPSHOT` (a composite build resolves them to this module, as the playground
does), and `com.github.shreypdev.undra:undra-compose:v<version>` from JitPack at a release (ADR-063); minSdk 26. Like `:android-adapters` it is included by `settings.gradle.kts` only when an Android SDK is found.
It depends on `compose-foundation` (`LazyListScope` is in its public API) and `compose-runtime`, versions from the Compose
BOM of `gradle/libs.versions.toml` (an app's own BOM wins). No ProGuard rules: nothing here is reflective.

## A lazily paged list: `items(list)`

A `Lazy<T>` signal of a generated store is an `UndraLazyList<T>` (see the runtime README): the core keeps the rows, the app
holds a window. Draw it with `items(list)`:

```kotlin
LazyColumn {
    items(library.books) { index, book ->            // book: Book?, null while its page loads
        if (book != null) BookRow(book) else BookPlaceholder()
    }
}
```

One item per row, `list.size` of them however large the list is; composing an item reads the row, which requests its page
and one page of prefetch on each side (it never blocks). The items recompose by themselves when the length changes, when a
page arrives and when the core changes the list and the window is re-paged: the builder reads a snapshot-state counter that
the list's change listener moves, so there is no `collectAsState` to forget. The index is the item's key.

## An infinite query: `LoadMoreWhenNearEnd`

```kotlin
val state = rememberLazyListState()
val feed = remember { FeedQuery(filter) }                    // generated; implements InfiniteQuery
val posts by feed.data.collectAsState()
LazyColumn(state = state) { items(posts, key = { it.id }) { PostRow(it) } }
state.LoadMoreWhenNearEnd(feed)                               // threshold = 5 items before the end
```

It calls `feed.fetchNextPage()` when the last visible item is within `threshold` items of the end, there is a next page and none
is on its way, and again after a page arrived if the list is still near its end (a short list fills up).

## Tests

`./gradlew :undra-compose:testDebugUnitTest` runs the logic of both helpers on the JVM (where "near the end" starts, when a
fetch is due, that the builder subscribes to the change counter and that the counter follows the list).
`./gradlew :undra-compose:connectedDebugAndroidTest` draws a real `LazyColumn` on an emulator or device: rows appear as pages
arrive, the list follows a change of the core, and the next page is fetched exactly once when scrolled near the end.
