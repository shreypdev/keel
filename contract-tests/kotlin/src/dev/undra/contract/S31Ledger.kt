package dev.undra.contract

import dev.undra.playground.core.Account
import dev.undra.playground.core.AccountId
import dev.undra.playground.core.Cents
import dev.undra.playground.core.Ledger
import dev.undra.playground.core.LedgerError
import dev.undra.playground.core.LoadableEntries
import dev.undra.playground.core.Price
import dev.undra.playground.core.Receipt
import dev.undra.playground.core.UndraIds
import dev.undra.playground.core.balances
import dev.undra.playground.core.deposit
import dev.undra.playground.core.echoAccount
import dev.undra.playground.core.echoCents
import dev.undra.playground.core.echoDecimal
import dev.undra.playground.core.echoPrice
import dev.undra.playground.core.echoReceipt
import dev.undra.playground.core.loadableStatement
import dev.undra.playground.core.openAccount
import dev.undra.playground.core.sampleReceipt
import dev.undra.playground.core.statement
import dev.undra.runtime.UndraReplyException
import dev.undra.runtime.wire.Codecs
import dev.undra.runtime.wire.KeyedPatch
import dev.undra.runtime.wire.PatchOp
import dev.undra.runtime.wire.Payloads.CallTarget
import dev.undra.runtime.wire.Payloads.ChangeOp
import dev.undra.runtime.wire.Timestamp
import dev.undra.runtime.wire.UndraCodec
import dev.undra.runtime.wire.UndraWriter
import dev.undra.runtime.wire.decodeAll
import dev.undra.runtime.wire.encodeToByteArray
import java.math.BigDecimal
import java.math.BigInteger
import java.util.UUID
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds

/**
 * S31 (ADR-042): newtypes are their inner value on the wire and a distinct type on the platform (a `@JvmInline value class`),
 * a newtype keys a map and a keyed list, generic instantiations are plain records and enums, decimals are exact, and the
 * leaf types of other crates cross as wire types. The ledger of the playground core, through the generated API.
 */
fun s31Ledger(w: World) {
    newtypes()
    keys(w)
    val account = instantiations()
    decimals(w, account)
    leafTypes()
}

private val ALL_ZERO_UUID = UUID(0L, 0L)

/** The runtime class of [value], boxed as it is when it is passed around as an `Any`. */
private fun classOf(value: Any): Class<*> = value.javaClass

/** Encodes [value] with [codec] into a fresh array. */
private fun <T> bytes(codec: UndraCodec<T>, value: T): ByteArray = codec.encodeToByteArray(value)

private fun price(text: String): Price = Price(BigDecimal(text))

/** 1. A newtype is its inner value on the wire; the platform type is distinct, and `Cents` is comparable. */
private fun newtypes() {
    val uuid = UUID.fromString("1ed9e400-0000-0000-0000-0000000000aa")
    val id = AccountId(uuid)
    expectEq("an AccountId's bytes are the Uuid's", bytes(Codecs.uuid, uuid), bytes(AccountId, id))
    expectEq("an AccountId is 16 bytes", 16, bytes(AccountId, id).size)
    expectEq("Cents(-5)'s bytes are an i64's", bytes(Codecs.i64, -5L), bytes(Cents, Cents(-5L)))
    expectEq("Cents is 8 bytes", 8, bytes(Cents, Cents(-5L)).size)
    val amount = BigDecimal("-1.50")
    expectEq("a Price's bytes are a Decimal's", bytes(Codecs.decimal, amount), bytes(Price, Price(amount)))
    expectEq("a Price is 17 bytes", 17, bytes(Price, Price(amount)).size)
    expectEq("a newtype decodes from its inner bytes", id, AccountId.decodeAll(bytes(Codecs.uuid, uuid)))

    expectEq("echo_account returns an equal AccountId", id, echoAccount(id))
    expectEq("echo_cents returns an equal Cents", Cents(-5L), echoCents(Cents(-5L)))
    expectEq("echo_cents at the ends of i64", listOf(Cents(Long.MIN_VALUE), Cents(Long.MAX_VALUE)), listOf(Cents(Long.MIN_VALUE), Cents(Long.MAX_VALUE)).map { echoCents(it) })
    expectEq("echo_price returns an equal Price", price("12.340"), echoPrice(price("12.340")))

    // The platform type is a distinct type: a value class is not its inner UUID or Long, and not another newtype.
    check(classOf(id) == AccountId::class.java && classOf(id) != UUID::class.java) { "an AccountId is its own type, not a UUID: ${classOf(id)}" }
    check(AccountId::class.java != UUID::class.java) { "AccountId and UUID are the same class" }
    check(AccountId::class.java.isAnnotationPresent(JvmInline::class.java)) { "AccountId is not a @JvmInline value class" }
    check(Cents::class.java.isAnnotationPresent(JvmInline::class.java)) { "Cents is not a @JvmInline value class" }
    check(Price::class.java.isAnnotationPresent(JvmInline::class.java)) { "Price is not a @JvmInline value class" }
    expectEq("an AccountId's value", uuid, id.value)
    check(Cents(1) < Cents(2) && Cents(2) > Cents(1) && Cents(-5) < Cents(0)) { "Cents is not ordered by its value" }
    expectEq("Cents(2) compared with Cents(2)", 0, Cents(2).compareTo(Cents(2)))
    expectEq("sorted Cents", listOf(Cents(-7), Cents(0), Cents(3)), listOf(Cents(3), Cents(-7), Cents(0)).sorted())
}

