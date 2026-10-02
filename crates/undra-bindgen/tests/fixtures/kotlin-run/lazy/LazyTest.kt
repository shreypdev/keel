// Runs the generated Kotlin of the `lazy` golden case (ADR-043): a `Lazy<T>` signal is the runtime's lazy list,
// which the store hands its change-set entries.

package golden.lazy

import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.UndraWriter
import golden.support.FakeCore
import golden.support.expect
import golden.support.expectEq

private fun value(handle: ULong, len: UInt, version: ULong): ByteArray {
    val w = UndraWriter()
    w.writeU64(handle)
    w.writeU32(len)
    w.writeU64(version)
    return w.toByteArray()
}

private fun invalidated(len: UInt, version: ULong): ByteArray {
    val w = UndraWriter()
    w.writeU32(len)
    w.writeU64(version)
    return w.toByteArray()
}

fun main() {
    val core = FakeCore()
    core.nextHandle = 4L
    val library = Library.create(core)

    // `books` and `recent` are lists, not flows; the ordinary signals are flows as ever.
    expectEq(library.books.size.value, 0, "an empty list")
    expectEq(library.recent.size.value, 0, "an empty derived list")
    expectEq(library.total.value, 0uL, "an ordinary signal")

    // `LazyValue`: handle u64, len u32, version u64; `LazyInvalidated`: len u32, version u64.
    core.deliver(4L, 1, ChangeOp.FULL, value(77uL, 120u, 1uL))
    expectEq(library.books.size.value, 120, "the length of the value")
    expectEq(library.recent.size.value, 0, "the other list is untouched")
    core.deliver(4L, 3, ChangeOp.FULL, value(78uL, 5u, 1uL))
    expectEq(library.recent.size.value, 5, "the derived list")
    core.deliver(4L, 1, ChangeOp.INVALIDATED, invalidated(121u, 2uL))
    expectEq(library.books.size.value, 121, "the length of the invalidation")
    // A keyed patch is not something a lazy list takes.
    core.deliver(4L, 1, ChangeOp.PATCH, ByteArray(0))
    expectEq(library.books.size.value, 121, "a patch is left alone")
    expect(core.reports.isEmpty(), "nothing failed: ${core.reports}")

    // A malformed entry is reported and skipped, never half applied.
    core.deliver(4L, 1, ChangeOp.FULL, byteArrayOf(1, 2, 3))
    expectEq(library.books.size.value, 121, "a malformed value changes nothing")
    expectEq(core.reports.size, 1, "and is reported")
    expect(core.reports[0].first.startsWith("Library.apply(signal: 1)"), core.reports[0].first)

    // The lists stop with the store.
    library.close()
    expect(library.isClosed, "closed")
    println("ok")
}
