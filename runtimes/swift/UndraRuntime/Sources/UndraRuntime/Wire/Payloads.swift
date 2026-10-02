// Envelope payloads (docs/SPEC.md sections 3.3 to 3.7 and 5.9), one Swift type per message.
// The payload structs are nested in the `Wire` namespace (`Wire.Call`, `Wire.Reply`, ...); the
// enums generated code names directly (`CallTarget`, `ReplyStatus`, `ChangeOp`) are top level.
//
// Every payload conforms to `UndraPayload`, which gives it `encode()` and `decode(_:)` on top of
// the `UndraCodec` requirements. Byte fields that hold a nested, already-encoded value (call
// arguments, reply bodies, stream items, change-set values) are `ArraySlice<UInt8>`: decoding
// produces slices of the input buffer and copies nothing. Use `UndraWriter.finishSlice()` to
// build one, and `UndraReader(slice:)` to read it.

/// A message payload with `encode()` and `decode(_:)` conveniences.
public protocol UndraPayload: UndraCodec, Sendable {}

extension UndraPayload {
    /// The complete payload bytes.
    public func encode() -> [UInt8] {
        var writer = UndraWriter()
        undraEncode(&writer)
        return writer.finish()
    }

    /// Decodes a payload that must span all of `bytes`; leftover bytes throw
    /// `WireError.trailingBytes`.
    public static func decode(_ bytes: [UInt8]) throws -> Self {
        var reader = UndraReader(bytes)
        let value = try Self.undraDecode(&reader)
        try reader.finish()
        return value
    }

    /// Decodes a payload that must span all of `slice`; leftover bytes throw
    /// `WireError.trailingBytes`.
    public static func decode(slice: ArraySlice<UInt8>) throws -> Self {
        var reader = UndraReader(slice: slice)
        let value = try Self.undraDecode(&reader)
        try reader.finish()
        return value
    }
}

// MARK: - Call (kind 1, host to core)

/// What a `Call` invokes (docs/SPEC.md section 3.3).
public enum CallTarget: Sendable, Equatable {
    /// Target 0: a free function. Wire layout writes a zero handle.
    case freeFunction(methodId: UInt32)
    /// Target 1: a method on the object behind `handle`.
    case objectMethod(handle: UndraHandle, methodId: UInt32)
    /// Target 2: a constructor. `typeId` names the object type, `methodId` selects the constructor.
    case constructor(typeId: UInt32, methodId: UInt32)
    /// Target 3: one page of a lazy list. Carries no arguments.
    case lazyListPage(handle: UndraHandle, offset: UInt32, limit: UInt32)
}

extension Wire {
    /// A call from the host to the core.
    ///
    /// Wire layouts:
    /// - target 0/1: `target u8, handle u64, method_id u32, call_id u32, args`
    /// - target 2:   `target u8, type_id u32, method_id u32, call_id u32, args`
    /// - target 3:   `target u8, handle u64, offset u32, limit u32, call_id u32`
    ///
    /// `args` is the parameters encoded in declaration order. For `lazyListPage` it is ignored when
    /// encoding and empty after decoding. When decoding a free function the handle field must be
    /// present but its value is ignored.
    public struct Call: UndraPayload, Equatable {
        public var target: CallTarget
        public var callId: UInt32
        public var args: ArraySlice<UInt8>

        public init(target: CallTarget, callId: UInt32, args: ArraySlice<UInt8> = []) {
            self.target = target
            self.callId = callId
            self.args = args
        }

        /// Writes everything that precedes `args`. Generated code calls this and then encodes the
        /// arguments straight into the same writer, which avoids building `args` separately.
        public static func writeHeader(into w: inout UndraWriter, target: CallTarget, callId: UInt32) {
            switch target {
            case .freeFunction(let methodId):
                w.writeU8(0)
                w.writeU64(0)
                w.writeU32(methodId)
                w.writeU32(callId)
            case .objectMethod(let handle, let methodId):
                w.writeU8(1)
                w.writeU64(handle.rawValue)
                w.writeU32(methodId)
                w.writeU32(callId)
            case .constructor(let typeId, let methodId):
                w.writeU8(2)
                w.writeU32(typeId)
                w.writeU32(methodId)
                w.writeU32(callId)
            case .lazyListPage(let handle, let offset, let limit):
                w.writeU8(3)
                w.writeU64(handle.rawValue)
                w.writeU32(offset)
                w.writeU32(limit)
                w.writeU32(callId)
            }
        }

