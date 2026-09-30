import Foundation
import XCTest
import KeelRuntime

/// Runs every vector of `contract-tests/wire-vectors.json` (kept in `Resources/` by
/// `scripts/sync-vectors.sh`) through the Swift codecs: the value must encode to exactly the
/// vector's bytes and those bytes must decode back to an equal value.
///
/// A vector whose `type` this runner does not know fails the test, so extending the contract
/// forces the runner to be extended.
final class WireVectorTests: XCTestCase {
    private enum VectorFileError: Error {
        case notFound
        case malformed
    }

    private func vectorsURL() throws -> URL {
        if let url = Bundle.module.url(forResource: "wire-vectors", withExtension: "json") {
            return url
        }
        if let url = Bundle.module.url(forResource: "wire-vectors", withExtension: "json", subdirectory: "Resources") {
            return url
        }
        let beside = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .appendingPathComponent("Resources")
            .appendingPathComponent("wire-vectors.json")
        if FileManager.default.fileExists(atPath: beside.path) {
            return beside
        }
        throw VectorFileError.notFound
    }

    private func loadVectors() throws -> [[String: Any]] {
        let url = try vectorsURL()
        let data = try Data(contentsOf: url)
        let object = try JSONSerialization.jsonObject(with: data)
        guard let root = object as? [String: Any], let vectors = root["vectors"] as? [[String: Any]] else {
            throw VectorFileError.malformed
        }
        return vectors
    }

    func testVectorFileIsPresentAndNonEmpty() throws {
        let vectors = try loadVectors()
        XCTAssertFalse(vectors.isEmpty)
    }

    func testEveryVectorEncodesAndDecodes() throws {
        let vectors = try loadVectors()
        var handled = 0
        for vector in vectors {
            if checkVector(vector) {
                handled += 1
            }
        }
        XCTAssertEqual(handled, vectors.count, "every vector must be understood and pass")
    }

    // MARK: - One vector

