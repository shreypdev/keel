import Foundation
import XCTest
import UndraRuntime

/// Round trips (empty, extremes, unicode) and exact encodings for every `UndraCodec` conformance.
final class CodecTests: XCTestCase {
    // MARK: Integers

    func testUnsignedIntegerExtremes() {
        assertRoundTrip(UInt8.min)
        assertRoundTrip(UInt8.max)
        assertRoundTrip(UInt16.min)
        assertRoundTrip(UInt16.max)
        assertRoundTrip(UInt32.min)
        assertRoundTrip(UInt32.max)
        assertRoundTrip(UInt64.min)
        assertRoundTrip(UInt64.max)
        assertCodec(UInt16(0x0102), hex: "0201")
        assertCodec(UInt32(0x0102_0304), hex: "04030201")
        assertCodec(UInt64(0x0102_0304_0506_0708), hex: "0807060504030201")
    }

    func testSignedIntegerExtremes() {
        assertRoundTrip(Int8.min)
        assertRoundTrip(Int8.max)
        assertRoundTrip(Int16.min)
        assertRoundTrip(Int16.max)
        assertRoundTrip(Int32.min)
        assertRoundTrip(Int32.max)
        assertRoundTrip(Int64.min)
        assertRoundTrip(Int64.max)
        assertRoundTrip(Int64(0))
        assertRoundTrip(Int64(-1))
        assertCodec(Int8.min, hex: "80")
        assertCodec(Int16.min, hex: "0080")
        assertCodec(Int32.min, hex: "00000080")
        assertCodec(Int64.min, hex: "0000000000000080")
        assertCodec(Int32(-2), hex: "feffffff")
    }

    // MARK: Floating point

    func testFloatSpecialValuesKeepTheirBits() throws {
        let floats: [Float] = [
            0.0, -0.0, 1.0, -1.0, Float.infinity, -Float.infinity,
            Float.leastNonzeroMagnitude, Float.leastNormalMagnitude, Float.greatestFiniteMagnitude,
            Float(bitPattern: 0x7FC0_1234), // a NaN with a payload
            Float(bitPattern: 0xFFC0_0001),
        ]
        for value in floats {
            let decoded = try Float.undraDecoded(from: value.undraEncoded())
            XCTAssertEqual(decoded.bitPattern, value.bitPattern, "bits of \(value)")
        }
        assertCodec(Float(3.14), hex: "c3f54840")
    }

    func testDoubleSpecialValuesKeepTheirBits() throws {
        let doubles: [Double] = [
            0.0, -0.0, 1.0, -1.0, Double.infinity, -Double.infinity,
            Double.leastNonzeroMagnitude, Double.leastNormalMagnitude, Double.greatestFiniteMagnitude,
            Double(bitPattern: 0x7FF8_0000_0000_1234),
            Double(bitPattern: 0xFFF8_0000_0000_0001),
        ]
        for value in doubles {
            let decoded = try Double.undraDecoded(from: value.undraEncoded())
            XCTAssertEqual(decoded.bitPattern, value.bitPattern, "bits of \(value)")
        }
        assertCodec(Double(2.718281828459045), hex: "6957148b0abf0540")
    }

    // MARK: Bool

    func testBool() {
        assertCodec(true, hex: "01")
        assertCodec(false, hex: "00")
    }

    // MARK: String