        public static func undraDecode(_ r: inout UndraReader) throws -> Call {
            let tagAt = r.position
            let tag = try r.readU8()
            switch tag {
            case 0:
                _ = try r.readU64()
                let methodId = try r.readU32()
                let callId = try r.readU32()
                let args = r.readRemaining()
                return Call(target: .freeFunction(methodId: methodId), callId: callId, args: args)
            case 1:
                let handle = try r.readU64()
                let methodId = try r.readU32()
                let callId = try r.readU32()
                let args = r.readRemaining()
                return Call(
                    target: .objectMethod(handle: UndraHandle(rawValue: handle), methodId: methodId),
                    callId: callId,
                    args: args
                )
            case 2:
                let typeId = try r.readU32()
                let methodId = try r.readU32()
                let callId = try r.readU32()
                let args = r.readRemaining()
                return Call(target: .constructor(typeId: typeId, methodId: methodId), callId: callId, args: args)
            case 3:
                let handle = try r.readU64()
                let offset = try r.readU32()
                let limit = try r.readU32()
                let callId = try r.readU32()
                return Call(
                    target: .lazyListPage(handle: UndraHandle(rawValue: handle), offset: offset, limit: limit),
                    callId: callId
                )
            default:
                throw WireError.invalidTag(tag: UInt32(tag), at: tagAt, type: "CallTarget")
            }
        }

        public func undraEncode(_ w: inout UndraWriter) {
            Call.writeHeader(into: &w, target: target, callId: callId)
            if case .lazyListPage = target {
                return
            }
            w.writeRaw(slice: args)
        }
    }
}

// MARK: - Reply (kind 2, core to host)

/// The outcome of a call (docs/SPEC.md section 3.4).
public enum ReplyStatus: UInt8, Sendable, Hashable {
    /// The body is the return value (empty for `Unit`; the `T` of a `Result<T, E>`).
    case ok = 0
    /// The body is the `E` of a `Result<T, E>`.
    case error = 1
    /// The call panicked; the body is `String message, String backtrace`.
    case panic = 2
    /// The call was cancelled; the body is empty.
    case cancelled = 3
    /// A stream was opened; the body is empty and items follow as `StreamItem`.
    case streamOpened = 4
    /// The request was malformed; the body is a `String` reason.
    case badRequest = 5
}

extension Wire {
    /// The reply to a `Call`: `call_id u32, status u8, body`.
    public struct Reply: UndraPayload, Equatable {
        public var callId: UInt32
        public var status: ReplyStatus
        /// The status-dependent body, undecoded.
        public var body: ArraySlice<UInt8>

        public init(callId: UInt32, status: ReplyStatus, body: ArraySlice<UInt8> = []) {
            self.callId = callId
            self.status = status
            self.body = body
        }

        /// The `(message, backtrace)` of a `panic` reply.
        public func panicDetails() throws -> (message: String, backtrace: String) {
            var reader = UndraReader(slice: body)
            let message = try reader.readString()
            let backtrace = try reader.readString()
            try reader.finish()
            return (message: message, backtrace: backtrace)
        }

        /// The reason string of a `badRequest` reply.
        public func badRequestReason() throws -> String {
            var reader = UndraReader(slice: body)
            let reason = try reader.readString()
            try reader.finish()
            return reason
        }

        public static func undraDecode(_ r: inout UndraReader) throws -> Reply {
            let callId = try r.readU32()
            let statusAt = r.position
            let rawStatus = try r.readU8()
            guard let status = ReplyStatus(rawValue: rawStatus) else {
                throw WireError.invalidTag(tag: UInt32(rawStatus), at: statusAt, type: "ReplyStatus")
            }
            let body = r.readRemaining()
            return Reply(callId: callId, status: status, body: body)
        }

        public func undraEncode(_ w: inout UndraWriter) {
            w.writeU32(callId)
            w.writeU8(status.rawValue)
            w.writeRaw(slice: body)
        }
    }
}

// MARK: - ChangeSet (kind 3, core to host)

/// How a change-set entry's value is to be interpreted (docs/SPEC.md section 3.5).
public enum ChangeOp: UInt8, Sendable, Hashable {
    /// The value is the signal's full `T`.
    case fullValue = 0
    /// The value is a keyed patch (docs/SPEC.md section 3.8), see `decodePatch`.
    case keyedPatch = 1
    /// A lazy list was invalidated; the value is empty and the host re-pages.
    case lazyListInvalidated = 2
}