/** The operations one account of the `Ledger` store goes through, as the model of the list it should mirror. */
private fun keys(w: World) {
    // 2a. A newtype is a map key: `balances()` has exactly the two accounts that were opened.
    val ada = openAccount("Ada")
    val bob = openAccount("Bob")
    check(ada != bob) { "two accounts share the identity $ada" }
    val all = balances()
    expectEq("the keys of balances()", setOf(ada, bob), all.keys)
    check(all.getValue(ada).value.signum() == 0 && all.getValue(bob).value.signum() == 0) { "a new account holds money: $all" }

    // 2b. A newtype keys a list: every operation of the `Ledger` store is one keyed operation, and the mirror follows.
    val ledger = Ledger.create()
    val raw = RawStore(w.core, UndraIds.Objects.Ledger.TYPE_ID, UndraIds.Objects.Ledger.NEW)
    raw.observe()
    val initial = raw.entries.single { it.signalId == 0u }
    expectEq("the op of the initial accounts entry", ChangeOp.FULL, initial.op)
    val shown = ArrayList(Codecs.vec(Account).decodeAll(initial.value)) // what a host that applies the patches holds
    expectEq("the initial accounts", emptyList<Account>(), shown.toList())
    val model = ArrayList<Account>()

    /** Runs [call] on the raw store; it must deliver one patch of exactly [expected], and the patched list must equal the model. */
    fun step(what: String, expected: List<PatchOp<Account>>, call: () -> Unit): ByteArray {
        val mark = raw.mark()
        call()
        flushMainThread()
        val entry = raw.since(mark).single()
        expectEq("$what: the signal of the only entry", 0u, entry.signalId)
        expectEq("$what: the op of the accounts entry", ChangeOp.PATCH, entry.op)
        val ops = KeyedPatch.decodePatch(entry.value, Account)
        expectEq("$what: the operations", expected, ops)
        val patched = KeyedPatch.applyPatch(shown, ops)
        shown.clear()
        shown.addAll(patched)
        expectEq("$what: the list the patch produces", model.toList(), shown.toList())
        return entry.value
    }

    fun rawOpen(owner: String): AccountId =
        AccountId.decodeAll(raw.callSync(UndraIds.Objects.Ledger.OPEN, UndraWriter().also { it.writeStr(owner) }.toByteArray()))

    val a = ledger.open("Ada")
    var rawA: AccountId? = null
    model.add(Account(a, "Ada"))
    val insert = step("open(\"Ada\")", listOf(PatchOp.Insert(0u, Account(a, "Ada")))) { rawA = rawOpen("Ada") }
    check(insert.size < 64) { "the patch of one insert is ${insert.size} bytes" }
    expectEq("the identity the raw store's open returned", a, rawA)
    val b = ledger.open("Bob")
    model.add(Account(b, "Bob"))
    step("open(\"Bob\")", listOf(PatchOp.Insert(1u, Account(b, "Bob")))) { rawOpen("Bob") }

    ledger.rename(a, "Ada L.")
    model[0] = Account(a, "Ada L.")
    step("rename(Ada, \"Ada L.\")", listOf(PatchOp.Update(0u, Account(a, "Ada L.")))) {
        raw.callSync(UndraIds.Objects.Ledger.RENAME, UndraWriter().also { AccountId.encode(it, a); it.writeStr("Ada L.") }.toByteArray())
    }

    ledger.closeAccount(b)
    model.removeAt(1)
    step("close_account(Bob)", listOf(PatchOp.Remove(1u))) {
        raw.callSync(UndraIds.Objects.Ledger.CLOSE_ACCOUNT, UndraWriter().also { AccountId.encode(it, b) }.toByteArray())
    }

    // An account that is not there changes nothing: no entry at all.
    val before = raw.mark()
    raw.callSync(UndraIds.Objects.Ledger.CLOSE_ACCOUNT, UndraWriter().also { AccountId.encode(it, b) }.toByteArray())
    flushMainThread()
    expectEq("closing an account twice delivers nothing", emptyList<Any>(), raw.since(before).map { it.toString() })

    // The generated store's own mirror equals the model after all of it.
    awaitEq("the generated Ledger's accounts", model.toList()) { ledger.accounts.value }
    raw.close()
    ledger.close()
}

