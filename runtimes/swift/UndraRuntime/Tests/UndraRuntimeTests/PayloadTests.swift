import XCTest
import UndraRuntime

/// The envelope payloads (docs/SPEC.md sections 3.3 to 3.7 and 5.9). The expected bytes were
/// derived independently from the spec, not from this code.
final class PayloadTests: XCTestCase {
    private let handle = UndraHandle(rawValue: 4_294_967_297)

    private func slice(_ hex: String) -> ArraySlice<UInt8> {
        return ArraySlice(hexToBytes(hex))
    }

    // MARK: Call

    func testCallFreeFunction() {
        let call = Wire.Call(target: .freeFunction(methodId: 2_353_348_832), callId: 9, args: slice("aabb"))
        assertCodec(call, hex: "000000000000000000e040458c09000000aabb")
    }

    func testCallObjectMethod() {
        // The contract vector: Calculator.add(2, 3) on handle (1, 1).
        let call = Wire.Call(
            target: .objectMethod(handle: handle, methodId: 2_353_348_832),
            callId: 9,
            args: slice("0200000003000000")
        )
        assertCodec(call, hex: "010100000001000000e040458c090000000200000003000000")
    }

    func testCallConstructorHasNoHandleField() {
        let call = Wire.Call(target: .constructor(typeId: 0x1122_3344, methodId: 0x5566_7788), callId: 1, args: slice("ff"))
        assertCodec(call, hex: "02443322118877665501000000ff")
    }

    func testCallLazyListPageCarriesNoArguments() throws {
        let page = Wire.Call(target: .lazyListPage(handle: handle, offset: 10, limit: 20), callId: 3)
        assertCodec(page, hex: "0301000000010000000a0000001400000003000000")
        // Arguments are ignored when encoding a page request.
        let withArgs = Wire.Call(target: .lazyListPage(handle: handle, offset: 10, limit: 20), callId: 3, args: slice("ffff"))
        XCTAssertEqual(bytesToHex(withArgs.encode()), "0301000000010000000a0000001400000003000000")
        XCTAssertTrue(try Wire.Call.decode(withArgs.encode()).args.isEmpty)
    }

    func testCallFreeFunctionIgnoresTheHandleFieldWhenDecoding() throws {
        let decoded = try Wire.Call.decode(hexToBytes("004d00000000000000e040458c09000000aabb"))
        XCTAssertEqual(decoded.target, .freeFunction(methodId: 2_353_348_832))
        XCTAssertEqual(decoded.callId, 9)
        XCTAssertEqual(Array(decoded.args), [0xAA, 0xBB])
    }

    func testCallWithEmptyArgs() {
        let call = Wire.Call(target: .freeFunction(methodId: 1), callId: 2)
        XCTAssertEqual(call.encode().count, 1 + 8 + 4 + 4)
        assertRoundTrip(call)
    }

    func testCallWriteHeaderThenArgsMatchesEncode() throws {
        var writer = UndraWriter()
        let target = CallTarget.objectMethod(handle: handle, methodId: 5)
        Wire.Call.writeHeader(into: &writer, target: target, callId: 77)
        Int32(2).undraEncode(&writer)
        Int32(3).undraEncode(&writer)
        var argWriter = UndraWriter()
        Int32(2).undraEncode(&argWriter)
        Int32(3).undraEncode(&argWriter)
        let viaStruct = Wire.Call(target: target, callId: 77, args: argWriter.finishSlice()).encode()
        XCTAssertEqual(writer.finish(), viaStruct)
    }

    // MARK: Reply

    func testReplyOk() {
        var body = UndraWriter()
        body.writeI32(5)
        assertCodec(Wire.Reply(callId: 9, status: .ok, body: body.finishSlice()), hex: "090000000005000000")
    }

    func testReplyEveryStatusRoundTrips() throws {
        let statuses: [(ReplyStatus, UInt8)] = [
            (.ok, 0), (.error, 1), (.panic, 2), (.cancelled, 3), (.streamOpened, 4), (.badRequest, 5),
        ]
        for (status, number) in statuses {
            XCTAssertEqual(status.rawValue, number)
            let reply = Wire.Reply(callId: 0xAABB_CCDD, status: status, body: slice("0102"))
            let decoded = try Wire.Reply.decode(reply.encode())
            XCTAssertEqual(decoded, reply)
        }
    }

