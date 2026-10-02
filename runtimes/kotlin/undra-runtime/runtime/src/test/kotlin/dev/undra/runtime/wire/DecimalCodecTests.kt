package dev.undra.runtime.wire

import dev.undra.runtime.testing.Suite
import dev.undra.runtime.testing.assertEq
import dev.undra.runtime.testing.assertTrue
import org.junit.jupiter.api.Test
import java.math.BigDecimal
import java.math.BigInteger
import java.math.RoundingMode
import kotlin.random.Random

/**
 * `BigDecimal` as the wire `Decimal` (ADR-042): exact for every wire value, and the documented behaviour (SPEC 17) for the
 * `BigDecimal`s the wire cannot hold. Added by the types-paging review: a value whose mantissa needed more than 128 bits
 * only because of digits after the point (`10 / 3` at scale 38) used to saturate at 1.7 x 10^38.
 */
class DecimalCodecTests : Suite() {
    @Test
    fun allCases() = assertPassed()

    private fun roundTrip(v: BigDecimal): BigDecimal = Codecs.decimal.decodeAll(Codecs.decimal.encodeToByteArray(v))

    init {
        case("every scale, the extreme mantissas and trailing zeros round-trip exactly; equals is scale-sensitive, compareTo is not") {
            val max = BigInteger.ONE.shiftLeft(127).subtract(BigInteger.ONE)
            val min = BigInteger.ONE.shiftLeft(127).negate()
            for (scale in 0..38) {
                for (m in listOf(BigInteger.ZERO, BigInteger.ONE, BigInteger.ONE.negate(), BigInteger.TEN, max, min)) {
                    val v = BigDecimal(m, scale)
                    val back = roundTrip(v)
                    assertEq(v, back, "$m at scale $scale")
                    assertEq(scale, back.scale(), "$m at scale $scale keeps its scale")
                }
            }
            val r = Random(42)
            repeat(2_000) {
                val m = BigInteger(128, java.util.Random(r.nextLong())).subtract(BigInteger.ONE.shiftLeft(127))
                val v = BigDecimal(m, r.nextInt(0, 39))
                assertEq(v, roundTrip(v), "random $v")
            }
            assertTrue(BigDecimal("1.0") != BigDecimal("1.00"), "equals sees the scale")
            assertEq(0, roundTrip(BigDecimal("1.0")).compareTo(roundTrip(BigDecimal("1.00"))), "compareTo does not")
        }

        case("a negative scale is whole digits; digits past the 38th after the point are rounded half up") {
            val thousand = roundTrip(BigDecimal("1E+3"))
            assertEq(BigDecimal("1000"), thousand)
            assertEq(0, thousand.scale())
            val tiny = "0." + "0".repeat(37)
            assertEq(BigDecimal(tiny + "2"), roundTrip(BigDecimal(tiny + "15")))
            assertEq(BigDecimal("-${tiny}2"), roundTrip(BigDecimal("-${tiny}15")))
        }

        case("a mantissa past 128 bits loses its last digits after the point, never its magnitude") {
            // 10 / 3 to the wire's full scale: 39 digits, a mantissa of 3.3 x 10^38 (past 2^127).
            val third = BigDecimal.TEN.divide(BigDecimal(3), 38, RoundingMode.HALF_UP)
            assertEq(BigDecimal("3.3333333333333333333333333333333333333"), roundTrip(third))
            val twoThirdsNeg = BigDecimal.TEN.negate().divide(BigDecimal(6), 38, RoundingMode.HALF_UP) // -1.666..67, fits
            assertEq(twoThirdsNeg, roundTrip(twoThirdsNeg))
            assertEq(BigDecimal("-6.6666666666666666666666666666666666667"), roundTrip(BigDecimal(-20).divide(BigDecimal(3), 38, RoundingMode.HALF_UP)))
            val two = BigDecimal(2).setScale(38)
            assertEq(0, two.compareTo(roundTrip(two)), "2 at scale 38 is still 2")
            // Rounded half up to the digits that fit: 9.99..95 (38 places, a 39-digit mantissa) is 10 with 37 places.
            assertEq(BigDecimal("10." + "0".repeat(37)), roundTrip(BigDecimal("9." + "9".repeat(37) + "5")))
        }

        case("only a whole part of 2^127 or more saturates") {
            val max = BigDecimal(BigInteger.ONE.shiftLeft(127).subtract(BigInteger.ONE))
            val min = BigDecimal(BigInteger.ONE.shiftLeft(127).negate())
            assertEq(max, roundTrip(BigDecimal("1E+40")))
            assertEq(min, roundTrip(BigDecimal("-1E+40")))
            assertEq(max, roundTrip(BigDecimal("1000000000000000000000000000000000000000.25")))
        }
    }
}
