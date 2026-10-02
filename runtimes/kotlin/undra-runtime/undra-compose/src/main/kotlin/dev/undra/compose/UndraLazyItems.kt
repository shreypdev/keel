package dev.undra.compose

import androidx.compose.foundation.lazy.LazyItemScope
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.runtime.Composable
import androidx.compose.runtime.MutableLongState
import androidx.compose.runtime.mutableLongStateOf
import dev.undra.runtime.UndraLazyList
import java.util.WeakHashMap

/**
 * Draws a lazily paged [list] (a `Lazy<T>` signal of a generated store) in a `LazyColumn` or `LazyRow`: one item per
 * row, [UndraLazyList.size] of them, however large the list is. [itemContent] gets the row's index and the row, or
 * `null` while its page is being loaded (draw a placeholder). Composing an item reads the row, which requests its page
 * and one page of prefetch on each side; nothing blocks.
 *
 * ```kotlin
 * LazyColumn {
 *     items(library.books) { index, book ->
 *         if (book != null) BookRow(book) else BookPlaceholder()
 *     }
 * }
 * ```
 *
 * The items recompose by themselves when the list's length changes, when a page arrives and when the core changes the
 * list and the window is re-paged; there is no state to collect. The index is the item's key: rows are positions, so
 * scroll state follows the position, as it does for `items(count)`.
 *
 * @param list the list to draw.
 * @param itemContent the content of one row.
 */
public fun <T> LazyListScope.items(
    list: UndraLazyList<T>,
    itemContent: @Composable LazyItemScope.(index: Int, item: T?) -> Unit,
) {
    // Reading the list's change counter in the builder makes the lazy layout run this builder again, and so recompose
    // its items, whenever the list's length or rows change (a StateFlow is not snapshot state, so it alone would not).
    changeTick(list).longValue
    items(
        count = list.size.value,
        key = { index -> index },
    ) { index ->
        itemContent(index, list[index])
    }
}

/**
 * The snapshot state that counts the changes of a list. It is made on first use and kept as long as the list is: the
 * list holds the listener that moves it, the map holds the list weakly, and the state never refers back to the list.
 */
internal fun changeTick(list: UndraLazyList<*>): MutableLongState =
    synchronized(ticks) {
        ticks.getOrPut(list) {
            val tick = mutableLongStateOf(0L)
            // Not closed: the listener lives and dies with the list. It captures the state only.
            list.addChangeListener { tick.longValue++ }
            tick
        }
    }

private val ticks = WeakHashMap<UndraLazyList<*>, MutableLongState>()