    func testReplyPanicDetails() throws {
        let reply = try Wire.Reply.decode(hexToBytes("050000000204000000626f6f6d020000006274"))
        XCTAssertEqual(reply.status, .panic)
        let details = try reply.panicDetails()
        XCTAssertEqual(details.message, "boom")
        XCTAssertEqual(details.backtrace, "bt")
        // A panic body with a missing backtrace is malformed.
        let truncated = Wire.Reply(callId: 1, status: .panic, body: slice("04000000626f6f6d"))
        XCTAssertThrowsError(try truncated.panicDetails())
    }

    func testReplyBadRequestReason() throws {
        let reply = try Wire.Reply.decode(hexToBytes("06000000050e000000756e6b6e6f776e206d6574686f64"))
        XCTAssertEqual(reply.status, .badRequest)
        XCTAssertEqual(try reply.badRequestReason(), "unknown method")
        // Trailing bytes after the reason are malformed.
        let padded = Wire.Reply(callId: 1, status: .badRequest, body: slice("00000000ff"))
        expectWireError(.trailingBytes(count: 1)) {
            _ = try padded.badRequestReason()
        }
    }

    func testReplyEmptyBodies() {
        assertCodec(Wire.Reply(callId: 6, status: .cancelled), hex: "0600000003")
        assertCodec(Wire.Reply(callId: 6, status: .streamOpened), hex: "0600000004")
    }

    // MARK: ChangeSet

    private func sampleChangeSet() -> Wire.ChangeSet {
        return Wire.ChangeSet(txnId: 42, entries: [
            Wire.ChangeEntry(handle: handle, signalId: 0, op: .fullValue, value: slice("010203")),
            Wire.ChangeEntry(handle: UndraHandle(rawValue: handle.rawValue + 1), signalId: 1, op: .lazyListInvalidated),
        ])
    }

    private let sampleChangeSetHex =
        "2a000000000000000200000001000000010000000000000000030000000102030200000001000000010000000200000000"

    func testChangeSetEncodesAndDecodes() {
        assertCodec(sampleChangeSet(), hex: sampleChangeSetHex)
        assertCodec(Wire.ChangeSet(txnId: 7, entries: []), hex: "070000000000000000000000")
    }

    func testChangeSetKeyedPatchEntry() throws {
        var patch = UndraWriter()
        let ops: [PatchOp<Int32>] = [.insert(index: 0, item: 5), .clear]
        encodePatch(ops, into: &patch)
        let changeSet = Wire.ChangeSet(txnId: 1, entries: [
            Wire.ChangeEntry(handle: handle, signalId: 2, op: .keyedPatch, value: patch.finishSlice()),
        ])
        let decoded = try Wire.ChangeSet.decode(changeSet.encode())
        XCTAssertEqual(decoded, changeSet)
        var reader = UndraReader(slice: decoded.entries[0].value)
        let decodedOps: [PatchOp<Int32>] = try decodePatch(&reader)
        try reader.finish()
        XCTAssertEqual(decodedOps, ops)
    }

    func testForEachEntryWalksWithoutCopyingValues() throws {
        let bytes = hexToBytes(sampleChangeSetHex)
        var seen: [(UInt64, UInt32, ChangeOp, [UInt8])] = []
        var starts: [Int] = []
        var reader = UndraReader(bytes)
        let txnId = try Wire.ChangeSet.forEachEntry(from: &reader) { entryHandle, signalId, op, value in
            // The reader is restricted to this entry's value and positioned at its first byte.
            starts.append(value.position)
            let contents = value.readRemaining()
            seen.append((entryHandle.rawValue, signalId, op, Array(contents)))
        }
        XCTAssertEqual(txnId, 42)
        XCTAssertTrue(reader.isAtEnd)
        // Entry 0's value starts after 8 (txn) + 4 (count) + 17 (entry header) = 29 and is 3 bytes long;
        // entry 1's header is another 17 bytes, so its (empty) value starts at 49, the end.
        XCTAssertEqual(starts, [29, 49])
        XCTAssertEqual(seen.count, 2)
        XCTAssertEqual(seen[0].0, handle.rawValue)
        XCTAssertEqual(seen[0].1, 0)
        XCTAssertEqual(seen[0].2, .fullValue)
        XCTAssertEqual(seen[0].3, [1, 2, 3])
        XCTAssertEqual(seen[1].0, handle.rawValue + 1)
        XCTAssertEqual(seen[1].1, 1)
        XCTAssertEqual(seen[1].2, .lazyListInvalidated)
        XCTAssertEqual(seen[1].3, [])
    }

