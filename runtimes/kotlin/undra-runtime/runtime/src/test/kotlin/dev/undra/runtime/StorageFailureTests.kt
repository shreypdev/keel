package dev.undra.runtime

import dev.undra.runtime.adapters.FileKv
import dev.undra.runtime.adapters.FsAdapter
import dev.undra.runtime.adapters.FsError
import dev.undra.runtime.adapters.HttpError
import dev.undra.runtime.adapters.JvmAdapters
import dev.undra.runtime.adapters.KeyValueBackend
import dev.undra.runtime.adapters.StandardPorts
import dev.undra.runtime.adapters.StorageError
import dev.undra.runtime.adapters.StoragePort
import dev.undra.runtime.support.FakeTransport
import dev.undra.runtime.support.HASH
import dev.undra.runtime.support.LogCapture
import dev.undra.runtime.support.TempDir
import dev.undra.runtime.support.attach
import dev.undra.runtime.support.eventually
import dev.undra.runtime.support.handledBy
import dev.undra.runtime.support.portMethods
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertBytes
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.testing.bytesOf
import dev.undra.runtime.testing.hex
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Payloads.PortReply
import dev.undra.runtime.wire.Payloads.PortStatus
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import dev.undra.testsupport.FaultyFileSystem
import java.nio.file.Files
import java.util.TreeMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger
import java.util.logging.Level
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

/**
 * ADR-049's failure-injection suite for the Kotlin runtime (the Swift and TypeScript runtimes have one of the same
 * name): storage that fails on demand, behind the real port implementations and the real port registry, and the exact
 * `PortReply` bytes the core receives for each failure.
 *
 *  - A typed failure is status 1 with the encoded [StorageError] (for `Fs`, the [FsError]), for every method of `Kv` and
 *    `SecureStore` and every variant; the next call after the failure succeeds with the same bytes as before ADR-049.
 *  - Anything else an adapter throws is status 2 with an empty body, one ERROR log record naming the port method, and
 *    `onError` (operation `port 0x... method 0x...`).
 *  - The runtime's own file adapters ([FileKv], [FsAdapter]) over a file system that fails the way the platform does
 *    ([FaultyFileSystem]): a full disk is `Full`, a damaged entry is `Corrupt`, anything else `Io`.
 */
class StorageFailureTests : Suite() {
    /** A [KeyValueBackend] in memory that fails every call with what [failure] makes while it is set. */
    private class FailingBackend : KeyValueBackend {
        val stored = TreeMap<String, ByteArray>()

        @Volatile
        var failure: (() -> Throwable)? = null

        val calls = AtomicInteger()

        private fun maybeFail() {
            calls.incrementAndGet()
            failure?.let { throw it() }
        }

        override suspend fun get(key: String): ByteArray? {
            maybeFail()
            return synchronized(stored) { stored[key] }
        }

        override suspend fun set(key: String, value: ByteArray) {
            maybeFail()
            synchronized(stored) { stored[key] = value }
        }

        override suspend fun delete(key: String) {
            maybeFail()
            synchronized(stored) { stored.remove(key) }
        }

        override suspend fun list(prefix: String): List<String> {
            maybeFail()
            return synchronized(stored) { stored.keys.filter { it.startsWith(prefix) } }
        }
    }

    private val errors: List<StorageError> = listOf(
        StorageError.Unavailable("no backend here"),
        StorageError.Full,
        StorageError.Locked,
        StorageError.Corrupt("bad tag"),
        StorageError.Io("Input/output error"),
    )

    private val nextCallId = AtomicInteger(1)

    private fun str(s: String): ByteArray = UndraWriter().also { it.writeStr(s) }.toByteArray()

    private fun keyValue(key: String, value: ByteArray): ByteArray = UndraWriter().also {
        it.writeStr(key)
        it.writeBytes(value)
    }.toByteArray()

    /** The four methods of [port] with arguments for key `k`. */
    private fun methods(port: StoragePort): List<Pair<UInt, ByteArray>> =
        listOf(port.getId to str("k"), port.setId to keyValue("k", bytesOf(1, 2)), port.deleteId to str("k"), port.listId to str(""))

