// Wire value types that have no native Swift spelling with the exact wire semantics
// (docs/SPEC.md section 3.1). The Foundation bridges (`Date`, `Foundation.UUID`) live in
// `Foundation+Keel.swift`; this file needs no Foundation.

// MARK: - Bytes

/// The wire `Bytes` type: a `u32` length followed by raw bytes.
///
/// `[UInt8]` also conforms to `KeelCodec` (as `Vec<u8>`, which is wire-identical), but
/// `KeelBytes` encodes and decodes with one bulk copy instead of one call per byte.
public struct KeelBytes: KeelCodec, Sendable, Hashable {
    /// The raw bytes.
    public var bytes: [UInt8]

    /// Wraps `bytes`.
    public init(_ bytes: [UInt8]) {
        self.bytes = bytes
    }

    public static func keelDecode(_ r: inout KeelReader) throws -> KeelBytes {
        let bytes = try r.readBytes()
        return KeelBytes(bytes)
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeBytes(bytes)
    }
}

// MARK: - Duration

/// The wire `Duration`: an `i64` count of nanoseconds. Wraps `Swift.Duration`.
///
/// The core's durations are non-negative, so decoding a negative value throws
/// `WireError.negativeDuration`. Encoding does not clamp: a negative `Duration` is written as
/// is and the core rejects it as a bad request. Values beyond the `Int64` nanosecond range
/// (about 292 years) saturate, and sub-nanosecond precision is truncated toward zero.
public struct KeelDuration: KeelCodec, Sendable, Hashable, Comparable {
    /// The wrapped duration.
    public var duration: Duration

    /// Wraps a Swift duration.
    public init(_ duration: Duration) {
        self.duration = duration
    }

    /// Creates a duration from a nanosecond count.
    public init(nanoseconds: Int64) {
        self.duration = Duration.nanoseconds(nanoseconds)
    }

    /// The duration in whole nanoseconds, saturating at `Int64.min` / `Int64.max`.
    public var nanoseconds: Int64 {
        let parts = duration.components
        let scaled = parts.seconds.multipliedReportingOverflow(by: 1_000_000_000)
        if scaled.overflow {
            return parts.seconds < 0 ? Int64.min : Int64.max
        }
        let fraction = parts.attoseconds / 1_000_000_000
        let total = scaled.partialValue.addingReportingOverflow(fraction)
        if total.overflow {
            return scaled.partialValue < 0 ? Int64.min : Int64.max
        }
        return total.partialValue
    }

    public static func < (lhs: KeelDuration, rhs: KeelDuration) -> Bool {
        return lhs.duration < rhs.duration
    }

    public static func keelDecode(_ r: inout KeelReader) throws -> KeelDuration {
        let at = r.position
        let value = try r.readI64()
        if value < 0 {
            throw WireError.negativeDuration(at: at)
        }
        return KeelDuration(nanoseconds: value)
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeI64(nanoseconds)
    }
}

extension Duration: KeelCodec {
    public static func keelDecode(_ r: inout KeelReader) throws -> Duration {
        let value = try KeelDuration.keelDecode(&r)
        return value.duration
    }

    public func keelEncode(_ w: inout KeelWriter) {
        KeelDuration(self).keelEncode(&w)
    }
}

// MARK: - Timestamp

/// The wire `Timestamp`: an `i64` count of milliseconds since the Unix epoch.
///
/// `Foundation.Date` conversions are in `Foundation+Keel.swift`.
public struct KeelTimestamp: KeelCodec, Sendable, Hashable, Comparable {
    /// Milliseconds since 1970-01-01T00:00:00Z (negative before the epoch).
    public var millisecondsSinceEpoch: Int64

    /// Creates a timestamp from a millisecond count.
    public init(millisecondsSinceEpoch: Int64) {
        self.millisecondsSinceEpoch = millisecondsSinceEpoch
    }

    public static func < (lhs: KeelTimestamp, rhs: KeelTimestamp) -> Bool {
        return lhs.millisecondsSinceEpoch < rhs.millisecondsSinceEpoch
    }

    public static func keelDecode(_ r: inout KeelReader) throws -> KeelTimestamp {
        let value = try r.readI64()
        return KeelTimestamp(millisecondsSinceEpoch: value)
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeI64(millisecondsSinceEpoch)
    }
}

