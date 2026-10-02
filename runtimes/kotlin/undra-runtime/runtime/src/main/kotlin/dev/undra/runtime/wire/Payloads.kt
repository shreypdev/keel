package dev.undra.runtime.wire

/**
 * Typed forms of every envelope payload in SPEC §3.3 to §3.7, §5.9 and the small fixed layouts of
 * §3.2, each with `encode` / `decode`.
 *
 * Conventions shared by all payloads:
 *  - `encode(w)` appends to an [UndraWriter]; `toByteArray()` returns a fresh array.
 *  - `decode(r)` reads from an [UndraReader] and stops after the payload; `decode(bytes)` decodes a
 *    whole message and rejects trailing bytes.
 *  - Payloads whose layout ends in a raw, unprefixed body (call `args`, reply `body`, port `args`,
 *    stream item `body`, event `payload`) keep that body as an opaque `ByteArray`, and `decode(r)`
 *    consumes the rest of the reader for it. **Bodies are copies**, so they stay valid after a JNI
 *    direct buffer is recycled. To decode a return value without that copy, read the fixed header
 *    fields with the [UndraReader] yourself and pass the same reader to the value's codec.
 *  - Ids use the wire's unsigned types: `UInt` for `u32`, `ULong` for `u64`, [Handle] for handles.
 *  - Classes holding a `ByteArray` implement `equals` / `hashCode` by content.
 */
public object Payloads {

    /** Common shape of every payload. */
    public sealed interface Payload {
        /** Appends this payload's wire encoding to [w]. */
        public fun encode(w: UndraWriter)

        /** This payload's wire encoding as a fresh byte array. */
        public fun toByteArray(): ByteArray {
            val w = UndraWriter()
            encode(w)
            return w.toByteArray()
        }
    }

    // ---- Call (§3.3) --------------------------------------------------------------------------------

    /** What a [Call] invokes; the `target` byte of SPEC §3.3 plus the fields that go with it. */
    public sealed interface CallTarget {
        /** Target 0: a free function. Its `handle` slot is always written as 0. */
        public data class FreeFunction(val methodId: UInt) : CallTarget

        /** Target 1: a method of the object behind [handle]. */
        public data class ObjectMethod(val handle: Handle, val methodId: UInt) : CallTarget

        /** Target 2: constructor [methodId] of the object type [typeId]. */
        public data class Constructor(val typeId: UInt, val methodId: UInt) : CallTarget

        /** Target 3: a page of [limit] items starting at [offset] of the lazy list behind [handle]. */
        public data class LazyListPage(val handle: Handle, val offset: UInt, val limit: UInt) : CallTarget
    }

    /**
     * host to core (envelope kind CALL).
     *
     * Layouts: targets 0 and 1 are `target u8, handle u64, method_id u32, call_id u32, args`; target 2
     * is `target u8, type_id u32, method_id u32, call_id u32, args`; target 3 is
     * `target u8, handle u64, offset u32, limit u32, call_id u32` and has no args.
     *
     * When decoding a free-function call the `handle` slot is skipped without validation.
     *
     * @property target what to invoke.
     * @property callId chosen by the host, unique among its in-flight calls; 0 is reserved.
     * @property args the encoded parameters in declaration order (empty for [CallTarget.LazyListPage]).
     * @throws IllegalArgumentException on construction if [args] is non-empty for a
     *   [CallTarget.LazyListPage] call, which has no argument area.
     */
    public data class Call(val target: CallTarget, val callId: UInt, val args: ByteArray) : Payload {
        init {
            require(target !is CallTarget.LazyListPage || args.isEmpty()) { "a lazy-list page call carries no args" }
        }

        override fun encode(w: UndraWriter) {
            when (val t = target) {
                is CallTarget.FreeFunction -> {
                    w.writeU8(TARGET_FREE_FUNCTION.toUByte())
                    w.writeU64(0uL)
                    w.writeU32(t.methodId)
                    w.writeU32(callId)
                    w.writeRaw(args)
                }
                is CallTarget.ObjectMethod -> {
                    w.writeU8(TARGET_OBJECT_METHOD.toUByte())
                    w.writeI64(t.handle.raw)
                    w.writeU32(t.methodId)
                    w.writeU32(callId)
                    w.writeRaw(args)
                }
                is CallTarget.Constructor -> {
                    w.writeU8(TARGET_CONSTRUCTOR.toUByte())
                    w.writeU32(t.typeId)
                    w.writeU32(t.methodId)
                    w.writeU32(callId)
                    w.writeRaw(args)
                }
                is CallTarget.LazyListPage -> {
                    w.writeU8(TARGET_LAZY_LIST_PAGE.toUByte())
                    w.writeI64(t.handle.raw)
                    w.writeU32(t.offset)
                    w.writeU32(t.limit)
                    w.writeU32(callId)
                }
            }
        }

        override fun equals(other: Any?): Boolean =
            this === other || (
                other is Call && target == other.target && callId == other.callId && args.contentEquals(other.args)
                )

        override fun hashCode(): Int = 31 * (31 * target.hashCode() + callId.hashCode()) + args.contentHashCode()

        override fun toString(): String = "Call(target=$target, callId=$callId, args=${args.size}B)"

