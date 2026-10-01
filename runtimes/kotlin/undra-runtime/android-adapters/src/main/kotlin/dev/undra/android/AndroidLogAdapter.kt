package dev.undra.android

import android.util.Log
import dev.undra.runtime.PortImpl
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.wire.UndraReader

/**
 * The `Log` port over `android.util.Log`: each record of the core goes to logcat with the record's target as the tag
 * (`undra` when empty) and the message as the text.
 *
 * Levels map one to one: 0 trace to verbose, 1 debug to debug, 2 info to info, 3 warn to warn, 4 error to error and
 * 5 fatal to assert (written with `Log.println`, so it never calls `Log.wtf`, which can end the process). Unlike
 * `java.util.logging`, which the runtime's default `Log` adapter uses and whose `FINE` and `FINER` records Android
 * drops, this keeps trace and debug records; filter them in logcat (`adb logcat undra_query:I *:S`).
 *
 * @param minLevel records below this level (0 to 5) are dropped; the default keeps everything.
 */
public class AndroidLogAdapter(private val minLevel: UByte = 0u) {
    /** Writes one record. */
    public fun log(level: UByte, target: String, message: String) {
        if (level < minLevel) return
        Log.println(priority(level), target.ifEmpty { DEFAULT_TAG }, message)
    }

    /** This adapter as a sync [PortImpl] for [StandardPorts.Log]. */
    public fun portImpl(): PortImpl = PortImpl(
        sync = true,
        methods = portMethods {
            this[StandardPorts.Log.LOG] = { args ->
                val reader = UndraReader(args)
                val level = reader.readU8()
                val target = reader.readStr()
                val message = reader.readStr()
                reader.finish()
                log(level, target, message)
                NO_REPLY
            }
        },
    )

    internal companion object {
        const val DEFAULT_TAG = "undra"

        /** The `android.util.Log` priority of an Undra level (SPEC section 8: 0 trace ... 5 fatal); an unknown level is info. */
        fun priority(level: UByte): Int =
            when (level.toInt()) {
                0 -> Log.VERBOSE
                1 -> Log.DEBUG
                2 -> Log.INFO
                3 -> Log.WARN
                4 -> Log.ERROR
                5 -> Log.ASSERT
                else -> Log.INFO
            }
    }
}