extension Wire {
    /// One signal update inside a `ChangeSet`.
    public struct ChangeEntry: Sendable, Equatable {
        public var handle: UndraHandle
        public var signalId: UInt32
        public var op: ChangeOp
        /// The undecoded value (`len` bytes on the wire).
        public var value: ArraySlice<UInt8>

        public init(handle: UndraHandle, signalId: UInt32, op: ChangeOp, value: ArraySlice<UInt8> = []) {
            self.handle = handle
            self.signalId = signalId
            self.op = op
            self.value = value
        }
    }

    /// The atomic result of one transaction: `txn_id u64, count u32, count x entry`, where an entry
    /// is `handle u64, signal_id u32, op u8, len u32, value bytes`.
    ///
    /// `len` lets a host skip an entry it cannot decode; `forEachEntry` always resumes after the
    /// declared `len` bytes whatever the visitor consumed.
    public struct ChangeSet: UndraPayload, Equatable {
        public var txnId: UInt64
        public var entries: [ChangeEntry]

        public init(txnId: UInt64, entries: [ChangeEntry]) {
            self.txnId = txnId
            self.entries = entries
        }

        /// Walks the entries of an encoded change-set without copying or allocating per entry.
        ///
        /// For each entry, `visit` receives the handle, the signal id, the op and an `UndraReader`
        /// restricted to that entry's value and positioned at its first byte. The visitor decodes
        /// what it understands (calling `finish()` on the reader if it wants exactness) and may
        /// ignore the rest. Any error thrown by `visit` aborts the walk and propagates.
        ///
        /// The reader `r` is left after the last entry; this method does not call `r.finish()`.
        ///
        /// - Returns: the change-set's transaction id.
        @discardableResult
        public static func forEachEntry(
            from r: inout UndraReader,
            _ visit: (UndraHandle, UInt32, ChangeOp, inout UndraReader) throws -> Void
        ) throws -> UInt64 {
            let txnId = try r.readU64()
            let count = try r.readLen()
            var index = 0
            while index < count {
                var entry = try readEntry(&r)
                try visit(entry.handle, entry.signalId, entry.op, &entry.value)
                index += 1
            }
            return txnId
        }

        /// Like `forEachEntry(from:_:)` but over a complete payload: bytes after the last entry
        /// throw `WireError.trailingBytes`.
        ///
        /// - Returns: the change-set's transaction id.
        @discardableResult
        public static func forEachEntry(
            slice: ArraySlice<UInt8>,
            _ visit: (UndraHandle, UInt32, ChangeOp, inout UndraReader) throws -> Void
        ) throws -> UInt64 {
            var reader = UndraReader(slice: slice)
            let txnId = try forEachEntry(from: &reader, visit)
            try reader.finish()
            return txnId
        }

        public static func undraDecode(_ r: inout UndraReader) throws -> ChangeSet {
            let txnId = try r.readU64()
            let count = try r.readLen()
            var entries: [ChangeEntry] = []
            entries.reserveCapacity(Swift.min(count, undraMaxPreallocatedElements))
            var index = 0
            while index < count {
                var entry = try readEntry(&r)
                let value = entry.value.readRemaining()
                entries.append(ChangeEntry(handle: entry.handle, signalId: entry.signalId, op: entry.op, value: value))
                index += 1
            }
            return ChangeSet(txnId: txnId, entries: entries)
        }

        public func undraEncode(_ w: inout UndraWriter) {
            w.writeU64(txnId)
            w.writeLen(entries.count)
            for entry in entries {
                w.writeU64(entry.handle.rawValue)
                w.writeU32(entry.signalId)
                w.writeU8(entry.op.rawValue)
                w.writeLen(entry.value.count)
                w.writeRaw(slice: entry.value)
            }
        }

        /// Reads one entry header and returns a reader restricted to its value.
        private static func readEntry(
            _ r: inout UndraReader
        ) throws -> (handle: UndraHandle, signalId: UInt32, op: ChangeOp, value: UndraReader) {
            let rawHandle = try r.readU64()
            let signalId = try r.readU32()
            let opAt = r.position
            let rawOp = try r.readU8()
            guard let op = ChangeOp(rawValue: rawOp) else {
                throw WireError.invalidTag(tag: UInt32(rawOp), at: opAt, type: "ChangeOp")
            }
            let length = try r.readLen()
            let value = try r.readSubReader(length: length)
            return (handle: UndraHandle(rawValue: rawHandle), signalId: signalId, op: op, value: value)
        }
    }