// MARK: - UUID

/// The wire `Uuid`: 16 raw bytes in RFC 4122 (big-endian) order.
///
/// Stored as two 64-bit halves so values are allocation-free, `Hashable` and cheap to compare.
/// `Foundation.UUID` conversions are in `Foundation+Keel.swift`.
public struct KeelUUID: KeelCodec, Sendable, Hashable, Comparable, CustomStringConvertible {
    /// Bytes 0 to 7, big-endian.
    public let high: UInt64
    /// Bytes 8 to 15, big-endian.
    public let low: UInt64

    /// Creates a UUID from its two 64-bit halves.
    public init(high: UInt64, low: UInt64) {
        self.high = high
        self.low = low
    }

    /// Creates a UUID from exactly 16 bytes; returns `nil` for any other length.
    public init?(bytes: [UInt8]) {
        if bytes.count != 16 {
            return nil
        }
        var high: UInt64 = 0
        var low: UInt64 = 0
        var index = 0
        while index < 8 {
            high = (high << 8) | UInt64(bytes[index])
            low = (low << 8) | UInt64(bytes[index + 8])
            index += 1
        }
        self.high = high
        self.low = low
    }

    /// Parses the canonical hyphenated form (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`, hex digits
    /// in either case); returns `nil` for anything else.
    public init?(uuidString: String) {
        let text = Array(uuidString.utf8)
        if text.count != 36 {
            return nil
        }
        var high: UInt64 = 0
        var low: UInt64 = 0
        var nibbles = 0
        var index = 0
        while index < 36 {
            let character = text[index]
            if index == 8 || index == 13 || index == 18 || index == 23 {
                if character != 0x2D {
                    return nil
                }
            } else {
                guard let digit = KeelUUID.hexValue(character) else {
                    return nil
                }
                if nibbles < 16 {
                    high = (high << 4) | UInt64(digit)
                } else {
                    low = (low << 4) | UInt64(digit)
                }
                nibbles += 1
            }
            index += 1
        }
        self.high = high
        self.low = low
    }

    /// The 16 bytes in RFC 4122 order.
    public var bytes: [UInt8] {
        var result: [UInt8] = []
        result.reserveCapacity(16)
        var index = 0
        while index < 16 {
            result.append(byte(at: index))
            index += 1
        }
        return result
    }

    /// The byte at `index` (0 to 15) in RFC 4122 order.
    ///
    /// - Precondition: `0 <= index < 16`.
    public func byte(at index: Int) -> UInt8 {
        precondition(index >= 0 && index < 16, "KeelUUID.byte(at:): index out of range")
        if index < 8 {
            return UInt8(truncatingIfNeeded: high >> UInt64((7 - index) * 8))
        }
        return UInt8(truncatingIfNeeded: low >> UInt64((15 - index) * 8))
    }

    /// The canonical lowercase hyphenated form, e.g. `123e4567-e89b-12d3-a456-426614174000`.
    public var uuidString: String {
        var text: [UInt8] = []
        text.reserveCapacity(36)
        var index = 0
        while index < 16 {
            if index == 4 || index == 6 || index == 8 || index == 10 {
                text.append(0x2D)
            }
            let value = byte(at: index)
            text.append(KeelUUID.hexDigit(value >> 4))
            text.append(KeelUUID.hexDigit(value & 0x0F))
            index += 1
        }
        return String(decoding: text, as: UTF8.self)
    }

    public var description: String {
        return uuidString
    }

    public static func < (lhs: KeelUUID, rhs: KeelUUID) -> Bool {
        if lhs.high != rhs.high {
            return lhs.high < rhs.high
        }
        return lhs.low < rhs.low
    }

    public static func keelDecode(_ r: inout KeelReader) throws -> KeelUUID {
        let high = try r.readU64BigEndian()
        let low = try r.readU64BigEndian()
        return KeelUUID(high: high, low: low)
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeU64BigEndian(high)
        w.writeU64BigEndian(low)
    }

    private static func hexDigit(_ nibble: UInt8) -> UInt8 {
        if nibble < 10 {
            return 0x30 + nibble
        }
        return 0x61 + (nibble - 10)
    }

