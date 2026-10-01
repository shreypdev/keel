import Foundation

/// A value behind a lock. Bodies are short and never call out.
final class Locked<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Value

    init(_ initial: Value) {
        self.value = initial
    }

    func withLock<R>(_ body: (inout Value) throws -> R) rethrows -> R {
        lock.lock()
        defer { lock.unlock() }
        return try body(&value)
    }
}

extension Array where Element == UInt8 {
    /// The bytes as lower-case hex, the encoding of every payload in a recording.
    var hex: String {
        let digits = [Character]("0123456789abcdef")
        var out = ""
        out.reserveCapacity(count * 2)
        for byte in self {
            out.append(digits[Int(byte >> 4)])
            out.append(digits[Int(byte & 15)])
        }
        return out
    }
}

extension String {
    /// Hex (either case) back to bytes; `nil` for an odd length or a character that is not a hex digit.
    func fromHex() -> [UInt8]? {
        let chars = Array(utf8)
        if chars.count % 2 != 0 {
            return nil
        }
        func digit(_ c: UInt8) -> UInt8? {
            switch c {
            case 48...57: return c - 48
            case 97...102: return c - 87
            case 65...70: return c - 55
            default: return nil
            }
        }
        var out = [UInt8]()
        out.reserveCapacity(chars.count / 2)
        var i = 0
        while i < chars.count {
            guard let high = digit(chars[i]), let low = digit(chars[i + 1]) else {
                return nil
            }
            out.append(high << 4 | low)
            i += 2
        }
        return out
    }

    /// Orders by Unicode scalar value, which is UTF-8 byte order, as the Rust fakes and the platform adapters do.
    /// (Swift's `<` on `String` follows Unicode canonical ordering, which is another order.)
    func utf8Precedes(_ other: String) -> Bool {
        return unicodeScalars.lexicographicallyPrecedes(other.unicodeScalars) { $0.value < $1.value }
    }
}

/// `0x` and sixteen lower-case hex digits.
func hex64(_ value: UInt64) -> String {
    let digits = String(value, radix: 16)
    return "0x" + String(repeating: "0", count: 16 - digits.count) + digits
}

/// Parses `0x..`; `nil` if the text is not that.
func parseHex64(_ text: String) -> UInt64? {
    guard text.hasPrefix("0x"), text.count >= 3, text.count <= 18 else {
        return nil
    }
    return UInt64(text.dropFirst(2), radix: 16)
}