    func testForEachEntryValueReaderStartsAtTheValueAndIsBounded() throws {
        // One entry holding a Vec<i32> [1, 2]; the reader must start at the count.
        let bytes = hexToBytes("2a0000000000000001000000010000000100000000000000000c000000020000000100000002000000")
        var decodedItems: [Int32] = []
        let txnId = try Wire.ChangeSet.forEachEntry(slice: ArraySlice(bytes)) { _, _, op, value in
            XCTAssertEqual(op, .fullValue)
            XCTAssertEqual(value.remaining, 12)
            do {
                decodedItems = try [Int32].undraDecode(&value)
                try value.finish()
            } catch {
                XCTFail("value decode failed: \(error)")
            }
        }
        XCTAssertEqual(txnId, 42)
        XCTAssertEqual(decodedItems, [1, 2])
    }

    func testForEachEntryResumesAfterTheDeclaredLengthWhateverTheVisitorReads() throws {
        var count = 0
        var lastSignal: UInt32 = 0
        try Wire.ChangeSet.forEachEntry(slice: ArraySlice(hexToBytes(sampleChangeSetHex))) { _, signalId, _, _ in
            // Reads nothing at all: the walker must still land on the next entry.
            count += 1
            lastSignal = signalId
        }
        XCTAssertEqual(count, 2)
        XCTAssertEqual(lastSignal, 1)
    }

    func testForEachEntryPropagatesTheVisitorsError() {
        struct Stop: Error {}
        var visited = 0
        do {
            try Wire.ChangeSet.forEachEntry(slice: ArraySlice(hexToBytes(sampleChangeSetHex))) { _, _, _, _ in
                visited += 1
                throw Stop()
            }
            XCTFail("the visitor's error must propagate")
        } catch is Stop {
            // expected
        } catch {
            XCTFail("unexpected error \(error)")
        }
        XCTAssertEqual(visited, 1)
    }

    func testChangeSetMalformed() {
        // Unknown op byte (entry 0): offsets 8 (txn) + 4 (count) + 8 (handle) + 4 (signal) = 24.
        var badOp = hexToBytes(sampleChangeSetHex)
        badOp[24] = 9
        expectWireError(.invalidTag(tag: 9, at: 24, type: "ChangeOp")) {
            _ = try Wire.ChangeSet.decode(badOp)
        }
        // Entry length larger than what remains.
        var badLength = hexToBytes(sampleChangeSetHex)
        badLength[25] = 0xFF
        expectWireError(.lengthTooLarge(len: 255, at: 25)) {
            _ = try Wire.ChangeSet.decode(badLength)
        }
        // Entry count larger than the bytes that remain.
        var badCount = hexToBytes(sampleChangeSetHex)
        badCount[8] = 0xFF
        badCount[9] = 0xFF
        expectWireError(.lengthTooLarge(len: 65535, at: 8)) {
            _ = try Wire.ChangeSet.decode(badCount)
        }
        // Trailing bytes after the last entry.
        var trailing = hexToBytes(sampleChangeSetHex)
        trailing.append(0)
        expectWireError(.trailingBytes(count: 1)) {
            _ = try Wire.ChangeSet.decode(trailing)
        }
        expectWireError(.trailingBytes(count: 1)) {
            _ = try Wire.ChangeSet.forEachEntry(slice: ArraySlice(trailing)) { _, _, _, _ in }
        }
        // Truncated inside an entry header.
        let truncated = Array(hexToBytes(sampleChangeSetHex)[0 ..< 20])
        expectWireError(.unexpectedEOF(needed: 4, at: 20)) {
            _ = try Wire.ChangeSet.decode(truncated)
        }
    }

    // MARK: Ports

