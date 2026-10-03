package dev.undra.runtime

import dev.undra.fixture.UndraCoreNative
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

/** The natives `undra-ffi`'s `JNI_OnLoad` registers on a core's `UndraCoreNative` (SPEC 6.1), by name and descriptor. */
private val NATIVES = mapOf(
    "abiVersion" to "()I",
    "schemaHash" to "()J",
    "schemaJson" to "()[B",
    "init" to "([BLdev/undra/runtime/NativeCallbacks;)I",
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

/** Checks that [cls] declares exactly [NATIVES] as public natives, with their descriptors. */
private fun assertNativeShape(cls: Class<*>) {
    val natives = cls.declaredMethods.filter { Modifier.isNative(it.modifiers) }
    assertEq(NATIVES.keys.sorted(), natives.map { it.name }.sorted(), "the natives of ${cls.name}")
    for (m in natives) {
        assertTrue(Modifier.isPublic(m.modifiers), "${cls.name}.${m.name} must be public")
        assertEq(NATIVES.getValue(m.name), descriptor(m), "${cls.name}.${m.name}")
    }
}

/**
 * Pins the JNI surface of SPEC 6.1 and ADR-044 exactly. `undra-ffi` registers these names and descriptors on each
 * core's own `UndraCoreNative` with `RegisterNatives`, and calls the [NativeCallbacks] methods by name and
 * descriptor, so a change here that this test does not catch would surface as a failed `JNI_OnLoad` (an
 * `UnsatisfiedLinkError`) or a `NoSuchMethodError` inside the native library.
 */
class NativeShapeTests : Suite() {
    init {
        case("a core's UndraCoreNative (the fixture's, declared as bindgen generates it) has exactly the natives of SPEC 6.1") {
            val cls = UndraCoreNative::class.java
            // The class the fixture's export_core!(undra_fixture, jni_class = "dev/undra/fixture/UndraCoreNative") names.
            assertEq("dev.undra.fixture.UndraCoreNative", cls.name)
            assertNativeShape(cls)
            assertTrue(NativeApi::class.java.isAssignableFrom(cls), "it implements NativeApi")
        }

        case("the generated UndraCoreNative of the golden `full` bindings has the same natives") {
            val cls = try {
                // Not initialized: that would load libplayground_core, which is not around here.
                Class.forName("golden.full.UndraCoreNative", false, NativeShapeTests::class.java.classLoader)
            } catch (e: ClassNotFoundException) {
                skip("golden.full.UndraCoreNative is not on the classpath (bindgen sources not found)")
            }
            assertNativeShape(cls)
            assertTrue(NativeApi::class.java.isAssignableFrom(cls), "it implements NativeApi")
        }

        case("NativeApi is the natives plus the namespace and the availability, nothing else") {
            val methods = NativeApi::class.java.declaredMethods
            val natives = methods.filter { it.name in NATIVES }
            for (m in natives) assertEq(NATIVES.getValue(m.name), descriptor(m), "NativeApi.${m.name}")
            assertEq(
                (NATIVES.keys + listOf("getNamespace", "isAvailable", "getUnavailableReason")).sorted(),
                methods.map { it.name }.sorted(),
            )
        }

        case("NativeCallbacks is the interface the shim calls back into") {
            val cls = NativeCallbacks::class.java
            assertEq("dev.undra.runtime.NativeCallbacks", cls.name)
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

        case("the runtime's R8 consumer rules keep NativeCallbacks and the methods of its implementations") {
            val rules = NativeShapeTests::class.java.classLoader.getResource("META-INF/proguard/undra-runtime.pro")
                ?: skip("META-INF/proguard/undra-runtime.pro is not on the test classpath")
            val text = rules.readText()
            val name = NativeCallbacks::class.java.name
            assertTrue(text.contains("-keep interface $name { *; }"), text)
            assertTrue(text.contains("-keepclassmembers class * implements $name {"), text)
        }

        case("NativeLibrary.load reports a missing library instead of throwing, by name and by path") {
            val missing = "undra_no_such_core_${System.nanoTime()}"
            val byName = NativeLibrary.load(missing)
            assertTrue(byName is UnsatisfiedLinkError, "by name: $byName")
            assertTrue(byName!!.message!!.contains(missing), byName.message!!)

            val property = "undra.native.$missing.path"
            assertEq(property, NativeLibrary.pathProperty(missing))
            val file = java.io.File(System.getProperty("java.io.tmpdir"), "lib$missing.dylib").absolutePath
            System.setProperty(property, file)
            try {
                val byPath = NativeLibrary.load(missing)
                assertTrue(byPath is UnsatisfiedLinkError, "by path: $byPath")
                assertTrue(byPath!!.message!!.contains(file), "the property's file is what was tried: ${byPath.message}")
                // A relative path (Bazel's `$(rootpath ..)` in a test's jvm_flags) is taken from the working directory.
                val relative = "relative/lib$missing.so"
                System.setProperty(property, relative)
                val byRelative = NativeLibrary.load(missing)
                assertTrue(byRelative is UnsatisfiedLinkError, "a missing relative path is reported, not thrown: $byRelative")
                val resolved = java.io.File(relative).absolutePath
                assertTrue(byRelative!!.message!!.contains(resolved), "it was tried from the working directory: ${byRelative.message}")
            } finally {
                System.clearProperty(property)
            }
        }

        case("without the fixture library, its natives are unavailable and say why; loading refuses with a hint; nothing throws") {
            if (UndraCoreNative.isAvailable) skip("the fixture library is loadable here; see NativeSmokeTests")
            val sharedBefore = UndraCore.current
            assertTrue(UndraCoreNative.unavailableReason is UnsatisfiedLinkError, "reason: ${UndraCoreNative.unavailableReason}")
            val direct = assertThrows<UndraException> {
                UndraCore.load(LoadOptions(expectedSchemaHash = 1uL, defaultAdapters = false), UndraCoreNative)
            }
            assertTrue(direct.cause is UnsatisfiedLinkError)
            for (hint in listOf("`undra_fixture`", "-Dundra.native.undra_fixture.path=", "Mode.REMOTE")) {
                assertTrue(direct.message!!.contains(hint), "the message should mention $hint: ${direct.message}")
            }
            val entry = CoreEntry(UndraCoreNative.NAMESPACE, 1uL) { UndraCoreNative }
            val viaEntry = assertThrows<UndraException> { entry.load(LoadOptions(defaultAdapters = false)) }
            assertTrue(viaEntry.cause is UnsatisfiedLinkError)
            assertThrows<UndraTransportException> { entry.core.callSync(CallTarget.FreeFunction(1u), 1u, ByteArray(0)) }
            assertTrue(UndraCore.current === sharedBefore, "a failed load never becomes shared")
        }
    }

    @Test
    fun allCases() = assertPassed()
}

/**
 * Talks to a real native core through JNI: `undra-ffi`'s fixture core, through its natives declared like a
 * generated `UndraCoreNative` ([UndraCoreNative]). It runs only when the fixture library can be loaded (build it
 * with `cargo build --manifest-path crates/undra-ffi/tests/fixture/Cargo.toml`, then point `scripts/test-local.sh`
 * at it with `UNDRA_NATIVE_LIB_DIR=crates/undra-ffi/tests/fixture/target/debug`, or set
 * `-Dundra.native.undra_fixture.path`); otherwise it reports itself as skipped. `crates/undra-ffi/tests/jni`
 * drives every callback of the same core with real payloads.
 *
 * It uses only what any core has: the wire contract. Calls to a method id that cannot exist must come
 * back as `BAD_REQUEST` replies, which exercises the reply path (direct buffer, decoding, resumption)
 * without knowing the core's schema.
 */
class NativeSmokeTests : Suite() {
    private fun requireFixture() {
        if (!UndraCoreNative.isAvailable) skip("no fixture library: ${UndraCoreNative.unavailableReason?.message}")
    }

    init {
        case("the library speaks ABI 2 and reports its schema") {
            requireFixture()
            assertEq(2, UndraCoreNative.abiVersion())
            val json = String(UndraCoreNative.schemaJson(), Charsets.UTF_8)
            assertTrue(json.trimStart().startsWith("{") && json.contains("Calculator"), "schema JSON: ${json.take(80)}")
            assertTrue(UndraCoreNative.schemaHash() != 0L)
        }

        case("load through the entry, unknown calls fail as bad requests through both paths, stats and snapshot work, then close") {
            requireFixture()
            val dataDir = java.nio.file.Files.createTempDirectory("undra-smoke")
            System.setProperty("undra.data.dir", dataDir.toString())
            val entry = CoreEntry(UndraCoreNative.NAMESPACE, UndraCoreNative.schemaHash().toULong()) { UndraCoreNative }
            val core = entry.load()
            try {
                assertTrue(entry.core === core, "the entry's core is the loaded one")
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
                assertEq(UndraCoreNative.schemaHash().toULong(), snapshot.schemaHash)
                assertTrue(snapshot.stores.all { snapshot.fingerprint(it.typeId) != null }, "every store's type is listed")
                core.restore(snapshotBytes)
                // A snapshot in the layout before ADR-037 (count, floor) is refused as malformed, and the core is unchanged.
                assertEq(UndraRestoreException.BAD_SNAPSHOT, assertThrows<UndraRestoreException> { core.restore(ByteArray(8)) }.code)
                assertTrue(Codecs.u32.encodeToBytes().isNotEmpty())
                // One in-process core per namespace: the entry and a direct load are both refused while it runs.
                assertThrows<UndraException> { entry.load() }
                val twin = assertThrows<UndraException> {
                    UndraCore.load(LoadOptions(expectedSchemaHash = UndraCoreNative.schemaHash().toULong()), UndraCoreNative)
                }
                assertTrue(twin.message!!.contains("`undra_fixture` is already loaded"), twin.message!!)
            } finally {
                core.close()
            }
            assertTrue(entry.core !== core, "after close the entry's core is the placeholder")
            // Closing ended the core's work (ADR-034): a new load in the same process starts a fresh core.
            val again = entry.load(LoadOptions())
            try {
                assertTrue(entry.core === again)
                assertEq(0, again.stats().liveHandles, "a fresh core has no handles")
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
