/// Appends Undra wire values to a growing byte buffer (docs/SPEC.md sections 3.1 and 3.9).
///
/// Everything is little-endian, unaligned and unpadded, except the explicitly named
/// `writeU64BigEndian` used for UUIDs. The writer needs no Foundation.
///
/// Writers never fail: the only limit is that a single length or count must fit in a `u32`,
/// which `writeLen` enforces with a precondition (a value that large cannot be represented on
/// the wire at all, so it is a programming error, not a runtime condition).
///
/// ```swift
/// var w = UndraWriter()
/// w.writeU32(7)
/// w.writeString("hé")
/// let bytes = w.finish()
/// ```
public struct UndraWriter: Sendable {
    private var buffer: [UInt8]

    /// Creates an empty writer.
    public init() {
        buffer = []
    }

    /// Creates an empty writer with room for at least `capacity` bytes.
    public init(capacity: Int) {
        buffer = []
        if capacity > 0 {
            buffer.reserveCapacity(capacity)
        }
    }

    /// The number of bytes written so far.
    public var count: Int {
        return buffer.count
    }

    /// Ensures room for at least `minimumCapacity` bytes in total.
    public mutating func reserveCapacity(_ minimumCapacity: Int) {
        if minimumCapacity > 0 {
            buffer.reserveCapacity(minimumCapacity)
        }
    }

    /// Returns everything written so far.
    public func finish() -> [UInt8] {
        return buffer
    }

    /// Returns everything written so far as an `ArraySlice` (no copy). Convenient for the
    /// payload structs, whose byte fields are slices.
    public func finishSlice() -> ArraySlice<UInt8> {
        return buffer[0 ..< buffer.count]
    }

    // MARK: Fixed-width integers

    /// Writes a `u8`.
    public mutating func writeU8(_ value: UInt8) {
        buffer.append(value)
    }

    /// Writes a little-endian `u16`.
    public mutating func writeU16(_ value: UInt16) {
        buffer.append(UInt8(truncatingIfNeeded: value))
        buffer.append(UInt8(truncatingIfNeeded: value >> 8))
    }

    /// Writes a little-endian `u32`.
    public mutating func writeU32(_ value: UInt32) {
        buffer.append(UInt8(truncatingIfNeeded: value))
        buffer.append(UInt8(truncatingIfNeeded: value >> 8))
        buffer.append(UInt8(truncatingIfNeeded: value >> 16))
        buffer.append(UInt8(truncatingIfNeeded: value >> 24))
    }

    /// Writes a little-endian `u64`.
    public mutating func writeU64(_ value: UInt64) {
        buffer.append(UInt8(truncatingIfNeeded: value))
        buffer.append(UInt8(truncatingIfNeeded: value >> 8))
        buffer.append(UInt8(truncatingIfNeeded: value >> 16))
        buffer.append(UInt8(truncatingIfNeeded: value >> 24))
        buffer.append(UInt8(truncatingIfNeeded: value >> 32))
        buffer.append(UInt8(truncatingIfNeeded: value >> 40))
        buffer.append(UInt8(truncatingIfNeeded: value >> 48))
        buffer.append(UInt8(truncatingIfNeeded: value >> 56))
    }

    /// Writes a big-endian `u64`. Only used for the two halves of a UUID (RFC 4122 byte order).
    public mutating func writeU64BigEndian(_ value: UInt64) {
        buffer.append(UInt8(truncatingIfNeeded: value >> 56))
        buffer.append(UInt8(truncatingIfNeeded: value >> 48))
        buffer.append(UInt8(truncatingIfNeeded: value >> 40))
        buffer.append(UInt8(truncatingIfNeeded: value >> 32))
        buffer.append(UInt8(truncatingIfNeeded: value >> 24))
        buffer.append(UInt8(truncatingIfNeeded: value >> 16))
        buffer.append(UInt8(truncatingIfNeeded: value >> 8))
        buffer.append(UInt8(truncatingIfNeeded: value))
    }

    /// Writes an `i8` (two's complement).
    public mutating func writeI8(_ value: Int8) {
        writeU8(UInt8(bitPattern: value))
    }

    /// Writes a little-endian `i16` (two's complement).
    public mutating func writeI16(_ value: Int16) {
        writeU16(UInt16(bitPattern: value))
    }

    /// Writes a little-endian `i32` (two's complement).
    public mutating func writeI32(_ value: Int32) {
        writeU32(UInt32(bitPattern: value))
    }

    /// Writes a little-endian `i64` (two's complement).
    public mutating func writeI64(_ value: Int64) {
        writeU64(UInt64(bitPattern: value))
    }

    // MARK: Floating point, bool

    /// Writes the IEEE 754 bits of a `Float`, little-endian. NaN payloads are preserved.
    public mutating func writeF32(_ value: Float) {
        writeU32(value.bitPattern)
    }

    /// Writes the IEEE 754 bits of a `Double`, little-endian. NaN payloads are preserved.
    public mutating func writeF64(_ value: Double) {
        writeU64(value.bitPattern)
    }

    /// Writes a `bool` as one byte, 0 or 1.
    public mutating func writeBool(_ value: Bool) {
        buffer.append(value ? 1 : 0)
    }

    // MARK: Lengths, strings, bytes

    /// Writes a `u32` length or element count.
    ///
    /// - Precondition: `0 <= length <= UInt32.max`.
    public mutating func writeLen(_ length: Int) {
        guard let value = UInt32(exactly: length) else {
            preconditionFailure("UndraWriter.writeLen: \(length) does not fit in a u32")
        }
        writeU32(value)
    }

    /// Writes a `u32` byte length followed by the UTF-8 bytes of `value`.
    public mutating func writeString(_ value: String) {
        let utf8 = value.utf8
        writeLen(utf8.count)
        buffer.append(contentsOf: utf8)
    }

    /// Writes a `u32` length followed by the raw bytes (the wire `Bytes` type).
    public mutating func writeBytes(_ value: [UInt8]) {
        writeLen(value.count)
        buffer.append(contentsOf: value)
    }

    /// Writes a `u32` length followed by the raw bytes of a slice (the wire `Bytes` type).
    public mutating func writeBytes(slice value: ArraySlice<UInt8>) {
        writeLen(value.count)
        buffer.append(contentsOf: value)
    }

    /// Appends bytes verbatim, with no length prefix. Used for already-encoded payloads.
    public mutating func writeRaw(_ value: [UInt8]) {
        buffer.append(contentsOf: value)
    }

    /// Appends the bytes of a slice verbatim, with no length prefix.
    public mutating func writeRaw(slice value: ArraySlice<UInt8>) {
        buffer.append(contentsOf: value)
    }
}