        public companion object {
            private const val TARGET_FREE_FUNCTION = 0
            private const val TARGET_OBJECT_METHOD = 1
            private const val TARGET_CONSTRUCTOR = 2
            private const val TARGET_LAZY_LIST_PAGE = 3

            /**
             * Reads a call from [r]; for targets 0 to 2 `args` takes the rest of the reader.
             *
             * @throws WireException.InvalidTag if the target byte is not 0 to 3.
             */
            public fun decode(r: UndraReader): Call {
                val at = r.position
                return when (val tag = r.readU8().toInt()) {
                    TARGET_FREE_FUNCTION -> {
                        r.skip(8) // handle slot, always 0
                        val methodId = r.readU32()
                        val callId = r.readU32()
                        Call(CallTarget.FreeFunction(methodId), callId, r.readRemaining())
                    }
                    TARGET_OBJECT_METHOD -> {
                        val handle = Handle(r.readI64())
                        val methodId = r.readU32()
                        val callId = r.readU32()
                        Call(CallTarget.ObjectMethod(handle, methodId), callId, r.readRemaining())
                    }
                    TARGET_CONSTRUCTOR -> {
                        val typeId = r.readU32()
                        val methodId = r.readU32()
                        val callId = r.readU32()
                        Call(CallTarget.Constructor(typeId, methodId), callId, r.readRemaining())
                    }
                    TARGET_LAZY_LIST_PAGE -> {
                        val handle = Handle(r.readI64())
                        val offset = r.readU32()
                        val limit = r.readU32()
                        val callId = r.readU32()
                        Call(CallTarget.LazyListPage(handle, offset, limit), callId, EMPTY)
                    }
                    else -> throw WireException.InvalidTag(tag.toUInt(), at, "CallTarget")
                }
            }

            /** Decodes a whole call message; rejects trailing bytes. */
            public fun decode(bytes: ByteArray): Call = decodeWhole(bytes) { decode(it) }
        }
    }

    // ---- Reply (§3.4) -------------------------------------------------------------------------------

    /** Outcome of a call; the `status` byte of SPEC §3.4. */
    public enum class ReplyStatus(
        /** The `u8` written on the wire. */
        public val code: UByte,
    ) {
        /** The body is the return value (empty for `Unit`; the `T` of a `Result<T, E>`). */
        OK(0u),

        /** The body is the typed error `E`. */
        ERROR(1u),

        /** The body is `String message, String backtrace` (see [Reply.readPanic]). */
        PANIC(2u),

        /** The call was cancelled; the body is empty. */
        CANCELLED(3u),

        /** A stream was opened; the body is empty and items follow as [StreamItem]s. */
        STREAM_OPENED(4u),

        /** The call was malformed; the body is a `String` reason (see [Reply.readBadRequestReason]). */
        BAD_REQUEST(5u);

        public companion object {
            /**
             * The status with wire code [b].
             *
             * @param at offset of the byte for the error report; `-1` when it is not known.
             * @throws WireException.InvalidTag if [b] is not 0 to 5.
             */
            public fun fromByte(b: UByte, at: Int = -1): ReplyStatus =
                entries.getOrNull(b.toInt()) ?: throw WireException.InvalidTag(b.toUInt(), at, "ReplyStatus")
        }
    }

    /** The message of a [ReplyStatus.PANIC] reply. */
    public data class PanicInfo(val message: String, val backtrace: String)

    /**
     * core to host (envelope kind REPLY): `call_id u32, status u8, body`.
     *
     * @property callId the call this answers.
     * @property status the outcome; it decides how [body] is to be read.
     * @property body the raw body (see [ReplyStatus]); decode it with the method's return codec via
     *   [bodyReader].
     */
    public data class Reply(val callId: UInt, val status: ReplyStatus, val body: ByteArray) : Payload {
        override fun encode(w: UndraWriter) {
            w.writeU32(callId)
            w.writeU8(status.code)
            w.writeRaw(body)
        }

        /** A reader over [body], for decoding the return value or typed error. */
        public fun bodyReader(): UndraReader = UndraReader(body)

        /**
         * Reads the body of a [ReplyStatus.PANIC] reply.
         *
         * @throws WireException if the body is not `String, String` exactly.
         */
        public fun readPanic(): PanicInfo {
            val r = bodyReader()
            val info = PanicInfo(r.readStr(), r.readStr())
            r.finish()
            return info
        }

        /**
         * Reads the body of a [ReplyStatus.BAD_REQUEST] reply.
         *
         * @throws WireException if the body is not a single `String`.
         */
        public fun readBadRequestReason(): String {
            val r = bodyReader()
            val reason = r.readStr()
            r.finish()
            return reason
        }

        override fun equals(other: Any?): Boolean =
            this === other || (
                other is Reply && callId == other.callId && status == other.status && body.contentEquals(other.body)
                )

        override fun hashCode(): Int = 31 * (31 * callId.hashCode() + status.hashCode()) + body.contentHashCode()

        override fun toString(): String = "Reply(callId=$callId, status=$status, body=${body.size}B)"

        public companion object {
            /** Reads a reply from [r]; `body` takes the rest of the reader. */
            public fun decode(r: UndraReader): Reply {
                val callId = r.readU32()
                val at = r.position
                val status = ReplyStatus.fromByte(r.readU8(), at)
                return Reply(callId, status, r.readRemaining())
            }

            /** Decodes a whole reply message. */
            public fun decode(bytes: ByteArray): Reply = decodeWhole(bytes) { decode(it) }
        }
    }

    // ---- ChangeSet (§3.5) ---------------------------------------------------------------------------