    func testPortCall() {
        assertCodec(Wire.PortCall(portId: 1, methodId: 2, portCallId: 3, args: slice("09")), hex: "01000000020000000300000009")
        assertCodec(Wire.PortCall(portId: 1, methodId: 2, portCallId: 3), hex: "010000000200000003000000")
    }

    func testPortReply() {
        var body = UndraWriter()
        body.writeI32(5)
        assertCodec(Wire.PortReply(portCallId: 3, status: .ok, body: body.finishSlice()), hex: "030000000005000000")
        assertCodec(Wire.PortReply(portCallId: 3, status: .unavailable), hex: "0300000002")
        assertCodec(Wire.PortReply(portCallId: 3, status: .error, body: slice("01")), hex: "030000000101")
        XCTAssertEqual(Wire.PortStatus.ok.rawValue, 0)
        XCTAssertEqual(Wire.PortStatus.error.rawValue, 1)
        XCTAssertEqual(Wire.PortStatus.unavailable.rawValue, 2)
    }

    // MARK: Cancel, credit, stream items

    func testCancelAndStreamCredit() {
        assertCodec(Wire.Cancel(callId: 7), hex: "07000000")
        assertCodec(Wire.StreamCredit(callId: 7, credit: 16), hex: "0700000010000000")
    }

    func testStreamItem() {
        var body = UndraWriter()
        body.writeI32(5)
        assertCodec(Wire.StreamItem(callId: 9, flag: .item, body: body.finishSlice()), hex: "090000000005000000")
        assertCodec(Wire.StreamItem(callId: 9, flag: .end), hex: "0900000001")
        assertCodec(Wire.StreamItem(callId: 9, flag: .error, body: slice("00")), hex: "090000000200")
        assertCodec(Wire.StreamItem(callId: 9, flag: .failed, body: slice("0300000000" + "00000000")), hex: "0900000003" + "0300000000" + "00000000")
        XCTAssertEqual(Wire.StreamFlag.item.rawValue, 0)
        XCTAssertEqual(Wire.StreamFlag.end.rawValue, 1)
        XCTAssertEqual(Wire.StreamFlag.error.rawValue, 2)
        XCTAssertEqual(Wire.StreamFlag.failed.rawValue, 3)
        XCTAssertNil(Wire.StreamFlag(rawValue: 4))
    }

    // MARK: StreamFailure (the body of a failed stream item, ADR-036)

    func testStreamFailureLayout() {
        // status u8, message String, detail String; the bytes match the contract vectors.
        assertCodec(
            Wire.StreamFailure(status: .panic, message: "boom", detail: "at core.rs:1"),
            hex: "02" + "04000000" + "626f6f6d" + "0c000000" + "617420636f72652e72733a31"
        )
        assertCodec(
            Wire.StreamFailure(status: .cancelled, message: "the runtime shut down"),
            hex: "03" + "15000000" + "7468652072756e74696d65207368757420646f776e" + "00000000"
        )
        assertCodec(
            Wire.StreamFailure(status: .badRequest, message: "stale handle"),
            hex: "05" + "0c000000" + "7374616c652068616e646c65" + "00000000"
        )
        assertRoundTrip(Wire.StreamFailure(status: .cancelled, message: "", detail: ""))
        assertRoundTrip(Wire.StreamFailure(status: .panic, message: "h\u{E9}llo \u{1F30A}", detail: "0: main\n1: start"))
        XCTAssertEqual(Wire.StreamFailure(status: .cancelled, message: "x").detail, "", "detail defaults to empty")
    }

    func testStreamFailureInsideAStreamItem() throws {
        let failure = Wire.StreamFailure(status: .panic, message: "boom", detail: "at core.rs:1")
        let item = Wire.StreamItem(callId: 9, flag: .failed, body: ArraySlice(failure.encode()))
        XCTAssertEqual(bytesToHex(item.encode()), "0900000003" + bytesToHex(failure.encode()))
        let decoded = try Wire.StreamItem.decode(item.encode())
        XCTAssertEqual(decoded.flag, .failed)
        XCTAssertEqual(try decoded.failure(), failure)
        // The body is a slice of the payload: offsets in an error count from the body's start.
        var bad = item.encode()
        bad[5] = 4
        expectWireError(.invalidTag(tag: 4, at: 0, type: "StreamFailure.status")) {
            _ = try Wire.StreamItem.decode(bad).failure()
        }
    }

