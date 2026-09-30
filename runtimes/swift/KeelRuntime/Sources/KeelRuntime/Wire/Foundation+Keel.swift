// The only file in the wire layer that imports Foundation: bridges between the wire value
// types and Foundation's `Date` and `UUID`.

import Foundation

// MARK: - Date <-> KeelTimestamp

extension KeelTimestamp {
    /// Creates a timestamp from a `Date`, rounding to the nearest millisecond (halves away from
    /// zero). Dates outside the `Int64` millisecond range saturate; a NaN date becomes the epoch.
    public init(_ date: Date) {
        let scaled = (date.timeIntervalSince1970 * 1000.0).rounded()
        if scaled.isNaN {
            self.init(millisecondsSinceEpoch: 0)
        } else if scaled >= Double(Int64.max) {
            self.init(millisecondsSinceEpoch: Int64.max)
        } else if scaled <= Double(Int64.min) {
            self.init(millisecondsSinceEpoch: Int64.min)
        } else {
            self.init(millisecondsSinceEpoch: Int64(scaled))
        }
    }

    /// The timestamp as a `Date`.
    public var date: Date {
        return Date(timeIntervalSince1970: Double(millisecondsSinceEpoch) / 1000.0)
    }
}

/// `Date` crosses the wire as a `Timestamp` (milliseconds since the Unix epoch), so sub-millisecond
/// precision is not preserved.
extension Date: KeelCodec {
    public static func keelDecode(_ r: inout KeelReader) throws -> Date {
        let timestamp = try KeelTimestamp.keelDecode(&r)
        return timestamp.date
    }

    public func keelEncode(_ w: inout KeelWriter) {
        KeelTimestamp(self).keelEncode(&w)
    }
}

// MARK: - UUID <-> KeelUUID

extension KeelUUID {
    /// Creates a wire UUID from a `Foundation.UUID`.
    public init(_ uuid: UUID) {
        let b = uuid.uuid
        let high = KeelUUID.word(b.0, b.1, b.2, b.3, b.4, b.5, b.6, b.7)
        let low = KeelUUID.word(b.8, b.9, b.10, b.11, b.12, b.13, b.14, b.15)
        self.init(high: high, low: low)
    }

    /// The same value as a `Foundation.UUID`.
    public var uuid: UUID {
        return UUID(uuid: (
            byte(at: 0), byte(at: 1), byte(at: 2), byte(at: 3),
            byte(at: 4), byte(at: 5), byte(at: 6), byte(at: 7),
            byte(at: 8), byte(at: 9), byte(at: 10), byte(at: 11),
            byte(at: 12), byte(at: 13), byte(at: 14), byte(at: 15)
        ))
    }

    /// Packs eight bytes, most significant first, into a `UInt64`.
    fileprivate static func word(
        _ b0: UInt8, _ b1: UInt8, _ b2: UInt8, _ b3: UInt8,
        _ b4: UInt8, _ b5: UInt8, _ b6: UInt8, _ b7: UInt8
    ) -> UInt64 {
        var value: UInt64 = 0
        value = (value << 8) | UInt64(b0)
        value = (value << 8) | UInt64(b1)
        value = (value << 8) | UInt64(b2)
        value = (value << 8) | UInt64(b3)
        value = (value << 8) | UInt64(b4)
        value = (value << 8) | UInt64(b5)
        value = (value << 8) | UInt64(b6)
        value = (value << 8) | UInt64(b7)
        return value
    }
}

/// `Foundation.UUID` crosses the wire as a `Uuid`: 16 raw bytes in RFC 4122 order.
extension UUID: KeelCodec {
    public static func keelDecode(_ r: inout KeelReader) throws -> UUID {
        let value = try KeelUUID.keelDecode(&r)
        return value.uuid
    }

    public func keelEncode(_ w: inout KeelWriter) {
        KeelUUID(self).keelEncode(&w)
    }
}
