package dev.undra.compose

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The decisions of `LazyListState.LoadMoreWhenNearEnd`, which need no device: where "near the end" starts and when a fetch is due. */
class LoadMoreTests {
    @Test
    fun theLastItemsAreNearTheEnd() {
        // 100 items, threshold 5: the last visible index must be 94 or more.
        assertFalse(isNearEnd(lastVisibleIndex = 93, totalItemsCount = 100, threshold = 5))
        assertTrue(isNearEnd(lastVisibleIndex = 94, totalItemsCount = 100, threshold = 5))
        assertTrue(isNearEnd(lastVisibleIndex = 99, totalItemsCount = 100, threshold = 5))
    }

    @Test
    fun aThresholdOfZeroMeansTheLastItemItself() {
        assertFalse(isNearEnd(lastVisibleIndex = 98, totalItemsCount = 100, threshold = 0))
        assertTrue(isNearEnd(lastVisibleIndex = 99, totalItemsCount = 100, threshold = 0))
    }

    @Test
    fun aListShorterThanTheThresholdIsAlwaysNearItsEnd() {
        assertTrue(isNearEnd(lastVisibleIndex = 0, totalItemsCount = 3, threshold = 5))
        assertTrue(isNearEnd(lastVisibleIndex = 2, totalItemsCount = 3, threshold = 5))
    }

    @Test
    fun nothingLaidOutOrNothingInTheListIsNotNearTheEnd() {
        assertFalse(isNearEnd(lastVisibleIndex = null, totalItemsCount = 100, threshold = 5))
        assertFalse(isNearEnd(lastVisibleIndex = null, totalItemsCount = 0, threshold = 5))
        assertFalse(isNearEnd(lastVisibleIndex = 0, totalItemsCount = 0, threshold = 5))
    }

    @Test
    fun aHugeThresholdDoesNotOverflow() {
        assertTrue(isNearEnd(lastVisibleIndex = 0, totalItemsCount = 100, threshold = Int.MAX_VALUE))
        assertTrue(isNearEnd(lastVisibleIndex = 0, totalItemsCount = Int.MAX_VALUE, threshold = Int.MAX_VALUE))
    }

    @Test
    fun aFetchIsDueOnlyNearTheEndWithANextPageAndNoneOnItsWay() {
        for (near in listOf(false, true)) for (hasNext in listOf(false, true)) for (fetching in listOf(false, true)) {
            assertEquals(
                "near=$near hasNext=$hasNext fetching=$fetching",
                near && hasNext && !fetching,
                shouldFetchNextPage(near, hasNext, fetching),
            )
        }
    }
}