    /** Calls [methodId] of [portId] as the core would and returns the whole `PortReply` payload the host answered with. */
    private fun answer(t: FakeTransport, portId: UInt, methodId: UInt, args: ByteArray): ByteArray {
        val callId = nextCallId.getAndIncrement().toUInt()
        val outcome = t.portCall(portId, methodId, callId, args)
        if (outcome is PortOutcome.Sync) return outcome.reply
        assertEq(PortOutcome.Async, outcome, "storage ports are async")
        var found: ByteArray? = null
        eventually("the reply to port call $callId") {
            found = t.portReplyBytes.firstOrNull { UndraReader(it).readU32() == callId }
            found != null
        }
        return found!!
    }

    /** The `PortReply` payload [reply] must be: its own call id, then [status] and [body]. */
    private fun assertReply(status: PortStatus, body: ByteArray, reply: ByteArray, message: String) {
        val callId = UndraReader(reply).readU32()
        assertBytes(hex(PortReply(callId, status, body).toByteArray()), reply, message)
    }

    private fun typed(error: StorageError): ByteArray = StorageError.encodeToByteArray(error)

    init {
        // ---- the bridge: StoragePort over a backend that fails on demand --------------------------------------------

        case("every Kv and SecureStore method answers each StorageError with status 1 and the encoded error, then recovers") {
            for (port in StoragePort.entries) {
                val backend = FailingBackend()
                val t = FakeTransport()
                attach(t, adapters = mapOf(port.portId to port.portImpl(backend))).use {
                    for (error in errors) {
                        backend.failure = { error }
                        for ((method, args) in methods(port)) {
                            val reply = answer(t, port.portId, method, args)
                            assertReply(PortStatus.ERROR, typed(error), reply, "${port.traitName} method 0x${method.toString(16)}: $error")
                            assertEq(error, StorageError.decodeAll(PortReply.decode(reply).body))
                        }
                    }
                    // The bytes the core is promised, spelled out once (crates/undra-ports/tests/wire_contract.rs).
                    backend.failure = { StorageError.Full }
                    assertBytes("0100", PortReply.decode(answer(t, port.portId, port.getId, str("k"))).body)
                    backend.failure = { StorageError.Corrupt("e") }
                    assertBytes("0300" + "01000000" + "65", PortReply.decode(answer(t, port.portId, port.listId, str(""))).body)

                    // Healed: the same calls succeed with the bodies they always had (status 0, no wire change).
                    backend.failure = null
                    assertReply(PortStatus.OK, ByteArray(0), answer(t, port.portId, port.setId, keyValue("k", bytesOf(1, 2))), "set")
                    assertReply(PortStatus.OK, Codecs.option(Codecs.bytes).encodeToByteArray(bytesOf(1, 2)), answer(t, port.portId, port.getId, str("k")), "get")
                    assertReply(PortStatus.OK, Codecs.vec(Codecs.string).encodeToByteArray(listOf("k")), answer(t, port.portId, port.listId, str("")), "list")
                    assertReply(PortStatus.OK, ByteArray(0), answer(t, port.portId, port.deleteId, str("k")), "delete")
                    assertReply(PortStatus.OK, bytesOf(0), answer(t, port.portId, port.getId, str("k")), "get after delete")
                }
            }
        }

        case("a failing backend is not logged or reported: a typed failure is the core's to handle") {
            val backend = FailingBackend().also { it.failure = { StorageError.Locked } }
            val t = FakeTransport()
            val reports = CopyOnWriteArrayList<UndraUnhandledError>()
            LogCapture("dev.undra.runtime").use { log ->
                UndraCore.attach(
                    t,
                    LoadOptions(expectedSchemaHash = HASH, defaultAdapters = false, adapters = mapOf(StoragePort.KV.portId to StoragePort.KV.portImpl(backend)), onError = { reports.add(it) }),
                    makeShared = false,
                ).use {
                    for ((method, args) in methods(StoragePort.KV)) answer(t, StoragePort.KV.portId, method, args)
                    Thread.sleep(50)
                    assertEq(0, reports.size, "onError")
                    assertEq(emptyList<String>(), log.records.filter { it.level.intValue() >= Level.WARNING.intValue() }.map { it.message })
                }
            }
        }

        case("an untyped throw is status 2 with an empty body, one ERROR log naming the port method, and onError") {
            for (port in StoragePort.entries) {
                val backend = FailingBackend().also { it.failure = { IllegalStateException("a bug in the adapter") } }
                val t = FakeTransport()
                val reports = CopyOnWriteArrayList<UndraUnhandledError>()
                LogCapture("dev.undra.runtime").use { log ->
                    UndraCore.attach(
                        t,
                        LoadOptions(expectedSchemaHash = HASH, defaultAdapters = false, adapters = mapOf(port.portId to port.portImpl(backend)), onError = { reports.add(it) }),
                        makeShared = false,
                    ).use {
                        val reply = answer(t, port.portId, port.getId, str("k"))
                        assertReply(PortStatus.UNAVAILABLE, ByteArray(0), reply, "${port.traitName}.get")
                        eventually("onError heard it") { reports.size == 1 }
                        val operation = "port 0x${port.portId.toString(16)} method 0x${port.getId.toString(16)}"
                        assertEq(operation, reports.single().operation)
                        assertTrue(reports.single().error is UndraCallError.Malformed, "${reports.single().error}")
                        val errors = log.records.filter { it.level == Level.SEVERE }
                        assertEq(1, errors.size, "one ERROR record, not one from the registry and another from report: ${errors.map { it.message }}")
                        val record = errors.single()
                        assertTrue(record.message.contains("the ${port.traitName}.get port method") && record.message.contains(operation), record.message)
                        assertTrue(record.message.contains("StorageError"), record.message)
                        assertTrue(record.thrown is IllegalStateException, "the record carries the exception")
                    }
                }
            }
        }

        case("an argument the method cannot decode is untyped too: status 2, logged") {
            val t = FakeTransport()
            LogCapture("dev.undra.runtime").use { log ->
                attach(t, adapters = mapOf(StoragePort.KV.portId to StoragePort.KV.portImpl(FailingBackend()))).use {
                    assertReply(PortStatus.UNAVAILABLE, ByteArray(0), answer(t, StoragePort.KV.portId, StoragePort.KV.getId, bytesOf(1)), "truncated key")
                    eventually("logged") { log.records.any { it.level == Level.SEVERE && it.message.contains("Kv.get") } }
                }
            }
        }

        // ---- the registry: a standard port's own error type thrown raw ----------------------------------------------

        case("a StorageError, FsError or HttpError thrown raw by a method of its own standard port is still a typed reply") {
            val t = FakeTransport()
            val kv = PortImpl(false, portMethods(StandardPorts.Kv.GET handledBy { _: ByteArray -> throw StorageError.Locked }))
            val secure = PortImpl(true, portMethods(StandardPorts.SecureStore.SET handledBy { _: ByteArray -> throw StorageError.Corrupt("x") }))
            val fs = PortImpl(false, portMethods(StandardPorts.Fs.WRITE handledBy { _: ByteArray -> throw FsError.Full }))
            val http = PortImpl(false, portMethods(StandardPorts.Http.REQUEST handledBy { _: ByteArray -> throw HttpError.Timeout }))
            LogCapture("dev.undra.runtime").use { log ->
                attach(
                    t,
                    adapters = mapOf(
                        StandardPorts.Kv.PORT_ID to kv,
                        StandardPorts.SecureStore.PORT_ID to secure,
                        StandardPorts.Fs.PORT_ID to fs,
                        StandardPorts.Http.PORT_ID to http,
                    ),
                ).use {
                    assertReply(PortStatus.ERROR, bytesOf(2, 0), answer(t, StandardPorts.Kv.PORT_ID, StandardPorts.Kv.GET, str("k")), "Kv: Locked")
                    // A sync implementation is answered inline, with the same typed reply.
                    assertReply(PortStatus.ERROR, typed(StorageError.Corrupt("x")), answer(t, StandardPorts.SecureStore.PORT_ID, StandardPorts.SecureStore.SET, keyValue("k", bytesOf(1))), "SecureStore: Corrupt")
                    assertReply(PortStatus.ERROR, bytesOf(3, 0), answer(t, StandardPorts.Fs.PORT_ID, StandardPorts.Fs.WRITE, keyValue("f", bytesOf(1))), "Fs: Full")
                    assertReply(PortStatus.ERROR, bytesOf(1, 0), answer(t, StandardPorts.Http.PORT_ID, StandardPorts.Http.REQUEST, bytesOf()), "Http: Timeout")
                    assertEq(0, log.records.count { it.level == Level.SEVERE })
                }
            }
        }

        case("another port's error type, or a standard error from a port it does not belong to, is untyped: status 2") {
            val appPort = 0xAAAA0001u
            val t = FakeTransport()
            val app = PortImpl(false, portMethods(0xBBBB0001u handledBy { _: ByteArray -> throw StorageError.Full }))
            val kv = PortImpl(false, portMethods(StandardPorts.Kv.GET handledBy { _: ByteArray -> throw FsError.Full }))
            val fs = PortImpl(false, portMethods(StandardPorts.Fs.READ handledBy { _: ByteArray -> throw StorageError.Full }))
            LogCapture("dev.undra.runtime").use { log ->
                attach(t, adapters = mapOf(appPort to app, StandardPorts.Kv.PORT_ID to kv, StandardPorts.Fs.PORT_ID to fs)).use {
                    // A StorageError's bytes would not decode as the app port's own error type: the core gets unavailable.
                    assertReply(PortStatus.UNAVAILABLE, ByteArray(0), answer(t, appPort, 0xBBBB0001u, ByteArray(0)), "app port")
                    assertReply(PortStatus.UNAVAILABLE, ByteArray(0), answer(t, StandardPorts.Kv.PORT_ID, StandardPorts.Kv.GET, str("k")), "FsError from Kv")
                    assertReply(PortStatus.UNAVAILABLE, ByteArray(0), answer(t, StandardPorts.Fs.PORT_ID, StandardPorts.Fs.READ, str("f")), "StorageError from Fs")
                    eventually("three ERROR records") { log.records.count { it.level == Level.SEVERE } == 3 }
                    val messages = log.records.filter { it.level == Level.SEVERE }.map { it.message }
                    assertTrue(messages.any { it.startsWith("port 0xaaaa0001 method 0xbbbb0001 failed") }, "$messages")
                    assertTrue(messages.any { it.contains("the Kv.get port method") }, "$messages")
                    assertTrue(messages.any { it.contains("the Fs.read port method") }, "$messages")
                }
            }
            assertEq(null, PortRegistry.typedBody(appPort, StorageError.Full))
            assertBytes("0100", PortRegistry.typedBody(StandardPorts.SecureStore.PORT_ID, StorageError.Full)!!)
        }

        // ---- the runtime's FileKv over a file system that fails like the platform -----------------------------------

        case("FileKv: a full disk or quota is Full, through the port, and the old value stays") {
            TempDir().use { dir ->
                val disk = FaultyFileSystem(dir.path)
                val store = FileKv(disk.root.resolve("kv"))
                val t = FakeTransport()
                attach(t, adapters = mapOf(StandardPorts.Kv.PORT_ID to store.portImpl("Kv"))).use {
                    assertReply(PortStatus.OK, ByteArray(0), answer(t, StandardPorts.Kv.PORT_ID, StandardPorts.Kv.SET, keyValue("k", bytesOf(1))), "first write")
                    for (error in listOf(FaultyFileSystem.noSpace(), FaultyFileSystem.noSpace(android = true), FaultyFileSystem.quotaExceeded())) {
                        disk.fail(FaultyFileSystem.Op.WRITE) { error }
                        val reply = answer(t, StandardPorts.Kv.PORT_ID, StandardPorts.Kv.SET, keyValue("k", bytesOf(2)))
                        assertReply(PortStatus.ERROR, bytesOf(1, 0), reply, "set on $error")
                        disk.heal()
                    }
                    assertReply(PortStatus.OK, Codecs.option(Codecs.bytes).encodeToByteArray(bytesOf(1)), answer(t, StandardPorts.Kv.PORT_ID, StandardPorts.Kv.GET, str("k")), "the old value")
                }
                val files = Files.list(dir.path.resolve("kv")).use { s -> s.map { it.fileName.toString() }.collect(java.util.stream.Collectors.toList()) }
                assertEq(1, files.size, "no temporary file is left behind: $files")
                assertTrue(disk.failures >= 3)
            }
        }

        case("FileKv: the first write into a directory that cannot be created on a full disk is Full") {
            TempDir().use { dir ->
                val disk = FaultyFileSystem(dir.path)
                val store = FileKv(disk.root.resolve("never").resolve("kv"))
                disk.fail(FaultyFileSystem.Op.WRITE) { FaultyFileSystem.noSpace() }
                assertEq(StorageError.Full, assertThrows<StorageError.Full> { runBlocking { store.set("k", bytesOf(1)) } })
                assertTrue(!Files.exists(dir.path.resolve("never")), "nothing was created")
                disk.heal()
                runBlocking { store.set("k", bytesOf(1)) }
                assertEq(listOf<Byte>(1), runBlocking { store.get("k") }!!.toList())
            }
        }

        case("FileKv: an entry that does not decode is Corrupt, through the port; the file stays and a new write replaces it") {
            TempDir().use { dir ->
                val store = FileKv(dir.path)
                runBlocking { store.set("undra.query.queue", bytesOf(1, 2, 3)) }
                val file = Files.list(dir.path).use { it.findFirst().get() }
                Files.write(file, bytesOf(0xff, 0xff, 0xff, 0xff, 1)) // a key length the file cannot hold
                val t = FakeTransport()
                attach(t, adapters = mapOf(StandardPorts.Kv.PORT_ID to store.portImpl("Kv"))).use {
                    val reply = PortReply.decode(answer(t, StandardPorts.Kv.PORT_ID, StandardPorts.Kv.GET, str("undra.query.queue")))
                    assertEq(PortStatus.ERROR, reply.status)
                    val error = StorageError.decodeAll(reply.body)
                    assertTrue(error is StorageError.Corrupt && error.reason.contains("undra.query.queue"), "$error")
                    assertTrue(Files.exists(file), "a Corrupt entry is not deleted: the bytes are still there")
                    // It is not listed (it is not readable as an entry), and writing the key again replaces it.
                    assertReply(PortStatus.OK, Codecs.vec(Codecs.string).encodeToByteArray(emptyList()), answer(t, StandardPorts.Kv.PORT_ID, StandardPorts.Kv.LIST, str("")), "list")
                    assertReply(PortStatus.OK, ByteArray(0), answer(t, StandardPorts.Kv.PORT_ID, StandardPorts.Kv.SET, keyValue("undra.query.queue", bytesOf(4))), "set")
                    assertReply(PortStatus.OK, Codecs.option(Codecs.bytes).encodeToByteArray(bytesOf(4)), answer(t, StandardPorts.Kv.PORT_ID, StandardPorts.Kv.GET, str("undra.query.queue")), "get")
                }
            }
        }

        case("FileKv: every other failure is Io with the platform's message, for each method, through SecureStore too") {
            TempDir().use { dir ->
                val disk = FaultyFileSystem(dir.path)
                val store = FileKv(disk.root.resolve("secure"))
                runBlocking { store.set("k", bytesOf(1)) }
                val t = FakeTransport()
                val io = typed(StorageError.Io("Input/output error"))
                attach(t, adapters = mapOf(StandardPorts.SecureStore.PORT_ID to store.portImpl("SecureStore"))).use {
                    val cases = listOf(
                        Triple(FaultyFileSystem.Op.READ, StandardPorts.SecureStore.GET, str("k")),
                        Triple(FaultyFileSystem.Op.WRITE, StandardPorts.SecureStore.SET, keyValue("k", bytesOf(2))),
                        Triple(FaultyFileSystem.Op.MOVE, StandardPorts.SecureStore.SET, keyValue("k", bytesOf(2))),
                        Triple(FaultyFileSystem.Op.DELETE, StandardPorts.SecureStore.DELETE, str("k")),
                        Triple(FaultyFileSystem.Op.LIST, StandardPorts.SecureStore.LIST, str("")),
                    )
                    for ((op, method, args) in cases) {
                        disk.fail(op) { FaultyFileSystem.ioError() }
                        assertReply(PortStatus.ERROR, io, answer(t, StandardPorts.SecureStore.PORT_ID, method, args), "$op")
                        disk.heal()
                    }
                    assertReply(PortStatus.OK, Codecs.option(Codecs.bytes).encodeToByteArray(bytesOf(1)), answer(t, StandardPorts.SecureStore.PORT_ID, StandardPorts.SecureStore.GET, str("k")), "unchanged")
                }
                val names = Files.list(dir.path.resolve("secure")).use { s -> s.map { it.fileName.toString() }.collect(java.util.stream.Collectors.toList()) }
                assertTrue(names.none { it.endsWith(".tmp") }, "a failed move leaves no temporary file: $names")
                // A permission refusal says what it was, not only the file it was about.
                val denied = StorageError.of(java.nio.file.AccessDeniedException("/data/kv/x"))
                assertEq(StorageError.Io("AccessDeniedException: /data/kv/x"), denied)
            }
        }

        case("FileKv's direct API throws the StorageError itself") {
            TempDir().use { dir ->
                val disk = FaultyFileSystem(dir.path)
                val store = FileKv(disk.root)
                runBlocking { store.set("k", bytesOf(1)) }
                disk.fail(FaultyFileSystem.Op.READ) { FaultyFileSystem.ioError() }
                assertEq(StorageError.Io("Input/output error"), assertThrows<StorageError.Io> { runBlocking { store.get("k") } })
                disk.heal()
                disk.fail(FaultyFileSystem.Op.WRITE, times = 1) { FaultyFileSystem.noSpace() }
                assertThrows<StorageError.Full> { runBlocking { store.set("k", bytesOf(2)) } }
                runBlocking { store.set("k", bytesOf(3)) } // the fault was for one write only
                assertEq(listOf<Byte>(3), runBlocking { store.get("k") }!!.toList())
            }
        }

        case("the default JVM adapters are these: a damaged Kv entry is Corrupt through JvmAdapters.standard") {
            TempDir().use { dir ->
                val all = JvmAdapters.standard(dir.path) { }
                val t = FakeTransport()
                attach(t, adapters = all).use {
                    assertReply(PortStatus.OK, ByteArray(0), answer(t, StandardPorts.Kv.PORT_ID, StandardPorts.Kv.SET, keyValue("k", bytesOf(1))), "set")
                    val file = Files.list(dir.path.resolve("kv")).use { it.findFirst().get() }
                    Files.write(file, bytesOf(9, 0, 0, 0, 0x6b)) // claims a 9-byte key, holds one byte
                    assertEq(PortStatus.ERROR, PortReply.decode(answer(t, StandardPorts.Kv.PORT_ID, StandardPorts.Kv.GET, str("k"))).status)
                }
            }
        }

        // ---- the runtime's FsAdapter: FsError.Full ------------------------------------------------------------------

        case("Fs: a full disk or quota is FsError.Full (status 1, 0300); other failures stay Io") {
            TempDir().use { dir ->
                val disk = FaultyFileSystem(dir.path)
                val t = FakeTransport()
                attach(t, adapters = mapOf(StandardPorts.Fs.PORT_ID to FsAdapter(disk.root.resolve("fs")).portImpl())).use {
                    for (error in listOf(FaultyFileSystem.noSpace(), FaultyFileSystem.noSpace(android = true), FaultyFileSystem.quotaExceeded())) {
                        disk.fail(FaultyFileSystem.Op.WRITE) { error }
                        assertReply(PortStatus.ERROR, bytesOf(3, 0), answer(t, StandardPorts.Fs.PORT_ID, StandardPorts.Fs.WRITE, keyValue("a/b.txt", bytesOf(1))), "write on $error")
                        disk.heal()
                    }
                    assertReply(PortStatus.OK, ByteArray(0), answer(t, StandardPorts.Fs.PORT_ID, StandardPorts.Fs.WRITE, keyValue("a/b.txt", bytesOf(1))), "healed")
                    disk.fail(FaultyFileSystem.Op.READ) { FaultyFileSystem.ioError() }
                    assertReply(PortStatus.ERROR, FsError.encodeToByteArray(FsError.Io("Input/output error")), answer(t, StandardPorts.Fs.PORT_ID, StandardPorts.Fs.READ, str("a/b.txt")), "read")
                    disk.heal()
                }
                val fs = FsAdapter(disk.root.resolve("fs"))
                disk.fail(FaultyFileSystem.Op.WRITE) { FaultyFileSystem.noSpace() }
                assertEq(FsError.Full, assertThrows<FsError.Full> { runBlocking { fs.write("c.txt", bytesOf(1)) } })
                disk.heal()
                assertEq(listOf("b.txt"), runBlocking { fs.list("a") })
            }
        }
    }

    @Test
    fun allCases() = assertPassed()
}