    func testStreamFailureStatusIsLimitedToPanickedCancelledAndRefused() {
        XCTAssertTrue(Wire.StreamFailure.allows(.panic))
        XCTAssertTrue(Wire.StreamFailure.allows(.cancelled))
        XCTAssertTrue(Wire.StreamFailure.allows(.badRequest))
        XCTAssertFalse(Wire.StreamFailure.allows(.ok))
        XCTAssertFalse(Wire.StreamFailure.allows(.error))
        XCTAssertFalse(Wire.StreamFailure.allows(.streamOpened))
        let rest = hexToBytes("00000000" + "00000000")
        for status: UInt8 in [0, 1, 4, 6, 7, 0x80, 0xFF] {
            expectWireError(.invalidTag(tag: UInt32(status), at: 0, type: "StreamFailure.status")) {
                _ = try Wire.StreamFailure.decode([status] + rest)
            }
        }
    }

    func testStreamFailureTruncationAndTrailingBytes() {
        let body = Wire.StreamFailure(status: .panic, message: "boom", detail: "bt").encode()
        XCTAssertEqual(body.count, 1 + 4 + 4 + 4 + 2)
        for cut in 0 ..< body.count {
            XCTAssertThrowsError(try Wire.StreamFailure.decode(Array(body[0 ..< cut])), "cut at \(cut)") { error in
                XCTAssertTrue(error is WireError, "cut at \(cut): \(error)")
            }
        }
        expectWireError(.unexpectedEOF(needed: 1, at: 0)) {
            _ = try Wire.StreamFailure.decode([])
        }
        expectWireError(.unexpectedEOF(needed: 4, at: 1)) {
            _ = try Wire.StreamFailure.decode([3])
        }
        expectWireError(.unexpectedEOF(needed: 4, at: 5)) {
            _ = try Wire.StreamFailure.decode(hexToBytes("0300000000"))
        }
        expectWireError(.lengthTooLarge(len: 9, at: 1)) {
            _ = try Wire.StreamFailure.decode(hexToBytes("0309000000626f6f6d"))
        }
        expectWireError(.trailingBytes(count: 1)) {
            _ = try Wire.StreamFailure.decode(body + [0])
        }
    }

    func testStreamFailureReplyBodyIsTheBodyOfAReplyWithThatStatus() throws {
        // docs/SPEC.md section 3.4: a panic carries message + backtrace, a refusal its reason, a
        // cancellation nothing.
        let panic = Wire.StreamFailure(status: .panic, message: "boom", detail: "bt")
        let asReply = Wire.Reply(callId: 1, status: .panic, body: ArraySlice(panic.replyBody()))
        let details = try asReply.panicDetails()
        XCTAssertEqual(details.message, "boom")
        XCTAssertEqual(details.backtrace, "bt")
        let refused = Wire.StreamFailure(status: .badRequest, message: "stale handle", detail: "dropped")
        XCTAssertEqual(
            try Wire.Reply(callId: 1, status: .badRequest, body: ArraySlice(refused.replyBody())).badRequestReason(),
            "stale handle"
        )
        XCTAssertEqual(Wire.StreamFailure(status: .cancelled, message: "the runtime shut down").replyBody(), [])
        // A status a stream failure cannot have has no body either (never decoded, only constructed).
        XCTAssertEqual(Wire.StreamFailure(status: .error, message: "x").replyBody(), [])
    }

    // MARK: Observe, release, event

    func testObserveAndRelease() {
        assertCodec(Wire.Observe(handle: handle, signalId: 3, on: true), hex: "01000000010000000300000001")
        assertCodec(Wire.Observe(handle: handle, signalId: Wire.Observe.allSignals, on: false), hex: "0100000001000000ffffffff00")
        XCTAssertEqual(Wire.Observe.allSignals, UInt32.max)
        assertCodec(Wire.Release(handle: handle), hex: "0100000001000000")
    }

    func testEvent() {
        assertCodec(Wire.Event(portId: 1, methodId: 2, payload: slice("dd")), hex: "0100000002000000dd")
        assertCodec(Wire.Event(portId: 1, methodId: 2), hex: "0100000002000000")
    }

