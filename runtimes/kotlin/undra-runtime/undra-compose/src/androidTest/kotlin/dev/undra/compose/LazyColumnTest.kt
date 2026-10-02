package dev.undra.compose

import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.unit.dp
import androidx.test.ext.junit.runners.AndroidJUnit4
import dev.undra.runtime.InfiniteQuery
import dev.undra.runtime.Mode
import dev.undra.runtime.UndraCore
import dev.undra.runtime.UndraLazyList
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Handle
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The Compose helpers drawn for real, on a device: rows appear as pages arrive, the list follows a change of the core,
 * and the next page of an infinite query is fetched when the list is scrolled near its end.
 */
@RunWith(AndroidJUnit4::class)
class LazyColumnTest {
    @get:Rule
    val rule = createComposeRule()

    /** A core in process serving pages of `Int` rows: row `i` is `base + i`. */
    private class PagedCore : UndraCore() {
        @Volatile var total = 500
        @Volatile var version = 1uL
        @Volatile var base = 0
        val pageCalls = AtomicInteger()

        override val mode: Mode get() = Mode.INPROC

        override fun callSync(target: CallTarget, methodId: UInt, args: ByteArray): ByteArray {
            val page = target as CallTarget.LazyListPage
            pageCalls.incrementAndGet()
            val count = maxOf(0, minOf(page.limit.toInt(), total - page.offset.toInt()))
            val w = UndraWriter()
            Payloads.LazyPageHeader(version, total.toUInt(), count.toUInt()).encode(w)
            for (i in 0 until count) Codecs.i32.encode(w, base + page.offset.toInt() + i)
            return w.toByteArray()
        }
    }

    private fun hasText(text: String): Boolean = rule.onAllNodesWithText(text).fetchSemanticsNodes().isNotEmpty()

    @Test
    fun rowsAppearAsPagesArriveAndTheListFollowsTheCore() {
        val core = PagedCore()
        val list = UndraLazyList(core, Codecs.i32)
        rule.runOnUiThread {
            list.applyFull(UndraReader(Payloads.LazyValue(Handle(7L), 500u, 1uL).toByteArray()))
        }
        rule.setContent {
            LazyColumn(Modifier.height(300.dp)) {
                items(list) { index, row -> BasicText(if (row == null) "loading $index" else "row $row") }
            }
        }
        rule.waitUntil(10_000) { hasText("row 0") && hasText("row 3") }
        // Only a screenful and a page of prefetch each side was asked for, never the 500 rows.
        assert(core.pageCalls.get() in 1..4) { "pages requested: ${core.pageCalls.get()}" }

        // The core grows the list and changes every row: the length follows at once, the window is re-paged.
        core.total = 600
        core.version = 2uL
        core.base = 1000
        rule.runOnUiThread { list.applyInvalidated(UndraReader(Payloads.LazyInvalidated(600u, 2uL).toByteArray())) }
        rule.waitUntil(10_000) { hasText("row 1000") && hasText("row 1003") }
        assertEquals(600, list.size.value)
        list.close()
    }

    @Test
    fun theNextPageIsFetchedWhenTheListIsScrolledNearItsEnd() {
        val query = object : InfiniteQuery {
            val next = MutableStateFlow(true)
            val fetching = MutableStateFlow(false)
            val fetches = AtomicInteger()
            override val hasNextPage: StateFlow<Boolean> get() = next
            override val fetchingNextPage: StateFlow<Boolean> get() = fetching
            override fun fetchNextPage() {
                fetches.incrementAndGet()
                fetching.value = true
            }
        }
        lateinit var state: androidx.compose.foundation.lazy.LazyListState
        rule.setContent {
            state = rememberLazyListState()
            LazyColumn(Modifier.height(200.dp), state = state) {
                items(count = 100) { BasicText("item $it") }
            }
            state.LoadMoreWhenNearEnd(query, threshold = 5)
        }
        rule.waitForIdle()
        assertEquals("nothing is fetched at the top of a long list", 0, query.fetches.get())

        rule.runOnIdle { runBlocking { state.scrollToItem(97) } }
        rule.waitUntil(10_000) { query.fetches.get() == 1 }

        // While the page is on its way nothing more is asked for, however long the list stays near its end.
        rule.runOnIdle { runBlocking { state.scrollToItem(98) } }
        rule.waitForIdle()
        assertEquals(1, query.fetches.get())

        // The page arrived and there is no next one.
        query.fetching.value = false
        query.next.value = false
        rule.waitForIdle()
        assertEquals(1, query.fetches.get())
    }
}