    /// Returns true if the vector was understood (assertion failures are reported through XCTest).
    private func checkVector(_ vector: [String: Any]) -> Bool {
        guard let name = vector["name"] as? String,
              let type = vector["type"] as? String,
              let hex = vector["hex"] as? String,
              let value = vector["value"]
        else {
            XCTFail("malformed vector: \(vector)")
            return false
        }

        switch type {
        case "bool":
            guard let v = jsonBool(value) else { return bad(name, "value") }
            assertCodec(v, hex: hex, name)
        case "u8":
            guard let raw = jsonUInt64(value), let v = UInt8(exactly: raw) else { return bad(name, "value") }
            assertCodec(v, hex: hex, name)
        case "i32":
            guard let raw = jsonInt64(value), let v = Int32(exactly: raw) else { return bad(name, "value") }
            assertCodec(v, hex: hex, name)
        case "u32":
            guard let raw = jsonUInt64(value), let v = UInt32(exactly: raw) else { return bad(name, "value") }
            assertCodec(v, hex: hex, name)
        case "i64":
            guard let v = jsonInt64(value) else { return bad(name, "value") }
            assertCodec(v, hex: hex, name)
        case "u64":
            guard let v = jsonUInt64(value) else { return bad(name, "value") }
            assertCodec(v, hex: hex, name)
        case "f32":
            guard let raw = jsonDouble(value) else { return bad(name, "value") }
            assertCodec(Float(raw), hex: hex, name)
        case "f64":
            guard let v = jsonDouble(value) else { return bad(name, "value") }
            assertCodec(v, hex: hex, name)
        case "string":
            guard let v = value as? String else { return bad(name, "value") }
            assertCodec(v, hex: hex, name)
            let decoded = (try? String.keelDecoded(from: hexToBytes(hex))) ?? ""
            XCTAssertEqual(utf8Bytes(decoded), utf8Bytes(v), "\(name) utf8 identity")
        case "bytes":
            guard let raw = jsonByteArray(value) else { return bad(name, "value") }
            assertCodec(KeelBytes(raw), hex: hex, name)
            // `Vec<u8>` is wire-identical to `Bytes`.
            assertCodec(raw, hex: hex, name)
        case "option<string>":
            let v: String? = value as? String
            assertCodec(v, hex: hex, name)
        case "vec<i32>":
            guard let v = jsonInt32Array(value) else { return bad(name, "value") }
            assertCodec(v, hex: hex, name)
        case "vec<string>":
            guard let items = value as? [Any] else { return bad(name, "value") }
            var v: [String] = []
            for item in items {
                guard let text = item as? String else { return bad(name, "item") }
                v.append(text)
            }
            assertCodec(v, hex: hex, name)
        case "map<string,i32>":
            guard let entries = value as? [String: Any] else { return bad(name, "value") }
            var v: [String: Int32] = [:]
            for (key, raw) in entries {
                guard let number = jsonInt64(raw), let narrowed = Int32(exactly: number) else {
                    return bad(name, "entry")
                }
                v[key] = narrowed
            }
            assertCodec(v, hex: hex, name)
        case "duration":
            guard let nanoseconds = jsonInt64(value) else { return bad(name, "value") }
            assertCodec(KeelDuration(nanoseconds: nanoseconds), hex: hex, name)
            assertCodec(Duration.nanoseconds(nanoseconds), hex: hex, name)
        case "timestamp":
            guard let milliseconds = jsonInt64(value) else { return bad(name, "value") }
            assertCodec(KeelTimestamp(millisecondsSinceEpoch: milliseconds), hex: hex, name)
            assertCodec(KeelTimestamp(millisecondsSinceEpoch: milliseconds).date, hex: hex, name)
        case "uuid":
            guard let text = value as? String, let v = KeelUUID(uuidString: text) else { return bad(name, "value") }
            assertCodec(v, hex: hex, name)
            XCTAssertEqual(v.uuidString, text, "\(name) hyphenated lowercase form")
            assertCodec(v.uuid, hex: hex, name)
        case "handle":
            guard let raw = jsonUInt64(value) else { return bad(name, "value") }
            let handle = KeelHandle(rawValue: raw)
            assertCodec(handle, hex: hex, name)
            XCTAssertEqual(handle.index, 1, "\(name) index")
            XCTAssertEqual(handle.generation, 1, "\(name) generation")
            XCTAssertFalse(handle.isNull)
        case "envelope":
            return checkEnvelope(name: name, hex: hex, value: value)
        case "call payload":
            return checkCall(name: name, hex: hex, value: value)
        case "reply payload":
            return checkReply(name: name, hex: hex, value: value)
        case "changeset payload":
            return checkChangeSet(name: name, hex: hex, value: value)
        case "keyed patch (item i32)":
            return checkPatch(name: name, hex: hex, value: value)
        default:
            return checkStructured(name: name, type: type, hex: hex, value: value)
        }
        return true
    }

    private func bad(_ name: String, _ what: String) -> Bool {
        XCTFail("\(name): cannot read \(what) from the vector")
        return false
    }

    // MARK: Types described by a prefix or a quoted argument

    private func checkStructured(name: String, type: String, hex: String, value: Any) -> Bool {
        if type.hasPrefix("record Todo") {
            guard let fields = value as? [String: Any],
                  let idText = fields["id"] as? String,
                  let id = KeelUUID(uuidString: idText),
                  let title = fields["title"] as? String,
                  let done = jsonBool(jsonField(fields, "done"))
            else { return bad(name, "record fields") }
            assertCodec(Todo(id: id, title: title, done: done), hex: hex, name)
            return true
        }
        if type.hasPrefix("enum Filter") {
            guard let text = value as? String else { return bad(name, "value") }
            let table: [String: Filter] = ["all": .all, "active": .active, "done": .done]
            guard let filter = table[text] else { return bad(name, "variant") }
            assertCodec(filter, hex: hex, name)
            return true
        }
        if type.hasPrefix("enum Shape") {
            guard let fields = value as? [String: Any], let kind = fields["kind"] as? String else {
                return bad(name, "value")
            }
            if kind == "circle" {
                guard let radius = jsonDouble(jsonField(fields, "radius")) else { return bad(name, "radius") }
                assertCodec(Shape.circle(radius: radius), hex: hex, name)
            } else if kind == "rect" {
                guard let w = jsonDouble(jsonField(fields, "w")), let h = jsonDouble(jsonField(fields, "h")) else {
                    return bad(name, "w/h")
                }
                assertCodec(Shape.rect(w: w, h: h), hex: hex, name)
            } else {
                return bad(name, "kind")
            }
            return true
        }
        if type.hasPrefix("result<i32,string>") {
            guard let fields = value as? [String: Any] else { return bad(name, "value") }
            if let okValue = fields["ok"] {
                guard let raw = jsonInt64(okValue), let v = Int32(exactly: raw) else { return bad(name, "ok") }
                let result: KeelResult<Int32, String> = .ok(v)
                assertCodec(result, hex: hex, name)
            } else if let errValue = fields["err"] as? String {
                let result: KeelResult<Int32, String> = .err(errValue)
                assertCodec(result, hex: hex, name)
            } else {
                return bad(name, "ok/err")
            }
            return true
        }
        if type.hasPrefix("fnv1a32(") {
            guard let argument = quotedArgument(type),
                  let text = value as? String,
                  let expected = UInt32(text)
            else { return bad(name, "fnv1a32") }
            XCTAssertEqual(fnv1a32(argument), expected, "\(name) hash")
            assertCodec(expected, hex: hex, name)
            return true
        }
        if type.hasPrefix("fnv1a64(") {
            guard let argument = quotedArgument(type),
                  let text = value as? String,
                  let expected = UInt64(text)
            else { return bad(name, "fnv1a64") }
            XCTAssertEqual(fnv1a64(argument), expected, "\(name) hash")
            assertCodec(expected, hex: hex, name)
            return true
        }
        XCTFail("\(name): the runner does not know vector type \"\(type)\"")
        return false
    }