    // MARK: Hello, log, timer

    func testHello() {
        let hello = Wire.Hello(undraVersion: "0.1.0", schemaHash: 0x0102_0304_0506_0708, platform: "ios", mode: "inproc")
        assertCodec(hello, hex: "05000000302e312e30080706050403020103000000696f7306000000696e70726f63")
        assertRoundTrip(Wire.Hello(undraVersion: "", schemaHash: 0, platform: "\u{1F30A}", mode: "dev"))
    }

    func testLog() {
        assertCodec(Wire.Log(level: 2, target: "net", message: "hi"), hex: "02030000006e6574020000006869")
        assertRoundTrip(Wire.Log(level: 255, target: "", message: "h\u{E9}llo \u{1F30A}"))
    }

    func testTimerFired() {
        assertCodec(Wire.TimerFired(timerId: 42), hex: "2a000000")
    }

    // MARK: Snapshot (layout 2, ADR-037)

    /// Two store types, two stores: the sample of the Rust codec's tests.
    private func sampleSnapshot() -> Wire.Snapshot {
        return Wire.Snapshot(
            generationFloor: 5,
            schemaHash: 0xFEED_BEEF_0000_0001,
            types: [Wire.SnapshotType(typeId: 7, fingerprint: 0x11), Wire.SnapshotType(typeId: 8, fingerprint: 0x22)],
            description: "{\"stores\":[]}",
            stores: [
                Wire.SnapshotStore(handle: handle, typeId: 7, signals: [
                    Wire.SnapshotSignal(signalId: 0, value: slice("010203")),
                    Wire.SnapshotSignal(signalId: 2, value: []),
                ]),
                Wire.SnapshotStore(handle: UndraHandle(rawValue: 0x0000_0005_0000_0002), typeId: 8, signals: []),
            ]
        )
    }

    /// The layout of ADR-022 (`count u32, generation_floor u32, stores`), which ADR-037 replaced.
    private func layout1(_ stores: [Wire.SnapshotStore], floor: UInt32) -> [UInt8] {
        var writer = UndraWriter()
        writer.writeLen(stores.count)
        writer.writeU32(floor)
        for store in stores {
            writer.writeU64(store.handle.rawValue)
            writer.writeU32(store.typeId)
            writer.writeLen(store.signals.count)
            for signal in store.signals {
                writer.writeU32(signal.signalId)
                writer.writeLen(signal.value.count)
                writer.writeRaw(slice: signal.value)
            }
        }
        return writer.finish()
    }

    func testSnapshotLayoutMatchesTheSpec() {
        // The bytes `layout_matches_the_spec` asserts in undra-wire.
        let snapshot = Wire.Snapshot(
            generationFloor: 0x0102_0304,
            schemaHash: 0x0807_0605_0403_0201,
            types: [Wire.SnapshotType(typeId: 7, fingerprint: 0x0A)],
            description: "{}",
            stores: [Wire.SnapshotStore(handle: handle, typeId: 7, signals: [Wire.SnapshotSignal(signalId: 2, value: slice("09"))])]
        )
        assertCodec(snapshot, hex:
            "01000000" // store count
            + "0403020100000000" // generation_floor (u64, ADR-040)
            + "0102030405060708" // schema_hash
            + "01000000" // type_count
            + "07000000" + "0a00000000000000" // type_id, fingerprint
            + "02000000" + "7b7d" // description
            + "0100000001000000" // handle
            + "07000000" // type_id
            + "01000000" // signal_count
            + "02000000" // signal_id
            + "01000000" + "09") // len + value
        // An empty snapshot is 28 zero bytes.
        assertCodec(Wire.Snapshot(generationFloor: 0, schemaHash: 0, types: [], description: "", stores: []), hex: String(repeating: "00", count: 28))
    }

    func testSnapshotRoundTripsAndFindsFingerprints() throws {
        let snapshot = sampleSnapshot()
        assertRoundTrip(snapshot)
        XCTAssertEqual(try Wire.Snapshot.decode(snapshot.encode()), snapshot)
        XCTAssertEqual(snapshot.fingerprint(typeId: 8), 0x22)
        XCTAssertNil(snapshot.fingerprint(typeId: 9))
        // A type may be listed without a store of it (its stores were all closed).
        var spare = snapshot
        spare.types.append(Wire.SnapshotType(typeId: 0xC0FFEE, fingerprint: UInt64.max))
        spare.description = "{\"name\":\"caf\u{E9} \u{1F30A}\"}"
        assertRoundTrip(spare)
    }

