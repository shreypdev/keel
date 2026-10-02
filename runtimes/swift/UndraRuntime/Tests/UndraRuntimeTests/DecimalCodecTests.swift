import Foundation
import XCTest
import UndraRuntime

/// `Foundation.Decimal` on the wire (ADR-042): exact for every wire value; the documented
/// behaviour for the Foundation values the wire cannot hold.
final class DecimalCodecTests: XCTestCase {
    private func wire(_ mantissaHigh: UInt64, _ mantissaLow: UInt64, _ scale: UInt8) -> [UInt8] {
        var w = UndraWriter()
        w.writeU64(mantissaLow)
        w.writeU64(mantissaHigh)
        w.writeU8(scale)
        return w.finish()
    }

    func testEveryWireValueDecodesToTheSameNumberAndEncodesBackByteForByte() throws {
        let cases: [(UInt64, UInt64, UInt8, String)] = [
            (0, 0, 0, "0"),
            (0, 1999, 2, "19.99"),
            (UInt64.max, UInt64.max &- 149, 2, "-1.5"),  // -150 / 100: not normalised, scale 2 kept
            (0, 1, 38, "0.00000000000000000000000000000000000001"),
            (0x7FFF_FFFF_FFFF_FFFF, UInt64.max, 0, "170141183460469231731687303715884105727"),
            (0x8000_0000_0000_0000, 0, 0, "-170141183460469231731687303715884105728"),
        ]
        for (high, low, scale, text) in cases {
            let bytes = wire(high, low, scale)
            let decoded = try Decimal.undraDecoded(from: bytes)
            XCTAssertEqual(decoded, Decimal(string: text, locale: Locale(identifier: "en_US_POSIX")), text)
            XCTAssertEqual(decoded.undraEncoded(), bytes, "\(text): the scale survives a round trip")
        }
    }

    func testAScaleAbove38IsRejectedAndATruncationIsATypedError() {
        XCTAssertThrowsError(try Decimal.undraDecoded(from: wire(0, 0, 39)))
        XCTAssertThrowsError(try Decimal.undraDecoded(from: Array(wire(0, 0, 0).dropLast())))
        XCTAssertThrowsError(try Decimal.undraDecoded(from: wire(0, 0, 0) + [0]))
    }

    func testWhatTheWireCannotHoldIsDocumentedNotHidden() throws {
        // A Foundation value with a positive exponent is whole digits: scale 0.
        let thousand = Decimal(sign: .plus, exponent: 3, significand: 1)
        XCTAssertEqual(try Decimal.undraDecoded(from: thousand.undraEncoded()), 1000)
        XCTAssertEqual(thousand.undraEncoded().last, 0)
        // NaN is zero, an integer of 2^127 or more saturates, digits past the 38th are cut.
        XCTAssertEqual(try Decimal.undraDecoded(from: Decimal.nan.undraEncoded()), 0)
        let huge = Decimal(sign: .plus, exponent: 100, significand: 1)
        XCTAssertEqual(
            try Decimal.undraDecoded(from: huge.undraEncoded()),
            Decimal(string: "170141183460469231731687303715884105727", locale: Locale(identifier: "en_US_POSIX"))
        )
        let tiny = Decimal(sign: .plus, exponent: -50, significand: 5)
        XCTAssertEqual(try Decimal.undraDecoded(from: tiny.undraEncoded()), 0)
        let negativeHuge = Decimal(sign: .minus, exponent: 100, significand: 1)
        XCTAssertEqual(negativeHuge.undraEncoded(), wire(0x8000_0000_0000_0000, 0, 0))
    }

    func testRandomValuesRoundTrip() throws {
        var generator = SystemRandomNumberGenerator()
        for _ in 0..<2_000 {
            let high = UInt64.random(in: 0...UInt64.max, using: &generator)
            let low = UInt64.random(in: 0...UInt64.max, using: &generator)
            let scale = UInt8.random(in: 0...38, using: &generator)
            let bytes = wire(high, low, scale)
            XCTAssertEqual(try Decimal.undraDecoded(from: bytes).undraEncoded(), bytes)
        }
    }
}
