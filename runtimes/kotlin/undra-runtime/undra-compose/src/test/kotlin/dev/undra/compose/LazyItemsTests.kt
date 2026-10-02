package dev.undra.compose

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.lazy.LazyItemScope
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.runtime.Composable
import androidx.compose.runtime.snapshots.Snapshot
import dev.undra.runtime.Mode
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraLazyList
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotSame
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** What `items(list)` hands to the lazy layout, and how the layout learns that the list changed. */
class LazyItemsTests {

    /** A core that serves pages of `Int` rows (row `i` is `i`) in process. */
    private class PagedCore(private val total: Int) : UndraCore() {
        val pages = ArrayList<Pair<UInt, UInt>>()

        override val mode: Mode get() = Mode.INPROC

        override fun callSync(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
            val page = target as CallTarget.LazyListPage
            pages.add(page.offset to page.limit)
            val count = maxOf(0, minOf(page.limit.toInt(), total - page.offset.toInt()))
            val w = UndraWriter()
            Payloads.LazyPageHeader(1uL, total.toUInt(), count.toUInt()).encode(w)
            for (i in 0 until count) Codecs.i32.encode(w, page.offset.toInt() + i)
            return w.toByteArray()
        }
    }

    /** A lazy layout scope that remembers what `items(count, key)` was given. */
    private class RecordingScope : LazyListScope {
        var count: Int? = null
        var key: ((Int) -> Any)? = null
        var itemCalls = 0

        override fun item(key: Any?, contentType: Any?, content: @Composable LazyItemScope.() -> Unit) {
            itemCalls++
        }

        override fun items(
            count: Int,
            key: ((index: Int) -> Any)?,
            contentType: (index: Int) -> Any?,
            itemContent: @Composable LazyItemScope.(index: Int) -> Unit,
        ) {
            this.count = count
            this.key = key
        }

        @ExperimentalFoundationApi
        override fun stickyHeader(key: Any?, contentType: Any?, content: @Composable LazyItemScope.() -> Unit) {
            itemCalls++
        }
    }

    private fun lazyListOf(core: UndraCore, length: Int): UndraLazyList<Int> {
        val list = UndraLazyList(core, Codecs.i32)
        list.applyFull(UndraReader(Payloads.LazyValue(Handle(7L), length.toUInt(), 1uL).toByteArray()))
        return list
    }

    @Test
    fun theLayoutGetsOneItemPerRowKeyedByItsIndex() {
        val list = lazyListOf(PagedCore(100_000), 100_000)
        val scope = RecordingScope()
        scope.items(list) { _, _ -> }
        assertEquals(100_000, scope.count)
        val key = scope.key!!
        assertEquals(0, key(0))
        assertEquals(99_999, key(99_999))
        assertEquals(0, scope.itemCalls)
        list.close()
    }

    @Test
    fun anEmptyListHasNoItems() {
        val list = UndraLazyList(PagedCore(0), Codecs.i32)
        val scope = RecordingScope()
        scope.items(list) { _, _ -> }
        assertEquals(0, scope.count)
        list.close()
    }

    @Test
    fun theBuilderReadsTheChangeCounterSoTheLayoutRebuildsWhenTheListChanges() {
        val list = lazyListOf(PagedCore(100), 100)
        val read = ArrayList<Any>()
        val snapshot = Snapshot.takeSnapshot(readObserver = { read.add(it) })
        try {
            snapshot.enter { RecordingScope().items(list) { _, _ -> } }
        } finally {
            snapshot.dispose()
        }
        assertTrue("the builder read the list's change counter", read.any { it === changeTick(list) })
        list.close()
    }

    @Test
    fun theCounterMovesWheneverTheLengthOrTheRowsChange() {
        val list = lazyListOf(PagedCore(100), 100)
        val tick = changeTick(list)
        assertSame(tick, changeTick(list))
        val start = tick.longValue
        list.applyInvalidated(UndraReader(Payloads.LazyInvalidated(101u, 2uL).toByteArray()))
        assertTrue(tick.longValue > start)
        val afterLength = tick.longValue
        list.applyInvalidated(UndraReader(Payloads.LazyInvalidated(101u, 2uL).toByteArray())) // nothing new
        assertEquals(afterLength, tick.longValue)
        list.pageSize = 10 // the pages are cut again
        assertTrue(tick.longValue > afterLength)
        list.close()
    }

    @Test
    fun everyListHasItsOwnCounter() {
        val a = lazyListOf(PagedCore(10), 10)
        val b = lazyListOf(PagedCore(10), 10)
        assertNotSame(changeTick(a), changeTick(b))
        a.close()
        b.close()
    }
}