    func testSnapshotHostileCountsAreRejected() {
        // Store count larger than the input could hold (16 bytes a store at least).
        expectWireError(.lengthTooLarge(len: 5, at: 0)) {
            _ = try Wire.Snapshot.decode(hexToBytes("05000000" + "00000000"))
        }
        expectWireError(.lengthTooLarge(len: 2, at: 0)) {
            _ = try Wire.Snapshot.decode(hexToBytes("02000000" + String(repeating: "00", count: 28)))
        }
        // Type count larger than the input could hold (12 bytes a type).
        expectWireError(.lengthTooLarge(len: UInt32.max, at: 20)) {
            _ = try Wire.Snapshot.decode(hexToBytes("00000000" + "0000000000000000" + "0000000000000000" + "ffffffff"))
        }
        expectWireError(.lengthTooLarge(len: 1, at: 20)) {
            _ = try Wire.Snapshot.decode(hexToBytes("00000000" + "0000000000000000" + "0000000000000000" + "01000000" + "0000000000000000"))
        }
        // Signal count larger than the input could hold (8 bytes a signal).
        var oneStore = sampleSnapshot()
        oneStore.stores = [Wire.SnapshotStore(handle: handle, typeId: 7, signals: [])]
        var bytes = oneStore.encode()
        bytes.replaceSubrange((bytes.count - 4) ..< bytes.count, with: [0xFF, 0xFF, 0xFF, 0xFF])
        expectWireError(.lengthTooLarge(len: UInt32.max, at: bytes.count - 4)) {
            _ = try Wire.Snapshot.decode(bytes)
        }
        // Signal value length larger than the remaining bytes.
        var oneSignal = oneStore
        oneSignal.stores[0].signals = [Wire.SnapshotSignal(signalId: 0, value: [])]
        var valueBytes = oneSignal.encode()
        valueBytes.replaceSubrange((valueBytes.count - 4) ..< valueBytes.count, with: [9, 0, 0, 0])
        expectWireError(.lengthTooLarge(len: 9, at: valueBytes.count - 4)) {
            _ = try Wire.Snapshot.decode(valueBytes)
        }
    }

    func testSnapshotRefusesAStoreOfAnUnlistedTypeAndATypeListedTwice() {
        var unlisted = sampleSnapshot()
        unlisted.types.removeLast()
        let unlistedBytes = unlisted.encode()
        // The second store starts after the first: 8 + 4 + 4 + (4 + 4 + 3) + (4 + 4 + 0) bytes.
        let secondStore = unlistedBytes.count - 16
        expectWireError(.invalidTag(tag: 8, at: secondStore, type: "Snapshot store type (not in the type table)")) {
            _ = try Wire.Snapshot.decode(unlistedBytes)
        }
        var twice = sampleSnapshot()
        twice.types[1].typeId = 7
        // The second type entry starts after the counts, the floor, the hash and the first entry.
        expectWireError(.duplicateKey(at: 4 + 8 + 8 + 4 + 12)) {
            _ = try Wire.Snapshot.decode(twice.encode())
        }
    }

    func testSnapshotRefusesADescriptionThatIsNotUTF8() {
        var bytes = Wire.Snapshot(generationFloor: 0, schemaHash: 0, types: [], description: "ab", stores: []).encode()
        bytes[bytes.count - 2] = 0xFF
        expectWireError(.invalidUTF8(at: bytes.count - 2)) {
            _ = try Wire.Snapshot.decode(bytes)
        }
    }