    /** How a [ChangeEntry] value is to be interpreted; the `op` byte of SPEC §3.5. */
    public enum class ChangeOp(
        /** The `u8` written on the wire. */
        public val code: UByte,
    ) {
        /** The value is the signal's whole `T`. */
        FULL(0u),

        /** The value is a keyed patch (see [KeyedPatch]). */
        PATCH(1u),

        /** A lazy list was invalidated; the value is a [LazyInvalidated] (the new length and version) and the host re-pages its window. */
        INVALIDATED(2u);

        public companion object {
            /**
             * The op with wire code [b].
             *
             * @param at offset of the byte for the error report; `-1` when it is not known.
             * @throws WireException.InvalidTag if [b] is not 0 to 2.
             */
            public fun fromByte(b: UByte, at: Int = -1): ChangeOp =
                entries.getOrNull(b.toInt()) ?: throw WireException.InvalidTag(b.toUInt(), at, "ChangeOp")
        }
    }

    /**
     * One signal update inside a [ChangeSet]: `handle u64, signal_id u32, op u8, len u32, value`.
     *
     * @property handle the store the signal belongs to.
     * @property signalId the signal's index within the store (`UInt.MAX_VALUE` means all signals).
     * @property op how to read [value].
     * @property value the encoded signal value or patch.
     */
    public class ChangeEntry(
        public val handle: Handle,
        public val signalId: UInt,
        public val op: ChangeOp,
        public val value: ByteArray,
    ) {
        override fun equals(other: Any?): Boolean =
            this === other || (
                other is ChangeEntry && handle == other.handle && signalId == other.signalId &&
                    op == other.op && value.contentEquals(other.value)
                )

        override fun hashCode(): Int {
            var h = handle.hashCode()
            h = 31 * h + signalId.hashCode()
            h = 31 * h + op.hashCode()
            return 31 * h + value.contentHashCode()
        }

        override fun toString(): String = "ChangeEntry(handle=$handle, signalId=$signalId, op=$op, value=${value.size}B)"
    }

    /**
     * core to host (envelope kind CHANGE_SET): `txn_id u64, count u32, entries`. A change-set is
     * delivered whole and in commit order.
     *
     * This class materializes every entry, which is convenient for tests and tools. The runtime's
     * hot path should use [forEachEntry], which allocates nothing per entry.
     *
     * @property txnId transaction id.
     * @property entries the updates, in order.
     */
    public data class ChangeSet(val txnId: ULong, val entries: List<ChangeEntry>) : Payload {
        override fun encode(w: UndraWriter) {
            w.writeU64(txnId)
            w.writeLen(entries.size)
            for (e in entries) {
                w.writeI64(e.handle.raw)
                w.writeU32(e.signalId)
                w.writeU8(e.op.code)
                w.writeBytes(e.value)
            }
        }

        public companion object {
            /** Smallest encoded entry: handle 8 + signal_id 4 + op 1 + len 4. */
            @PublishedApi
            internal const val MIN_ENTRY_BYTES: Int = 17

            /** Reads a change-set from [r], copying every entry value. */
            public fun decode(r: UndraReader): ChangeSet {
                val txnId = r.readU64()
                val count = r.readLen(MIN_ENTRY_BYTES)
                val entries = ArrayList<ChangeEntry>(count)
                for (i in 0 until count) {
                    val handle = Handle(r.readI64())
                    val signalId = r.readU32()
                    val at = r.position
                    val op = ChangeOp.fromByte(r.readU8(), at)
                    entries.add(ChangeEntry(handle, signalId, op, r.readBytes()))
                }
                return ChangeSet(txnId, entries)
            }

            /** Decodes a whole change-set message. */
            public fun decode(bytes: ByteArray): ChangeSet = decodeWhole(bytes) { decode(it) }

            /**
             * Walks the entries of the change-set in [r] without materializing them: nothing is
             * allocated per entry. It allocates two small objects per call and no arrays: one
             * reusable view reader that is re-pointed at each entry's value, plus whatever the
             * inlined [block] itself allocates.
             *
             * For each entry, [block] receives the store [Handle], the signal id, the [ChangeOp] and a
             * reader positioned at the start of the entry's value and limited to it. The reader is
             * only valid during [block] (it is re-used for the next entry) and is bounded by the entry's
             * `len`, so a value that cannot be decoded is skipped simply by not reading it; reading
             * past the value throws [WireException.UnexpectedEof]. The block need not consume the value.
             *
             * When [r] is a reader over a direct `ByteBuffer` (a JNI callback), nothing is copied
             * at all.
             *
             * @return the transaction id.
             * @throws WireException if the change-set is malformed, including trailing bytes after the
             *   last entry. An exception thrown by [block] propagates and stops the walk.
             */
            public inline fun forEachEntry(
                r: UndraReader,
                block: (handle: Handle, signalId: UInt, op: ChangeOp, value: UndraReader) -> Unit,
            ): ULong {
                val txnId = r.readU64()
                val count = r.readLen(MIN_ENTRY_BYTES)
                val view = r.newView()
                for (i in 0 until count) {
                    val handle = Handle(r.readI64())
                    val signalId = r.readU32()
                    val at = r.position
                    val op = ChangeOp.fromByte(r.readU8(), at)
                    val len = r.readLen()
                    view.aimAt(r, len)
                    block(handle, signalId, op, view)
                    r.skip(len)
                }
                r.finish()
                return txnId
            }

            /** [forEachEntry] over a byte array holding one whole change-set. */
            public inline fun forEachEntry(
                bytes: ByteArray,
                block: (handle: Handle, signalId: UInt, op: ChangeOp, value: UndraReader) -> Unit,
            ): ULong = forEachEntry(UndraReader(bytes), block)
        }
    }

    // ---- Ports (§3.6) -------------------------------------------------------------------------------

