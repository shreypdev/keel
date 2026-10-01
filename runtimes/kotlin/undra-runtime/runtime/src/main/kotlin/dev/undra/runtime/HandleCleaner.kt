package dev.undra.runtime

import java.lang.ref.PhantomReference
import java.lang.ref.ReferenceQueue
import java.util.concurrent.ConcurrentHashMap

/**
 * The backstop that releases a handle when its [UndraObject] is garbage collected without `close()`.
 *
 * It uses `java.lang.ref.Cleaner` where that exists (JVM 9+, Android 13+) and a phantom-reference queue
 * with one daemon thread elsewhere, because the runtime supports Android API 26. Either way the action
 * runs at most once, on a daemon thread, and must not reference the object it cleans.
 */
internal object HandleCleaner {
    /** A registration; [clean] runs the action now (once) and unregisters. */
    interface Cleanable {
        fun clean()
    }

    private val backend: Backend = try {
        Class.forName("java.lang.ref.Cleaner")
        JdkBackend()
    } catch (e: ClassNotFoundException) {
        QueueBackend()
    } catch (e: LinkageError) {
        QueueBackend()
    }

    /** Registers [action] to run when [referent] becomes unreachable, or earlier through [Cleanable.clean]. */
    fun register(referent: Any, action: Runnable): Cleanable = backend.register(referent, action)

    /** Same as [register] but forcing the phantom-queue backend (tests exercise both). */
    internal fun registerWithQueue(referent: Any, action: Runnable): Cleanable = QUEUE.register(referent, action)

    private val QUEUE: QueueBackend by lazy { QueueBackend() }

    private interface Backend {
        fun register(referent: Any, action: Runnable): Cleanable
    }

    private class JdkBackend : Backend {
        private val cleaner: java.lang.ref.Cleaner = java.lang.ref.Cleaner.create(NamedDaemonThreads("undra-cleaner"))

        override fun register(referent: Any, action: Runnable): Cleanable {
            val cleanable = cleaner.register(referent, action)
            return object : Cleanable {
                override fun clean() = cleanable.clean()
            }
        }
    }

    private class QueueBackend : Backend {
        private val queue = ReferenceQueue<Any>()
        private val live = ConcurrentHashMap.newKeySet<Entry>()

        private inner class Entry(referent: Any, private val action: Runnable) :
            PhantomReference<Any>(referent, queue), Cleanable {
            override fun clean() {
                if (live.remove(this)) {
                    clear()
                    try {
                        action.run()
                    } catch (e: Throwable) {
                        UndraLog.warn("a handle cleanup action failed", e)
                    }
                }
            }
        }

        private val thread: Thread by lazy {
            NamedDaemonThreads("undra-cleaner").newThread {
                while (true) {
                    val ref = try {
                        queue.remove()
                    } catch (e: InterruptedException) {
                        return@newThread
                    }
                    (ref as? Cleanable)?.clean()
                }
            }.also { it.start() }
        }

        override fun register(referent: Any, action: Runnable): Cleanable {
            thread // start the daemon on first use
            val entry = Entry(referent, action)
            live.add(entry)
            return entry
        }
    }
}
