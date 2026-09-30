/// A type with a canonical Keel wire encoding (docs/SPEC.md section 3.1).
///
/// Generated records, enums and errors conform to it (through `KeelRecord` / `KeelEnum` /
/// `KeelError`), and so do the wire primitives below. `keelDecode` must consume exactly the
/// bytes `keelEncode` produced and must throw `WireError` (never trap) on malformed input.
public protocol KeelCodec {
    /// Decodes one value from the reader, advancing it past the value.
    static func keelDecode(_ r: inout KeelReader) throws -> Self

    /// Appends the encoding of this value to the writer.
    func keelEncode(_ w: inout KeelWriter)
}

extension KeelCodec {
    /// The complete encoding of this value as a fresh byte array.
    public func keelEncoded() -> [UInt8] {
        var writer = KeelWriter()
        keelEncode(&writer)
        return writer.finish()
    }

    /// Decodes a value that must span all of `bytes`; leftover bytes throw
    /// `WireError.trailingBytes`.
    public static func keelDecoded(from bytes: [UInt8]) throws -> Self {
        var reader = KeelReader(bytes)
        let value = try Self.keelDecode(&reader)
        try reader.finish()
        return value
    }

    /// Decodes a value that must span all of `slice`; leftover bytes throw
    /// `WireError.trailingBytes`.
    public static func keelDecoded(slice: ArraySlice<UInt8>) throws -> Self {
        var reader = KeelReader(slice: slice)
        let value = try Self.keelDecode(&reader)
        try reader.finish()
        return value
    }
}

// MARK: - Fixed-width integers, floating point, bool, string

extension UInt8: KeelCodec {
    public static func keelDecode(_ r: inout KeelReader) throws -> UInt8 {
        return try r.readU8()
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeU8(self)
    }
}

extension UInt16: KeelCodec {
    public static func keelDecode(_ r: inout KeelReader) throws -> UInt16 {
        return try r.readU16()
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeU16(self)
    }
}

extension UInt32: KeelCodec {
    public static func keelDecode(_ r: inout KeelReader) throws -> UInt32 {
        return try r.readU32()
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeU32(self)
    }
}

extension UInt64: KeelCodec {
    public static func keelDecode(_ r: inout KeelReader) throws -> UInt64 {
        return try r.readU64()
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeU64(self)
    }
}

extension Int8: KeelCodec {
    public static func keelDecode(_ r: inout KeelReader) throws -> Int8 {
        return try r.readI8()
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeI8(self)
    }
}

extension Int16: KeelCodec {
    public static func keelDecode(_ r: inout KeelReader) throws -> Int16 {
        return try r.readI16()
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeI16(self)
    }
}

extension Int32: KeelCodec {
    public static func keelDecode(_ r: inout KeelReader) throws -> Int32 {
        return try r.readI32()
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeI32(self)
    }
}

extension Int64: KeelCodec {
    public static func keelDecode(_ r: inout KeelReader) throws -> Int64 {
        return try r.readI64()
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeI64(self)
    }
}

extension Float: KeelCodec {
    public static func keelDecode(_ r: inout KeelReader) throws -> Float {
        return try r.readF32()
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeF32(self)
    }
}

extension Double: KeelCodec {
    public static func keelDecode(_ r: inout KeelReader) throws -> Double {
        return try r.readF64()
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeF64(self)
    }
}

extension Bool: KeelCodec {
    public static func keelDecode(_ r: inout KeelReader) throws -> Bool {
        return try r.readBool()
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeBool(self)
    }
}

extension String: KeelCodec {
    public static func keelDecode(_ r: inout KeelReader) throws -> String {
        return try r.readString()
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeString(self)
    }
}

// MARK: - Option

extension Optional: KeelCodec where Wrapped: KeelCodec {
    /// `Option<T>`: a `u8` tag (0 = none, 1 = some) followed by the value when present.
    public static func keelDecode(_ r: inout KeelReader) throws -> Wrapped? {
        let at = r.position
        let tag = try r.readU8()
        switch tag {
        case 0:
            return nil
        case 1:
            let value = try Wrapped.keelDecode(&r)
            return .some(value)
        default:
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "Option")
        }
    }

    public func keelEncode(_ w: inout KeelWriter) {
        switch self {
        case .none:
            w.writeU8(0)
        case .some(let value):
            w.writeU8(1)
            value.keelEncode(&w)
        }
    }
}

// MARK: - Vec

/// The most elements a decoder pre-reserves up front. A count is already bounded by the bytes
/// that remain (`KeelReader.readLen`), but an element can be much larger than one byte, so the
/// reservation is capped to keep a hostile count from amplifying memory use; the array still
/// grows normally past the cap when the data really is there.
let keelMaxPreallocatedElements = 1 << 16

extension Array: KeelCodec where Element: KeelCodec {
    /// `Vec<T>`: a `u32` element count followed by the elements.
    ///
    /// Note: an array of `UInt8` is wire-identical to `Bytes`; `KeelBytes` is the faster path.
    /// Arrays of zero-width elements (`KeelUnit`) are rejected when the count exceeds the
    /// remaining byte count (see `KeelReader.readLen`).
    public static func keelDecode(_ r: inout KeelReader) throws -> [Element] {
        let count = try r.readLen()
        var result: [Element] = []
        result.reserveCapacity(Swift.min(count, keelMaxPreallocatedElements))
        var index = 0
        while index < count {
            let element = try Element.keelDecode(&r)
            result.append(element)
            index += 1
        }
        return result
    }

    public func keelEncode(_ w: inout KeelWriter) {
        w.writeLen(count)
        for element in self {
            element.keelEncode(&w)
        }
    }
}

// MARK: - Map

extension Dictionary: KeelCodec where Key: KeelCodec, Value: KeelCodec {
    /// `Map<K, V>`: a `u32` entry count followed by `(key, value)` pairs.
    /// Throws `WireError.duplicateKey` if a key repeats.
    public static func keelDecode(_ r: inout KeelReader) throws -> [Key: Value] {
        let count = try r.readLen()
        var result = [Key: Value](minimumCapacity: Swift.min(count, keelMaxPreallocatedElements))
        var index = 0
        while index < count {
            let at = r.position
            let key = try Key.keelDecode(&r)
            let value = try Value.keelDecode(&r)
            if result.updateValue(value, forKey: key) != nil {
                throw WireError.duplicateKey(at: at)
            }
            index += 1
        }
        return result
    }

    /// Encodes the entries sorted by the bytes of their encoded key (unsigned lexicographic
    /// order), so equal maps always encode to equal bytes.
    public func keelEncode(_ w: inout KeelWriter) {
        w.writeLen(count)
        if isEmpty {
            return
        }
        var entries: [(key: [UInt8], value: Value)] = []
        entries.reserveCapacity(count)
        for (key, value) in self {
            var keyWriter = KeelWriter()
            key.keelEncode(&keyWriter)
            entries.append((key: keyWriter.finish(), value: value))
        }
        entries.sort { left, right in
            return left.key.lexicographicallyPrecedes(right.key)
        }
        for entry in entries {
            w.writeRaw(entry.key)
            entry.value.keelEncode(&w)
        }
    }
}
