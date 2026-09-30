package dev.undra.runtime.wire

/**
 * The transport frame of SPEC §3.2, used on WebSocket and Worker transports (in-process calls pass
 * the payload alone).
 *
 * ```
 * magic    4 bytes  "UNDRA"
 * version  u16      1
 * schema   u64      schema hash of the core that produced / expects this message
 * kind     u8       see [Kind]
 * seq      u32      per-direction, monotonically increasing
 * len      u32      payload length
 * payload  len bytes
 * ```
 *
 * The header is [HEADER_LEN] (23) bytes. An [Envelope] compares by content, including [payload].
 *
 * @property kind what the payload is.
 * @property seq per-direction sequence number, for ordering and debugging.
 * @property schemaHash schema hash of the core that produced or expects this message.
 * @property payload the message body, encoded as described for [kind] (see [Payloads]). Owned by the
 *   envelope; do not mutate it.
 */
public class Envelope(
    public val kind: Kind,
    public val seq: UInt,
    public val schemaHash: ULong,
    public val payload: ByteArray,
) {

    /** The message kinds of SPEC §3.2, with their wire codes. */
    public enum class Kind(
        /** The `u8` written in the envelope header. */
        public val code: UByte,
    ) {
        /** host to core: [Payloads.Call]. */
        CALL(1u),

        /** core to host: [Payloads.Reply]. */
        REPLY(2u),

        /** core to host: [Payloads.ChangeSet]. */
        CHANGE_SET(3u),

        /** core to host: [Payloads.PortCall]. */
        PORT_CALL(4u),

        /** host to core: [Payloads.PortReply]. */
        PORT_REPLY(5u),

        /** host to core: [Payloads.Cancel]. */
        CANCEL(6u),

        /** host to core: [Payloads.StreamCredit]. */
        STREAM_CREDIT(7u),

        /** core to host: [Payloads.StreamItem]. */
        STREAM_ITEM(8u),

        /** host to core: [Payloads.Observe]. */
        OBSERVE(9u),

        /** host to core: [Payloads.Release]. */
        RELEASE(10u),

        /** host to core: [Payloads.Event]. */
        EVENT(11u),

        /** both directions: [Payloads.Hello]. */
        HELLO(12u),

        /** core to host: [Payloads.Log]. */
        LOG(13u),

        /** host to core: [Payloads.TimerFired]. */
        TIMER_FIRED(14u),

        /** core to host: [Payloads.Snapshot]. */
        SNAPSHOT(15u),

        /** host to core: [Payloads.Snapshot] (same layout, SPEC §5.9). */
        RESTORE(16u);

        public companion object {
            private val byCode: Array<Kind?> = arrayOfNulls<Kind>(17).also { table ->
                for (k in entries) table[k.code.toInt()] = k
            }

            /**
             * The kind with wire code [b].
             *
             * @param at offset of the byte for the error report; `-1` when it is not known.
             * @throws WireException.InvalidTag if [b] is not a kind code (0 and anything above 16).
             */
            public fun fromByte(b: UByte, at: Int = -1): Kind {
                val i = b.toInt()
                return (if (i < byCode.size) byCode[i] else null)
                    ?: throw WireException.InvalidTag(i.toUInt(), at, "Envelope.Kind")
            }
        }
    }

    /** Encodes this envelope; see [Envelope.encode]. */
    public fun encode(): ByteArray = encode(kind, seq, schemaHash, payload)

    override fun equals(other: Any?): Boolean =
        this === other || (
            other is Envelope && kind == other.kind && seq == other.seq &&
                schemaHash == other.schemaHash && payload.contentEquals(other.payload)
            )

    override fun hashCode(): Int {
        var h = kind.hashCode()
        h = 31 * h + seq.hashCode()
        h = 31 * h + schemaHash.hashCode()
        return 31 * h + payload.contentHashCode()
    }

    override fun toString(): String = "Envelope(kind=$kind, seq=$seq, schemaHash=$schemaHash, payload=${payload.size}B)"

    public companion object {
        /** Size of the fixed header in bytes: 4 magic + 2 version + 8 schema + 1 kind + 4 seq + 4 len. */
        public const val HEADER_LEN: Int = 23

        /** The only envelope version this runtime speaks. */
        public const val VERSION: UShort = 1u

        private val MAGIC: ByteArray = byteArrayOf('K'.code.toByte(), 'E'.code.toByte(), 'E'.code.toByte(), 'L'.code.toByte())

        /** The magic as one little-endian `i32` (what reading the first four bytes yields). */
        private const val MAGIC_LE_INT: Int = 0x4C45454B

        /**
         * Builds an envelope frame around [payload].
         *
         * @throws IllegalArgumentException if the frame would exceed the maximum array size.
         */
        public fun encode(kind: Kind, seq: UInt, schemaHash: ULong, payload: ByteArray): ByteArray {
            require(payload.size <= Int.MAX_VALUE - 8 - HEADER_LEN) {
                "payload of ${payload.size} bytes is too large for an envelope"
            }
            val w = UndraWriter(HEADER_LEN + payload.size)
            w.writeRaw(MAGIC)
            w.writeU16(VERSION)
            w.writeU64(schemaHash)
            w.writeU8(kind.code)
            w.writeU32(seq)
            w.writeLen(payload.size)
            w.writeRaw(payload)
            return w.takeArray()
        }

        /**
         * Parses exactly one envelope that spans all of [bytes] (one WebSocket message).
         *
         * @param expectedSchema when non-null, the schema hash the message must carry. It is checked
         *   as soon as the header field is read, before the rest of the frame is trusted.
         * @throws WireException.UnexpectedEof if [bytes] is shorter than the header.
         * @throws WireException.BadMagic if the magic is not `UNDRA`.
         * @throws WireException.UnsupportedVersion if the version is not [VERSION].
         * @throws WireException.SchemaMismatch if [expectedSchema] is given and differs.
         * @throws WireException.InvalidTag if the kind code is unknown.
         * @throws WireException.LengthTooLarge if `len` exceeds the bytes that follow the header.
         * @throws WireException.TrailingBytes if bytes follow the payload.
         */
        public fun decode(bytes: ByteArray, expectedSchema: ULong? = null): Envelope {
            val r = UndraReader(bytes)
            if (r.readI32() != MAGIC_LE_INT) throw WireException.BadMagic(hex(bytes, 4))
            val version = r.readU16()
            if (version != VERSION) throw WireException.UnsupportedVersion(version)
            val schema = r.readU64()
            if (expectedSchema != null && schema != expectedSchema) throw WireException.SchemaMismatch(expectedSchema, schema)
            val kindAt = r.position
            val kind = Kind.fromByte(r.readU8(), kindAt)
            val seq = r.readU32()
            val payload = r.readRaw(r.readLen())
            r.finish()
            return Envelope(kind, seq, schema, payload)
        }

        private fun hex(bytes: ByteArray, count: Int): String {
            val sb = StringBuilder(count * 2)
            for (i in 0 until minOf(count, bytes.size)) {
                val b = bytes[i].toInt() and 0xFF
                sb.append(HEX_DIGITS[b shr 4]).append(HEX_DIGITS[b and 0xF])
            }
            return sb.toString()
        }

        private const val HEX_DIGITS = "0123456789abcdef"
    }
}
