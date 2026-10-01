/// A type with a canonical Undra wire encoding (docs/SPEC.md section 3.1).
///
/// Generated records, enums and errors conform to it (through `UndraRecord` / `UndraEnum` /
/// `UndraError`), and so do the wire primitives below. `undraDecode` must consume exactly the
/// bytes `undraEncode` produced and must throw `WireError` (never trap) on malformed input.
public protocol UndraCodec {
    /// Decodes one value from the reader, advancing it past the value.
    static func undraDecode(_ r: inout UndraReader) throws -> Self

    /// Appends the encoding of this value to the writer.
    func undraEncode(_ w: inout UndraWriter)
}

extension UndraCodec {
    /// The complete encoding of this value as a fresh byte array.
    public func undraEncoded() -> [UInt8] {
        var writer = UndraWriter()
        undraEncode(&writer)
        return writer.finish()
    }

    /// Decodes a value that must span all of `bytes`; leftover bytes throw
    /// `WireError.trailingBytes`.
    public static func undraDecoded(from bytes: [UInt8]) throws -> Self {
        var reader = UndraReader(bytes)
        let value = try Self.undraDecode(&reader)
        try reader.finish()
        return value
    }

    /// Decodes a value that must span all of `slice`; leftover bytes throw
    /// `WireError.trailingBytes`.
    public static func undraDecoded(slice: ArraySlice<UInt8>) throws -> Self {
        var reader = UndraReader(slice: slice)
        let value = try Self.undraDecode(&reader)
        try reader.finish()
        return value
    }
}

// MARK: - Fixed-width integers, floating point, bool, string

extension UInt8: UndraCodec {
    public static func undraDecode(_ r: inout UndraReader) throws -> UInt8 {
        return try r.readU8()
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeU8(self)
    }
}

extension UInt16: UndraCodec {
    public static func undraDecode(_ r: inout UndraReader) throws -> UInt16 {
        return try r.readU16()
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeU16(self)
    }
}

extension UInt32: UndraCodec {
    public static func undraDecode(_ r: inout UndraReader) throws -> UInt32 {
        return try r.readU32()
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeU32(self)
    }
}

extension UInt64: UndraCodec {
    public static func undraDecode(_ r: inout UndraReader) throws -> UInt64 {
        return try r.readU64()
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeU64(self)
    }
}

extension Int8: UndraCodec {
    public static func undraDecode(_ r: inout UndraReader) throws -> Int8 {
        return try r.readI8()
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeI8(self)
    }
}

extension Int16: UndraCodec {
    public static func undraDecode(_ r: inout UndraReader) throws -> Int16 {
        return try r.readI16()
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeI16(self)
    }
}

extension Int32: UndraCodec {
    public static func undraDecode(_ r: inout UndraReader) throws -> Int32 {
        return try r.readI32()
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeI32(self)
    }
}

extension Int64: UndraCodec {
    public static func undraDecode(_ r: inout UndraReader) throws -> Int64 {
        return try r.readI64()
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeI64(self)
    }
}

extension Float: UndraCodec {
    public static func undraDecode(_ r: inout UndraReader) throws -> Float {
        return try r.readF32()
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeF32(self)
    }
}

extension Double: UndraCodec {
    public static func undraDecode(_ r: inout UndraReader) throws -> Double {
        return try r.readF64()
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeF64(self)
    }
}

extension Bool: UndraCodec {
    public static func undraDecode(_ r: inout UndraReader) throws -> Bool {
        return try r.readBool()
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeBool(self)
    }
}

extension String: UndraCodec {
    public static func undraDecode(_ r: inout UndraReader) throws -> String {
        return try r.readString()
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeString(self)
    }
}

// MARK: - Option

extension Optional: UndraCodec where Wrapped: UndraCodec {
    /// `Option<T>`: a `u8` tag (0 = none, 1 = some) followed by the value when present.
    public static func undraDecode(_ r: inout UndraReader) throws -> Wrapped? {
        let at = r.position
        let tag = try r.readU8()
        switch tag {
        case 0:
            return nil
        case 1:
            let value = try Wrapped.undraDecode(&r)
            return .some(value)
        default:
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "Option")
        }
    }

    public func undraEncode(_ w: inout UndraWriter) {
        switch self {
        case .none:
            w.writeU8(0)
        case .some(let value):
            w.writeU8(1)
            value.undraEncode(&w)
        }
    }
}

// MARK: - Vec

/// The most elements a decoder pre-reserves up front. A count is already bounded by the bytes
/// that remain (`UndraReader.readLen`), but an element can be much larger than one byte, so the
/// reservation is capped to keep a hostile count from amplifying memory use; the array still
/// grows normally past the cap when the data really is there.
let undraMaxPreallocatedElements = 1 << 16

extension Array: UndraCodec where Element: UndraCodec {
    /// `Vec<T>`: a `u32` element count followed by the elements.
    ///
    /// Note: an array of `UInt8` is wire-identical to `Bytes`; `UndraBytes` is the faster path.
    /// Arrays of zero-width elements (`UndraUnit`) are rejected when the count exceeds the
    /// remaining byte count (see `UndraReader.readLen`).
    public static func undraDecode(_ r: inout UndraReader) throws -> [Element] {
        let count = try r.readLen()
        var result: [Element] = []
        result.reserveCapacity(Swift.min(count, undraMaxPreallocatedElements))
        var index = 0
        while index < count {
            let element = try Element.undraDecode(&r)
            result.append(element)
            index += 1
        }
        return result
    }

    public func undraEncode(_ w: inout UndraWriter) {
        w.writeLen(count)
        for element in self {
            element.undraEncode(&w)
        }
    }
}

// MARK: - Map

extension Dictionary: UndraCodec where Key: UndraCodec, Value: UndraCodec {
    /// `Map<K, V>`: a `u32` entry count followed by `(key, value)` pairs.
    /// Throws `WireError.duplicateKey` if a key repeats.
    public static func undraDecode(_ r: inout UndraReader) throws -> [Key: Value] {
        let count = try r.readLen()
        var result = [Key: Value](minimumCapacity: Swift.min(count, undraMaxPreallocatedElements))
        var index = 0
        while index < count {
            let at = r.position
            let key = try Key.undraDecode(&r)
            let value = try Value.undraDecode(&r)
            if result.updateValue(value, forKey: key) != nil {
                throw WireError.duplicateKey(at: at)
            }
            index += 1
        }
        return result
    }

    /// Encodes the entries sorted by the bytes of their encoded key (unsigned lexicographic
    /// order), so equal maps always encode to equal bytes.
    public func undraEncode(_ w: inout UndraWriter) {
        w.writeLen(count)
        if isEmpty {
            return
        }
        var entries: [(key: [UInt8], value: Value)] = []
        entries.reserveCapacity(count)
        for (key, value) in self {
            var keyWriter = UndraWriter()
            key.undraEncode(&keyWriter)
            entries.append((key: keyWriter.finish(), value: value))
        }
        entries.sort { left, right in
            return left.key.lexicographicallyPrecedes(right.key)
        }
        for entry in entries {
            w.writeRaw(entry.key)
            entry.value.undraEncode(&w)
        }
    }
}
