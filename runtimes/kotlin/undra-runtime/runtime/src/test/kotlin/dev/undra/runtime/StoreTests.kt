package dev.undra.runtime

import dev.undra.runtime.support.FakeTransport
import dev.undra.runtime.support.HASH
import dev.undra.runtime.support.LogCapture
import dev.undra.runtime.support.attach
import dev.undra.runtime.support.changeSet
import dev.undra.runtime.support.eventually
import dev.undra.runtime.support.full
import dev.undra.runtime.support.patch
import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertTrue
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.UndraReader
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.KeyedPatch
import dev.undra.runtime.wire.PatchOp
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.WireException
import dev.undra.runtime.wire.encodeToByteArray
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.junit.jupiter.api.Test

private data class Item(val id: UInt, val name: String) {
    companion object : UndraCodec<Item> {
        override fun encode(w: UndraWriter, v: Item) {
            w.writeU32(v.id)
            w.writeStr(v.name)
        }

        override fun decode(r: UndraReader): Item = Item(r.readU32(), r.readStr())
    }
}

private val itemList = Codecs.vec(Item)

/**
 * A store shaped exactly like the generated ones (see the golden `Stores.kt`): flows created with
 * `signal(...)`, `apply` decoding by signal id, a keyed patch on the list, a resync on a bad patch, and an
 * `observe` in `init` after the fields exist.
 */
private class CounterStore(core: UndraCore, handle: Long) : UndraStore(core, handle) {
    private val _count: MutableStateFlow<UInt> = signal(0u)
    val count: StateFlow<UInt> = _count.asStateFlow()
    private val _items: MutableStateFlow<List<Item>> = signal(emptyList())
    val items: StateFlow<List<Item>> = _items.asStateFlow()
    val appliedOn = CopyOnWriteArrayList<String>()

    init {
        core.observe(handle, UInt.MAX_VALUE, true)
    }

    override fun apply(signalId: UInt, op: ChangeOp, reader: UndraReader) {
        appliedOn.add(Thread.currentThread().name)
        when (signalId) {
            0u -> if (op == ChangeOp.FULL) {
                _count.value = Codecs.u32.decode(reader)
                reader.finish()
            }
            1u -> {
                if (op == ChangeOp.FULL) {
                    _items.value = itemList.decode(reader)
                    reader.finish()
                } else if (op == ChangeOp.PATCH) {
                    val ops = KeyedPatch.decodePatch(reader, Item)
                    reader.finish()
                    try {
                        _items.value = KeyedPatch.applyPatch(_items.value, ops)
                    } catch (e: WireException.PatchOutOfBounds) {
                        core.observe(handle, 1u, false)
                        core.observe(handle, 1u, true)
                    }
                }
            }
            else -> Unit
        }
    }
}

private fun u32(v: Int): ByteArray = Codecs.u32.encodeToByteArray(v.toUInt())

