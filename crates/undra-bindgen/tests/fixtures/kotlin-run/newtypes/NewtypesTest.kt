// Runs the generated Kotlin of the `newtypes` golden case (ADR-042): a newtype is its inner value on the wire,
// compares and hashes as it does, and is `Comparable` only where the order of the inner type means something.

package golden.newtypes

import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.Timestamp
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import golden.support.expect
import golden.support.expectEq
import golden.support.hex
import java.util.UUID
import kotlin.time.Duration.Companion.milliseconds

private fun <N, V> sameBytes(newtype: UndraCodec<N>, value: N, inner: UndraCodec<V>, innerValue: V, message: String) {
    expectEq(newtype.encodeToByteArray(value).hex(), inner.encodeToByteArray(innerValue).hex(), message)
}

fun main() {
    val uuid = UUID(0L, 0xabL)
    sameBytes(UserId, UserId(uuid), Codecs.uuid, uuid, "UserId")
    sameBytes(TodoId, TodoId("t"), Codecs.string, "t", "TodoId")
    sameBytes(OrderNo, OrderNo(7uL), Codecs.u64, 7uL, "OrderNo")
    sameBytes(Meters, Meters(1.5), Codecs.f64, 1.5, "Meters")
    sameBytes(Timeout, Timeout(1500.milliseconds), Codecs.duration, 1500.milliseconds, "Timeout")
    sameBytes(Created, Created(Timestamp(1700000000000L)), Codecs.timestamp, Timestamp(1700000000000L), "Created")
    sameBytes(Flag, Flag(true), Codecs.bool, true, "Flag")
    sameBytes(Blob, Blob(byteArrayOf(1, 2, 3)), Codecs.bytes, byteArrayOf(1, 2, 3), "Blob")
    sameBytes(Tags, Tags(listOf("a", "b")), Codecs.vec(Codecs.string), listOf("a", "b"), "Tags")
    sameBytes(Nickname, Nickname("n"), Codecs.option(Codecs.string), "n", "Nickname")
    expectEq(Nickname.encodeToByteArray(Nickname(null)).hex(), "00", "an empty Nickname")
    sameBytes(Level, Level(Priority.HIGH), Priority, Priority.HIGH, "Level")
    // Newtypes of newtypes, and an option of one, are the same bytes again.
    val boss = Boss(Owner(UserId(uuid)))
    sameBytes(Boss, boss, Codecs.uuid, uuid, "Boss")
    sameBytes(Maybe, Maybe(UserId(uuid)), Codecs.option(Codecs.uuid), uuid, "Maybe")
    expectEq(Maybe.encodeToByteArray(Maybe(null)).hex(), "00", "an empty Maybe")
    val todo = Todo(TodoId("t"), "x", UserId(uuid), Created(Timestamp(5L)), Level(Priority.LOW))
    sameBytes(Wrapped, Wrapped(todo), Todo, todo, "Wrapped")

    // They round trip, and compare and hash by value.
    expectEq(UserId.decodeAll(UserId.encodeToByteArray(UserId(uuid))), UserId(uuid), "UserId round trip")
    expectEq(boss.value.value.value, uuid, "unwrapping")
    expectEq(UserId(uuid).hashCode(), UserId(uuid).hashCode(), "hash")
    expect(UserId(uuid) != UserId(UUID(0L, 1L)), "different ids differ")
    val byUser = mapOf(UserId(uuid) to Meters(2.0))
    expectEq(byUser[UserId(UUID(0L, 0xabL))], Meters(2.0), "a newtype is a map key by value")

    // A newtype of bytes compares the bytes' content, which a value class cannot do.
    expectEq(Blob(byteArrayOf(1, 2)), Blob(byteArrayOf(1, 2)), "bytes by content")
    expectEq(Blob(byteArrayOf(1, 2)).hashCode(), Blob(byteArrayOf(1, 2)).hashCode(), "bytes hash by content")
    expect(Blob(byteArrayOf(1, 2)) != Blob(byteArrayOf(1, 3)), "different bytes differ")
    expectEq(Blob(byteArrayOf(1)).toString(), "Blob(value=[1])", "bytes string")

    // The order of the inner type, where it means something.
    expect(Meters(1.0) < Meters(2.0), "Meters")
    expect(Span(Meters(1.0)) < Span(Meters(2.0)), "a newtype of an ordered newtype")
    expect(TodoId("a") < TodoId("b"), "TodoId")
    expect(OrderNo(1uL) < OrderNo(2uL), "OrderNo")
    expect(Timeout(1.milliseconds) < Timeout(2.milliseconds), "Timeout")
    expect(Created(Timestamp(1L)) < Created(Timestamp(2L)), "Created")
    expectEq(listOf(Meters(3.0), Meters(1.0)).sorted(), listOf(Meters(1.0), Meters(3.0)), "sorted")

    // A whole record, map keys and all.
    val account = Account(
        id = UserId(uuid), nickname = Nickname(null), tags = Tags(listOf("x")), boss = boss,
        friends = listOf(UserId(uuid)), scores = mapOf(UserId(uuid) to Meters(3.0)),
        byOrder = mapOf(OrderNo(9uL) to UserId(uuid)), timeout = Timeout(1.milliseconds),
        blob = Blob(byteArrayOf(9)), flag = Flag(false), maybe = Maybe(null), reach = Span(Meters(4.0)),
    )
    val back = Account.decodeAll(Account.encodeToByteArray(account))
    expectEq(back, account, "Account round trip")
    expectEq(back.scores[UserId(uuid)], Meters(3.0), "a decoded map is keyed by value")
    println("ok")
}