    func testStringRoundTripsByteForByte() throws {
        let strings: [String] = [
            "",
            "a",
            "hello",
            "h\u{E9}llo \u{1F30A}",
            "e\u{301}", // decomposed e + combining acute: must not be normalised
            "\u{E9}", // precomposed: equal to the line above as Swift strings, different bytes
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}", // family emoji: ZWJ sequence
            "\u{1F1EF}\u{1F1F5}", // regional indicators
            "a\u{0}b", // embedded NUL
            "\u{0}",
            "\u{FFFF}", // noncharacter, still valid UTF-8
            "\u{10FFFF}", // highest scalar
            "\u{7F}\u{80}\u{7FF}\u{800}\u{FFFF}\u{10000}", // every UTF-8 length boundary
            "\u{65E5}\u{672C}\u{8A9E} \u{440}\u{443}\u{441}\u{441}\u{43A}\u{438}\u{439} \u{5E2}\u{5D1}\u{5E8}\u{5D9}\u{5EA}",
            "line1\nline2\r\n\ttab",
            String(repeating: "x\u{1F30A}", count: 10_000),
        ]
        for text in strings {
            let bytes = text.undraEncoded()
            XCTAssertEqual(bytes.count, 4 + text.utf8.count)
            let decoded = try String.undraDecoded(from: bytes)
            XCTAssertEqual(utf8Bytes(decoded), utf8Bytes(text))
        }
    }

    func testStringEncodingIsLengthPrefixedUTF8() {
        assertCodec("", hex: "00000000")
        assertCodec("h\u{E9}llo \u{1F30A}", hex: "0b00000068c3a96c6c6f20f09f8c8a")
        // The decomposed and precomposed forms are equal Swift strings but distinct on the wire.
        XCTAssertEqual(bytesToHex("e\u{301}".undraEncoded()), "0300000065cc81")
        XCTAssertEqual(bytesToHex("\u{E9}".undraEncoded()), "02000000c3a9")
    }

    // MARK: Bytes

    func testBytes() throws {
        assertCodec(UndraBytes([]), hex: "00000000")
        assertCodec(UndraBytes([1, 2, 3, 255]), hex: "04000000010203ff")
        var every: [UInt8] = []
        for value in 0 ... 255 {
            every.append(UInt8(value))
        }
        assertRoundTrip(UndraBytes(every))
        assertRoundTrip(UndraBytes([UInt8](repeating: 0xAB, count: 100_000)))
    }

    func testByteArrayAndUndraBytesShareAnEncoding() throws {
        let samples: [[UInt8]] = [[], [0], [1, 2, 3], [UInt8](repeating: 7, count: 300)]
        for sample in samples {
            XCTAssertEqual(sample.undraEncoded(), UndraBytes(sample).undraEncoded())
            let viaBytes = try UndraBytes.undraDecoded(from: sample.undraEncoded())
            XCTAssertEqual(viaBytes.bytes, sample)
        }
    }

    // MARK: Option

    func testOption() {
        assertCodec(String?.none, hex: "00")
        assertCodec(String?.some("x"), hex: "010100000078")
        assertCodec(Int32?.some(-1), hex: "01ffffffff")
        assertCodec(Int32?.none, hex: "00")
        assertRoundTrip(Bool?.some(false))
    }

    func testNestedOption() {
        let none: Int32?? = .none
        let someNone: Int32?? = .some(.none)
        let someSome: Int32?? = .some(.some(7))
        assertCodec(none, hex: "00")
        assertCodec(someNone, hex: "0100")
        assertCodec(someSome, hex: "010107000000")
    }

    // MARK: Vec

    func testVec() {
        assertCodec([Int32](), hex: "00000000")
        assertCodec([Int32(1), -1, 7], hex: "0300000001000000ffffffff07000000")
        assertCodec([String](), hex: "00000000")
        let strings: [String] = ["a", ""]
        assertCodec(strings, hex: "02000000010000006100000000")
        let nested: [[Int32]] = [[], [1], [2, 3]]
        assertRoundTrip(nested)
        let optionals: [String?] = [nil, "x", nil, ""]
        assertRoundTrip(optionals)
        let big: [Int64] = (0 ..< 5000).map { Int64($0) * -1_000_003 }
        assertRoundTrip(big)
        let doubles: [Double] = [0.5, -0.25, Double.infinity]
        assertRoundTrip(doubles)
    }

    func testVecOfUnicodeStrings() throws {
        let items = ["", "\u{E9}", "\u{1F30A}", "\u{65E5}\u{672C}"]
        let decoded = try [String].undraDecoded(from: items.undraEncoded())
        XCTAssertEqual(decoded.map { utf8Bytes($0) }, items.map { utf8Bytes($0) })
    }

    // MARK: Map

    func testMapVectorAndDeterminism() {
        assertCodec(["b": Int32(2), "a": 1], hex: "02000000010000006101000000010000006202000000")
        assertCodec([String: Int32](), hex: "00000000")

        // Whatever the insertion order, the bytes are identical.
        var forward: [String: Int32] = [:]
        var backward: [String: Int32] = [:]
        let keys = (0 ..< 64).map { "key-\($0)" }
        for (offset, key) in keys.enumerated() {
            forward[key] = Int32(offset)
        }
        for (offset, key) in keys.enumerated().reversed() {
            backward[key] = Int32(offset)
        }
        XCTAssertEqual(forward.undraEncoded(), backward.undraEncoded())
        assertRoundTrip(forward)
    }

    func testMapSortsByEncodedKeyBytesNotByKeyOrder() {
        // Strings: the encoding starts with the u32 LE length, so "b" (length 1) sorts before "aa".
        let byLength: [String: Int32] = ["aa": 1, "b": 2]
        XCTAssertEqual(bytesToHex(byLength.undraEncoded()), "0200000001000000620200000002000000616101000000")
        // Integers: 256 encodes as 00 01 00 00 and 1 as 01 00 00 00, so 256 sorts first.
        let byBytes: [UInt32: UInt8] = [1: 0xAA, 256: 0xBB]
        XCTAssertEqual(bytesToHex(byBytes.undraEncoded()), "0200000000010000bb01000000aa")
    }

    func testMapWithCompoundValuesRoundTrips() {
        let value: [String: [Int32]] = ["": [], "x": [1, 2, 3], "\u{1F30A}": [-1]]
        assertRoundTrip(value)
        let optional: [UInt16: String?] = [0: nil, 65535: "max", 7: ""]
        assertRoundTrip(optional)
    }

    // MARK: Random round trips

    func testRandomValuesRoundTrip() throws {
        var rng = SplitMix64(seed: 0x1234_5678)
        for _ in 0 ..< 300 {
            let ints = randomInt64Array(&rng)
            assertRoundTrip(ints)

            let strings = randomStringArray(&rng)
            let decoded = try [String].undraDecoded(from: strings.undraEncoded())
            XCTAssertEqual(decoded.map { utf8Bytes($0) }, strings.map { utf8Bytes($0) })

            var map: [UInt32: [String]] = [:]
            let entries = Int(rng.next() % 6)
            for _ in 0 ..< entries {
                let key = UInt32(truncatingIfNeeded: rng.next())
                let value = randomStringArray(&rng)
                map[key] = value
            }
            let decodedMap = try [UInt32: [String]].undraDecoded(from: map.undraEncoded())
            XCTAssertEqual(decodedMap.count, map.count)
            for (key, expected) in map {
                XCTAssertEqual((decodedMap[key] ?? []).map { utf8Bytes($0) }, expected.map { utf8Bytes($0) })
            }
        }
    }

    private func randomInt64Array(_ rng: inout SplitMix64) -> [Int64] {
        let count = Int(rng.next() % 40)
        var out: [Int64] = []
        for _ in 0 ..< count {
            out.append(Int64(bitPattern: rng.next()))
        }
        return out
    }

    private func randomStringArray(_ rng: inout SplitMix64) -> [String] {
        let count = Int(rng.next() % 6)
        var out: [String] = []
        for _ in 0 ..< count {
            out.append(randomString(&rng))
        }
        return out
    }

    private func randomString(_ rng: inout SplitMix64) -> String {
        var scalars = String.UnicodeScalarView()
        let count = Int(rng.next() % 24)
        var index = 0
        while index < count {
            let raw = UInt32(truncatingIfNeeded: rng.next() % 0x11_0000)
            if let scalar = Unicode.Scalar(raw) {
                scalars.append(scalar)
            }
            index += 1
        }
        return String(scalars)
    }

    // MARK: Records and enums built from the primitives (what generated code does)

    func testGeneratedShapeRecordEnum() throws {
        let id = try XCTUnwrap(UndraUUID(uuidString: "123e4567-e89b-12d3-a456-426614174000"))
        assertCodec(
            Todo(id: id, title: "Milk", done: false),
            hex: "123e4567e89b12d3a456426614174000040000004d696c6b00"
        )
        assertCodec(Filter.done, hex: "0200")
        assertCodec(Shape.rect(w: 2.0, h: 3.0), hex: "010000000000000000400000000000000840")
        assertCodec(Shape.circle(radius: 1.0), hex: "0000000000000000f03f")
        assertRoundTrip([Todo(id: id, title: "\u{1F30A}", done: true), Todo(id: id, title: "", done: false)])
    }
}
