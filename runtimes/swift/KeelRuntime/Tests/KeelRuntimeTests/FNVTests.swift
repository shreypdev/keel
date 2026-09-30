import XCTest
import KeelRuntime

/// FNV-1a identifiers (docs/SPEC.md section 1.1). Expected values come from the published FNV
/// test vectors and from the contract vectors, and were re-derived independently.
final class FNVTests: XCTestCase {
    func testContractVectors() {
        XCTAssertEqual(fnv1a32("Calculator.add"), 2_353_348_832)
        XCTAssertEqual(fnv1a64("keel"), 6_367_360_722_358_687_308)
    }

    func testEmptyStringIsTheOffsetBasis() {
        XCTAssertEqual(fnv1a32(""), 0x811C_9DC5)
        XCTAssertEqual(fnv1a64(""), 0xCBF2_9CE4_8422_2325)
    }

    func testPublishedFnv1aVectors() {
        XCTAssertEqual(fnv1a32("a"), 0xE40C_292C)
        XCTAssertEqual(fnv1a32("foobar"), 0xBF9C_F968)
        XCTAssertEqual(fnv1a64("a"), 0xAF63_DC4C_8601_EC8C)
        XCTAssertEqual(fnv1a64("foobar"), 0x8594_4171_F739_67E8)
    }

    func testSpecIdentifierShapes() {
        // type_id / method_id / port_id are fnv1a32 of "<TypeName>", "<TypeName>.<method>" and
        // "port.<TraitName>"; free functions use "fn.<name>".
        XCTAssertEqual(fnv1a32("port.Http"), 0x1EBE_B908)
        XCTAssertEqual(fnv1a32("Todos.add"), 0xD13D_7AD7)
        XCTAssertEqual(fnv1a32("fn.greet"), 0x0594_D9AE)
    }

    func testHashesTheUTF8BytesNotTheScalars() {
        // "é" is the two bytes c3 a9.
        XCTAssertEqual(fnv1a32("\u{E9}"), 0x1E9D_E8C1)
        XCTAssertEqual(fnv1a64("\u{E9}"), 0x0AC2_1707_B718_1E01)
        // Canonically equivalent strings with different bytes hash differently.
        XCTAssertNotEqual(fnv1a32("e\u{301}"), fnv1a32("\u{E9}"))
    }

    func testDistinctInputsGiveDistinctHashesHere() {
        XCTAssertNotEqual(fnv1a32("Calculator.add"), fnv1a32("Calculator.sub"))
        XCTAssertNotEqual(fnv1a64("Calculator.add"), fnv1a64("Calculator.sub"))
    }

    func testAgreesWithTheReferenceOverManyInputs() {
        // A slow reference written differently from the implementation: byte array and widening
        // multiplication with an explicit mask.
        func reference32(_ text: String) -> UInt32 {
            var hash: UInt64 = 0x811C_9DC5
            for byte in Array(text.utf8) {
                hash = hash ^ UInt64(byte)
                hash = (hash * 0x0100_0193) & 0xFFFF_FFFF
            }
            return UInt32(hash)
        }
        var rng = SplitMix64(seed: 42)
        for _ in 0 ..< 200 {
            var scalars = String.UnicodeScalarView()
            let count = Int(rng.next() % 30)
            for _ in 0 ..< count {
                let raw = UInt32(truncatingIfNeeded: rng.next() % 0x11_0000)
                if let scalar = Unicode.Scalar(raw) {
                    scalars.append(scalar)
                }
            }
            let text = String(scalars)
            XCTAssertEqual(fnv1a32(text), reference32(text))
        }
    }
}