    private func quotedArgument(_ type: String) -> String? {
        guard let open = type.firstIndex(of: "\""), let close = type.lastIndex(of: "\""), open < close else {
            return nil
        }
        return String(type[type.index(after: open) ..< close])
    }

    // MARK: Envelope and payload vectors

    private func checkEnvelope(name: String, hex: String, value: Any) -> Bool {
        guard let fields = value as? [String: Any],
              let rawKind = jsonUInt64(jsonField(fields, "kind")),
              let kindByte = UInt8(exactly: rawKind),
              let kind = Envelope.Kind(rawValue: kindByte),
              let rawSeq = jsonUInt64(jsonField(fields, "seq")),
              let seq = UInt32(exactly: rawSeq),
              let schema = jsonUInt64(jsonField(fields, "schema")),
              let payloadHex = fields["payload_hex"] as? String
        else { return bad(name, "envelope fields") }
        let payload = hexToBytes(payloadHex)
        let expected = hexToBytes(hex)
        XCTAssertEqual(expected.count, Envelope.headerLength + payload.count, "\(name) header is 23 bytes")

        let encoded = encodeEnvelope(kind: kind, seq: seq, schemaHash: schema, payload: payload)
        XCTAssertEqual(bytesToHex(encoded), bytesToHex(expected), "\(name) encode")
        do {
            let decoded = try decodeEnvelope(expected)
            XCTAssertEqual(decoded.kind, kind, "\(name) kind")
            XCTAssertEqual(decoded.seq, seq, "\(name) seq")
            XCTAssertEqual(decoded.schemaHash, schema, "\(name) schema")
            XCTAssertEqual(bytesToHex(Array(decoded.payload)), bytesToHex(payload), "\(name) payload")
        } catch {
            XCTFail("\(name) decode threw \(error)")
        }
        return true
    }

    private func checkCall(name: String, hex: String, value: Any) -> Bool {
        guard let fields = value as? [String: Any],
              let target = jsonUInt64(jsonField(fields, "target")),
              let handle = jsonUInt64(jsonField(fields, "handle")),
              let rawMethod = jsonUInt64(jsonField(fields, "method_id")),
              let methodId = UInt32(exactly: rawMethod),
              let rawCall = jsonUInt64(jsonField(fields, "call_id")),
              let callId = UInt32(exactly: rawCall),
              let args = jsonInt32Array(jsonField(fields, "args"))
        else { return bad(name, "call fields") }
        guard target == 1 else { return bad(name, "target (runner covers method calls)") }
        var argWriter = KeelWriter()
        for arg in args {
            argWriter.writeI32(arg)
        }
        let call = Wire.Call(
            target: .objectMethod(handle: KeelHandle(rawValue: handle), methodId: methodId),
            callId: callId,
            args: argWriter.finishSlice()
        )
        assertCodec(call, hex: hex, name)
        return true
    }