    // MARK: - PortCall (kind 4, core to host) and PortReply (kind 5, host to core)

    /// A call from the core to a host-implemented port (docs/SPEC.md section 3.6):
    /// `port_id u32, method_id u32, port_call_id u32, args`.
    public struct PortCall: UndraPayload, Equatable {
        public var portId: UInt32
        public var methodId: UInt32
        public var portCallId: UInt32
        /// The port method's parameters, undecoded.
        public var args: ArraySlice<UInt8>

        public init(portId: UInt32, methodId: UInt32, portCallId: UInt32, args: ArraySlice<UInt8> = []) {
            self.portId = portId
            self.methodId = methodId
            self.portCallId = portCallId
            self.args = args
        }

        public static func undraDecode(_ r: inout UndraReader) throws -> PortCall {
            let portId = try r.readU32()
            let methodId = try r.readU32()
            let portCallId = try r.readU32()
            let args = r.readRemaining()
            return PortCall(portId: portId, methodId: methodId, portCallId: portCallId, args: args)
        }

        public func undraEncode(_ w: inout UndraWriter) {
            w.writeU32(portId)
            w.writeU32(methodId)
            w.writeU32(portCallId)
            w.writeRaw(slice: args)
        }
    }

    /// The outcome of a port call (docs/SPEC.md section 3.6).
    public enum PortStatus: UInt8, Sendable, Hashable {
        /// The body is the method's return value (the `T` of a `Result<T, E>`).
        case ok = 0
        /// The body is the `E` of a `Result<T, E>`.
        case error = 1
        /// The port cannot serve the call; the core fails it with `PortError::Unavailable`.
        case unavailable = 2
    }

    /// The host's answer to a `PortCall`: `port_call_id u32, status u8, body`.
    public struct PortReply: UndraPayload, Equatable {
        public var portCallId: UInt32
        public var status: PortStatus
        /// The method's return value or error, undecoded.
        public var body: ArraySlice<UInt8>

        public init(portCallId: UInt32, status: PortStatus, body: ArraySlice<UInt8> = []) {
            self.portCallId = portCallId
            self.status = status
            self.body = body
        }

        public static func undraDecode(_ r: inout UndraReader) throws -> PortReply {
            let portCallId = try r.readU32()
            let statusAt = r.position
            let rawStatus = try r.readU8()
            guard let status = PortStatus(rawValue: rawStatus) else {
                throw WireError.invalidTag(tag: UInt32(rawStatus), at: statusAt, type: "PortStatus")
            }
            let body = r.readRemaining()
            return PortReply(portCallId: portCallId, status: status, body: body)
        }

        public func undraEncode(_ w: inout UndraWriter) {
            w.writeU32(portCallId)
            w.writeU8(status.rawValue)
            w.writeRaw(slice: body)
        }
    }

    // MARK: - Cancel (kind 6) and StreamCredit (kind 7), host to core

    /// Cancels an in-flight call or closes a stream: `call_id u32`.
    public struct Cancel: UndraPayload, Equatable {
        public var callId: UInt32

        public init(callId: UInt32) {
            self.callId = callId
        }

        public static func undraDecode(_ r: inout UndraReader) throws -> Cancel {
            let callId = try r.readU32()
            return Cancel(callId: callId)
        }

        public func undraEncode(_ w: inout UndraWriter) {
            w.writeU32(callId)
        }
    }

    /// Grants a stream `credit` more items: `call_id u32, credit u32`.
    public struct StreamCredit: UndraPayload, Equatable {
        public var callId: UInt32
        public var credit: UInt32

        public init(callId: UInt32, credit: UInt32) {
            self.callId = callId
            self.credit = credit
        }

        public static func undraDecode(_ r: inout UndraReader) throws -> StreamCredit {
            let callId = try r.readU32()
            let credit = try r.readU32()
            return StreamCredit(callId: callId, credit: credit)
        }

        public func undraEncode(_ w: inout UndraWriter) {
            w.writeU32(callId)
            w.writeU32(credit)
        }
    }

    // MARK: - StreamItem (kind 8, core to host)

    /// What a `StreamItem` carries (docs/SPEC.md section 3.7, ADR-036).
    public enum StreamFlag: UInt8, Sendable, Hashable {
        /// The body is one item `T`.
        case item = 0
        /// The stream ended; the body is empty.
        case end = 1
        /// The stream ended with **its own** typed error; the body is the encoded `E`. Only a
        /// method whose schema return is `Result<Stream<T>, E>` sends it, so a stream without an
        /// error type that receives it has received something malformed.
        case error = 2
        /// The call failed, in the reply-failure vocabulary; the body is a `StreamFailure`. The
        /// core sends it for a stream it ended itself (a restore or a shutdown) or that panicked.
        case failed = 3
    }

