package dev.keel.runtime

import dev.keel.runtime.testing.Suite
import dev.keel.runtime.testing.assertEq
import dev.keel.runtime.testing.assertThrows
import dev.keel.runtime.testing.assertTrue
import dev.keel.runtime.wire.Codecs
import dev.keel.runtime.wire.Payloads
import dev.keel.runtime.wire.Payloads.CallTarget
import dev.keel.runtime.wire.Payloads.ReplyStatus
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
 * Pins the JNI surface of SPEC 6.1 exactly. `keel-ffi` registers these names and descriptors with
 * `RegisterNatives`, and calls the `Callbacks` methods by name and descriptor, so a change here that this
 * test does not catch would surface as a `NoSuchMethodError` inside the native library.
 */
class NativeShapeTests : Suite() {
    init {
        case("KeelNative declares exactly the static natives of SPEC 6.1, with their JNI descriptors") {
            val expected = mapOf(
                "abiVersion" to "()I",
                "schemaHash" to "()J",
                "schemaJson" to "()[B",
                "init" to "([BLdev/keel/runtime/KeelNative\$Callbacks;)I",
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
            )
            val natives = KeelNative::class.java.declaredMethods.filter { Modifier.isNative(it.modifiers) }
            assertEq(expected.keys.sorted(), natives.map { it.name }.sorted())
            for (m in natives) {
                assertTrue(Modifier.isStatic(m.modifiers), "${m.name} must be static")
                assertTrue(Modifier.isPublic(m.modifiers), "${m.name} must be public")
                assertEq(expected.getValue(m.name), descriptor(m), m.name)
            }
            assertEq("dev.keel.runtime.KeelNative", KeelNative::class.java.name)
        }

        case("KeelNative.Callbacks is the interface the shim calls back into") {
            val cls = KeelNative.Callbacks::class.java
            assertEq("dev.keel.runtime.KeelNative\$Callbacks", cls.name)
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
            assertEq("keel.native.name", KeelNative.NAME_PROPERTY)
            assertEq("keel.native.path", KeelNative.PATH_PROPERTY)
            assertEq("keel_core", KeelNative.DEFAULT_NAME)
        }

        case("without the native library, isAvailable is false and says why; nothing throws") {
            if (KeelNative.isAvailable) skip("the native library is loadable here; see NativeSmokeTests")
            assertTrue(KeelNative.unavailableReason is UnsatisfiedLinkError, "reason: ${KeelNative.unavailableReason}")
            val e = assertThrows<KeelException> {
                KeelCore.load(LoadOptions(expectedSchemaHash = 1uL, defaultAdapters = false))
            }
            assertTrue(e.cause is UnsatisfiedLinkError)
            assertTrue(e.message!!.contains("Mode.REMOTE"), e.message!!)
        }
    }

    @Test
    fun allCases() = assertPassed()
}

/**
 * Talks to the real native core through JNI. It runs only when `keel_core` can be loaded (put the library on
 * `java.library.path`, or set `keel.native.path`; `scripts/test-local.sh` forwards `KEEL_NATIVE_LIB_DIR` and
 * `KEEL_NATIVE_NAME`), so until `keel-ffi` lands it reports itself as skipped.
 *
 * It uses only what any core has: the wire contract. Calls to a method id that cannot exist must come
 * back as `BAD_REQUEST` replies, which exercises the reply path (direct buffer, decoding, resumption)
 * without knowing the core's schema.
 */
class NativeSmokeTests : Suite() {
    init {
        case("the library speaks ABI 1 and reports a schema") {
            if (!KeelNative.isAvailable) skip("no native library: ${KeelNative.unavailableReason?.message}")
            assertEq(1, KeelNative.abiVersion())
            val json = String(KeelNative.schemaJson(), Charsets.UTF_8)
            assertTrue(json.trimStart().startsWith("{"), "schema JSON: ${json.take(80)}")
        }

        case("load, unknown calls fail as bad requests through both paths, stats and snapshot work, then close") {
            if (!KeelNative.isAvailable) skip("no native library: ${KeelNative.unavailableReason?.message}")
            val dataDir = java.nio.file.Files.createTempDirectory("keel-smoke")
            System.setProperty("keel.data.dir", dataDir.toString())
            val core = KeelCore.load(LoadOptions(expectedSchemaHash = KeelNative.schemaHash().toULong()))
            try {
                val unknown = CallTarget.FreeFunction(0xDEADBEEFu)
                val sync = assertThrows<KeelReplyException> { core.callSync(unknown, 0xDEADBEEFu, ByteArray(0)) }
                assertEq(ReplyStatus.BAD_REQUEST, sync.status)
                val async = assertThrows<KeelReplyException> { runBlocking { core.call(unknown, 0xDEADBEEFu, ByteArray(0)) } }
                assertEq(ReplyStatus.BAD_REQUEST, async.status)
                assertTrue(async.badRequestReason != null, "the reason is a readable string")
                // Unknown handles are ignored by the core, not fatal.
                core.observe(0x7777777700000001L, 0u, true)
                core.release(0x7777777700000001L)
                val stats = core.stats()
                assertTrue(stats.liveHandles >= 0, "stats: $stats")
                assertTrue(stats.raw.contains("live_handles"), stats.raw)
                Payloads.Snapshot.decode(core.snapshot())
                assertTrue(Codecs.u32.encodeToBytes().isNotEmpty())
            } finally {
                core.close()
            }
            // A second in-process core in the same process is refused, by design.
            assertThrows<KeelException> { KeelCore.load(LoadOptions(expectedSchemaHash = KeelNative.schemaHash().toULong())) }
        }
    }

    private fun dev.keel.runtime.wire.KeelCodec<UInt>.encodeToBytes(): ByteArray =
        dev.keel.runtime.wire.KeelWriter().also { encode(it, 1u) }.toByteArray()

    @Test
    fun allCases() = assertPassed()
}