    func testASnapshotInTheLayoutBeforeADR037IsRefused() {
        // The layout before ADR-022 (`count u32, stores`): four bytes end where the floor should be.
        expectWireError(.unexpectedEOF(needed: 8, at: 4)) {
            _ = try Wire.Snapshot.decode(hexToBytes("00000000"))
        }
        // Layout 1, empty: four bytes are left where the (u64) floor should be.
        expectWireError(.unexpectedEOF(needed: 8, at: 4)) {
            _ = try Wire.Snapshot.decode(layout1([], floor: 0))
        }
        // Layout 1 with stores: the first handle reads as the hash and the type id as the type
        // count, and the type table does not fit or names nothing the stores use.
        XCTAssertThrowsError(try Wire.Snapshot.decode(layout1(sampleSnapshot().stores, floor: 5)))
        for typeId: UInt32 in [1, 7, 0x00C0_FFEE, UInt32.max] {
            let stores = [Wire.SnapshotStore(handle: handle, typeId: typeId, signals: [Wire.SnapshotSignal(signalId: 0, value: slice("01020304"))])]
            XCTAssertThrowsError(try Wire.Snapshot.decode(layout1(stores, floor: 1)), "type id \(typeId)") { error in
                XCTAssertTrue(error is WireError, "\(error)")
            }
        }
    }

    func testSnapshotTruncationIsAnErrorAtEveryLength() {
        let bytes = sampleSnapshot().encode()
        for cut in 0 ..< bytes.count {
            XCTAssertThrowsError(try Wire.Snapshot.decode(Array(bytes[0 ..< cut])), "cut \(cut)") { error in
                XCTAssertTrue(error is WireError, "\(error)")
            }
        }
        // And a byte too many is trailing.
        expectWireError(.trailingBytes(count: 1)) {
            _ = try Wire.Snapshot.decode(bytes + [0])
        }
    }

    // MARK: Malformed discriminants and truncation

    func testUnknownDiscriminants() {
        expectWireError(.invalidTag(tag: 4, at: 0, type: "CallTarget")) {
            _ = try Wire.Call.decode(hexToBytes("04"))
        }
        expectWireError(.invalidTag(tag: 6, at: 4, type: "ReplyStatus")) {
            _ = try Wire.Reply.decode(hexToBytes("0900000006"))
        }
        expectWireError(.invalidTag(tag: 3, at: 4, type: "PortStatus")) {
            _ = try Wire.PortReply.decode(hexToBytes("0300000003"))
        }
        // Flag 3 is `failed` (ADR-036); 4 and above are unknown.
        XCTAssertEqual(try? Wire.StreamItem.decode(hexToBytes("0900000003")).flag, .failed)
        expectWireError(.invalidTag(tag: 4, at: 4, type: "StreamFlag")) {
            _ = try Wire.StreamItem.decode(hexToBytes("0900000004"))
        }
        expectWireError(.invalidTag(tag: 255, at: 4, type: "StreamFlag")) {
            _ = try Wire.StreamItem.decode(hexToBytes("09000000ff"))
        }
        expectWireError(.invalidTag(tag: 2, at: 12, type: "bool")) {
            _ = try Wire.Observe.decode(hexToBytes("01000000010000000300000002"))
        }
    }

    func testTrailingBytesAndTruncation() {
        expectWireError(.trailingBytes(count: 1)) {
            _ = try Wire.Cancel.decode(hexToBytes("0700000000"))
        }
        expectWireError(.trailingBytes(count: 2)) {
            _ = try Wire.TimerFired.decode(hexToBytes("2a0000000000"))
        }
        expectWireError(.unexpectedEOF(needed: 4, at: 0)) {
            _ = try Wire.Cancel.decode(hexToBytes("070000"))
        }
        expectWireError(.unexpectedEOF(needed: 4, at: 4)) {
            _ = try Wire.StreamCredit.decode(hexToBytes("07000000100000"))
        }
        expectWireError(.unexpectedEOF(needed: 1, at: 0)) {
            _ = try Wire.Call.decode([])
        }
        expectWireError(.lengthTooLarge(len: 5, at: 0)) {
            _ = try Wire.Hello.decode(hexToBytes("050000003078"))
        }
    }

    func testDecodeFromSliceWithNonZeroStartIndex() throws {
        let padded: [UInt8] = [0xEE] + hexToBytes("0700000010000000") + [0xEE]
        let decoded = try Wire.StreamCredit.decode(slice: padded[1 ..< 9])
        XCTAssertEqual(decoded, Wire.StreamCredit(callId: 7, credit: 16))
    }
}
