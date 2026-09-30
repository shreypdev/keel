package dev.keel.runtime.support

import dev.keel.runtime.testing.fail
import dev.keel.runtime.wire.Handle
import dev.keel.runtime.wire.Payloads
import java.io.File
import java.nio.file.Files
import java.nio.file.Path
import java.util.Comparator
import java.util.concurrent.CopyOnWriteArrayList
import java.util.logging.Handler
import java.util.logging.Level
import java.util.logging.LogRecord
import java.util.logging.Logger

/** The schema hash every fake core in the tests reports unless a test says otherwise. */
const val HASH: ULong = 0x691eee0733e4a44fuL

val NO_BYTES = ByteArray(0)

/** Polls [condition] every 5 ms until it holds; fails with [message] after [timeoutMs]. */
fun eventually(message: String = "condition", timeoutMs: Long = 10_000, condition: () -> Boolean) {
    val deadline = System.nanoTime() + timeoutMs * 1_000_000
    while (!condition()) {
        if (System.nanoTime() > deadline) fail("timed out after ${timeoutMs}ms waiting for: $message")
        Thread.sleep(5)
    }
}

/** A `ChangeSet` payload with one entry per argument. */
fun changeSet(txn: ULong, vararg entries: Payloads.ChangeEntry): ByteArray = Payloads.ChangeSet(txn, entries.toList()).toByteArray()

fun full(handle: Long, signal: UInt, value: ByteArray): Payloads.ChangeEntry =
    Payloads.ChangeEntry(Handle(handle), signal, Payloads.ChangeOp.FULL, value)

fun patch(handle: Long, signal: UInt, value: ByteArray): Payloads.ChangeEntry =
    Payloads.ChangeEntry(Handle(handle), signal, Payloads.ChangeOp.PATCH, value)

fun invalidated(handle: Long, signal: UInt): Payloads.ChangeEntry =
    Payloads.ChangeEntry(Handle(handle), signal, Payloads.ChangeOp.INVALIDATED, NO_BYTES)

/** A whole `Reply` payload. */
fun replyPayload(callId: UInt, status: Payloads.ReplyStatus, body: ByteArray = NO_BYTES): ByteArray =
    Payloads.Reply(callId, status, body).toByteArray()

/** A temporary directory removed by [use]. */
class TempDir : AutoCloseable {
    val path: Path = Files.createTempDirectory("keel-test")

    override fun close() {
        val root = File(path.toString())
        // Symbolic links are deleted as links, never followed.
        Files.walk(path).use { stream ->
            stream.sorted(Comparator.reverseOrder()).forEach { p ->
                try {
                    Files.deleteIfExists(p)
                } catch (e: java.io.IOException) {
                    root.deleteRecursively()
                }
            }
        }
    }
}

/** Collects what is logged to the JUL logger [name] (and below) while it is installed. */
class LogCapture(private val name: String, level: Level = Level.ALL) : AutoCloseable {
    val records = CopyOnWriteArrayList<LogRecord>()
    private val logger: Logger = Logger.getLogger(name)
    private val previousLevel: Level? = logger.level
    private val previousParentHandlers = logger.useParentHandlers
    private val handler = object : Handler() {
        override fun publish(record: LogRecord) {
            records.add(record)
        }

        override fun flush() = Unit

        override fun close() = Unit
    }

    init {
        handler.level = Level.ALL
        logger.addHandler(handler)
        logger.level = level
        logger.useParentHandlers = false // keep test output clean
    }

    fun messages(): List<String> = records.map { it.message }

    override fun close() {
        logger.removeHandler(handler)
        logger.level = previousLevel
        logger.useParentHandlers = previousParentHandlers
    }
}

/** A core over [transport] that is not the shared one, with the default adapters off unless asked for. */
internal fun attach(
    transport: FakeTransport,
    adapters: Map<UInt, dev.keel.runtime.PortImpl> = emptyMap(),
    defaultAdapters: Boolean = false,
    timeout: kotlin.time.Duration = kotlin.time.Duration.parse("5s"),
): dev.keel.runtime.KeelCore =
    dev.keel.runtime.KeelCore.attach(
        transport,
        dev.keel.runtime.LoadOptions(
            expectedSchemaHash = HASH,
            adapters = adapters,
            defaultAdapters = defaultAdapters,
            remoteTimeout = timeout,
        ),
        makeShared = false,
    )
