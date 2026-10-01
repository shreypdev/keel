// The transport envelope (docs/SPEC.md section 3.2), used on WebSocket and Worker transports.
//
//   magic      4 bytes  55 4E 44 52, the ASCII of `UNDR` (fixed by the wire format)
//   version    u16      1
//   schema     u64      schema_hash of the core that produced/expects this message
//   kind       u8       see Envelope.Kind
//   seq        u32      per-direction monotonically increasing, for ordering and debugging
//   len        u32      payload length
//   payload    len bytes
//
// The header is 23 bytes. In-process calls do not use envelopes.

/// Namespace for the envelope constants and the message kinds.
public enum Envelope {
    /// Size of the envelope header in bytes.
    public static let headerLength = 23

    /// The magic bytes at the start of every envelope: `55 4E 44 52`, the ASCII of `UNDR`.
    public static let magic: [UInt8] = [0x55, 0x4E, 0x44, 0x52]

    /// The only envelope version this runtime speaks.
    public static let version: UInt16 = 1

    /// The envelope's `kind` byte (docs/SPEC.md section 3.2).
    public enum Kind: UInt8, Sendable, Hashable, CaseIterable {
        /// host to core: `Call`
        case call = 1
        /// core to host: `Reply`
        case reply = 2
        /// core to host: `ChangeSet`
        case changeSet = 3
        /// core to host: `PortCall`
        case portCall = 4
        /// host to core: `PortReply`
        case portReply = 5
        /// host to core: `Cancel`
        case cancel = 6
        /// host to core: `StreamCredit`
        case streamCredit = 7
        /// core to host: `StreamItem`
        case streamItem = 8
        /// host to core: `Observe`
        case observe = 9
        /// host to core: `Release`
        case release = 10
        /// host to core: `Event`
        case event = 11
        /// both directions: `Hello`
        case hello = 12
        /// core to host: `Log`
        case log = 13
        /// host to core: `TimerFired`
        case timerFired = 14
        /// core to host: `Snapshot`
        case snapshot = 15
        /// host to core: `Snapshot` payload layout, asking the core to restore it
        case restore = 16
    }

    /// The result of `decodeEnvelope`.
    public typealias Decoded = (kind: Kind, seq: UInt32, schemaHash: UInt64, payload: ArraySlice<UInt8>)

    /// Writes the 23-byte header for a payload of `payloadLength` bytes. Callers that build
    /// the payload in the same writer (to avoid a copy) write the header first, then the payload.
    ///
    /// - Precondition: `0 <= payloadLength <= UInt32.max`.
    public static func writeHeader(
        into w: inout UndraWriter,
        kind: Kind,
        seq: UInt32,
        schemaHash: UInt64,
        payloadLength: Int
    ) {
        w.writeU32(0x5244_4E55) // the magic as a little-endian u32: bytes 55 4E 44 52
        w.writeU16(version)
        w.writeU64(schemaHash)
        w.writeU8(kind.rawValue)
        w.writeU32(seq)
        w.writeLen(payloadLength)
    }

    /// Throws `WireError.schemaMismatch` unless `got == expected`. The runtime calls this with
    /// the hash from a received envelope or `Hello` and the hash of the schema it was built from.
    public static func requireSchemaHash(_ got: UInt64, expected: UInt64) throws {
        if got != expected {
            throw WireError.schemaMismatch(expected: expected, got: got)
        }
    }
}

/// Encodes an envelope: the 23-byte header followed by `payload`.
public func encodeEnvelope(
    kind: Envelope.Kind,
    seq: UInt32,
    schemaHash: UInt64,
    payload: [UInt8]
) -> [UInt8] {
    var w = UndraWriter(capacity: Envelope.headerLength + payload.count)
    Envelope.writeHeader(
        into: &w,
        kind: kind,
        seq: seq,
        schemaHash: schemaHash,
        payloadLength: payload.count
    )
    w.writeRaw(payload)
    return w.finish()
}

/// Decodes an envelope that must span all of `bytes`.
///
/// The returned payload is a slice of `bytes` (no copy). Failures, in the order they are
/// checked: `unexpectedEOF` (fewer bytes than a header), `badMagic`, `unsupportedVersion`,
/// `invalidTag` (unknown kind), `lengthTooLarge` (payload longer than what follows the header),
/// `trailingBytes` (bytes after the payload). The schema hash is returned, not checked; compare
/// it with `Envelope.requireSchemaHash`.
public func decodeEnvelope(_ bytes: [UInt8]) throws -> Envelope.Decoded {
    return try decodeEnvelope(slice: bytes[0 ..< bytes.count])
}

/// Decodes an envelope that must span all of `slice`. See `decodeEnvelope(_:)`.
public func decodeEnvelope(slice: ArraySlice<UInt8>) throws -> Envelope.Decoded {
    var r = UndraReader(slice: slice)
    let magic = try r.readU32()
    if magic != 0x5244_4E55 {
        throw WireError.badMagic
    }
    let version = try r.readU16()
    if version != Envelope.version {
        throw WireError.unsupportedVersion(version)
    }
    let schemaHash = try r.readU64()
    let kindAt = r.position
    let rawKind = try r.readU8()
    guard let kind = Envelope.Kind(rawValue: rawKind) else {
        throw WireError.invalidTag(tag: UInt32(rawKind), at: kindAt, type: "Kind")
    }
    let seq = try r.readU32()
    let length = try r.readLen()
    let payload = try r.readSlice(length)
    try r.finish()
    return (kind: kind, seq: seq, schemaHash: schemaHash, payload: payload)
}
