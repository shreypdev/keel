import XCTest
@testable import UndraTestKit
import UndraRuntime

final class RecordingTests: XCTestCase {
    func testFixturesReadAndWriteBackByteForByte() throws {
        for name in ["recording-all-kinds.json", "session-todos.json", "ports-remote-todos.json"] {
            let text = try fixture("fixtures/\(name)")
            XCTAssertEqual(try Recording(json: text).toJSON(), text, name)
        }
    }

    func testEveryKindReadsWithItsTypedFields() throws {
        let r = try Recording(json: try fixture("fixtures/recording-all-kinds.json"))
        XCTAssertEqual(r.schemaHash, 0xdead_beef)
        XCTAssertEqual(r.platform, "ios")
        var calls = [RecordedTarget]()
        var observes = [(UInt64, UInt32, Bool)]()
        var kinds = Set<String>()
        for event in r.events {
            switch event.kind {
            case .call(let target, _, _): calls.append(target); kinds.insert("call")
            case .observe(let handle, let signal, let on): observes.append((handle, signal, on)); kinds.insert("observe")
            case .reply: kinds.insert("reply")
            case .changeSet: kinds.insert("change_set")
            case .streamItem: kinds.insert("stream_item")
            case .portCall: kinds.insert("port_call")
            case .portReply: kinds.insert("port_reply")
            case .portEvent: kinds.insert("event")
            case .timerFired: kinds.insert("timer_fired")
            case .release: kinds.insert("release")
            case .cancel: kinds.insert("cancel")
            }
        }
        XCTAssertEqual(kinds.count, 11)
        XCTAssertTrue(calls.contains(.page(handle: 0x2_0000_0003, offset: 0, limit: 50)))
        XCTAssertEqual(observes.first?.1, UInt32.max)
        XCTAssertEqual(observes.first?.2, true)
    }

    func testTheStandardPortsCarryANameAndOnlyThey() {
        XCTAssertEqual(undraStandardName(port: undraPortId("Http"), method: undraMethodId("Http", "request")), "Http.request")
        XCTAssertEqual(undraStandardName(port: undraPortId("SecureStore"), method: undraMethodId("SecureStore", "get")), "SecureStore.get")
        XCTAssertNil(undraStandardName(port: 1, method: 2))
    }

    func testStringsAreEscapedOnlyWhereJSONRequires() throws {
        let text = Recording(schemaHash: 1, source: "a\"b\\c\nd\u{01}é").toJSON()
        XCTAssertTrue(text.contains("\"source\": \"a\\\"b\\\\c\\nd\\u0001é\""), text)
        XCTAssertEqual(try Recording(json: text).source, "a\"b\\c\nd\u{01}é")
        XCTAssertTrue(text.hasSuffix("\"events\": []\n}\n"))
    }

    func testWhatItCannotReadIsATypedErrorNamingTheField() {
        let head = "{\"format\":\"undra.recording\",\"version\":1,\"schema_hash\":\"0x1\",\"source\":\"t\",\"events\":"
        func fails(_ text: String) -> RecordingError {
            do {
                _ = try Recording(json: text)
            } catch let error as RecordingError {
                return error
            } catch {
                XCTFail("unexpected \(error)")
            }
            XCTFail("expected a RecordingError")
            return RecordingError(message: "", event: nil, field: nil)
        }
        XCTAssertTrue(fails("nope").message.contains("not valid JSON"))
        XCTAssertTrue(fails("{\"format\":\"other\",\"version\":1}").message.contains("not a recording"))
        XCTAssertTrue(fails("{\"format\":\"undra.recording\",\"version\":2}").message.contains("version 2"))
        let bad = fails("\(head)[{\"t\":0,\"kind\":\"cancel\",\"call\":\"x\"}]}")
        XCTAssertEqual(bad.event, 0)
        XCTAssertEqual(bad.field, "call")
        XCTAssertEqual(fails("\(head)[{\"t\":0,\"kind\":\"warp\"}]}").field, "kind")
        XCTAssertEqual(fails("\(head)[{\"t\":0,\"kind\":\"reply\",\"call\":1,\"status\":\"ok\",\"body\":\"zz\"}]}").field, "body")
        XCTAssertEqual(fails("\(head)[{\"t\":0,\"kind\":\"release\",\"handle\":\"7\"}]}").field, "handle")
    }
}