    /// The body of a `.failed` stream item (docs/SPEC.md section 3.7, ADR-036):
    /// `status u8, message String, detail String`.
    ///
    /// The status is a reply status (docs/SPEC.md section 3.4), so a failed stream item means
    /// exactly what a failed reply with that status means:
    ///
    /// | `status`      | `message`                | `detail`          |
    /// |---------------|--------------------------|-------------------|
    /// | `.panic`      | the panic message        | the backtrace     |
    /// | `.cancelled`  | why the core cancelled   | empty             |
    /// | `.badRequest` | why the core refused     | empty             |
    ///
    /// Decoding any other status throws `WireError.invalidTag` with the type name
    /// `"StreamFailure.status"`.
    ///
    /// ```swift
    /// let failure = Wire.StreamFailure(status: .cancelled, message: "the runtime shut down")
    /// let item = Wire.StreamItem(callId: 7, flag: .failed, body: ArraySlice(failure.encode()))
    /// let back = try item.failure()                       // == failure
    /// ```
    public struct StreamFailure: UndraPayload, Equatable {
        /// How the call failed: `.panic`, `.cancelled` (by the core) or `.badRequest` (refused).
        public var status: ReplyStatus
        /// The panic message, the cancellation reason or the refusal reason.
        public var message: String
        /// The backtrace of a panic; empty otherwise.
        public var detail: String

        public init(status: ReplyStatus, message: String, detail: String = "") {
            self.status = status
            self.message = message
            self.detail = detail
        }

        /// Whether `status` may appear in a stream failure: panicked, cancelled or refused.
        public static func allows(_ status: ReplyStatus) -> Bool {
            switch status {
            case .panic, .cancelled, .badRequest:
                return true
            case .ok, .error, .streamOpened:
                return false
            }
        }

        /// The body a failed reply with this status carries (docs/SPEC.md section 3.4):
        /// `String message, String backtrace` for `.panic`, the `String` reason for
        /// `.badRequest`, and nothing for `.cancelled` (or any status a stream failure cannot
        /// have).
        public func replyBody() -> [UInt8] {
            var writer = UndraWriter()
            switch status {
            case .panic:
                writer.writeString(message)
                writer.writeString(detail)
            case .badRequest:
                writer.writeString(message)
            case .cancelled, .ok, .error, .streamOpened:
                break
            }
            return writer.finish()
        }

        public static func undraDecode(_ r: inout UndraReader) throws -> StreamFailure {
            let statusAt = r.position
            let rawStatus = try r.readU8()
            guard let status = ReplyStatus(rawValue: rawStatus), StreamFailure.allows(status) else {
                throw WireError.invalidTag(tag: UInt32(rawStatus), at: statusAt, type: "StreamFailure.status")
            }
            let message = try r.readString()
            let detail = try r.readString()
            return StreamFailure(status: status, message: message, detail: detail)
        }

        public func undraEncode(_ w: inout UndraWriter) {
            w.writeU8(status.rawValue)
            w.writeString(message)
            w.writeString(detail)
        }
    }

    /// One element of a stream: `call_id u32, flag u8, body`, where the body is the item `T`
    /// (flag 0), nothing (1), the stream's own error `E` (2) or a `StreamFailure` (3).
    ///
    /// Flags 1, 2 and 3 end the stream and need no credit.
    public struct StreamItem: UndraPayload, Equatable {
        public var callId: UInt32
        public var flag: StreamFlag
        /// The flag-dependent body, undecoded.
        public var body: ArraySlice<UInt8>

        public init(callId: UInt32, flag: StreamFlag, body: ArraySlice<UInt8> = []) {
            self.callId = callId
            self.flag = flag
            self.body = body
        }

        /// The failure carried by a `.failed` item: its body decoded as a `StreamFailure` that
        /// spans all of it. Offsets in a thrown `WireError` count from the start of the body.
        public func failure() throws -> StreamFailure {
            return try StreamFailure.decode(slice: body)
        }