/** 3. Generic instantiations are plain records and enums. Returns the account the entries went into. */
private fun instantiations(): AccountId {
    val account = openAccount("Cleo")
    for (n in 0 until 5) deposit(account, price("1.50"), "#$n", Timestamp(n.toLong()))

    val first = statement(account, 0u, 2u)
    expectEq("statement(a, 0, 2): the items", 2, first.items.size)
    expectEq("statement(a, 0, 2): next", 2u, first.next)
    expectEq("an entry's account", account, first.items[0].account)
    expectEq("an entry's amount", price("1.50"), first.items[0].amount)
    expectEq("an entry's memo", "#0", first.items[0].memo)
    expectEq("an entry's time", Timestamp(1L), first.items[1].at)
    val last = statement(account, 4u, 10u)
    expectEq("statement(a, 4, 10): the items", 1, last.items.size)
    expectEq("statement(a, 4, 10): next", null, last.next)
    expectEq("statement past the end", 0, statement(account, 99u, 1u).items.size)

    expectEq("loadable_statement(a, false, 3)", LoadableEntries.Loading, loadableStatement(account, false, 3u))
    val loaded = loadableStatement(account, true, 3u)
    check(loaded is LoadableEntries.Loaded) { "loadable_statement(a, true, 3) is $loaded" }
    expectEq("the loaded slice's items", 3, (loaded as LoadableEntries.Loaded).value.items.size)
    expectEq("the loaded slice's next", 3u, loaded.value.next)
    val unknown = loadableStatement(AccountId(UUID.fromString("09090909-0909-0909-0909-090909090909")), true, 3u)
    expectEq("loadable_statement of an unknown account", LoadableEntries.Failed("no such account"), unknown)
    return account
}