    /**
     * core to host (envelope kind PORT_CALL): `port_id u32, method_id u32, port_call_id u32, args`.
     *
     * @property portId the port being called.
     * @property methodId the port method.
     * @property portCallId chosen by the core, unique among its in-flight port calls.
     * @property args the encoded parameters in declaration order.
     */
    public data class PortCall(val portId: UInt, val methodId: UInt, val portCallId: UInt, val args: ByteArray) : Payload {
        override fun encode(w: UndraWriter) {
            w.writeU32(portId)
            w.writeU32(methodId)
            w.writeU32(portCallId)
            w.writeRaw(args)
        }

        override fun equals(other: Any?): Boolean =
            this === other || (
                other is PortCall && portId == other.portId && methodId == other.methodId &&
                    portCallId == other.portCallId && args.contentEquals(other.args)
                )

        override fun hashCode(): Int {
            var h = portId.hashCode()
            h = 31 * h + methodId.hashCode()
            h = 31 * h + portCallId.hashCode()
            return 31 * h + args.contentHashCode()
        }

        override fun toString(): String =
            "PortCall(portId=$portId, methodId=$methodId, portCallId=$portCallId, args=${args.size}B)"

        public companion object {
            /** Reads a port call from [r]; `args` takes the rest of the reader. */
            public fun decode(r: UndraReader): PortCall {
                val portId = r.readU32()
                val methodId = r.readU32()
                val portCallId = r.readU32()
                return PortCall(portId, methodId, portCallId, r.readRemaining())
            }

            /** Decodes a whole port-call message. */
            public fun decode(bytes: ByteArray): PortCall = decodeWhole(bytes) { decode(it) }
        }
    }

    /** Outcome of a port call; the `status` byte of SPEC §3.6. */
    public enum class PortStatus(
        /** The `u8` written on the wire. */
        public val code: UByte,
    ) {
        /** The body is the port method's return value. */
        OK(0u),

        /** The body is the typed error of a `Result` return. */
        ERROR(1u),

        /** The host cannot serve the call (`PortError::Unavailable`); the body is empty. */
        UNAVAILABLE(2u);

        public companion object {
            /**
             * The status with wire code [b].
             *
             * @param at offset of the byte for the error report; `-1` when it is not known.
             * @throws WireException.InvalidTag if [b] is not 0 to 2.
             */
            public fun fromByte(b: UByte, at: Int = -1): PortStatus =
                entries.getOrNull(b.toInt()) ?: throw WireException.InvalidTag(b.toUInt(), at, "PortStatus")
        }
    }

    /**
     * host to core (envelope kind PORT_REPLY): `port_call_id u32, status u8, body`.
     *
     * @property portCallId the port call this answers.
     * @property status the outcome.
     * @property body the encoded return value or error (empty for [PortStatus.UNAVAILABLE]).
     */
    public data class PortReply(val portCallId: UInt, val status: PortStatus, val body: ByteArray) : Payload {
        override fun encode(w: UndraWriter) {
            w.writeU32(portCallId)
            w.writeU8(status.code)
            w.writeRaw(body)
        }

        override fun equals(other: Any?): Boolean =
            this === other || (
                other is PortReply && portCallId == other.portCallId && status == other.status &&
                    body.contentEquals(other.body)
                )

        override fun hashCode(): Int = 31 * (31 * portCallId.hashCode() + status.hashCode()) + body.contentHashCode()

        override fun toString(): String = "PortReply(portCallId=$portCallId, status=$status, body=${body.size}B)"

        public companion object {
            /** Reads a port reply from [r]; `body` takes the rest of the reader. */
            public fun decode(r: UndraReader): PortReply {
                val portCallId = r.readU32()
                val at = r.position
                val status = PortStatus.fromByte(r.readU8(), at)
                return PortReply(portCallId, status, r.readRemaining())
            }

            /** Decodes a whole port-reply message. */
            public fun decode(bytes: ByteArray): PortReply = decodeWhole(bytes) { decode(it) }
        }
    }

    // ---- Small fixed payloads (§3.2) ----------------------------------------------------------------

    /**
     * host to core (envelope kind CANCEL): `call_id u32`. Also closes a stream.
     *
     * @property callId the call to cancel.
     */
    public data class Cancel(val callId: UInt) : Payload {
        override fun encode(w: UndraWriter) {
            w.writeU32(callId)
        }

        public companion object {
            /** Reads a cancel from [r]. */
            public fun decode(r: UndraReader): Cancel = Cancel(r.readU32())

            /** Decodes a whole cancel message. */
            public fun decode(bytes: ByteArray): Cancel = decodeWhole(bytes) { decode(it) }
        }
    }

    /**
     * host to core (envelope kind STREAM_CREDIT): `call_id u32, credit u32`. Grants the core the
     * right to send [credit] more items on the stream.
     *
     * @property callId the stream's call id.
     * @property credit number of additional items the host is ready to receive.
     */
    public data class StreamCredit(val callId: UInt, val credit: UInt) : Payload {
        override fun encode(w: UndraWriter) {
            w.writeU32(callId)
            w.writeU32(credit)
        }

        public companion object {
            /** Reads a stream credit from [r]. */
            public fun decode(r: UndraReader): StreamCredit {
                val callId = r.readU32()
                return StreamCredit(callId, r.readU32())
            }

            /** Decodes a whole stream-credit message. */
            public fun decode(bytes: ByteArray): StreamCredit = decodeWhole(bytes) { decode(it) }
        }
    }

