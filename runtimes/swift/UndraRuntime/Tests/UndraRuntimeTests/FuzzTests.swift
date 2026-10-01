import Foundation
import XCTest
import UndraRuntime

/// Byte-fuzzing of every decoder: whatever the input, the only thing a decoder may do is return
/// a value or throw `WireError`. A trap (crash) fails the whole run; a different error type
/// fails the assertion. Seeds are fixed, so any failure reproduces.
final class FuzzTests: XCTestCase {
    private struct NamedDecoder {
        let name: String
        let run: ([UInt8]) throws -> Void
    }

    private func codecDecoder<T: UndraCodec>(_ type: T.Type, _ name: String) -> NamedDecoder {
        return NamedDecoder(name: name, run: { bytes in
            _ = try T.undraDecoded(from: bytes)
        })
    }

    /// Every decoder in the wire layer, plus a few generated-code stand-ins.
    private func allDecoders() -> [NamedDecoder] {
        var list: [NamedDecoder] = []
        // Primitives.
        list.append(codecDecoder(Bool.self, "Bool"))
        list.append(codecDecoder(UInt8.self, "UInt8"))
        list.append(codecDecoder(UInt16.self, "UInt16"))
        list.append(codecDecoder(UInt32.self, "UInt32"))
        list.append(codecDecoder(UInt64.self, "UInt64"))
        list.append(codecDecoder(Int8.self, "Int8"))
        list.append(codecDecoder(Int16.self, "Int16"))
        list.append(codecDecoder(Int32.self, "Int32"))
        list.append(codecDecoder(Int64.self, "Int64"))
        list.append(codecDecoder(Float.self, "Float"))
        list.append(codecDecoder(Double.self, "Double"))
        list.append(codecDecoder(String.self, "String"))
        list.append(codecDecoder(UndraBytes.self, "UndraBytes"))
        // Wire value types.
        list.append(codecDecoder(UndraDuration.self, "UndraDuration"))
        list.append(codecDecoder(UndraTimestamp.self, "UndraTimestamp"))
        list.append(codecDecoder(UndraUUID.self, "UndraUUID"))
        list.append(codecDecoder(UndraHandle.self, "UndraHandle"))
        list.append(codecDecoder(UndraUnit.self, "UndraUnit"))
        list.append(codecDecoder(Duration.self, "Duration"))
        list.append(codecDecoder(Date.self, "Date"))
        list.append(codecDecoder(UUID.self, "UUID"))
        // Containers.
        list.append(codecDecoder([UInt8].self, "[UInt8]"))
        list.append(codecDecoder(String?.self, "String?"))
        list.append(codecDecoder([String].self, "[String]"))
        list.append(codecDecoder([Int32].self, "[Int32]"))
        list.append(codecDecoder([Int32?].self, "[Int32?]"))
        list.append(codecDecoder([[String]].self, "[[String]]"))
        list.append(codecDecoder([String: Int32].self, "[String: Int32]"))
        list.append(codecDecoder([UInt32: [String]].self, "[UInt32: [String]]"))
        list.append(codecDecoder(UndraResult<Int32, String>.self, "UndraResult<Int32, String>"))
        list.append(codecDecoder(PatchOp<Int32>.self, "PatchOp<Int32>"))
        // Generated-code stand-ins.
        list.append(codecDecoder(Todo.self, "Todo"))
        list.append(codecDecoder([Todo].self, "[Todo]"))
        list.append(codecDecoder(Shape.self, "Shape"))
        list.append(codecDecoder(Filter.self, "Filter"))
        // Payloads.
        list.append(codecDecoder(Wire.Call.self, "Call"))
        list.append(codecDecoder(Wire.Reply.self, "Reply"))
        list.append(codecDecoder(Wire.ChangeSet.self, "ChangeSet"))
        list.append(codecDecoder(Wire.PortCall.self, "PortCall"))
        list.append(codecDecoder(Wire.PortReply.self, "PortReply"))
        list.append(codecDecoder(Wire.Cancel.self, "Cancel"))
        list.append(codecDecoder(Wire.StreamCredit.self, "StreamCredit"))
        list.append(codecDecoder(Wire.StreamItem.self, "StreamItem"))
        list.append(codecDecoder(Wire.Observe.self, "Observe"))
        list.append(codecDecoder(Wire.Release.self, "Release"))
        list.append(codecDecoder(Wire.Event.self, "Event"))
        list.append(codecDecoder(Wire.Hello.self, "Hello"))
        list.append(codecDecoder(Wire.Log.self, "Log"))
        list.append(codecDecoder(Wire.TimerFired.self, "TimerFired"))
        list.append(codecDecoder(Wire.Snapshot.self, "Snapshot"))
        // Envelope, change-set walker and patch decoders.
        list.append(NamedDecoder(name: "decodeEnvelope", run: { bytes in
            _ = try decodeEnvelope(bytes)
        }))
        list.append(NamedDecoder(name: "decodeEnvelope(slice:)", run: { bytes in
            _ = try decodeEnvelope(slice: bytes[0 ..< bytes.count])
        }))
        list.append(NamedDecoder(name: "ChangeSet.forEachEntry", run: { bytes in
            _ = try Wire.ChangeSet.forEachEntry(slice: ArraySlice(bytes)) { _, _, _, value in
                let _: [Int32] = try [Int32].undraDecode(&value)
            }
        }))
        list.append(NamedDecoder(name: "decodePatch<Int32>", run: { bytes in
            var reader = UndraReader(bytes)
            let _: [PatchOp<Int32>] = try decodePatch(&reader)
            try reader.finish()
        }))
        list.append(NamedDecoder(name: "decodePatch<String>", run: { bytes in
            var reader = UndraReader(bytes)
            let _: [PatchOp<String>] = try decodePatch(&reader)
            try reader.finish()
        }))
        // Reader primitives driven directly, in sequence, until something throws.
        list.append(NamedDecoder(name: "UndraReader mixed reads", run: { bytes in
            var reader = UndraReader(bytes)
            _ = try reader.readU8()
            _ = try reader.readString()
            _ = try reader.readBytes()
            _ = try reader.readLen()
            _ = try reader.readSubReader(length: 2)
            _ = try reader.readSlice(3)
            try reader.skip(1)
            _ = try reader.readU64BigEndian()
            try reader.finish()
        }))
        return list
    }

