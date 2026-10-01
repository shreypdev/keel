package dev.undra.android

import android.util.Log
import org.junit.Assert.assertEquals
import org.junit.Test

/** The mapping of Undra log levels (SPEC section 8) to logcat priorities. */
class AndroidLogAdapterTest {
    @Test
    fun the_six_levels_map_to_the_six_priorities() {
        assertEquals(Log.VERBOSE, AndroidLogAdapter.priority(0u))
        assertEquals(Log.DEBUG, AndroidLogAdapter.priority(1u))
        assertEquals(Log.INFO, AndroidLogAdapter.priority(2u))
        assertEquals(Log.WARN, AndroidLogAdapter.priority(3u))
        assertEquals(Log.ERROR, AndroidLogAdapter.priority(4u))
        assertEquals(Log.ASSERT, AndroidLogAdapter.priority(5u))
    }

    @Test
    fun an_unknown_level_is_info() {
        assertEquals(Log.INFO, AndroidLogAdapter.priority(6u))
        assertEquals(Log.INFO, AndroidLogAdapter.priority(255u))
    }
}
