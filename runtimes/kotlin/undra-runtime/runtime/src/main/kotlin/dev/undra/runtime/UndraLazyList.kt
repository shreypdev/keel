package dev.undra.runtime

import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.UndraReader
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.coroutines.EmptyCoroutineContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

/**
 * A list the host pages through (ADR-043 decision 3): the platform half of a `Lazy<T>` signal. The core keeps all the
 * items; this class holds the list's length, a window of decoded pages and asks the core for the rest on demand, so a
 * 100,000-row list costs a screenful of memory and one page of work per change.
 *
 * Generated stores hold one per `Lazy<T>` signal; you read it:
 *
 * ```kotlin
 * val books: UndraLazyList<Book> = library.books
 * val count by books.size.collectAsState()
 * val rows = books[0]                       // the first Book, or null while its page loads
 * ```
 *
 * In Compose use the `items(list)` helper of the optional `undra-compose` module, which also recomposes when pages
 * arrive. Anywhere else, observe [size] and [revision].
 *
 * ### What reading does
 *
 * [get] returns the cached row, or `null` while its page is not here, and **requests** the page that holds the row and
 * one page on each side of it (prefetch), once. Nothing blocks: the requests made within one main-thread turn go out
 * together in the next one, as page calls ([CallTarget.LazyListPage]), through `callSync` where the core is in
 * process and as suspending calls otherwise, and a page already being fetched is not asked for again. An index
 * outside `0 until size` returns `null` and requests nothing.
 *
 * ### Versions and changes
 *
 * Every page reply carries the version of the list it was read at. A reply older than the list's current version is
 * dropped and the page asked for again; a newer one raises the list's version and [size] (the invalidation that
 * follows it then changes nothing). When the core changes the list it sends the new length and version
 * ([applyInvalidated]): [size] changes at once, the cached rows stay visible (stale) and only the **window** is
 * re-paged, the pages read since the previous invalidation, so a change costs O(window), never O(list), and a list
 * nobody reads costs nothing. At most [maxCachedPages] pages are kept (the least recently read go first). When the core
 * is restored the next value carries a new page server and the cache is dropped.
 *
 * ### Threads
 *
 * Reading ([get], [prefetch]) is safe from any thread and meant for the main thread (Compose). Pages are requested and
 * their replies applied on the main thread ([UndraDispatchers.main]); [applyFull] and [applyInvalidated] are called
 * there by the mirror. A page that cannot be fetched or does not decode is reported through [UndraCore.report] and
 * leaves its rows `null`; it never throws into the caller of [get]. A page whose request failed is not asked for again
 * by [get] until the list changes or [prefetch] asks for it.
 *
 * @param core the core the list lives in.
 * @param codec how one item is decoded.
 */
