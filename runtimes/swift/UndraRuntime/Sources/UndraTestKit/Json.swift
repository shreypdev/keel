import Foundation

/// The JSON the kit reads: recordings and seeds. Not a general-purpose parser (no streaming, no big numbers), and the kit needs nothing
/// beyond the standard library for it.
enum JSON: Sendable {
    case object([String: JSON])
    case array([JSON])
    case string(String)
    case number(String)
    case bool(Bool)
    case null

    var object: [String: JSON]? {
        if case .object(let fields) = self {
            return fields
        }
        return nil
    }

    var array: [JSON]? {
        if case .array(let items) = self {
            return items
        }
        return nil
    }

    var string: String? {
        if case .string(let text) = self {
            return text
        }
        return nil
    }

    var int: Int64? {
        if case .number(let raw) = self {
            return Int64(raw)
        }
        return nil
    }

    var bool: Bool? {
        if case .bool(let value) = self {
            return value
        }
        return nil
    }
}

struct JSONError: Error, CustomStringConvertible {
    let message: String
    var description: String { message }
}

func parseJSON(_ text: String) throws -> JSON {
    var parser = JSONParser(Array(text.unicodeScalars))
    return try parser.parseDocument()
}

private struct JSONParser {
    private let s: [Unicode.Scalar]
    private var i = 0

    init(_ scalars: [Unicode.Scalar]) {
        self.s = scalars
    }

    mutating func parseDocument() throws -> JSON {
        let v = try value()
        ws()
        if i != s.count {
            throw fail("unexpected data after the document")
        }
        return v
    }

    private func fail(_ message: String) -> JSONError {
        return JSONError(message: "\(message) (at offset \(i))")
    }

    private mutating func ws() {
        while i < s.count, s[i] == " " || s[i] == "\n" || s[i] == "\r" || s[i] == "\t" {
            i += 1
        }
    }

    private mutating func value() throws -> JSON {
        ws()
        guard i < s.count else {
            throw fail("unexpected end of input")
        }
        switch s[i] {
        case "{": return try object()
        case "[": return try array()
        case "\"": return .string(try string())
        case "t": return try literal("true", .bool(true))
        case "f": return try literal("false", .bool(false))
        case "n": return try literal("null", .null)
        default:
            if s[i] == "-" || ("0"..."9").contains(s[i]) {
                return number()
            }
            throw fail("unexpected character")
        }
    }

    private mutating func literal(_ word: String, _ v: JSON) throws -> JSON {
        let scalars = Array(word.unicodeScalars)
        if i + scalars.count > s.count || Array(s[i..<i + scalars.count]) != scalars {
            throw fail("expected \(word)")
        }
        i += scalars.count
        return v
    }

    private mutating func number() -> JSON {
        let start = i
        if s[i] == "-" {
            i += 1
        }
        while i < s.count, ("0"..."9").contains(s[i]) || s[i] == "." || s[i] == "e" || s[i] == "E" || s[i] == "+" || s[i] == "-" {
            i += 1
        }
        var raw = ""
        for scalar in s[start..<i] {
            raw.unicodeScalars.append(scalar)
        }
        return .number(raw)
    }

    private mutating func object() throws -> JSON {
        i += 1
        var fields = [String: JSON]()
        ws()
        if i < s.count, s[i] == "}" {
            i += 1
            return .object(fields)
        }
        while true {
            ws()
            guard i < s.count, s[i] == "\"" else {
                throw fail("expected a string key")
            }
            let key = try string()
            ws()
            guard i < s.count, s[i] == ":" else {
                throw fail("expected ':'")
            }
            i += 1
            fields[key] = try value()
            ws()
            guard i < s.count else {
                throw fail("unterminated object")
            }
            if s[i] == "," {
                i += 1
                continue
            }
            if s[i] == "}" {
                i += 1
                return .object(fields)
            }
            throw fail("expected ',' or '}'")
        }
    }

    private mutating func array() throws -> JSON {
        i += 1
        var items = [JSON]()
        ws()
        if i < s.count, s[i] == "]" {
            i += 1
            return .array(items)
        }
        while true {
            items.append(try value())
            ws()
            guard i < s.count else {
                throw fail("unterminated array")
            }
            if s[i] == "," {
                i += 1
                continue
            }
            if s[i] == "]" {
                i += 1
                return .array(items)
            }
            throw fail("expected ',' or ']'")
        }
    }

    private mutating func string() throws -> String {
        i += 1
        var out = String.UnicodeScalarView()
        var pendingHigh: UInt32?
        while true {
            guard i < s.count else {
                throw fail("unterminated string")
            }
            let c = s[i]
            i += 1
            if c == "\"" {
                if pendingHigh != nil {
                    throw fail("unpaired surrogate")
                }
                return String(out)
            }
            if c == "\\" {
                guard i < s.count else {
                    throw fail("unterminated escape")
                }
                let e = s[i]
                i += 1
                switch e {
                case "\"": out.append("\"")
                case "\\": out.append("\\")
                case "/": out.append("/")
                case "b": out.append("\u{08}")
                case "f": out.append("\u{0c}")
                case "n": out.append("\n")
                case "r": out.append("\r")
                case "t": out.append("\t")
                case "u":
                    guard i + 4 <= s.count else {
                        throw fail("short \\u escape")
                    }
                    var hex = ""
                    for scalar in s[i..<i + 4] {
                        hex.unicodeScalars.append(scalar)
                    }
                    guard let unit = UInt32(hex, radix: 16) else {
                        throw fail("bad \\u escape")
                    }
                    i += 4
                    if let high = pendingHigh {
                        guard (0xDC00...0xDFFF).contains(unit), let scalar = Unicode.Scalar(0x10000 + ((high - 0xD800) << 10) + (unit - 0xDC00)) else {
                            throw fail("unpaired surrogate")
                        }
                        out.append(scalar)
                        pendingHigh = nil
                    } else if (0xD800...0xDBFF).contains(unit) {
                        pendingHigh = unit
                    } else if let scalar = Unicode.Scalar(unit) {
                        out.append(scalar)
                    } else {
                        throw fail("bad \\u escape")
                    }
                default:
                    throw fail("bad escape")
                }
            } else {
                if pendingHigh != nil {
                    throw fail("unpaired surrogate")
                }
                if c.value < 0x20 {
                    throw fail("control character in a string")
                }
                out.append(c)
            }
        }
    }
}

/// A JSON string escaped only where JSON requires, so every language's writer gives the same bytes.
func jsonString(_ text: String) -> String {
    var out = "\""
    for scalar in text.unicodeScalars {
        switch scalar {
        case "\"": out += "\\\""
        case "\\": out += "\\\\"
        case "\n": out += "\\n"
        case "\r": out += "\\r"
        case "\t": out += "\\t"
        default:
            if scalar.value < 0x20 {
                let hex = String(scalar.value, radix: 16)
                out += "\\u" + String(repeating: "0", count: 4 - hex.count) + hex
            } else {
                out.unicodeScalars.append(scalar)
            }
        }
    }
    return out + "\""
}
