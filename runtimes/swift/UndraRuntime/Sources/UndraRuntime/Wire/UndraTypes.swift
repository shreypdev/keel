// Wire value types that have no native Swift spelling with the exact wire semantics
// (docs/SPEC.md section 3.1). The Foundation bridges (`Date`, `Foundation.UUID`) live in
// `Foundation+Undra.swift`; this file needs Foundation only for `TimeInterval`.

import Foundation

// MARK: - Bytes

/// The wire `Bytes` type: a `u32` length followed by raw bytes.
///
/// `[UInt8]` also conforms to `UndraCodec` (as `Vec<u8>`, which is wire-identical), but
/// `UndraBytes` encodes and decodes with one bulk copy instead of one call per byte.
public struct UndraBytes: UndraCodec, Sendable, Hashable {
    /// The raw bytes.
    public var bytes: [UInt8]

    /// Wraps `bytes`.
    public init(_ bytes: [UInt8]) {
        self.bytes = bytes
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> UndraBytes {
        let bytes = try r.readBytes()
        return UndraBytes(bytes)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeBytes(bytes)
    }
}

// MARK: - Duration

/// The wire `Duration`: an `i64` count of nanoseconds, exactly. It exists so that a floor of iOS 15
/// or macOS 12, which have no `Swift.Duration`, can use durations at all (ADR-045): generated code
/// maps the wire `Duration` to `Swift.Duration` when its deployment target is iOS 16 or later and to
/// this type below. From iOS 16 / macOS 13 it converts to and from `Swift.Duration`.
///
/// The core's durations are non-negative, so decoding a negative value throws
/// `WireError.negativeDuration`. Encoding does not clamp: a negative `UndraDuration` is written as
/// is and the core rejects it as a bad request. Values converted from a `Swift.Duration` beyond the
/// `Int64` nanosecond range (about 292 years) saturate, and sub-nanosecond precision is truncated
/// toward zero.
public struct UndraDuration: UndraCodec, Sendable, Hashable, Comparable {
    /// The duration in whole nanoseconds.
    public var nanoseconds: Int64

    /// Creates a duration from a nanosecond count.
    public init(nanoseconds: Int64) {
        self.nanoseconds = nanoseconds
    }

    /// The zero duration.
    public static let zero = UndraDuration(nanoseconds: 0)

    /// The duration in seconds. A `Double` keeps about 15 significant digits, so the nanosecond
    /// count (`nanoseconds`) is the exact value.
    public var timeInterval: TimeInterval {
        return Double(nanoseconds) / 1_000_000_000
    }

    /// Wraps a Swift duration, saturating at `Int64.min` / `Int64.max` nanoseconds.
    @available(iOS 16, macOS 13, *)
    public init(_ duration: Duration) {
        let parts = duration.components
        let scaled = parts.seconds.multipliedReportingOverflow(by: 1_000_000_000)
        if scaled.overflow {
            self.nanoseconds = parts.seconds < 0 ? Int64.min : Int64.max
            return
        }
        let fraction = parts.attoseconds / 1_000_000_000
        let total = scaled.partialValue.addingReportingOverflow(fraction)
        if total.overflow {
            self.nanoseconds = scaled.partialValue < 0 ? Int64.min : Int64.max
            return
        }
        self.nanoseconds = total.partialValue
    }

    /// The same duration as a Swift `Duration`.
    @available(iOS 16, macOS 13, *)
    public var duration: Duration {
        get { return Duration.nanoseconds(nanoseconds) }
        set { self = UndraDuration(newValue) }
    }

    public static func < (lhs: UndraDuration, rhs: UndraDuration) -> Bool {
        return lhs.nanoseconds < rhs.nanoseconds
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> UndraDuration {
        let at = r.position
        let value = try r.readI64()
        if value < 0 {
            throw WireError.negativeDuration(at: at)
        }
        return UndraDuration(nanoseconds: value)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeI64(nanoseconds)
    }
}

@available(iOS 16, macOS 13, *)
extension Duration: UndraCodec {
    public static func undraDecode(_ r: inout UndraReader) throws -> Duration {
        let value = try UndraDuration.undraDecode(&r)
        return value.duration
    }

    public func undraEncode(_ w: inout UndraWriter) {
        UndraDuration(self).undraEncode(&w)
    }
}

// MARK: - Timestamp

/// The wire `Timestamp`: an `i64` count of milliseconds since the Unix epoch.
///
/// `Foundation.Date` conversions are in `Foundation+Undra.swift`.
public struct UndraTimestamp: UndraCodec, Sendable, Hashable, Comparable {
    /// Milliseconds since 1970-01-01T00:00:00Z (negative before the epoch).
    public var millisecondsSinceEpoch: Int64

    /// Creates a timestamp from a millisecond count.
    public init(millisecondsSinceEpoch: Int64) {
        self.millisecondsSinceEpoch = millisecondsSinceEpoch
    }