    private func runAll(_ bytes: [UInt8], _ decoders: [NamedDecoder]) {
        for decoder in decoders {
            do {
                try decoder.run(bytes)
            } catch is WireError {
                // The only acceptable failure.
            } catch {
                XCTFail("\(decoder.name) threw a non-WireError (\(error)) for input \(bytesToHex(bytes))")
            }
        }
    }

    // MARK: Random bytes

    private func randomBytes(_ rng: inout SplitMix64, maxLength: Int) -> [UInt8] {
        let length = Int(rng.next() % UInt64(maxLength + 1))
        var out: [UInt8] = []
        out.reserveCapacity(length)
        var index = 0
        while index < length {
            out.append(UInt8(truncatingIfNeeded: rng.next()))
            index += 1
        }
        return out
    }

    /// Bytes biased toward small values, so that lengths, counts and tags look plausible and the
    /// decoders get past their first checks.
    private func plausibleBytes(_ rng: inout SplitMix64, maxLength: Int) -> [UInt8] {
        let length = Int(rng.next() % UInt64(maxLength + 1))
        var out: [UInt8] = []
        out.reserveCapacity(length)
        var index = 0
        while index < length {
            if rng.next() % 4 == 0 {
                out.append(UInt8(truncatingIfNeeded: rng.next()))
            } else {
                out.append(UInt8(truncatingIfNeeded: rng.next() % 4))
            }
            index += 1
        }
        return out
    }

    func testRandomBytesOnlyEverThrowWireError() {
        var rng = SplitMix64(seed: 0x4B45_454C)
        let decoders = allDecoders()
        for _ in 0 ..< 2_000 {
            runAll(randomBytes(&rng, maxLength: 96), decoders)
        }
    }

    func testPlausibleRandomBytesOnlyEverThrowWireError() {
        var rng = SplitMix64(seed: 0x0BAD_CAFE)
        let decoders = allDecoders()
        for _ in 0 ..< 2_000 {
            runAll(plausibleBytes(&rng, maxLength: 64), decoders)
        }
    }