    /** What a [StreamItem] carries; the `flag` byte of SPEC §3.7 (ADR-036). */
    public enum class StreamFlag(
        /** The `u8` written on the wire. */
        public val code: UByte,
    ) {
        /** The body is one item `T`. */
        ITEM(0u),

        /** The stream ended normally; the body is empty. */
        END(1u),

        /**
         * The stream ended with **its own** typed error; the body is the encoded `E`. Only a method whose
         * schema return is `Result<Stream<T>, E>` sends it: a failed asynchronous opening, or an `Err(e)`
         * item of an `impl Stream<Item = Result<T, E>>`. Never a core-made string.
         */
        ERROR(2u),

        /**
         * The call failed, in the reply-failure vocabulary: the body is a [StreamFailure]. Sent by the core
         * for a stream it ended itself (a restore that replaced the receiver, shutdown) or that panicked.
         * Ends the stream and needs no credit, like [END] and [ERROR].
         */
        FAILED(3u);

        public companion object {
            /**
             * The flag with wire code [b].
             *
             * @param at offset of the byte for the error report; `-1` when it is not known.
             * @throws WireException.InvalidTag if [b] is not 0 to 3.
             */
            public fun fromByte(b: UByte, at: Int = -1): StreamFlag =
                entries.getOrNull(b.toInt()) ?: throw WireException.InvalidTag(b.toUInt(), at, "StreamFlag")
        }
    }

    /**
     * The body of a [StreamFlag.FAILED] stream item (SPEC §3.7, ADR-036): the call failed, with the
     * status codes of a failed [Reply], so the runtime maps it exactly as it maps a failed reply with
     * that status (see [replyBody]). Layout: `status u8, message String, detail String`.
     *
     * The status is one of [ReplyStatus.PANIC] (the stream panicked: [message] is the panic message,
     * [detail] its backtrace), [ReplyStatus.CANCELLED] (the core ended the stream itself, after a restore
     * that replaced its receiver or at shutdown: [message] is the reason, [detail] is empty) or
     * [ReplyStatus.BAD_REQUEST] (refused: [message] is the reason, [detail] is empty).
     *
     * ```kotlin
     * val failure = Payloads.StreamFailure.decode(item.body)  // item.flag == StreamFlag.FAILED
     * throw UndraReplyException(failure.status, failure.replyBody())
     * ```
     *
     * @property status how the call failed: [ReplyStatus.PANIC], [ReplyStatus.CANCELLED] or
     *   [ReplyStatus.BAD_REQUEST].
     * @property message the panic message, the cancellation reason or the refusal reason.
     * @property detail the backtrace of a panic; empty otherwise.
     * @throws IllegalArgumentException on construction if [status] is not one of the three above.
     */
    public data class StreamFailure(val status: ReplyStatus, val message: String, val detail: String) : Payload {
        init {
            require(allows(status)) { "a stream failure has status PANIC, CANCELLED or BAD_REQUEST, not $status" }
        }

        override fun encode(w: UndraWriter) {
            w.writeU8(status.code)
            w.writeStr(message)
            w.writeStr(detail)
        }

        /**
         * The body of the failed [Reply] this failure stands for (SPEC §3.4): `String message, String
         * backtrace` for [ReplyStatus.PANIC] (this failure's encoding without its status byte), empty for
         * [ReplyStatus.CANCELLED], `String reason` for [ReplyStatus.BAD_REQUEST]. [Reply.readPanic] and
         * [Reply.readBadRequestReason] read it back.
         */
        public fun replyBody(): ByteArray = when (status) {
            ReplyStatus.PANIC -> UndraWriter().also {
                it.writeStr(message)
                it.writeStr(detail)
            }.toByteArray()
            ReplyStatus.BAD_REQUEST -> UndraWriter().also { it.writeStr(message) }.toByteArray()
            else -> EMPTY // CANCELLED, the only other status the init block admits: a §3.4 status 3 body is empty
        }

        public companion object {
            /**
             * Whether [status] can end a stream as a failure: [ReplyStatus.PANIC], [ReplyStatus.CANCELLED]
             * or [ReplyStatus.BAD_REQUEST].
             */
            public fun allows(status: ReplyStatus): Boolean =
                status == ReplyStatus.PANIC || status == ReplyStatus.CANCELLED || status == ReplyStatus.BAD_REQUEST

            /**
             * Reads a stream failure from [r].
             *
             * @throws WireException.InvalidTag with type `"StreamFailure.status"` and the offset of the status
             *   byte if the status is not 2, 3 or 5.
             */
            public fun decode(r: UndraReader): StreamFailure {
                val at = r.position
                val code = r.readU8()
                val status = ReplyStatus.entries.getOrNull(code.toInt())?.takeIf { allows(it) }
                    ?: throw WireException.InvalidTag(code.toUInt(), at, "StreamFailure.status")
                val message = r.readStr()
                return StreamFailure(status, message, r.readStr())
            }

            /** Decodes the whole body of a [StreamFlag.FAILED] item; rejects trailing bytes. */
            public fun decode(bytes: ByteArray): StreamFailure = decodeWhole(bytes) { decode(it) }
        }
    }

    /**
     * core to host (envelope kind STREAM_ITEM): `call_id u32, flag u8, body`, where the body is the rest
     * of the payload: the item `T` ([StreamFlag.ITEM]), nothing ([StreamFlag.END]), the stream's own `E`
     * ([StreamFlag.ERROR]) or a [StreamFailure] ([StreamFlag.FAILED]).
     *
     * @property callId the stream's call id.
     * @property flag what [body] holds.
     * @property body the encoded item, error or failure (empty for [StreamFlag.END]).
     */
    public data class StreamItem(val callId: UInt, val flag: StreamFlag, val body: ByteArray) : Payload {
        override fun encode(w: UndraWriter) {
            w.writeU32(callId)
            w.writeU8(flag.code)
            w.writeRaw(body)
        }