public class UndraLazyList<T> internal constructor(
    private val core: UndraCore,
    private val codec: UndraCodec<T>,
    private val post: (Runnable) -> Unit,
    private val dispatcher: () -> CoroutineDispatcher,
) : AutoCloseable {

    /** A list over [core] whose items are decoded with [codec]; this is what a generated store creates. */
    public constructor(core: UndraCore, codec: UndraCodec<T>) : this(
        core,
        codec,
        { task -> UndraDispatchers.main.dispatch(EmptyCoroutineContext, task) },
        { UndraDispatchers.main },
    )

    /** What one page request carries: the page, and which page server and generation of the cache it was made for. */
    private class Request(val page: Int, val handle: Long, val epoch: Int, val offset: Int, val limit: Int)

    /** A decoded page and the version of the list it was read at. */
    private class Loaded<T>(val rows: List<T>, val version: ULong)

    private val lock = Any()
    private val length = MutableStateFlow(0)
    private val revisions = MutableStateFlow(0L)
    private val listeners = CopyOnWriteArrayList<() -> Unit>()

    // ---- everything below is guarded by `lock` ----
    private var handle = 0L
    private var known = 0uL
    private var epoch = 0
    private var pageRows = DEFAULT_PAGE_SIZE
    private var maxPages = DEFAULT_MAX_CACHED_PAGES
    private var closed = false
    private var flushPosted = false

    /** Decoded pages, least recently read first. */
    private val loaded = LinkedHashMap<Int, Loaded<T>>(16, 0.75f, true)

    /** The pages read since the last invalidation, least recently read first: what an invalidation re-pages. */
    private val window = LinkedHashSet<Int>()

    /** Pages to ask the core for in the next turn. */
    private val wanted = LinkedHashSet<Int>()
    private val inFlight = HashSet<Int>()

    /** Pages whose last request failed: `get` leaves them alone until the list changes or `prefetch` asks. */
    private val failed = HashSet<Int>()
    private val staleReplies = HashMap<Int, Int>()

    private val flushTask = Runnable { flush() }

    /** Where asynchronous page calls run and resume: created by the first one. */
    private val asyncScope = lazy { CoroutineScope(SupervisorJob() + dispatcher()) }

    /** The number of items: the length of the last value or invalidation the core sent (0 before the first). */
    public val size: StateFlow<Int> get() = length

    /**
     * Bumped whenever what a read of [get] would return may have changed: a page arrived or was replaced, the length
     * changed, or the cache was dropped. A view that read rows reads them again when it changes (it is how Compose
     * recomposes).
     */
    public val revision: StateFlow<Long> get() = revisions

    /**
     * The version of the core's list the last value, invalidation or page reply told this list (`0` before the first value).
     * It increases with every change the core makes; a page reply read at an older version is dropped.
     */
    public val version: ULong get() = synchronized(lock) { known }

    /**
     * Items per page, default 50. Changing it drops the cached pages (they are cut differently), so set it before
     * the list is read.
     *
     * @throws IllegalArgumentException when set outside `1..65536`.
     */
    public var pageSize: Int
        get() = synchronized(lock) { pageRows }
        set(value) {
            require(value in 1..MAX_PAGE_SIZE) { "pageSize must be in 1..$MAX_PAGE_SIZE, was $value" }
            synchronized(lock) {
                if (value == pageRows) return
                pageRows = value
                dropCache()
                bump()
            }
            notifyChanged()
        }

    /**
     * The most pages kept at once, default 24. The window an invalidation re-pages never holds more. Below 3 the
     * prefetch of a read cannot be kept.
     *
     * @throws IllegalArgumentException when set below 1.
     */
    public var maxCachedPages: Int
        get() = synchronized(lock) { maxPages }
        set(value) {
            require(value >= 1) { "maxCachedPages must be at least 1, was $value" }
            synchronized(lock) {
                maxPages = value
                trim()
            }
        }

    /**
     * The item at [index], or `null` while its page is being loaded, when the list is empty or [index] is not in
     * `0 until size`. Requests the page that holds it and one page on each side, unless they are cached and current
     * or already requested; an index outside the list requests nothing. Never blocks and never throws.
     */
    public operator fun get(index: Int): T? {
        val row: T?
        synchronized(lock) {
            if (closed || handle == 0L || index < 0 || index >= length.value) return null
            val page = index / pageRows
            val last = lastPage()
            for (p in maxOf(0, page - 1)..minOf(last, page + 1)) read(p)
            row = loaded[page]?.rows?.getOrNull(index - page * pageRows)
        }
        scheduleFlush()
        return row
    }

    /**
     * Requests the pages that hold [range] (clamped to the list; at most [maxCachedPages] pages from its start are
     * kept) and asks again for pages whose last request failed. Use it to warm the cache before the rows are drawn.
     */
    public fun prefetch(range: IntRange) {
        synchronized(lock) {
            if (closed || handle == 0L || range.isEmpty()) return
            val from = maxOf(range.first, 0)
            val to = minOf(range.last, length.value - 1)
            if (from > to) return
            val first = from / pageRows
            val last = minOf(to / pageRows.toLong(), first.toLong() + maxPages - 1).toInt()
            for (p in first..last) {
                failed.remove(p)
                read(p)
            }
        }
        scheduleFlush()
    }

    /**
     * Applies the signal's `Full` value, a [Payloads.LazyValue] (generated stores call this from `apply`, on the main
     * thread, and it finishes [reader]). A page server the list has not seen (the first value, or after a restore)
     * drops the cache; the same page server with a newer version or another length is an invalidation; anything else
     * changes nothing.
     *
     * @throws dev.undra.runtime.wire.WireException if the value does not decode.
     * @throws UndraProtocolException if the page server is the null handle or the length does not fit an `Int`.
     */
    public fun applyFull(reader: UndraReader) {
        val value = Payloads.LazyValue.decode(reader)
        reader.finish()
        if (value.handle.isNull) throw UndraProtocolException("a lazy list's page server is the null handle")
        val len = checkedLength(value.len)
        var changed = false
        synchronized(lock) {
            if (closed) return
            if (value.handle.raw != handle) {
                dropCache()
                handle = value.handle.raw
                known = value.version
                length.value = len
                bump()
                changed = true
            } else {
                if (failed.isNotEmpty()) {
                    // Observed again with nothing new (a reconnect): let the rows that failed to load be asked for.
                    failed.clear()
                    bump()
                    changed = true
                }
                if (value.version > known || (value.version == known && len != length.value)) {
                    advance(value.version, len)
                    changed = true
                }
            }
        }
        if (changed) notifyChanged()
        scheduleFlush()
    }

    /**
     * Applies an invalidation, a [Payloads.LazyInvalidated] (change-set op 2; generated stores call this from `apply`
     * on the main thread, and it finishes [reader]): [size] takes the new length at once, the cached rows stay visible
     * and the window is re-requested. An invalidation that is not newer than what a page reply already told the list
     * changes nothing.
     *
     * @throws dev.undra.runtime.wire.WireException if the value does not decode.
     * @throws UndraProtocolException if the length does not fit an `Int`.
     */
    public fun applyInvalidated(reader: UndraReader) {
        val value = Payloads.LazyInvalidated.decode(reader)
        reader.finish()
        val len = checkedLength(value.len)
        var changed = false
        synchronized(lock) {
            if (closed || handle == 0L) return
            if (value.version > known || (value.version == known && len != length.value)) {
                advance(value.version, len)
                changed = true
            }
        }
        if (changed) notifyChanged()
        scheduleFlush()
    }

    /**
     * Calls [listener] after [size] or [revision] changed, on the thread that changed them (the main thread in
     * practice), outside the list's lock. For UI toolkits that track changes their own way: the `undra-compose` module
     * bridges to Compose's snapshot state with it. Closing the returned handle removes the listener. A listener that
     * throws is logged and the others still run.
     */
    public fun addChangeListener(listener: () -> Unit): AutoCloseable {
        listeners.add(listener)
        return AutoCloseable { listeners.remove(listener) }
    }

    /** Stops requesting pages, drops the cache and ignores what is still in flight. Idempotent; later reads return `null`. */
    override fun close() {
        synchronized(lock) {
            if (closed) return
            closed = true
            dropCache()
        }
        listeners.clear()
        if (asyncScope.isInitialized()) asyncScope.value.cancel()
    }

    // ---- the cache (call with `lock` held) -----------------------------------------------------------

    private fun lastPage(): Int = (length.value - 1) / pageRows

    /** Marks [page] read: it joins the window, becomes the most recently used and is requested if it is not current. */
    private fun read(page: Int) {
        window.remove(page)
        window.add(page)
        if (window.size > maxPages) {
            // Scrolled past: a page that left the window within this turn is not worth asking for any more.
            val gone = window.first()
            window.remove(gone)
            wanted.remove(gone)
        }
        loaded[page] // refreshes the page's place in the least-recently-used order
        want(page)
    }

    private fun want(page: Int) {
        val current = loaded[page]?.let { it.version >= known } ?: false
        if (!current && page !in inFlight && page !in failed) wanted.add(page)
    }

    private fun bump() {
        revisions.value = revisions.value + 1
    }

    /** Forgets every page and request: they belong to a page server or a page size that is gone. */
    private fun dropCache() {
        epoch++
        loaded.clear()
        window.clear()
        wanted.clear()
        inFlight.clear()
        failed.clear()
        staleReplies.clear()
    }

    /** Takes a newer version or another length: the window is re-requested and the rows stay (stale) until it arrives. */
    private fun advance(newVersion: ULong, newLength: Int) {
        known = newVersion
        length.value = newLength
        failed.clear()
        staleReplies.clear()
        val previous = window.toList()
        window.clear()
        // Pages past the new end are of no use; the last page may now be shorter or longer.
        val end = if (newLength == 0) 0 else lastPage() + 1
        loaded.keys.removeAll { it >= end }
        for (page in previous) if (page < end) want(page)
        bump()
    }

    /** Keeps at most [maxPages] pages, evicting the least recently read, those outside the window first. */
    private fun trim() {
        while (loaded.size > maxPages) {
            val victim = loaded.keys.firstOrNull { it !in window } ?: loaded.keys.first()
            loaded.remove(victim)
            window.remove(victim)
        }
        while (window.size > maxPages) window.remove(window.first())
    }

    // ---- requests -------------------------------------------------------------------------------------

    /** Posts the turn's flush when something is wanted and none is posted yet. Never called with `lock` held. */
    private fun scheduleFlush() {
        synchronized(lock) {
            if (closed || flushPosted || wanted.isEmpty()) return
            flushPosted = true
        }
        try {
            post(flushTask)
        } catch (e: Exception) {
            synchronized(lock) { flushPosted = false }
            UndraLog.warn("an UndraLazyList could not schedule its page requests", e)
        }
    }

    /** One turn's requests, together: every wanted page that is still needed and not already asked for. */
    private fun flush() {
        val batch = ArrayList<Request>()
        synchronized(lock) {
            flushPosted = false
            if (closed || handle == 0L) {
                wanted.clear()
                return
            }
            for (page in wanted) {
                val offset = page.toLong() * pageRows
                if (offset >= length.value || page in inFlight) continue
                if (loaded[page]?.let { it.version >= known } == true) continue
                inFlight.add(page)
                batch.add(Request(page, handle, epoch, offset.toInt(), pageRows))
            }
            wanted.clear()
        }
        if (batch.isEmpty()) return
        val synchronous = isSynchronous()
        for (request in batch) {
            if (synchronous) requestSync(request) else requestAsync(request)
        }
    }

    private fun isSynchronous(): Boolean =
        try {
            core.mode == Mode.INPROC
        } catch (e: UnsupportedOperationException) {
            false
        }

    private fun target(request: Request): CallTarget =
        CallTarget.LazyListPage(Handle(request.handle), request.offset.toUInt(), request.limit.toUInt())

    private fun requestSync(request: Request) {
        val body = try {
            core.callSync(target(request), 0u, NO_BYTES)
        } catch (e: Exception) {
            requestFailed(request, e)
            return
        }
        received(request, body)
    }

    private fun requestAsync(request: Request) {
        asyncScope.value.launch {
            val body = try {
                core.call(target(request), 0u, NO_BYTES)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                requestFailed(request, e)
                return@launch
            }
            received(request, body)
        }
    }

    private fun requestFailed(request: Request, error: Throwable) {
        synchronized(lock) {
            if (closed || request.epoch != epoch) return
            inFlight.remove(request.page)
            failed.add(request.page)
        }
        core.report(error, "UndraLazyList.page(${request.page})")
    }

    /** Decodes and applies the reply to [request]; whatever is wrong with it is reported, not thrown. */
    private fun received(request: Request, body: ByteArray) {
        val header: Payloads.LazyPageHeader
        val rows: List<T>
        try {
            val reader = UndraReader(body)
            header = Payloads.LazyPageHeader.decode(reader)
            if (header.count > request.limit.toUInt()) {
                throw UndraProtocolException("a page of ${header.count} items answers a request for at most ${request.limit}")
            }
            if (header.total > Int.MAX_VALUE.toUInt()) {
                throw UndraProtocolException("a lazy list of ${header.total} items does not fit Kotlin's Int index")
            }
            val decoded = ArrayList<T>(header.count.toInt())
            for (i in 0 until header.count.toInt()) decoded.add(codec.decode(reader))
            reader.finish()
            rows = decoded
        } catch (e: Exception) {
            requestFailed(request, e)
            return
        }
        var problem: Throwable? = null
        var changed = false
        synchronized(lock) {
            if (closed || request.epoch != epoch) return
            inFlight.remove(request.page)
            val total = header.total.toInt()
            if (header.version < known) {
                // Read before the last change: dropped, and the page asked for again (a few times at most).
                val times = (staleReplies[request.page] ?: 0) + 1
                if (times > MAX_STALE_REPLIES) {
                    staleReplies.remove(request.page)
                    failed.add(request.page)
                    problem = UndraProtocolException("the core keeps answering page ${request.page} with a version older than $known")
                } else {
                    staleReplies[request.page] = times
                    wanted.add(request.page)
                }
            } else {
                staleReplies.remove(request.page)
                if (header.version > known) {
                    advance(header.version, total)
                    changed = true
                } else if (total != length.value) {
                    failed.add(request.page)
                    problem = UndraProtocolException("a page read at version $known says the list has $total items, not ${length.value}")
                }
                if (problem == null) {
                    val expected = minOf(request.limit.toLong(), length.value.toLong() - request.offset).toInt()
                    if (rows.size != maxOf(expected, 0)) {
                        failed.add(request.page)
                        problem = UndraProtocolException("page ${request.page} holds ${rows.size} items; expected $expected")
                    } else {
                        loaded[request.page] = Loaded(rows, header.version)
                        wanted.remove(request.page)
                        trim()
                        bump()
                        changed = true
                    }
                }
            }
        }
        if (changed) notifyChanged()
        problem?.let { core.report(it, "UndraLazyList.page(${request.page})") }
        scheduleFlush()
    }

    private fun checkedLength(len: UInt): Int {
        if (len > Int.MAX_VALUE.toUInt()) throw UndraProtocolException("a lazy list of $len items does not fit Kotlin's Int index")
        return len.toInt()
    }

    private fun notifyChanged() {
        for (listener in listeners) {
            try {
                listener()
            } catch (e: Exception) {
                UndraLog.warn("an UndraLazyList change listener failed", e)
            }
        }
    }

    // ---- for tests ---------------------------------------------------------------------------------

    /** The pages whose rows are cached, least recently read first. */
    internal fun cachedPages(): List<Int> = synchronized(lock) { loaded.keys.toList() }

    /** The pages an invalidation would re-page now, least recently read first. */
    internal fun windowPages(): List<Int> = synchronized(lock) { window.toList() }


    internal companion object {
        const val DEFAULT_PAGE_SIZE: Int = 50
        const val DEFAULT_MAX_CACHED_PAGES: Int = 24
        const val MAX_PAGE_SIZE: Int = 65_536

        /** How often a page is asked for again after replies older than the list's version. */
        const val MAX_STALE_REPLIES: Int = 4

        private val NO_BYTES = ByteArray(0)
    }
}