    func testEdgeInputsOnlyEverThrowWireError() {
        let decoders = allDecoders()
        var inputs: [[UInt8]] = []
        inputs.append([])
        inputs.append([0])
        inputs.append([UInt8](repeating: 0, count: 64))
        inputs.append([UInt8](repeating: 0xFF, count: 64))
        inputs.append([UInt8](repeating: 0x80, count: 64))
        inputs.append(hexToBytes("ffffffff"))
        inputs.append(hexToBytes("ffffffffffffffffffffffff"))
        inputs.append(hexToBytes("0000000100000000"))
        for input in inputs {
            runAll(input, decoders)
        }
    }

    // MARK: Mutated valid encodings

    private func fuzzSeeds() -> [[UInt8]] {
        var seeds: [[UInt8]] = []
        let id = UndraUUID(high: 0x123E_4567_E89B_12D3, low: 0xA456_4266_1417_4000)
        let todo = Todo(id: id, title: "Milk \u{1F30A}", done: true)
        let todos: [Todo] = [todo, todo]
        let names: [String] = ["a", "", "\u{65E5}\u{672C}"]
        let ints: [Int32] = [1, -1, 7]
        let map: [String: Int32] = ["b": 2, "a": 1]
        let nestedMap: [UInt32: [String]] = [7: ["x", "y"], 300: []]
        let maybe: String? = "x"
        seeds.append(todo.undraEncoded())
        seeds.append(todos.undraEncoded())
        seeds.append(Shape.rect(w: 2.0, h: 3.0).undraEncoded())
        seeds.append("h\u{E9}llo \u{1F30A}".undraEncoded())
        seeds.append(names.undraEncoded())
        seeds.append(ints.undraEncoded())
        seeds.append(map.undraEncoded())
        seeds.append(nestedMap.undraEncoded())
        seeds.append(maybe.undraEncoded())
        let ok: UndraResult<Int32, String> = .ok(5)
        seeds.append(ok.undraEncoded())
        seeds.append(UndraDuration(nanoseconds: 1_500_000_000).undraEncoded())
        seeds.append(UndraHandle(index: 1, generation: 1).undraEncoded())
        let patch: [PatchOp<Int32>] = [.insert(index: 0, item: 5), .remove(index: 1), .move(from: 0, to: 1), .clear]
        seeds.append(patch.undraEncoded())

        let handle = UndraHandle(index: 1, generation: 1)
        var args = UndraWriter()
        args.writeI32(2)
        args.writeI32(3)
        seeds.append(Wire.Call(target: .objectMethod(handle: handle, methodId: 9), callId: 9, args: args.finishSlice()).encode())
        seeds.append(Wire.Call(target: .lazyListPage(handle: handle, offset: 1, limit: 2), callId: 3).encode())
        seeds.append(Wire.Reply(callId: 9, status: .ok, body: args.finishSlice()).encode())
        seeds.append(Wire.PortCall(portId: 1, methodId: 2, portCallId: 3, args: args.finishSlice()).encode())
        seeds.append(Wire.PortReply(portCallId: 3, status: .ok, body: args.finishSlice()).encode())
        seeds.append(Wire.StreamItem(callId: 9, flag: .item, body: args.finishSlice()).encode())
        seeds.append(Wire.Observe(handle: handle, signalId: 3, on: true).encode())
        seeds.append(Wire.Hello(undraVersion: "0.1.0", schemaHash: 1, platform: "ios", mode: "inproc").encode())
        seeds.append(Wire.Log(level: 2, target: "net", message: "hi").encode())

        let items: [Int32] = [1, 2]
        let value = items.undraEncoded()
        let changeSet = Wire.ChangeSet(txnId: 42, entries: [
            Wire.ChangeEntry(handle: handle, signalId: 0, op: .fullValue, value: ArraySlice(value)),
            Wire.ChangeEntry(handle: handle, signalId: 1, op: .lazyListInvalidated),
        ])
        seeds.append(changeSet.encode())
        let snapshot = Wire.Snapshot(generationFloor: 1, stores: [
            Wire.SnapshotStore(handle: handle, typeId: 5, signals: [Wire.SnapshotSignal(signalId: 0, value: ArraySlice(value))]),
        ])
        seeds.append(snapshot.encode())
        seeds.append(encodeEnvelope(kind: .changeSet, seq: 3, schemaHash: 0x0102_0304_0506_0708, payload: changeSet.encode()))
        return seeds
    }