    public static func < (lhs: UndraTimestamp, rhs: UndraTimestamp) -> Bool {
        return lhs.millisecondsSinceEpoch < rhs.millisecondsSinceEpoch
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> UndraTimestamp {
        let value = try r.readI64()
        return UndraTimestamp(millisecondsSinceEpoch: value)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeI64(millisecondsSinceEpoch)
    }
}

// MARK: - UUID

/// The wire `Uuid`: 16 raw bytes in RFC 4122 (big-endian) order.
///
/// Stored as two 64-bit halves so values are allocation-free, `Hashable` and cheap to compare.
/// `Foundation.UUID` conversions are in `Foundation+Undra.swift`.
public struct UndraUUID: UndraCodec, Sendable, Hashable, Comparable, CustomStringConvertible {
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
                guard let digit = UndraUUID.hexValue(character) else {
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
        precondition(index >= 0 && index < 16, "UndraUUID.byte(at:): index out of range")
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
            text.append(UndraUUID.hexDigit(value >> 4))
            text.append(UndraUUID.hexDigit(value & 0x0F))
            index += 1
        }
        return String(decoding: text, as: UTF8.self)
    }

    public var description: String {
        return uuidString
    }

    public static func < (lhs: UndraUUID, rhs: UndraUUID) -> Bool {
        if lhs.high != rhs.high {
            return lhs.high < rhs.high
        }
        return lhs.low < rhs.low
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> UndraUUID {
        let high = try r.readU64BigEndian()
        let low = try r.readU64BigEndian()
        return UndraUUID(high: high, low: low)
    }

    public func undraEncode(_ w: inout UndraWriter) {
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

/// An object or store handle (docs/SPEC.md section 1.2): a `u64` whose low 24 bits are the slot
/// index and whose high 40 bits are the generation (starting at 1, ADR-040). `0` is the null handle.
///
/// Handles are only meaningful inside the runtime instance that issued them: two cores issue the
/// same numbers (ADR-044), which is why the bindings never pass one core's object to another.
public struct UndraHandle: UndraCodec, Sendable, Hashable, CustomStringConvertible {
    /// The raw `u64` as it appears on the wire.
    public var rawValue: UInt64

    /// The null handle (`0`).
    public static let null = UndraHandle(rawValue: 0)

    /// Bits of the slot index.
    static let indexBits: UInt64 = 24

    /// Wraps a raw wire value.
    public init(rawValue: UInt64) {
        self.rawValue = rawValue
    }

    /// Builds a handle from a slot index (its low 24 bits) and a generation (its low 40 bits).
    public init(index: UInt32, generation: UInt64) {
        let slot = UInt64(index) & ((1 << UndraHandle.indexBits) - 1)
        self.rawValue = (generation << UndraHandle.indexBits) | slot
    }

    /// The slot index (low 24 bits).
    public var index: UInt32 {
        return UInt32(truncatingIfNeeded: rawValue & ((1 << UndraHandle.indexBits) - 1))
    }

    /// The generation (high 40 bits).
    public var generation: UInt64 {
        return rawValue >> UndraHandle.indexBits
    }

    /// Whether this is the null handle (`rawValue == 0`).
    public var isNull: Bool {
        return rawValue == 0
    }

    public var description: String {
        return isNull ? "handle(null)" : "handle(index: \(index), generation: \(generation))"
    }

    public static func undraDecode(_ r: inout UndraReader) throws -> UndraHandle {
        let raw = try r.readU64()
        return UndraHandle(rawValue: raw)
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeU64(rawValue)
    }
}

// MARK: - Result

/// The wire `Result<T, E>`: a `u8` tag (0 = ok, 1 = err) followed by the `T` or the `E`.
///
/// `E` is not required to be an `Error`; where it is, `get()` and `result` bridge to Swift's
/// own error handling.
public enum UndraResult<T, E> {
    case ok(T)
    case err(E)
}

extension UndraResult: Sendable where T: Sendable, E: Sendable {}

extension UndraResult: Equatable where T: Equatable, E: Equatable {}

extension UndraResult: Hashable where T: Hashable, E: Hashable {}

extension UndraResult where E: Error {
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

extension UndraResult: UndraCodec where T: UndraCodec, E: UndraCodec {
    public static func undraDecode(_ r: inout UndraReader) throws -> UndraResult<T, E> {
        let at = r.position
        let tag = try r.readU8()
        switch tag {
        case 0:
            let value = try T.undraDecode(&r)
            return .ok(value)
        case 1:
            let error = try E.undraDecode(&r)
            return .err(error)
        default:
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "Result")
        }
    }

    public func undraEncode(_ w: inout UndraWriter) {
        switch self {
        case .ok(let value):
            w.writeU8(0)
            value.undraEncode(&w)
        case .err(let error):
            w.writeU8(1)
            error.undraEncode(&w)
        }
    }
}

// MARK: - Unit

/// The wire `Unit`: a value with no bytes. Swift's `Void` cannot conform to a protocol, so
/// generic positions (for example `UndraResult<UndraUnit, E>`) use this type instead.
public struct UndraUnit: UndraCodec, Sendable, Hashable {
    /// Creates the unit value.
    public init() {}

    public static func undraDecode(_ r: inout UndraReader) throws -> UndraUnit {
        return UndraUnit()
    }

    public func undraEncode(_ w: inout UndraWriter) {}
}

/// Encodes a `Void` value: writes nothing. Generated code calls this for `Unit` results.
public func undraEncodeVoid(_ w: inout UndraWriter) {}

/// Decodes a `Void` value: reads nothing.
public func undraDecodeVoid(_ r: inout UndraReader) throws {}
