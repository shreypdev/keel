// `Foundation.Decimal` as the wire `Decimal` (ADR-042): a 128-bit two's-complement mantissa in
// 16 little-endian bytes, then a scale byte of at most 38; value = mantissa x 10^-scale.

import Foundation

/// A 128-bit unsigned integer, just enough for the conversion.
private struct Magnitude128 {
    var high: UInt64
    var low: UInt64

    static let zero = Magnitude128(high: 0, low: 0)

    var isZero: Bool { high == 0 && low == 0 }

    /// Two's-complement negation.
    func negated() -> Magnitude128 {
        let nl = ~low &+ 1
        let carry: UInt64 = nl == 0 ? 1 : 0
        return Magnitude128(high: ~high &+ carry, low: nl)
    }

    /// `self * 10`, or `nil` on overflow.
    func timesTen() -> Magnitude128? {
        let (lowCarry, lowProduct) = low.multipliedFullWidth(by: 10)
        let (highProduct, overflow) = high.multipliedReportingOverflow(by: 10)
        let (sum, addOverflow) = highProduct.addingReportingOverflow(lowCarry)
        if overflow || addOverflow { return nil }
        return Magnitude128(high: sum, low: lowProduct)
    }

    /// `self / 10` (truncating).
    func dividedByTen() -> Magnitude128 {
        let (highQuotient, highRemainder) = high.quotientAndRemainder(dividingBy: 10)
        let (lowQuotient, _) = (10 as UInt64).dividingFullWidth((highRemainder, low))
        return Magnitude128(high: highQuotient, low: lowQuotient)
    }

    /// The most a signed 128-bit mantissa can hold: 2^127 - 1 positive, 2^127 negative.
    func fits(negative: Bool) -> Bool {
        if high < 0x8000_0000_0000_0000 { return true }
        return negative && high == 0x8000_0000_0000_0000 && low == 0
    }
}

extension Decimal {
    /// The words of the mantissa, least significant first.
    fileprivate var undraWords: [UInt16] {
        let m = _mantissa
        return [m.0, m.1, m.2, m.3, m.4, m.5, m.6, m.7]
    }

    fileprivate static func undraMake(magnitude: Magnitude128, negative: Bool, scale: Int) -> Decimal {
        func word(_ value: UInt64, _ index: Int) -> UInt16 { UInt16(truncatingIfNeeded: value >> UInt64(16 * index)) }
        let words = (0..<4).map { word(magnitude.low, $0) } + (0..<4).map { word(magnitude.high, $0) }
        var length: UInt32 = 8
        while length > 0 && words[Int(length) - 1] == 0 { length -= 1 }
        let isNegative: UInt32 = (negative && length > 0) ? 1 : 0
        return Decimal(
            _exponent: Int32(-scale),
            _length: length,
            _isNegative: isNegative,
            _isCompact: 0,
            _reserved: 0,
            _mantissa: (words[0], words[1], words[2], words[3], words[4], words[5], words[6], words[7])
        )
    }
}

/// `Foundation.Decimal` crosses the wire as a `Decimal`. Decoding is exact: Foundation's 128-bit
/// mantissa and its exponent range hold every wire value (the scale is kept, so `1.00` stays
/// `1.00`). Encoding is exact for every value the wire can hold; the rest is documented, not
/// hidden: a NaN encodes as zero, a value of 2^127 or more saturates, and digits beyond the 38th
/// after the point, or beyond what a 128-bit mantissa holds, are cut off (rounded toward zero). Convert to your decimal library's type
/// through `description` if you need other behaviour.
extension Decimal: UndraCodec {
    public static func undraDecode(_ r: inout UndraReader) throws -> Decimal {
        let low = try r.readU64()
        let high = try r.readU64()
        let at = r.position
        let scale = try r.readU8()
        guard scale <= 38 else {
            throw WireError.invalidTag(tag: UInt32(scale), at: at, type: "decimal scale")
        }
        let negative = high >> 63 == 1
        var magnitude = Magnitude128(high: high, low: low)
        if negative { magnitude = magnitude.negated() }
        return Decimal.undraMake(magnitude: magnitude, negative: negative, scale: Int(scale))
    }

    public func undraEncode(_ w: inout UndraWriter) {
        var low: UInt64 = 0
        var high: UInt64 = 0
        var scale = 0
        if !isNaN {
            let words = undraWords
            var magnitude = Magnitude128.zero
            for i in 0..<4 { magnitude.low |= UInt64(words[i]) << UInt64(16 * i) }
            for i in 0..<4 { magnitude.high |= UInt64(words[4 + i]) << UInt64(16 * i) }
            let negative = _isNegative != 0 && !magnitude.isZero
            var exponent = Int(_exponent)
            var saturated = false
            // A positive exponent is whole digits: scale 0, so multiply them in.
            while exponent > 0 && !magnitude.isZero {
                guard let next = magnitude.timesTen() else { saturated = true; break }
                magnitude = next
                exponent -= 1
            }
            // More than 38 digits after the point do not fit the wire's scale: cut them off.
            while exponent < -38 {
                magnitude = magnitude.dividedByTen()
                exponent += 1
            }
            // A mantissa past 127 bits (Foundation keeps 39 digits after some parses) loses its last
            // digits after the point the same way, never its magnitude.
            while exponent < 0 && !magnitude.fits(negative: negative) {
                magnitude = magnitude.dividedByTen()
                exponent += 1
            }
            scale = max(0, -exponent)
            if saturated || !magnitude.fits(negative: negative) {
                magnitude = Magnitude128(high: negative ? 0x8000_0000_0000_0000 : 0x7FFF_FFFF_FFFF_FFFF,
                                         low: negative ? 0 : UInt64.max)
                scale = 0
            }
            let signed = negative ? magnitude.negated() : magnitude
            low = signed.low
            high = signed.high
        }
        w.writeU64(low)
        w.writeU64(high)
        w.writeU8(UInt8(scale))
    }
}
