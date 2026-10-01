package dev.undra.contract

import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

/**
 * Records every value a `StateFlow` takes, starting with the value it has now. A `StateFlow` only
 * conflates equal values, and the collector runs where the store sets the value (`Dispatchers.Unconfined`),
 * so a change that comes and goes within one main-thread hop is the only thing it could miss, and the
 * scenarios that use it space their changes by network delays of 50 ms.
 */
class Recorder<T>(flow: StateFlow<T>) : AutoCloseable {
    /** The values seen, oldest first. */
    val values = CopyOnWriteArrayList<T>()

    private val job: Job = CoroutineScope(Dispatchers.Unconfined).launch { flow.collect { values.add(it) } }

    /** Stops recording. */
    override fun close() {
        job.cancel()
    }
}