        override fun equals(other: Any?): Boolean =
            this === other || (
                other is StreamItem && callId == other.callId && flag == other.flag && body.contentEquals(other.body)
                )

        override fun hashCode(): Int = 31 * (31 * callId.hashCode() + flag.hashCode()) + body.contentHashCode()

        override fun toString(): String = "StreamItem(callId=$callId, flag=$flag, body=${body.size}B)"

        public companion object {
            /** Reads a stream item from [r]; `body` takes the rest of the reader. */
            public fun decode(r: UndraReader): StreamItem {
                val callId = r.readU32()
                val at = r.position
                val flag = StreamFlag.fromByte(r.readU8(), at)
                return StreamItem(callId, flag, r.readRemaining())
            }

            /** Decodes a whole stream-item message. */
            public fun decode(bytes: ByteArray): StreamItem = decodeWhole(bytes) { decode(it) }
        }
    }

    /**
     * host to core (envelope kind OBSERVE): `handle u64, signal_id u32, on u8`.
     *
     * @property handle the store.
     * @property signalId the signal, or `UInt.MAX_VALUE` for all signals of the store.
     * @property on `true` to start observing, `false` to stop.
     */
    public data class Observe(val handle: Handle, val signalId: UInt, val on: Boolean) : Payload {
        override fun encode(w: UndraWriter) {
            w.writeI64(handle.raw)
            w.writeU32(signalId)
            w.writeBool(on)
        }

        public companion object {
            /** Reads an observe request from [r]; `on` must be 0 or 1. */
            public fun decode(r: UndraReader): Observe {
                val handle = Handle(r.readI64())
                val signalId = r.readU32()
                return Observe(handle, signalId, r.readBool())
            }

            /** Decodes a whole observe message. */
            public fun decode(bytes: ByteArray): Observe = decodeWhole(bytes) { decode(it) }
        }
    }

    /**
     * host to core (envelope kind RELEASE): `handle u64`.
     *
     * @property handle the object to release.
     */
    public data class Release(val handle: Handle) : Payload {
        override fun encode(w: UndraWriter) {
            w.writeI64(handle.raw)
        }

        public companion object {
            /** Reads a release from [r]. */
            public fun decode(r: UndraReader): Release = Release(Handle(r.readI64()))

            /** Decodes a whole release message. */
            public fun decode(bytes: ByteArray): Release = decodeWhole(bytes) { decode(it) }
        }
    }

    // ---- lazy lists (ADR-043 decision 3.2) -------------------------------------------------------------

    /**
     * The value of a `Lazy<T>` signal (change-set op 0 [ChangeOp.FULL] of that signal, and a restore's re-send):
     * `handle u64, len u32, version u64`.
     *
     * @property handle the page server: the object a [CallTarget.LazyListPage] call is addressed to. It is
     *   transient: a restore sends a new one.
     * @property len the number of items.
     * @property version the version of the list [len] was read at; it increases with every change.
     */
    public data class LazyValue(val handle: Handle, val len: UInt, val version: ULong) : Payload {
        override fun encode(w: UndraWriter) {
            w.writeI64(handle.raw)
            w.writeU32(len)
            w.writeU64(version)
        }

        public companion object {
            /** Reads a lazy value from [r]; it does not require the reader to be exhausted. */
            public fun decode(r: UndraReader): LazyValue = LazyValue(Handle(r.readI64()), r.readU32(), r.readU64())

            /** Decodes a whole lazy value. */
            public fun decode(bytes: ByteArray): LazyValue = decodeWhole(bytes) { decode(it) }
        }
    }

    /**
     * The value of a change-set entry with op [ChangeOp.INVALIDATED] (op 2): `len u32, version u64`. A host
     * knows the new length and version without a round trip and re-pages its window.
     *
     * @property len the new number of items.
     * @property version the new version.
     */
    public data class LazyInvalidated(val len: UInt, val version: ULong) : Payload {
        override fun encode(w: UndraWriter) {
            w.writeU32(len)
            w.writeU64(version)
        }

        public companion object {
            /** Reads a lazy invalidation from [r]; it does not require the reader to be exhausted. */
            public fun decode(r: UndraReader): LazyInvalidated = LazyInvalidated(r.readU32(), r.readU64())

            /** Decodes a whole lazy invalidation. */
            public fun decode(bytes: ByteArray): LazyInvalidated = decodeWhole(bytes) { decode(it) }
        }
    }

    /**
     * The header of the reply to a [CallTarget.LazyListPage] call: `version u64, total u32, count u32`, followed
     * by [count] items, each encoded as the list's item type.
     *
     * @property version the version of the list the page was read at.
     * @property total the number of items the list had.
     * @property count how many items follow.
     */
    public data class LazyPageHeader(val version: ULong, val total: UInt, val count: UInt) : Payload {
        override fun encode(w: UndraWriter) {
            w.writeU64(version)
            w.writeU32(total)
            w.writeU32(count)
        }

        public companion object {
            /** Reads a page header from [r]; the items are left for the caller to read. */
            public fun decode(r: UndraReader): LazyPageHeader = LazyPageHeader(r.readU64(), r.readU32(), r.readU32())
        }
    }