class StoreTests : Suite() {
    init {
        case("signal() gives a StateFlow holding the placeholder until the core reports a value") {
            val t = FakeTransport()
            attach(t).use { core ->
                val store = CounterStore(core, 5L)
                assertEq(0u, store.count.value)
                assertEq(emptyList<Item>(), store.items.value)
                store.close()
            }
        }

        case("observe in INPROC applies the initial change-set before it returns, even from a thread that is not the main thread") {
            val t = FakeTransport()
            t.onObserve = { handle, _, on ->
                if (on) t.events.onChangeSet(changeSet(1uL, full(handle, 0u, u32(41)), full(handle, 1u, itemList.encodeToByteArray(listOf(Item(1u, "a"))))))
            }
            attach(t).use { core ->
                val store = CounterStore(core, 5L)
                // No waiting here: the values are there the moment the constructor returns.
                assertEq(41u, store.count.value)
                assertEq(listOf(Item(1u, "a")), store.items.value)
                assertEq(listOf("undra-main"), store.appliedOn.distinct())
                assertEq(listOf(Triple(5L, UInt.MAX_VALUE, true)), t.observes.toList())
                store.close()
            }
        }

        case("observe on the main thread itself applies inline") {
            val t = FakeTransport()
            t.onObserve = { handle, _, on -> if (on) t.events.onChangeSet(changeSet(1uL, full(handle, 0u, u32(7)))) }
            attach(t).use { core ->
                val result = CopyOnWriteArrayList<Any>()
                val done = CountDownLatch(1)
                UndraDispatchers.main.dispatch(kotlin.coroutines.EmptyCoroutineContext, Runnable {
                    val store = CounterStore(core, 6L)
                    result.add(store.count.value)
                    result.add(UndraDispatchers.isMainThread())
                    store.close()
                    done.countDown()
                })
                assertTrue(done.await(10, TimeUnit.SECONDS))
                assertEq(listOf<Any>(7u, true), result.toList())
            }
        }

        case("observe over a remote-style transport does not wait; changes arrive when the core sends them") {
            val t = FakeTransport(isSynchronous = false)
            attach(t).use { core ->
                val started = System.nanoTime()
                val store = CounterStore(core, 5L)
                assertTrue(System.nanoTime() - started < 1_000_000_000L, "observe must not block on a remote core")
                assertEq(0u, store.count.value)
                t.onCore { t.events.onChangeSet(changeSet(1uL, full(5L, 0u, u32(3)))) }
                eventually("the change is applied") { store.count.value == 3u }
                store.close()
            }
        }

        case("changes from the core thread update the flows on the main thread") {
            val t = FakeTransport()
            attach(t).use { core ->
                val store = CounterStore(core, 5L)
                t.onCore { t.events.onChangeSet(changeSet(1uL, full(5L, 0u, u32(1)))) }
                eventually("count becomes 1") { store.count.value == 1u }
                t.onCore { t.events.onChangeSet(changeSet(2uL, full(5L, 0u, u32(2)), full(5L, 1u, itemList.encodeToByteArray(listOf(Item(1u, "x"), Item(2u, "y")))))) }
                eventually("count becomes 2") { store.count.value == 2u }
                assertEq(listOf(Item(1u, "x"), Item(2u, "y")), store.items.value)
                assertTrue(store.appliedOn.all { it == "undra-main" }, "applied on ${store.appliedOn.distinct()}")
                store.close()
            }
        }

        case("keyed patches update a list in place") {
            val t = FakeTransport()
            attach(t).use { core ->
                val store = CounterStore(core, 5L)
                t.onCore {
                    t.events.onChangeSet(changeSet(1uL, full(5L, 1u, itemList.encodeToByteArray(listOf(Item(1u, "a"), Item(2u, "b"))))))
                    t.events.onChangeSet(
                        changeSet(
                            2uL,
                            patch(
                                5L,
                                1u,
                                KeyedPatch.encodePatch(listOf(PatchOp.Update(0u, Item(1u, "A")), PatchOp.Insert(2u, Item(3u, "c")), PatchOp.Remove(1u)), Item),
                            ),
                        ),
                    )
                }
                eventually("the patch is applied") { store.items.value == listOf(Item(1u, "A"), Item(3u, "c")) }
                store.close()
            }
        }

        case("a patch that does not fit makes the store re-observe the signal for a full value") {
            val t = FakeTransport()
            attach(t).use { core ->
                val store = CounterStore(core, 5L)
                t.observes.clear()
                t.onCore { t.events.onChangeSet(changeSet(1uL, patch(5L, 1u, KeyedPatch.encodePatch(listOf(PatchOp.Remove(9u)), Item)))) }
                eventually("the store resyncs") { t.observes.size == 2 }
                assertEq(listOf(Triple(5L, 1u, false), Triple(5L, 1u, true)), t.observes.toList())
                store.close()
            }
        }

        case("closing a store releases its handle, stops routing to it, and is idempotent") {
            val t = FakeTransport()
            attach(t).use { core ->
                val store = CounterStore(core, 5L)
                assertEq(1, core.stats().hostMirrorHandles)
                store.close()
                store.close()
                assertTrue(store.isClosed)
                assertEq(listOf(5L), t.releases.toList())
                assertEq(0, core.stats().hostMirrorHandles)
                t.onCore { t.events.onChangeSet(changeSet(1uL, full(5L, 0u, u32(9)))) }
                t.awaitCore()
                Thread.sleep(50)
                assertEq(0u, store.count.value, "a closed store no longer changes")
            }
        }

        case("a store that is dropped without close() is released by the cleaner, and its mirror entry goes away") {
            val t = FakeTransport()
            attach(t).use { core ->
                @Suppress("UNUSED_VALUE")
                var store: CounterStore? = CounterStore(core, 77L)
                assertEq(1, core.stats().hostMirrorHandles)
                store = null
                eventually("the handle is released after garbage collection", timeoutMs = 20_000) {
                    System.gc()
                    t.releases.contains(77L)
                }
                assertEq(0, core.stats().hostMirrorHandles)
                assertEq(1, t.releases.count { it == 77L }, "released exactly once")
            }
        }

        case("an object released explicitly is not released again when it is collected") {
            val t = FakeTransport()
            attach(t).use { core ->
                var store: CounterStore? = CounterStore(core, 78L)
                store!!.close()
                store = null
                repeat(5) {
                    System.gc()
                    Thread.sleep(20)
                }
                assertEq(1, t.releases.count { it == 78L })
            }
        }

        case("a null handle is never released") {
            val t = FakeTransport()
            attach(t).use { core ->
                val obj = object : UndraObject(core, 0L) {}
                obj.close()
                assertEq(0, t.releases.size)
            }
        }

        case("two stores on one core keep their own state") {
            val t = FakeTransport()
            attach(t).use { core ->
                val a = CounterStore(core, 1L)
                val b = CounterStore(core, 2L)
                t.onCore { t.events.onChangeSet(changeSet(1uL, full(1L, 0u, u32(10)), full(2L, 0u, u32(20)))) }
                eventually("both update") { a.count.value == 10u && b.count.value == 20u }
                a.close()
                t.onCore { t.events.onChangeSet(changeSet(2uL, full(1L, 0u, u32(11)), full(2L, 0u, u32(21)))) }
                eventually("b keeps following") { b.count.value == 21u }
                assertEq(10u, a.count.value)
                b.close()
            }
        }

        case("a failing signal does not stop the others in the same change-set") {
            val t = FakeTransport()
            LogCapture("dev.undra.runtime").use { log ->
                attach(t).use { core ->
                    val store = CounterStore(core, 5L)
                    t.onCore {
                        // Signal 0 carries garbage (a truncated u32), signal 1 is fine.
                        t.events.onChangeSet(changeSet(1uL, full(5L, 0u, byteArrayOf(1)), full(5L, 1u, itemList.encodeToByteArray(listOf(Item(1u, "ok"))))))
                    }
                    eventually("the good signal is applied") { store.items.value.isNotEmpty() }
                    assertEq(0u, store.count.value)
                    assertEq(1, log.records.count { it.thrown is WireException })
                    store.close()
                }
            }
        }

        case("the schema hash constant used by these tests is what the golden bindings declare") {
            assertEq(0x691eee0733e4a44fuL, HASH)
        }
    }

    @Test
    fun allCases() = assertPassed()
}
