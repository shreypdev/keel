// The only file in the wire layer that imports Foundation: bridges between the wire value
// types and Foundation's `Date`, `UUID` and `LocalizedError`.

import Foundation

// MARK: - Date <-> UndraTimestamp

extension UndraTimestamp {
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
extension Date: UndraCodec {
    public static func undraDecode(_ r: inout UndraReader) throws -> Date {
        let timestamp = try UndraTimestamp.undraDecode(&r)
        return timestamp.date
    }

    public func undraEncode(_ w: inout UndraWriter) {
        UndraTimestamp(self).undraEncode(&w)
    }
}

// MARK: - UUID <-> UndraUUID

extension UndraUUID {
    /// Creates a wire UUID from a `Foundation.UUID`.
    public init(_ uuid: UUID) {
        let b = uuid.uuid
        let high = UndraUUID.word(b.0, b.1, b.2, b.3, b.4, b.5, b.6, b.7)
        let low = UndraUUID.word(b.8, b.9, b.10, b.11, b.12, b.13, b.14, b.15)
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
extension UUID: UndraCodec {
    public static func undraDecode(_ r: inout UndraReader) throws -> UUID {
        let value = try UndraUUID.undraDecode(&r)
        return value.uuid
    }

    public func undraEncode(_ w: inout UndraWriter) {
        UndraUUID(self).undraEncode(&w)
    }
}

// MARK: - LocalizedError

/// `HttpError` is a `LocalizedError` whose description is its message, as every generated error
/// is, so `error.localizedDescription` reads the same on every platform.
extension HttpError: LocalizedError {
    public var errorDescription: String? {
        return description
    }
}

/// `FsError` is a `LocalizedError` whose description is its message.
extension FsError: LocalizedError {
    public var errorDescription: String? {
        return description
    }
}

/// `StorageError` is a `LocalizedError` whose description is its message.
extension StorageError: LocalizedError {
    public var errorDescription: String? {
        return description
    }
}

/// `WsError` is a `LocalizedError` whose description is its message.
extension WsError: LocalizedError {
    public var errorDescription: String? {
        return description
    }
}

/// `SseError` is a `LocalizedError` whose description is its message.
extension SseError: LocalizedError {
    public var errorDescription: String? {
        return description
    }
}

/// `DbError` is a `LocalizedError` whose description is its message.
extension DbError: LocalizedError {
    public var errorDescription: String? {
        return description
    }
}
