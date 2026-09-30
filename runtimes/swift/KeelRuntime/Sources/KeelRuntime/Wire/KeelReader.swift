/// Reads Keel wire values from a byte buffer with a cursor (docs/SPEC.md sections 3.1 and 3.9).
///
/// The reader is a plain value over safe Swift storage: it holds an `ArraySlice<UInt8>` (which
/// shares, and never copies, the storage of the array or slice it was created from) plus a
/// cursor and an end limit. No `unsafe` pointer is ever dereferenced. Every read is bounds
/// checked and throws `WireError` on malformed input; nothing here can trap on hostile bytes.
///
/// Offsets in errors, and `position`, are measured from the start of the buffer the reader
/// was created over. Sub-readers created by `readSubReader(length:)` keep that origin.
///
/// ```swift
/// var r = KeelReader(bytes)
/// let id = try r.readU32()
/// let title = try r.readString()
/// try r.finish()          // throws WireError.trailingBytes if anything is left
/// ```
public struct KeelReader: Sendable {
    private let storage: ArraySlice<UInt8>
    private let origin: Int
    private let limit: Int
    private var cursor: Int

    // MARK: Creating a reader

    /// Creates a reader over `bytes`. O(1): the array's storage is shared, not copied.
    public init(_ bytes: [UInt8]) {
        self.init(slice: bytes[0 ..< bytes.count])
    }

    /// Creates a reader over a slice. O(1): the slice's storage is shared, not copied.
    public init(slice: ArraySlice<UInt8>) {
        self.storage = slice
        self.origin = slice.startIndex
        self.limit = slice.endIndex
        self.cursor = slice.startIndex
    }

    /// Creates a reader over a **copy** of the bytes in `buffer`.
    ///
    /// Use this for memory whose lifetime the runtime does not control, such as the
    /// `(ptr, len)` pair handed to a C callback, which is only valid until the callback returns.
    /// The copy is what keeps the reader (and any `ArraySlice` obtained from it) safe to use
    /// after the callback has returned.
    public init(copying buffer: UnsafeBufferPointer<UInt8>) {
        let copy = Array(buffer)
        self.init(copy)
    }

    /// Creates a reader over a **copy** of `count` bytes at `pointer`. A `nil` pointer or a
    /// count of zero (or less) yields an empty reader.
    public init(copying pointer: UnsafePointer<UInt8>?, count: Int) {
        if let pointer = pointer, count > 0 {
            let copy = Array(UnsafeBufferPointer(start: pointer, count: count))
            self.init(copy)
        } else {
            let empty: [UInt8] = []
            self.init(empty)
        }
    }

    private init(storage: ArraySlice<UInt8>, origin: Int, limit: Int, cursor: Int) {
        self.storage = storage
        self.origin = origin
        self.limit = limit
        self.cursor = cursor
    }

    // MARK: Position

    /// The offset of the next unread byte from the start of the buffer.
    public var position: Int {
        return cursor - origin
    }

    /// The number of bytes that can still be read.
    public var remaining: Int {
        return limit - cursor
    }

    /// Whether every byte has been read.
    public var isAtEnd: Bool {
        return cursor == limit
    }

    /// Succeeds only if every byte has been consumed; otherwise throws
    /// `WireError.trailingBytes`. Call it after decoding a complete top-level value.
    public func finish() throws {
        if cursor != limit {
            throw WireError.trailingBytes(count: limit - cursor)
        }
    }

    // MARK: Fixed-width integers

    /// Reads a `u8`.
    public mutating func readU8() throws -> UInt8 {
        let start = try advance(by: 1)
        return storage[start]
    }

    /// Reads a little-endian `u16`.
    public mutating func readU16() throws -> UInt16 {
        let start = try advance(by: 2)
        let low = UInt16(storage[start])
        let high = UInt16(storage[start + 1]) << 8
        return low | high
    }

    /// Reads a little-endian `u32`.
    public mutating func readU32() throws -> UInt32 {
        let start = try advance(by: 4)
        var value: UInt32 = 0
        var index = start + 3
        while index >= start {
            value = (value << 8) | UInt32(storage[index])
            index -= 1
        }
        return value
    }

    /// Reads a little-endian `u64`.
    public mutating func readU64() throws -> UInt64 {
        let start = try advance(by: 8)
        var value: UInt64 = 0
        var index = start + 7
        while index >= start {
            value = (value << 8) | UInt64(storage[index])
            index -= 1
        }
        return value
    }

    /// Reads a big-endian `u64`. Only used for the two halves of a UUID (RFC 4122 byte order).
    public mutating func readU64BigEndian() throws -> UInt64 {
        let start = try advance(by: 8)
        var value: UInt64 = 0
        var index = start
        while index < start + 8 {
            value = (value << 8) | UInt64(storage[index])
            index += 1
        }
        return value
    }

    /// Reads an `i8` (two's complement).
    public mutating func readI8() throws -> Int8 {
        return Int8(bitPattern: try readU8())
    }

    /// Reads a little-endian `i16` (two's complement).
    public mutating func readI16() throws -> Int16 {
        return Int16(bitPattern: try readU16())
    }

    /// Reads a little-endian `i32` (two's complement).
    public mutating func readI32() throws -> Int32 {
        return Int32(bitPattern: try readU32())
    }