        public static func undraDecode(_ r: inout UndraReader) throws -> StreamItem {
            let callId = try r.readU32()
            let flagAt = r.position
            let rawFlag = try r.readU8()
            guard let flag = StreamFlag(rawValue: rawFlag) else {
                throw WireError.invalidTag(tag: UInt32(rawFlag), at: flagAt, type: "StreamFlag")
            }
            let body = r.readRemaining()
            return StreamItem(callId: callId, flag: flag, body: body)
        }

        public func undraEncode(_ w: inout UndraWriter) {
            w.writeU32(callId)
            w.writeU8(flag.rawValue)
            w.writeRaw(slice: body)
        }
    }

    // MARK: - Observe (kind 9) and Release (kind 10), host to core

    /// Starts or stops observing a store signal: `handle u64, signal_id u32, on u8`.
    public struct Observe: UndraPayload, Equatable {
        /// The `signal_id` that means "all signals of the store".
        public static let allSignals: UInt32 = UInt32.max

        public var handle: UndraHandle
        public var signalId: UInt32
        public var on: Bool

        public init(handle: UndraHandle, signalId: UInt32, on: Bool) {
            self.handle = handle
            self.signalId = signalId
            self.on = on
        }

        public static func undraDecode(_ r: inout UndraReader) throws -> Observe {
            let handle = try r.readU64()
            let signalId = try r.readU32()
            let on = try r.readBool()
            return Observe(handle: UndraHandle(rawValue: handle), signalId: signalId, on: on)
        }

        public func undraEncode(_ w: inout UndraWriter) {
            w.writeU64(handle.rawValue)
            w.writeU32(signalId)
            w.writeBool(on)
        }
    }

    /// Releases an object or store handle: `handle u64`.
    public struct Release: UndraPayload, Equatable {
        public var handle: UndraHandle

        public init(handle: UndraHandle) {
            self.handle = handle
        }

        public static func undraDecode(_ r: inout UndraReader) throws -> Release {
            let handle = try r.readU64()
            return Release(handle: UndraHandle(rawValue: handle))
        }

        public func undraEncode(_ w: inout UndraWriter) {
            w.writeU64(handle.rawValue)
        }
    }

    // MARK: - Event (kind 11), host to core

    /// A host-originated event for an event port (for example `Connectivity`):
    /// `port_id u32, method_id u32, payload`.
    public struct Event: UndraPayload, Equatable {
        public var portId: UInt32
        public var methodId: UInt32
        /// The event method's parameters, undecoded.
        public var payload: ArraySlice<UInt8>

        public init(portId: UInt32, methodId: UInt32, payload: ArraySlice<UInt8> = []) {
            self.portId = portId
            self.methodId = methodId
            self.payload = payload
        }

        public static func undraDecode(_ r: inout UndraReader) throws -> Event {
            let portId = try r.readU32()
            let methodId = try r.readU32()
            let payload = r.readRemaining()
            return Event(portId: portId, methodId: methodId, payload: payload)
        }

        public func undraEncode(_ w: inout UndraWriter) {
            w.writeU32(portId)
            w.writeU32(methodId)
            w.writeRaw(slice: payload)
        }
    }

    // MARK: - Hello (kind 12), both directions

    /// The handshake: `undra_version String, schema_hash u64, platform String, mode String`.
    public struct Hello: UndraPayload, Equatable {
        public var undraVersion: String
        public var schemaHash: UInt64
        public var platform: String
        public var mode: String

        public init(undraVersion: String, schemaHash: UInt64, platform: String, mode: String) {
            self.undraVersion = undraVersion
            self.schemaHash = schemaHash
            self.platform = platform
            self.mode = mode
        }

        public static func undraDecode(_ r: inout UndraReader) throws -> Hello {
            let undraVersion = try r.readString()
            let schemaHash = try r.readU64()
            let platform = try r.readString()
            let mode = try r.readString()
            return Hello(undraVersion: undraVersion, schemaHash: schemaHash, platform: platform, mode: mode)
        }

        public func undraEncode(_ w: inout UndraWriter) {
            w.writeString(undraVersion)
            w.writeU64(schemaHash)
            w.writeString(platform)
            w.writeString(mode)
        }
    }

    // MARK: - Log (kind 13, core to host)

    /// A log line from the core: `level u8, target String, message String`.
    public struct Log: UndraPayload, Equatable {
        public var level: UInt8
        public var target: String
        public var message: String

        public init(level: UInt8, target: String, message: String) {
            self.level = level
            self.target = target
            self.message = message
        }

        public static func undraDecode(_ r: inout UndraReader) throws -> Log {
            let level = try r.readU8()
            let target = try r.readString()
            let message = try r.readString()
            return Log(level: level, target: target, message: message)
        }