    private func mutate(_ seed: [UInt8], _ rng: inout SplitMix64) -> [UInt8] {
        var bytes = seed
        let rounds = 1 + Int(rng.next() % 3)
        var round = 0
        while round < rounds {
            switch rng.next() % 6 {
            case 0:
                // Replace one byte.
                if !bytes.isEmpty {
                    let position = Int(rng.next() % UInt64(bytes.count))
                    bytes[position] = UInt8(truncatingIfNeeded: rng.next())
                }
            case 1:
                // Truncate.
                if !bytes.isEmpty {
                    bytes.removeLast(Int(rng.next() % UInt64(bytes.count + 1)))
                }
            case 2:
                // Append junk.
                let extra = Int(rng.next() % 8)
                var count = 0
                while count < extra {
                    bytes.append(UInt8(truncatingIfNeeded: rng.next()))
                    count += 1
                }
            case 3:
                // Saturate a four-byte window: a hostile length or count.
                if bytes.count >= 4 {
                    let start = Int(rng.next() % UInt64(bytes.count - 3))
                    var offset = 0
                    while offset < 4 {
                        bytes[start + offset] = 0xFF
                        offset += 1
                    }
                }
            case 4:
                // Insert a byte.
                let position = Int(rng.next() % UInt64(bytes.count + 1))
                bytes.insert(UInt8(truncatingIfNeeded: rng.next()), at: position)
            default:
                // Zero one byte.
                if !bytes.isEmpty {
                    let position = Int(rng.next() % UInt64(bytes.count))
                    bytes[position] = 0
                }
            }
            round += 1
        }
        return bytes
    }

    func testMutatedValidEncodingsOnlyEverThrowWireError() {
        var rng = SplitMix64(seed: 0x00C0_FFEE)
        let decoders = allDecoders()
        let seeds = fuzzSeeds()
        XCTAssertFalse(seeds.isEmpty)
        // Unmutated seeds first: they must not crash either.
        for seed in seeds {
            runAll(seed, decoders)
        }
        for iteration in 0 ..< 4_000 {
            let seed = seeds[iteration % seeds.count]
            runAll(mutate(seed, &rng), decoders)
        }
    }

    // MARK: applyPatch against a reference model

    /// A second, independently written implementation: op by op with a check before each
    /// mutation, on a working copy. Returns nil when any op is out of range.
    private func referenceApply(_ ops: [PatchOp<Int>], _ start: [Int]) -> [Int]? {
        var list = start
        for op in ops {
            switch op {
            case .insert(let index, let item):
                if Int(index) > list.count {
                    return nil
                }
                list.insert(item, at: Int(index))
            case .remove(let index):
                if Int(index) >= list.count {
                    return nil
                }
                list.remove(at: Int(index))
            case .update(let index, let item):
                if Int(index) >= list.count {
                    return nil
                }
                list[Int(index)] = item
            case .move(let from, let to):
                if Int(from) >= list.count || Int(to) >= list.count {
                    return nil
                }
                let item = list.remove(at: Int(from))
                list.insert(item, at: Int(to))
            case .clear:
                list.removeAll()
            }
        }
        return list
    }

    func testApplyPatchMatchesTheReferenceAndIsAtomic() {
        var rng = SplitMix64(seed: 99)
        for _ in 0 ..< 3_000 {
            var list: [Int] = []
            let length = Int(rng.next() % 6)
            for value in 0 ..< length {
                list.append(value)
            }
            let original = list

            var ops: [PatchOp<Int>] = []
            let opCount = Int(rng.next() % 6)
            for _ in 0 ..< opCount {
                let index = UInt32(truncatingIfNeeded: rng.next() % 8)
                switch rng.next() % 5 {
                case 0:
                    ops.append(.insert(index: index, item: 100))
                case 1:
                    ops.append(.remove(index: index))
                case 2:
                    ops.append(.update(index: index, item: 200))
                case 3:
                    let target = UInt32(truncatingIfNeeded: rng.next() % 8)
                    ops.append(.move(from: index, to: target))
                default:
                    ops.append(.clear)
                }
            }

            let expected = referenceApply(ops, original)
            do {
                try applyPatch(ops, to: &list)
                XCTAssertEqual(list, expected, "ops \(ops) on \(original)")
            } catch is PatchError {
                XCTAssertNil(expected, "the reference accepted ops \(ops) on \(original)")
                XCTAssertEqual(list, original, "a failed patch must not modify the list")
            } catch {
                XCTFail("unexpected error \(error)")
            }
        }
    }
}