    /// Reads a little-endian `i64` (two's complement).
    public mutating func readI64() throws -> Int64 {
        return Int64(bitPattern: try readU64())
    }

    // MARK: Floating point, bool

    /// Reads a little-endian IEEE 754 `Float`.
    public mutating func readF32() throws -> Float {
        return Float(bitPattern: try readU32())
    }

    /// Reads a little-endian IEEE 754 `Double`.
    public mutating func readF64() throws -> Double {
        return Double(bitPattern: try readU64())
    }

    /// Reads a `bool`. Only the bytes 0 and 1 are valid; anything else throws
    /// `WireError.invalidTag` with type `"bool"`.
    public mutating func readBool() throws -> Bool {
        let at = position
        let value = try readU8()
        switch value {
        case 0:
            return false
        case 1:
            return true
        default:
            throw WireError.invalidTag(tag: UInt32(value), at: at, type: "bool")
        }
    }

    // MARK: Lengths, strings, bytes

    /// Reads a `u32` length or element count and checks that it does not exceed the number of
    /// bytes that remain after it; otherwise throws `WireError.lengthTooLarge` (with `at` set
    /// to the offset of the length itself).
    ///
    /// The check assumes every counted element occupies at least one byte, which holds for
    /// every wire type except a zero-width element such as `KeelUnit`. It is what stops a
    /// hostile count from making a decoder reserve gigabytes.
    public mutating func readLen() throws -> Int {
        let at = position
        let raw = try readU32()
        if UInt64(raw) > UInt64(remaining) {
            throw WireError.lengthTooLarge(len: raw, at: at)
        }
        return Int(raw)
    }

    /// Reads a `u32` byte length and that many UTF-8 bytes.
    ///
    /// The bytes are validated as well-formed UTF-8 (no overlong forms, no surrogates, nothing
    /// above U+10FFFF); on failure this throws `WireError.invalidUTF8` with `at` set to the
    /// offset of the first byte of the string body. Embedded NUL characters are valid.
    public mutating func readString() throws -> String {
        let length = try readLen()
        let start = cursor
        let bytes = storage[start ..< start + length]
        // `String(decoding:)` repairs ill-formed input with U+FFFD, so a repaired string can
        // never be byte-identical to its ill-formed source. Comparing the UTF-8 of the result
        // with the source is therefore an exact well-formedness check, with no hand-written
        // validator to get wrong.
        let text = String(decoding: bytes, as: UTF8.self)
        guard text.utf8.count == length, text.utf8.elementsEqual(bytes) else {
            throw WireError.invalidUTF8(at: start - origin)
        }
        cursor += length
        return text
    }

    /// Reads a `u32` length and that many raw bytes, copying them into a new array.
    public mutating func readBytes() throws -> [UInt8] {
        let length = try readLen()
        let start = cursor
        cursor += length
        return Array(storage[start ..< start + length])
    }

    /// Reads a `u32` length and that many raw bytes as a slice of the underlying storage
    /// (no copy).
    public mutating func readBytesSlice() throws -> ArraySlice<UInt8> {
        let length = try readLen()
        let start = cursor
        cursor += length
        return storage[start ..< start + length]
    }

    // MARK: Unprefixed byte runs

    /// Reads exactly `count` bytes with no length prefix, copying them.
    ///
    /// - Precondition: `count >= 0`.
    public mutating func readRaw(_ count: Int) throws -> [UInt8] {
        precondition(count >= 0, "KeelReader.readRaw: negative count")
        let start = try advance(by: count)
        return Array(storage[start ..< start + count])
    }

    /// Reads exactly `count` bytes with no length prefix as a slice of the underlying storage
    /// (no copy).
    ///
    /// - Precondition: `count >= 0`.
    public mutating func readSlice(_ count: Int) throws -> ArraySlice<UInt8> {
        precondition(count >= 0, "KeelReader.readSlice: negative count")
        let start = try advance(by: count)
        return storage[start ..< start + count]
    }

    /// Consumes every remaining byte and returns them as a slice of the underlying storage
    /// (no copy). Used for the trailing `args` / `body` of a payload.
    public mutating func readRemaining() -> ArraySlice<UInt8> {
        let slice = storage[cursor ..< limit]
        cursor = limit
        return slice
    }

    /// Skips `count` bytes.
    ///
    /// - Precondition: `count >= 0`.
    public mutating func skip(_ count: Int) throws {
        precondition(count >= 0, "KeelReader.skip: negative count")
        _ = try advance(by: count)
    }

    /// Returns a reader restricted to the next `length` bytes and advances this reader past
    /// them. The sub-reader shares storage (no copy) and keeps this reader's error-offset origin.
    ///
    /// - Precondition: `length >= 0`.
    public mutating func readSubReader(length: Int) throws -> KeelReader {
        precondition(length >= 0, "KeelReader.readSubReader: negative length")
        let start = try advance(by: length)
        return KeelReader(storage: storage, origin: origin, limit: start + length, cursor: start)
    }

    // MARK: Internals

    /// Reserves `count` bytes: returns the absolute index of the first one and moves the
    /// cursor past them, or throws `unexpectedEOF` without moving.
    private mutating func advance(by count: Int) throws -> Int {
        if count > limit - cursor {
            throw WireError.unexpectedEOF(needed: count, at: cursor - origin)
        }
        let start = cursor
        cursor += count
        return start
    }
}