        public func undraEncode(_ w: inout UndraWriter) {
            w.writeU8(level)
            w.writeString(target)
            w.writeString(message)
        }
    }

    // MARK: - TimerFired (kind 14, host to core)

    /// A timer the core armed through the `Timer` port has come due: `timer_id u32`.
    public struct TimerFired: UndraPayload, Equatable {
        public var timerId: UInt32

        public init(timerId: UInt32) {
            self.timerId = timerId
        }

        public static func undraDecode(_ r: inout UndraReader) throws -> TimerFired {
            let timerId = try r.readU32()
            return TimerFired(timerId: timerId)
        }

        public func undraEncode(_ w: inout UndraWriter) {
            w.writeU32(timerId)
        }
    }

    // MARK: - Snapshot (kind 15, core to host) and Restore (kind 16, host to core)

    /// One signal value inside a snapshot: `signal_id u32, len u32, value bytes`.
    public struct SnapshotSignal: Sendable, Equatable {
        /// The signal's id within its store type.
        public var signalId: UInt32
        /// The undecoded signal value.
        public var value: ArraySlice<UInt8>

        /// Creates a signal entry.
        public init(signalId: UInt32, value: ArraySlice<UInt8>) {
            self.signalId = signalId
            self.value = value
        }
    }

    /// One store inside a snapshot: `handle u64, type_id u32, signal_count u32, signals`.
    /// Computed signals are not part of it; they are recomputed after a restore.
    public struct SnapshotStore: Sendable, Equatable {
        /// The store's handle; a restore re-issues the same handle.
        public var handle: UndraHandle
        /// `fnv1a32("<TypeName>")` of the store type; listed in ``Snapshot/types``.
        public var typeId: UInt32
        /// The non-computed signals and their encoded values.
        public var signals: [SnapshotSignal]

        /// Creates a store entry.
        public init(handle: UndraHandle, typeId: UInt32, signals: [SnapshotSignal]) {
            self.handle = handle
            self.typeId = typeId
            self.signals = signals
        }
    }

    /// The identity of one store type in a ``Snapshot`` (ADR-037): `type_id u32, fingerprint u64`.
    public struct SnapshotType: Sendable, Hashable {
        /// `fnv1a32("<TypeName>")` of the store type.
        public var typeId: UInt32
        /// `fnv1a64` of the canonical closure of the store's non-computed signals when the snapshot
        /// was taken. A restore compares it with the current build's: equal means the values decode
        /// as they are, different means the core migrates them by name using ``Snapshot/description``.
        public var fingerprint: UInt64

        /// Creates a type entry.
        public init(typeId: UInt32, fingerprint: UInt64) {
            self.typeId = typeId
            self.fingerprint = fingerprint
        }
    }

    /// The persisted state of every store (docs/SPEC.md section 5.9, layout 2 of ADR-037). The
    /// same layout is the payload of a `Restore` (kind 16) message and of `undra_restore`.
    ///
    /// ```text
    /// count u32, generation_floor u32,                              ADR-022's two leading words
    /// schema_hash u64,                                              of the core that wrote it
    /// type_count u32, types x { type_id u32, fingerprint u64 },     each store type once
    /// description_len u32, description bytes,                       canonical JSON (UTF-8)
    /// count x { handle u64, type_id u32, signal_count u32, signals x { signal_id u32, len u32, value } }
    /// ```
    ///
    /// A host passes a snapshot back to the core unchanged; this codec exists for tests and tools.
    /// Decoding refuses a store whose type is not in ``types`` (`WireError.invalidTag`), a type listed
    /// twice (`WireError.duplicateKey`) and a description that is not UTF-8
    /// (`WireError.invalidUTF8`). That, and the 64-bit hash after the floor, makes a snapshot in the
    /// layout before ADR-037 (`count, generation_floor, stores`) fail with a typed error instead of
    /// decoding as something else.
    public struct Snapshot: UndraPayload, Equatable {
        /// The highest handle generation the core had issued when the snapshot was taken. A
        /// restore resumes the core's generation counter above it, so no handle issued before the
        /// snapshot (or between it and the restore) is issued again to another object (ADR-022).
        /// Opaque to the host: pass it back unchanged.
        public var generationFloor: UInt32
        /// The schema hash of the core that took the snapshot.
        public var schemaHash: UInt64
        /// Each store type of the snapshot, once, with its fingerprint.
        public var types: [SnapshotType]
        /// The canonical JSON description of the store types' signals and the records and enums
        /// they reach (ADR-037). The core reads it only when a fingerprint differs; opaque to hosts.
        public var description: String
        /// The stores, in the order the core produced them.
        public var stores: [SnapshotStore]