/** 4. Decimals are exact. */
private fun decimals(w: World, account: AccountId) {
    val fresh = openAccount("Dev")
    expectEq("deposit(0.1) returns the balance", price("0.1"), deposit(fresh, price("0.1"), "a", Timestamp(1L)))
    val sum = deposit(fresh, price("0.2"), "b", Timestamp(2L))
    expectEq("0.1 + 0.2 as text", "0.3", sum.value.toPlainString())
    expectEq("the balances map holds the sum", "0.3", balances().getValue(fresh).value.toPlainString())

    val maxMantissa = BigInteger.ONE.shiftLeft(127).subtract(BigInteger.ONE)
    val minMantissa = BigInteger.ONE.shiftLeft(127).negate()
    for ((what, value) in listOf(
        "0" to BigDecimal("0"),
        "1.10" to BigDecimal("1.10"),
        "-1.50" to BigDecimal("-1.50"),
        "a scale of 38" to BigDecimal(BigInteger("123456789012345678901234567890123456789"), 38),
        "the smallest 128-bit mantissa" to BigDecimal(minMantissa, 0),
        "the largest 128-bit mantissa" to BigDecimal(maxMantissa, 0),
        "the largest mantissa at a scale of 38" to BigDecimal(maxMantissa, 38),
    )) {
        val back = echoDecimal(value)
        expectEq("echo_decimal($what) is equal, scale included", value, back)
        expectEq("echo_decimal($what): the scale", value.scale(), back.scale())
        expectEq("echo_decimal($what): the bytes", bytes(Codecs.decimal, value), bytes(Codecs.decimal, back))
    }
    check(BigDecimal("1.1") != BigDecimal("1.10")) { "BigDecimal.equals ignores the scale, so this step proves nothing" }

    // A wire decimal with a scale of 39 is refused by the core as a bad request, never decoded.
    val raw39 = ByteArray(17).also {
        it[0] = 1 // the mantissa, little-endian
        it[16] = 39 // the scale
    }
    val badRequests = w.stats().badRequests
    val refused = expectFails<UndraReplyException>("echo_decimal with a scale of 39") {
        w.core.callSync(CallTarget.FreeFunction(UndraIds.Functions.ECHO_DECIMAL), UndraIds.Functions.ECHO_DECIMAL, raw39)
    }
    expectBadRequest("echo_decimal with a scale of 39", refused)
    expectEq("bad_requests grown by the refused decimal", 1L, w.stats().badRequests - badRequests)
    // The same value at the largest scale the wire allows is fine.
    expectEq("a scale of 38 through the raw API", BigDecimal(BigInteger.ONE, 38), Codecs.decimal.decodeAll(
        w.core.callSync(CallTarget.FreeFunction(UndraIds.Functions.ECHO_DECIMAL), UndraIds.Functions.ECHO_DECIMAL, raw39.copyOf().also { it[16] = 38 }),
    ))

    // An amount `rust_decimal` cannot hold (a mantissa of more than 96 bits) is the ledger's own error.
    val tooBig = Price(BigDecimal(BigInteger.ONE.shiftLeft(100), 0))
    val before = balances().getValue(account)
    expectFails<LedgerError.OutOfRange>("deposit of a mantissa of 101 bits") { deposit(account, tooBig, "huge", Timestamp(0L)) }
    expectEq("the refused deposit changed nothing", before, balances().getValue(account))
    expectFails<LedgerError.NoSuchAccount>("deposit into an unknown account") { deposit(AccountId(ALL_ZERO_UUID), price("1"), "x", Timestamp(0L)) }
}

/** 5. Leaf types of other crates cross as wire types. */
private fun leafTypes() {
    val receipt = sampleReceipt()
    expectEq("the receipt's id", UUID.fromString("1ed9e400-0000-0000-0000-000000000007"), receipt.id)
    expectEq("the receipt's issue time in milliseconds", 1_790_856_000_123L, receipt.issued.epochMillis)
    expectEq("the receipt's issue time", "2026-10-01T12:00:00.123Z", receipt.issued.toInstant().toString())
    expectEq("the receipt's validity", 90.seconds, receipt.validFor)
    expectEq("the receipt's total", BigDecimal("19.990"), receipt.total)
    expectEq("the receipt's total keeps its scale", 3, receipt.total.scale())
    expectEq("the receipt's signature", byteArrayOf(1, 2, 3, -1), receipt.signature)

    val built = Receipt(
        id = UUID.fromString("00000000-0000-0000-0000-0000000000ff"),
        issued = Timestamp(-86_400_123L),
        validFor = 1_500.milliseconds,
        total = BigDecimal("-0.0010"),
        signature = byteArrayOf(0, 127, -128, -1),
    )
    expectEq("echo_receipt of a receipt the platform built", built, echoReceipt(built))
    expectEq("echo_receipt of the sample", receipt, echoReceipt(receipt))
}
