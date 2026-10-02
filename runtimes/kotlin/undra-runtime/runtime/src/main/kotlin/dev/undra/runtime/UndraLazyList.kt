package dev.undra.runtime

import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.UndraReader
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * A list the host pages through (ADR-043 decision 3): the platform half of a `Lazy<T>` signal. The core keeps the
 * items; this class holds the length, a window of decoded pages and fetches the rest on demand.
 *
 * ```kotlin
 * val books: UndraLazyList<Book> = library.books           // a property of the generated store
 * val count by books.size.collectAsState()
 * LazyColumn { items(books) { index, book -> if (book != null) BookRow(book) else Placeholder() } }
 * ```
 *
 * @param core the core the list lives in.
 * @param codec how one item is decoded.
 */
public class UndraLazyList<T>(private val core: UndraCore, private val codec: UndraCodec<T>) : AutoCloseable {
    private val length = MutableStateFlow(0)
    private val revisions = MutableStateFlow(0L)

    /** The number of items: the length of the last value or invalidation the core sent. */
    public val size: StateFlow<Int> get() = length

    /** Bumped whenever the rows or the length change, so a view that read [get] knows to read again. */
    public val revision: StateFlow<Long> get() = revisions

    /** Items per page. Default 50. */
    public var pageSize: Int = DEFAULT_PAGE_SIZE

    /** The most pages kept at once. Default 24. */
    public var maxCachedPages: Int = DEFAULT_MAX_CACHED_PAGES

    /** The item at [index], or `null` while its page loads or when [index] is out of range. */
    public operator fun get(index: Int): T? = null

    /** Requests the pages that hold [range]. */
    public fun prefetch(range: IntRange) {}

    /** Applies the list's `Full` value: a [Payloads.LazyValue]. Called by generated stores on the main thread. */
    public fun applyFull(reader: UndraReader) {
        val value = Payloads.LazyValue.decode(reader)
        reader.finish()
        length.value = value.len.toInt()
    }

    /** Applies an invalidation: a [Payloads.LazyInvalidated]. Called by generated stores on the main thread. */
    public fun applyInvalidated(reader: UndraReader) {
        val value = Payloads.LazyInvalidated.decode(reader)
        reader.finish()
        length.value = value.len.toInt()
    }

    /** Stops requesting pages and drops the cache. Idempotent. */
    override fun close() {}

    internal companion object {
        const val DEFAULT_PAGE_SIZE: Int = 50
        const val DEFAULT_MAX_CACHED_PAGES: Int = 24
    }
}