        /// Smallest encoded store: `handle u64, type_id u32, signal_count u32`.
        private static let storeMinimumSize = 16
        /// Smallest encoded signal: `signal_id u32, len u32`.
        private static let signalMinimumSize = 8
        /// An encoded type entry: `type_id u32, fingerprint u64`.
        private static let typeSize = 12

        /// Creates a snapshot.
        public init(
            generationFloor: UInt32,
            schemaHash: UInt64,
            types: [SnapshotType],
            description: String,
            stores: [SnapshotStore]
        ) {
            self.generationFloor = generationFloor
            self.schemaHash = schemaHash
            self.types = types
            self.description = description
            self.stores = stores
        }

        /// The fingerprint recorded for `typeId`, if the snapshot lists it.
        public func fingerprint(typeId: UInt32) -> UInt64? {
            return types.first { $0.typeId == typeId }?.fingerprint
        }

        /// Reads a `u32` element count and checks that that many elements of at least
        /// `minimumSize` bytes fit in what remains (`WireError.lengthTooLarge` otherwise).
        private static func readCount(_ r: inout UndraReader, minimumSize: Int) throws -> Int {
            let at = r.position
            let raw = try r.readU32()
            if UInt64(raw) * UInt64(minimumSize) > UInt64(r.remaining) {
                throw WireError.lengthTooLarge(len: raw, at: at)
            }
            return Int(raw)
        }

        public static func undraDecode(_ r: inout UndraReader) throws -> Snapshot {
            let storeCount = try readCount(&r, minimumSize: storeMinimumSize)
            let generationFloor = try r.readU32()
            let schemaHash = try r.readU64()
            let typeCount = try readCount(&r, minimumSize: typeSize)
            var types: [SnapshotType] = []
            types.reserveCapacity(Swift.min(typeCount, undraMaxPreallocatedElements))
            var listed = Set<UInt32>()
            var typeIndex = 0
            while typeIndex < typeCount {
                let at = r.position
                let typeId = try r.readU32()
                let fingerprint = try r.readU64()
                if !listed.insert(typeId).inserted {
                    throw WireError.duplicateKey(at: at)
                }
                types.append(SnapshotType(typeId: typeId, fingerprint: fingerprint))
                typeIndex += 1
            }
            let description = try r.readString()
            var stores: [SnapshotStore] = []
            stores.reserveCapacity(Swift.min(storeCount, undraMaxPreallocatedElements))
            var storeIndex = 0
            while storeIndex < storeCount {
                let at = r.position
                let rawHandle = try r.readU64()
                let typeId = try r.readU32()
                let signalCount = try readCount(&r, minimumSize: signalMinimumSize)
                var signals: [SnapshotSignal] = []
                signals.reserveCapacity(Swift.min(signalCount, undraMaxPreallocatedElements))
                var signalIndex = 0
                while signalIndex < signalCount {
                    let signalId = try r.readU32()
                    let length = try r.readLen()
                    let value = try r.readSlice(length)
                    signals.append(SnapshotSignal(signalId: signalId, value: value))
                    signalIndex += 1
                }
                if !listed.contains(typeId) {
                    throw WireError.invalidTag(tag: typeId, at: at, type: "Snapshot store type (not in the type table)")
                }
                stores.append(SnapshotStore(handle: UndraHandle(rawValue: rawHandle), typeId: typeId, signals: signals))
                storeIndex += 1
            }
            return Snapshot(
                generationFloor: generationFloor,
                schemaHash: schemaHash,
                types: types,
                description: description,
                stores: stores
            )
        }

        public func undraEncode(_ w: inout UndraWriter) {
            w.writeLen(stores.count)
            w.writeU32(generationFloor)
            w.writeU64(schemaHash)
            w.writeLen(types.count)
            for type in types {
                w.writeU32(type.typeId)
                w.writeU64(type.fingerprint)
            }
            w.writeString(description)
            for store in stores {
                w.writeU64(store.handle.rawValue)
                w.writeU32(store.typeId)
                w.writeLen(store.signals.count)
                for signal in store.signals {
                    w.writeU32(signal.signalId)
                    w.writeLen(signal.value.count)
                    w.writeRaw(slice: signal.value)
                }
            }
        }
    }
}

