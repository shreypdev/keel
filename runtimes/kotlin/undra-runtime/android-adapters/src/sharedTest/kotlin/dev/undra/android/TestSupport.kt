package dev.undra.android

import dev.undra.runtime.BackgroundStats
import dev.undra.runtime.PortImpl
import dev.undra.runtime.UndraStats
import dev.undra.runtime.UndraCore
import dev.undra.runtime.wire.UndraWriter
import java.io.File
import kotlinx.coroutines.runBlocking

/** Whether the tests run on an Android runtime (instrumented) rather than a desktop JVM (unit tests). */
val isAndroidRuntime: Boolean = "Dalvik" == System.getProperty("java.vm.name")

/**
 * An [UndraCore] that records what the adapters tell it: the ports they register and the events they send.
 * (`UndraCore` is open with a protected constructor so that tests can do this.)
 */
class RecordingCore : UndraCore() {
    /** A host-to-core event of an event port. */
    class Event(val portId: UInt, val methodId: UInt, val payload: ByteArray)

    private val lock = Object()
    private val registeredPorts = LinkedHashMap<UInt, PortImpl>()
    private val sentEvents = ArrayList<Event>()
    private val firedTimers = ArrayList<UInt>()

    /** The ports registered, by id (the last registration of an id wins). */
    val ports: Map<UInt, PortImpl> get() = synchronized(lock) { LinkedHashMap(registeredPorts) }

    /** The events sent, oldest first. */
    val events: List<Event> get() = synchronized(lock) { sentEvents.toList() }

    /** The timers the core was told came due. */
    val timers: List<UInt> get() = synchronized(lock) { firedTimers.toList() }

    /** What `stats().background.pending` answers: the work a background window would drain (ADR-046). */
    @Volatile
    var backgroundPending: Int = 0

    override fun stats(): UndraStats = UndraStats(0, background = BackgroundStats(3, backgroundPending, 0L, 0L, 0L, 0L))

    override fun registerPort(portId: UInt, impl: PortImpl) {
        synchronized(lock) { registeredPorts[portId] = impl }
    }

    override fun event(portId: UInt, methodId: UInt, payload: ByteArray) {
        synchronized(lock) {
            sentEvents.add(Event(portId, methodId, payload))
            lock.notifyAll()
        }
    }

    override fun timerFired(timerId: UInt) {
        synchronized(lock) { firedTimers.add(timerId) }
    }

    /** Waits until an event satisfying [predicate] has been sent; returns the events so far (also on timeout). */
    fun awaitEvents(timeoutMs: Long = 5_000, predicate: (List<Event>) -> Boolean): List<Event> {
        val deadline = System.nanoTime() + timeoutMs * 1_000_000
        synchronized(lock) {
            while (!predicate(sentEvents)) {
                val left = (deadline - System.nanoTime()) / 1_000_000
                if (left <= 0) break
                lock.wait(left)
            }
            return sentEvents.toList()
        }
    }
}

/** Calls method [methodId] of [impl] with [args] and returns the encoded reply; throws what the method throws. */
fun call(impl: PortImpl, methodId: UInt, args: ByteArray): ByteArray = runBlocking { impl.methods.getValue(methodId)(args) }

/** The encoded arguments of a method taking one string. */
fun argsOf(text: String): ByteArray = UndraWriter(8 + text.length).also { it.writeStr(text) }.toByteArray()

/** The encoded arguments of a method taking a string and some bytes. */
fun argsOf(text: String, bytes: ByteArray): ByteArray = UndraWriter(16 + text.length + bytes.size).also {
    it.writeStr(text)
    it.writeBytes(bytes)
}.toByteArray()

/** A fresh empty directory for one test. */
fun scratchDir(name: String): File {
    val parent = File(System.getProperty("java.io.tmpdir") ?: ".", "undra-adapter-tests")
    val dir = File(parent, "$name-${System.nanoTime()}")
    check(dir.mkdirs()) { "cannot create $dir" }
    return dir
}