    private static func hexValue(_ character: UInt8) -> UInt8? {
        switch character {
        case 0x30 ... 0x39:
            return character - 0x30
        case 0x61 ... 0x66:
            return character - 0x61 + 10
        case 0x41 ... 0x46:
            return character - 0x41 + 10
        default:
            return nil
        }
    }
}

// MARK: - Handle

/// An object or store handle (docs/SPEC.md section 1.2): a `u64` whose low 32 bits are the slot
/// index and whose high 32 bits are the generation (starting at 1). `0` is the null handle.
///
/// Handles are only meaningful inside the runtime instance that issued them.
public struct KeelHandle: KeelCodec, Sendable, Hashable, CustomStringConvertible {
    /// The raw `u64` as it appears on the wire.
    public var rawValue: UInt64

    /// The null handle (`0`).
    public static let null = KeelHandle(rawValue: 0)

    /// Wraps a raw wire value.
    public init(rawValue: UInt64) {
        self.rawValue = rawValue
    }

    /// Builds a handle from a slot index and a generation.
    public init(index: UInt32, generation: UInt32) {
        self.rawValue = (UInt64(generation) << 32) | UInt64(index)
    }

    /// The slot index (low 32 bits).
    public var index: UInt32 {
        return UInt32(truncatingIfNeeded: rawValue)
    }

    /// The generation (high 32 bits).
    public var generation: UInt32 {
        return UInt32(truncatingIfNeeded: rawValue >> 32)
    }

    /// Whether this is the null handle (`rawValue == 0`).
    public var isNull: Bool {
        return rawValue == 0
    }

    public var description: String {
        return isNull ? "handle(null)" : "handle(index: \(index), generation: \(generation))"
    }

    public static func keelDecode(_ r: inout KeelReader) throws -> KeelHandle {
        let raw = try r.readU64()
        return KeelHandle(rawValue: raw)
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeU64(rawValue)
    }
}

// MARK: - Result

/// The wire `Result<T, E>`: a `u8` tag (0 = ok, 1 = err) followed by the `T` or the `E`.
///
/// `E` is not required to be an `Error`; where it is, `get()` and `result` bridge to Swift's
/// own error handling.
public enum KeelResult<T, E> {
    case ok(T)
    case err(E)
}

extension KeelResult: Sendable where T: Sendable, E: Sendable {}

extension KeelResult: Equatable where T: Equatable, E: Equatable {}

extension KeelResult: Hashable where T: Hashable, E: Hashable {}

extension KeelResult where E: Error {
    /// The success value, or throws the error.
    public func get() throws -> T {
        switch self {
        case .ok(let value):
            return value
        case .err(let error):
            throw error
        }
    }

    /// The same outcome as a Swift `Result`.
    public var result: Result<T, E> {
        switch self {
        case .ok(let value):
            return .success(value)
        case .err(let error):
            return .failure(error)
        }
    }
}

extension KeelResult: KeelCodec where T: KeelCodec, E: KeelCodec {
    public static func keelDecode(_ r: inout KeelReader) throws -> KeelResult<T, E> {
        let at = r.position
        let tag = try r.readU8()
        switch tag {
        case 0:
            let value = try T.keelDecode(&r)
            return .ok(value)
        case 1:
            let error = try E.keelDecode(&r)
            return .err(error)
        default:
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "Result")
        }
    }

    public func keelEncode(_ w: inout KeelWriter) {
        switch self {
        case .ok(let value):
            w.writeU8(0)
            value.keelEncode(&w)
        case .err(let error):
            w.writeU8(1)
            error.keelEncode(&w)
        }
    }
}

// MARK: - Unit

/// The wire `Unit`: a value with no bytes. Swift's `Void` cannot conform to a protocol, so
/// generic positions (for example `KeelResult<KeelUnit, E>`) use this type instead.
public struct KeelUnit: KeelCodec, Sendable, Hashable {
    /// Creates the unit value.
    public init() {}

    public static func keelDecode(_ r: inout KeelReader) throws -> KeelUnit {
        return KeelUnit()
    }

    public func keelEncode(_ w: inout KeelWriter) {}
}

/// Encodes a `Void` value: writes nothing. Generated code calls this for `Unit` results.
public func keelEncodeVoid(_ w: inout KeelWriter) {}

/// Decodes a `Void` value: reads nothing.
public func keelDecodeVoid(_ r: inout KeelReader) throws {}
