/// Errors produced by the Undra wire layer (docs/SPEC.md section 3).
///
/// Decoding never traps on malformed input: every failure is one of these typed values. All
/// `at` offsets are byte offsets from the start of the buffer the failing `UndraReader` was
/// created over (for a sub-reader handed out by `UndraReader.readSubReader(length:)` the offset
/// is still relative to the original buffer, which makes it directly usable for debugging).
public enum WireError: Error, Sendable, Equatable {
    /// A read needed `needed` bytes starting at offset `at` but fewer were available.
    case unexpectedEOF(needed: Int, at: Int)

    /// A string payload starting at offset `at` is not well-formed UTF-8.
    case invalidUTF8(at: Int)

    /// A tag, discriminant or variant index has no meaning for `type`.
    /// `tag` is the offending value, `at` the offset of the tag itself.
    case invalidTag(tag: UInt32, at: Int, type: String)

    /// A `u32` length or element count at offset `at` claims more than the bytes that remain.
    case lengthTooLarge(len: UInt32, at: Int)

    /// A decode that must consume its whole input left `count` unread bytes.
    case trailingBytes(count: Int)

    /// An envelope does not start with the magic bytes `UNDRA`.
    case badMagic

    /// An envelope carries a protocol version other than 1.
    case unsupportedVersion(UInt16)

    /// The peer was built from a different schema than this runtime.
    case schemaMismatch(expected: UInt64, got: UInt64)

    /// A map contains the same key twice; `at` is the offset of the second occurrence.
    case duplicateKey(at: Int)

    /// A duration is negative; wire durations are non-negative. `at` is the offset of the value.
    case negativeDuration(at: Int)
}

extension WireError: CustomStringConvertible {
    public var description: String {
        switch self {
        case .unexpectedEOF(let needed, let at):
            return "unexpected end of input: needed \(needed) byte(s) at offset \(at)"
        case .invalidUTF8(let at):
            return "invalid UTF-8 in string at offset \(at)"
        case .invalidTag(let tag, let at, let type):
            return "invalid tag \(tag) for \(type) at offset \(at)"
        case .lengthTooLarge(let len, let at):
            return "length \(len) at offset \(at) exceeds the remaining input"
        case .trailingBytes(let count):
            return "\(count) trailing byte(s) after the end of the value"
        case .badMagic:
            return "bad envelope magic (expected \"UNDRA\")"
        case .unsupportedVersion(let version):
            return "unsupported envelope version \(version) (this runtime speaks version 1)"
        case .schemaMismatch(let expected, let got):
            return "schema mismatch: expected 0x\(String(expected, radix: 16)), got 0x\(String(got, radix: 16))"
        case .duplicateKey(let at):
            return "duplicate map key at offset \(at)"
        case .negativeDuration(let at):
            return "negative duration at offset \(at)"
        }
    }
}