    /**
     * host to core (envelope kind EVENT): `port_id u32, method_id u32, payload`. A fire-and-forget
     * call into an event port (`#[undra::port(event)]`).
     *
     * @property portId the event port.
     * @property methodId the event method.
     * @property payload the encoded parameters.
     */
    public data class Event(val portId: UInt, val methodId: UInt, val payload: ByteArray) : Payload {
        override fun encode(w: UndraWriter) {
            w.writeU32(portId)
            w.writeU32(methodId)
            w.writeRaw(payload)
        }

        override fun equals(other: Any?): Boolean =
            this === other || (
                other is Event && portId == other.portId && methodId == other.methodId &&
                    payload.contentEquals(other.payload)
                )

        override fun hashCode(): Int = 31 * (31 * portId.hashCode() + methodId.hashCode()) + payload.contentHashCode()

        override fun toString(): String = "Event(portId=$portId, methodId=$methodId, payload=${payload.size}B)"

        public companion object {
            /** Reads an event from [r]; `payload` takes the rest of the reader. */
            public fun decode(r: UndraReader): Event {
                val portId = r.readU32()
                val methodId = r.readU32()
                return Event(portId, methodId, r.readRemaining())
            }

            /** Decodes a whole event message. */
            public fun decode(bytes: ByteArray): Event = decodeWhole(bytes) { decode(it) }
        }
    }

    /**
     * Both directions (envelope kind HELLO): `undra_version String, schema_hash u64, platform String,
     * mode String`. Exchanged when attaching; the runtime compares [schemaHash] and fails with a
     * schema-mismatch error if it differs.
     *
     * @property undraVersion the sender's Undra version.
     * @property schemaHash the sender's schema hash.
     * @property platform for example `"android"`, `"ios"`, `"web"`, `"jvm"`.
     * @property mode `"inproc"` or `"dev"`.
     */
    public data class Hello(val undraVersion: String, val schemaHash: ULong, val platform: String, val mode: String) : Payload {
        override fun encode(w: UndraWriter) {
            w.writeStr(undraVersion)
            w.writeU64(schemaHash)
            w.writeStr(platform)
            w.writeStr(mode)
        }

        public companion object {
            /** Reads a hello from [r]. */
            public fun decode(r: UndraReader): Hello {
                val undraVersion = r.readStr()
                val schemaHash = r.readU64()
                val platform = r.readStr()
                return Hello(undraVersion, schemaHash, platform, r.readStr())
            }

            /** Decodes a whole hello message. */
            public fun decode(bytes: ByteArray): Hello = decodeWhole(bytes) { decode(it) }
        }
    }

    /**
     * core to host (envelope kind LOG): `level u8, target String, message String`.
     *
     * @property level severity as the `Log` port defines it (SPEC §8).
     * @property target the emitting module or component.
     * @property message the text.
     */
    public data class Log(val level: UByte, val target: String, val message: String) : Payload {
        override fun encode(w: UndraWriter) {
            w.writeU8(level)
            w.writeStr(target)
            w.writeStr(message)
        }

        public companion object {
            /** Reads a log record from [r]. */
            public fun decode(r: UndraReader): Log {
                val level = r.readU8()
                val target = r.readStr()
                return Log(level, target, r.readStr())
            }

            /** Decodes a whole log message. */
            public fun decode(bytes: ByteArray): Log = decodeWhole(bytes) { decode(it) }
        }
    }

    /**
     * host to core (envelope kind TIMER_FIRED): `timer_id u32`.
     *
     * @property timerId the timer that came due.
     */
    public data class TimerFired(val timerId: UInt) : Payload {
        override fun encode(w: UndraWriter) {
            w.writeU32(timerId)
        }

        public companion object {
            /** Reads a timer-fired notice from [r]. */
            public fun decode(r: UndraReader): TimerFired = TimerFired(r.readU32())

            /** Decodes a whole timer-fired message. */
            public fun decode(bytes: ByteArray): TimerFired = decodeWhole(bytes) { decode(it) }
        }
    }

    // ---- Snapshot (§5.9) ----------------------------------------------------------------------------