    private func checkReply(name: String, hex: String, value: Any) -> Bool {
        guard let fields = value as? [String: Any],
              let rawCall = jsonUInt64(jsonField(fields, "call_id")),
              let callId = UInt32(exactly: rawCall),
              let rawStatus = jsonUInt64(jsonField(fields, "status")),
              let statusByte = UInt8(exactly: rawStatus),
              let status = ReplyStatus(rawValue: statusByte),
              let rawBody = jsonInt64(jsonField(fields, "body")),
              let body = Int32(exactly: rawBody)
        else { return bad(name, "reply fields") }
        var bodyWriter = KeelWriter()
        bodyWriter.writeI32(body)
        assertCodec(Wire.Reply(callId: callId, status: status, body: bodyWriter.finishSlice()), hex: hex, name)
        return true
    }

    private func checkChangeSet(name: String, hex: String, value: Any) -> Bool {
        guard let fields = value as? [String: Any],
              let txnId = jsonUInt64(jsonField(fields, "txn_id")),
              let rawEntries = fields["entries"] as? [Any]
        else { return bad(name, "change-set fields") }
        var entries: [Wire.ChangeEntry] = []
        for rawEntry in rawEntries {
            guard let entry = rawEntry as? [String: Any],
                  let handle = jsonUInt64(jsonField(entry, "handle")),
                  let rawSignal = jsonUInt64(jsonField(entry, "signal_id")),
                  let signalId = UInt32(exactly: rawSignal),
                  let rawOp = jsonUInt64(jsonField(entry, "op")),
                  let opByte = UInt8(exactly: rawOp),
                  let op = ChangeOp(rawValue: opByte),
                  let items = jsonInt32Array(jsonField(entry, "value"))
            else { return bad(name, "change-set entry") }
            // The vector's value is the signal's `Vec<i32>`.
            entries.append(Wire.ChangeEntry(
                handle: KeelHandle(rawValue: handle),
                signalId: signalId,
                op: op,
                value: ArraySlice(items.keelEncoded())
            ))
        }
        assertCodec(Wire.ChangeSet(txnId: txnId, entries: entries), hex: hex, name)
        return true
    }

    private func checkPatch(name: String, hex: String, value: Any) -> Bool {
        guard let fields = value as? [String: Any], let rawOps = fields["ops"] as? [Any] else {
            return bad(name, "patch ops")
        }
        var ops: [PatchOp<Int32>] = []
        for rawOp in rawOps {
            guard let op = rawOp as? [String: Any], let kind = op["op"] as? String else {
                return bad(name, "patch op")
            }
            switch kind {
            case "insert", "update":
                guard let rawIndex = jsonUInt64(jsonField(op, "index")),
                      let index = UInt32(exactly: rawIndex),
                      let rawItem = jsonInt64(jsonField(op, "item")),
                      let item = Int32(exactly: rawItem)
                else { return bad(name, "\(kind) fields") }
                if kind == "insert" {
                    ops.append(.insert(index: index, item: item))
                } else {
                    ops.append(.update(index: index, item: item))
                }
            case "remove":
                guard let rawIndex = jsonUInt64(jsonField(op, "index")), let index = UInt32(exactly: rawIndex) else {
                    return bad(name, "remove fields")
                }
                ops.append(.remove(index: index))
            case "move":
                guard let rawFrom = jsonUInt64(jsonField(op, "from")),
                      let from = UInt32(exactly: rawFrom),
                      let rawTo = jsonUInt64(jsonField(op, "to")),
                      let to = UInt32(exactly: rawTo)
                else { return bad(name, "move fields") }
                ops.append(.move(from: from, to: to))
            case "clear":
                ops.append(.clear)
            default:
                return bad(name, "patch op kind \(kind)")
            }
        }
        // A patch is a `u32` count followed by the ops: exactly the `Vec<PatchOp<T>>` layout.
        assertCodec(ops, hex: hex, name)

        var writer = KeelWriter()
        encodePatch(ops, into: &writer)
        XCTAssertEqual(bytesToHex(writer.finish()), bytesToHex(hexToBytes(hex)), "\(name) encodePatch")
        do {
            var reader = KeelReader(hexToBytes(hex))
            let decoded: [PatchOp<Int32>] = try decodePatch(&reader)
            try reader.finish()
            XCTAssertEqual(decoded, ops, "\(name) decodePatch")
        } catch {
            XCTFail("\(name) decodePatch threw \(error)")
        }
        return true
    }
}
