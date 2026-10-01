package dev.undra.runtime

import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertThrows
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.Payloads
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ReplyStatus
import java.lang.reflect.Method
import java.lang.reflect.Modifier
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Test

/** The JVM descriptor of [m], for example `(II[B)V`. */
private fun descriptor(m: Method): String {
    fun d(c: Class<*>): String = when {
        c == Void.TYPE -> "V"
        c == Integer.TYPE -> "I"
        c == java.lang.Byte.TYPE -> "B"
        c == java.lang.Long.TYPE -> "J"
        c == java.lang.Boolean.TYPE -> "Z"
        c.isArray -> "[" + d(c.componentType)
        else -> "L" + c.name.replace('.', '/') + ";"
    }
    return "(" + m.parameterTypes.joinToString("") { d(it) } + ")" + d(m.returnType)
}

/**
 * Pins the JNI surface of SPEC 6.1 exactly. `undra-ffi` registers these names and descriptors with
 * `RegisterNatives`, and calls the `Callbacks` methods by name and descriptor, so a change here that this
 * test does not catch would surface as a `NoSuchMethodError` inside the native library.
 */
class NativeShapeTests : Suite() {
    init {
        case("UndraNative declares exactly the static natives of SPEC 6.1, with their JNI descriptors") {
            val expected = mapOf(
                "abiVersion" to "()I",
                "schemaHash" to "()J",
                "schemaJson" to "()[B",
                "init" to "([BLdev/undra/runtime/UndraNative\$Callbacks;)I",
                "call" to "([B)I",
                "callSync" to "([B)[B",
                "cancel" to "(I)V",
                "streamCredit" to "(II)V",
                "observe" to "(JIZ)V",
                "release" to "(J)V",
                "portReply" to "([B)V",
                "event" to "(II[B)V",
                "timerFired" to "(I)V",
                "snapshot" to "()[B",
                "restore" to "([B)I",
                "statsJson" to "()Ljava/lang/String;",
                "shutdown" to "()V",
            )
            val natives = UndraNative::class.java.declaredMethods.filter { Modifier.isNative(it.modifiers) }
            assertEq(expected.keys.sorted(), natives.map { it.name }.sorted())
            for (m in natives) {
                assertTrue(Modifier.isStatic(m.modifiers), "${m.name} must be static")
                assertTrue(Modifier.isPublic(m.modifiers), "${m.name} must be public")
                assertEq(expected.getValue(m.name), descriptor(m), m.name)
            }
            assertEq("dev.undra.runtime.UndraNative", UndraNative::class.java.name)
        }

        case("UndraNative.Callbacks is the interface the shim calls back into") {
            val cls = UndraNative.Callbacks::class.java
            assertEq("dev.undra.runtime.UndraNative\$Callbacks", cls.name)
            assertTrue(cls.isInterface)
            val expected = mapOf(
                "onReply" to "(ILjava/nio/ByteBuffer;)V",
                "onChangeSet" to "(Ljava/nio/ByteBuffer;)V",
                "onStream" to "(ILjava/nio/ByteBuffer;)V",
                "onPortCall" to "(IIILjava/nio/ByteBuffer;)I",
                "portSyncReply" to "()[B",
            )
            assertEq(expected.keys.sorted(), cls.declaredMethods.map { it.name }.sorted())
            for (m in cls.declaredMethods) assertEq(expected.getValue(m.name), descriptor(m), m.name)
        }

        case("library naming: the system properties and the default name are as documented") {
            assertEq("undra.native.name", UndraNative.NAME_PROPERTY)
            assertEq("undra.native.path", UndraNative.PATH_PROPERTY)
            assertEq("undra_core", UndraNative.DEFAULT_NAME)
        }

        case("without the native library, isAvailable is false and says why; nothing throws") {
            if (UndraNative.isAvailable) skip("the native library is loadable here; see NativeSmokeTests")
            assertTrue(UndraNative.unavailableReason is UnsatisfiedLinkError, "reason: ${UndraNative.unavailableReason}")
            val e = assertThrows<UndraException> {
                UndraCore.load(LoadOptions(expectedSchemaHash = 1uL, defaultAdapters = false))
            }
            assertTrue(e.cause is UnsatisfiedLinkError)
            assertTrue(e.message!!.contains("Mode.REMOTE"), e.message!!)
        }
    }