    /**
     * core to host (envelope kind SNAPSHOT), and host to core to restore it (kind RESTORE, same
     * layout): layout 2 of SPEC 5.9 (ADR-037), all little-endian,
     *
     * ```text
     * count u32, generation_floor u64,
     * schema_hash u64,
     * type_count u32, types × { type_id u32, fingerprint u64 },
     * description_len u32, description (UTF-8),
     * count × { handle u64, type_id u32, signal_count u32, signals × { signal_id u32, len u32, value } }
     * ```
     *
     * where `count` is the number of stores. Computed signals are excluded. A host treats a snapshot as
     * opaque bytes and passes it back to [dev.undra.runtime.UndraCore.restore]; this class exists so tools
     * and tests can look inside.
     *
     * [decode] refuses a store whose type is not in [types] ([WireException.InvalidTag]), a type listed
     * twice ([WireException.DuplicateKey]) and a description that is not UTF-8 ([WireException.InvalidUtf8]),
     * so a snapshot in the layout before ADR-037 (`count, generation_floor, stores`) fails with a typed error
     * instead of decoding as something else. [encode] writes what it is given, valid or not.
     *
     * @property generationFloor the highest handle generation the core had issued when the snapshot
     *   was taken (a `u64` since ADR-040: generations are 40 bits). A restore resumes the core's generation counter above it, so no handle issued before
     *   the snapshot (or between it and the restore) is issued again to another object (ADR-022). Opaque
     *   to the host: pass it back unchanged.
     * @property schemaHash the schema hash of the core that took the snapshot.
     * @property types each store type of the snapshot, once, with the fingerprint of its signals.
     * @property description the canonical JSON description of the store types' signals and the records
     *   and enums they reach, which a build whose types changed migrates the values by (ADR-037). Opaque
     *   to hosts.
     * @property stores every snapshotted store.
     */
    public data class Snapshot(
        val generationFloor: ULong,
        val schemaHash: ULong,
        val types: List<StoreType>,
        val description: String,
        val stores: List<Store>,
    ) : Payload {

        /** The fingerprint [types] records for the store type [typeId], or `null` if it is not listed. */
        public fun fingerprint(typeId: UInt): ULong? = types.firstOrNull { it.typeId == typeId }?.fingerprint

        /**
         * The identity of one store type in a [Snapshot] (ADR-037).
         *
         * @property typeId the store type's id, `fnv1a32("<TypeName>")`.
         * @property fingerprint `fnv1a64` of the canonical closure of the store's non-computed signals when
         *   the snapshot was taken; a restore whose build has the same fingerprint decodes the values as they are.
         */
        public data class StoreType(val typeId: UInt, val fingerprint: ULong)

        /**
         * One store of a [Snapshot].
         *
         * @property handle the store's handle, re-issued unchanged on restore.
         * @property typeId the store's type id; listed in [Snapshot.types].
         * @property signals the store's persisted signals.
         */
        public data class Store(val handle: Handle, val typeId: UInt, val signals: List<Signal>)

        /**
         * One persisted signal value.
         *
         * @property signalId the signal's index within the store.
         * @property value the encoded signal value.
         */
        public class Signal(public val signalId: UInt, public val value: ByteArray) {
            override fun equals(other: Any?): Boolean =
                this === other || (other is Signal && signalId == other.signalId && value.contentEquals(other.value))

            override fun hashCode(): Int = 31 * signalId.hashCode() + value.contentHashCode()

            override fun toString(): String = "Signal(signalId=$signalId, value=${value.size}B)"
        }

        override fun encode(w: UndraWriter) {
            w.writeLen(stores.size)
            w.writeU64(generationFloor)
            w.writeU64(schemaHash)
            w.writeLen(types.size)
            for (t in types) {
                w.writeU32(t.typeId)
                w.writeU64(t.fingerprint)
            }
            w.writeStr(description)
            for (s in stores) {
                w.writeI64(s.handle.raw)
                w.writeU32(s.typeId)
                w.writeLen(s.signals.size)
                for (sig in s.signals) {
                    w.writeU32(sig.signalId)
                    w.writeBytes(sig.value)
                }
            }
        }

        public companion object {
            /** Smallest encoded store: handle 8 + type_id 4 + signal_count 4. */
            private const val MIN_STORE_BYTES = 16

            /** Smallest encoded signal: signal_id 4 + len 4. */
            private const val MIN_SIGNAL_BYTES = 8

            /** An encoded store type: type_id 4 + fingerprint 8. */
            private const val TYPE_BYTES = 12

            /** What [WireException.InvalidTag] names for a store whose type is not in the type table (the Rust decoder's text). */
            public const val UNLISTED_TYPE: String = "Snapshot store type (not in the type table)"

            /**
             * Reads a snapshot from [r], copying every signal value.
             *
             * @throws WireException.InvalidTag (type [UNLISTED_TYPE], at the store's offset) for a store whose type is
             *   not listed; [WireException.DuplicateKey] (at the entry's offset) for a type listed twice;
             *   [WireException.InvalidUtf8] for a description that is not UTF-8; and the reader's own failures for a
             *   count the input cannot hold or a truncated input.
             */
            public fun decode(r: UndraReader): Snapshot {
                val storeCount = r.readLen(MIN_STORE_BYTES)
                val generationFloor = r.readU64()
                val schemaHash = r.readU64()
                val typeCount = r.readLen(TYPE_BYTES)
                val types = ArrayList<StoreType>(typeCount)
                val listed = HashSet<UInt>()
                for (i in 0 until typeCount) {
                    val at = r.position
                    val typeId = r.readU32()
                    val fingerprint = r.readU64()
                    if (!listed.add(typeId)) throw WireException.DuplicateKey(at)
                    types.add(StoreType(typeId, fingerprint))
                }
                val description = r.readStr()
                val stores = ArrayList<Store>(storeCount)
                for (i in 0 until storeCount) {
                    val at = r.position
                    val handle = Handle(r.readI64())
                    val typeId = r.readU32()
                    val signalCount = r.readLen(MIN_SIGNAL_BYTES)
                    val signals = ArrayList<Signal>(signalCount)
                    for (j in 0 until signalCount) {
                        val signalId = r.readU32()
                        signals.add(Signal(signalId, r.readBytes()))
                    }
                    if (typeId !in listed) throw WireException.InvalidTag(typeId, at, UNLISTED_TYPE)
                    stores.add(Store(handle, typeId, signals))
                }
                return Snapshot(generationFloor, schemaHash, types, description, stores)
            }

            /** Decodes a whole snapshot message. */
            public fun decode(bytes: ByteArray): Snapshot = decodeWhole(bytes) { decode(it) }
        }
    }
}

private val EMPTY = ByteArray(0)

/** Decodes with [decode] and requires that the whole of [bytes] was consumed. */
private inline fun <T> decodeWhole(bytes: ByteArray, decode: (UndraReader) -> T): T {
    val r = UndraReader(bytes)
    val v = decode(r)
    r.finish()
    return v
}