    @Test
    fun allCases() = assertPassed()
}

/**
 * Talks to the real native core through JNI. It runs only when `undra_core` can be loaded (put the library on
 * `java.library.path`, or set `undra.native.path`; `scripts/test-local.sh` forwards `UNDRA_NATIVE_LIB_DIR` and
 * `UNDRA_NATIVE_NAME`), so until `undra-ffi` lands it reports itself as skipped.
 *
 * It uses only what any core has: the wire contract. Calls to a method id that cannot exist must come
 * back as `BAD_REQUEST` replies, which exercises the reply path (direct buffer, decoding, resumption)
 * without knowing the core's schema.
 */
class NativeSmokeTests : Suite() {
    init {
        case("the library speaks ABI 1 and reports a schema") {
            if (!UndraNative.isAvailable) skip("no native library: ${UndraNative.unavailableReason?.message}")
            assertEq(1, UndraNative.abiVersion())
            val json = String(UndraNative.schemaJson(), Charsets.UTF_8)
            assertTrue(json.trimStart().startsWith("{"), "schema JSON: ${json.take(80)}")
        }

        case("load, unknown calls fail as bad requests through both paths, stats and snapshot work, then close") {
            if (!UndraNative.isAvailable) skip("no native library: ${UndraNative.unavailableReason?.message}")
            val dataDir = java.nio.file.Files.createTempDirectory("undra-smoke")
            System.setProperty("undra.data.dir", dataDir.toString())
            val core = UndraCore.load(LoadOptions(expectedSchemaHash = UndraNative.schemaHash().toULong()))
            try {
                val unknown = CallTarget.FreeFunction(0xDEADBEEFu)
                val sync = assertThrows<UndraReplyException> { core.callSync(unknown, 0xDEADBEEFu, ByteArray(0)) }
                assertEq(ReplyStatus.BAD_REQUEST, sync.status)
                val async = assertThrows<UndraReplyException> { runBlocking { core.call(unknown, 0xDEADBEEFu, ByteArray(0)) } }
                assertEq(ReplyStatus.BAD_REQUEST, async.status)
                assertTrue(async.badRequestReason != null, "the reason is a readable string")
                // Unknown handles are ignored by the core, not fatal.
                core.observe(0x7777777700000001L, 0u, true)
                core.release(0x7777777700000001L)
                val stats = core.stats()
                assertTrue(stats.liveHandles >= 0, "stats: $stats")
                assertTrue(stats.raw.contains("live_handles"), stats.raw)
                // Layout 2 (ADR-037): the core's own schema hash travels with the snapshot, and it restores as it is.
                val snapshotBytes = core.snapshot()
                val snapshot = Payloads.Snapshot.decode(snapshotBytes)
                assertEq(UndraNative.schemaHash().toULong(), snapshot.schemaHash)
                assertTrue(snapshot.stores.all { snapshot.fingerprint(it.typeId) != null }, "every store's type is listed")
                core.restore(snapshotBytes)
                // A snapshot in the layout before ADR-037 (count, floor) is refused as malformed, and the core is unchanged.
                assertEq(UndraRestoreException.BAD_SNAPSHOT, assertThrows<UndraRestoreException> { core.restore(ByteArray(8)) }.code)
                assertTrue(Codecs.u32.encodeToBytes().isNotEmpty())
            } finally {
                core.close()
            }
            // Closing ended the core's work (ADR-034): a new load in the same process starts a fresh core, and a
            // second one while it is loaded is refused.
            val again = UndraCore.load(LoadOptions(expectedSchemaHash = UndraNative.schemaHash().toULong()))
            try {
                assertEq(0, again.stats().liveHandles, "a fresh core has no handles")
                assertThrows<UndraException> { UndraCore.load(LoadOptions(expectedSchemaHash = UndraNative.schemaHash().toULong())) }
            } finally {
                again.close()
            }
        }
    }

    private fun dev.undra.runtime.wire.UndraCodec<UInt>.encodeToBytes(): ByteArray =
        dev.undra.runtime.wire.UndraWriter().also { encode(it, 1u) }.toByteArray()

    @Test
    fun allCases() = assertPassed()
}
