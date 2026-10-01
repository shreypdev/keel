package dev.undra.runtime.wire

/**
 * One operation of a keyed patch (SPEC §3.8) on a host-side `List<T>`. Indices are unsigned 32-bit
 * on the wire and refer to the list as it is after the previous op.
 */
public sealed interface PatchOp<out T> {
    /** Inserts [item] so that it ends up at [index] (0 up to and including the list size). */
    public data class Insert<out T>(val index: UInt, val item: T) : PatchOp<T>

    /** Removes the item at [index]. */
    public data class Remove(val index: UInt) : PatchOp<Nothing>

    /** Replaces the item at [index] with [item]. */
    public data class Update<out T>(val index: UInt, val item: T) : PatchOp<T>

    /** Removes the item at [from] and re-inserts it so that it ends up at [to]. */
    public data class Move(val from: UInt, val to: UInt) : PatchOp<Nothing>

    /** Removes every item. */
    public data object Clear : PatchOp<Nothing>
}

/**
 * Decoding, encoding and applying keyed patches for `Signal<Vec<T>>` fields declared with
 * `#[undra(key = "field")]` (SPEC §3.8).
 *
 * The core computes the patch; the host applies it with [applyPatch] to its current list and
 * publishes the resulting list. Wire layout: `count u32`, then per op an `op u8` tag (0 Insert,
 * 1 Remove, 2 Update, 3 Move, 4 Clear) followed by `index u32` (+ item) or `from u32, to u32`.
 */
public object KeyedPatch {
    private const val OP_INSERT = 0
    private const val OP_REMOVE = 1
    private const val OP_UPDATE = 2
    private const val OP_MOVE = 3
    private const val OP_CLEAR = 4

    /**
     * Reads a patch from [r], decoding each item with [itemCodec]. Does not require the reader to be
     * exhausted afterwards (call [UndraReader.finish] when the patch is the whole value).
     *
     * @throws WireException.LengthTooLarge if the op count cannot fit in the remaining bytes.
     * @throws WireException.InvalidTag if an op tag is unknown.
     */
    public fun <T> decodePatch(r: UndraReader, itemCodec: UndraCodec<T>): List<PatchOp<T>> {
        val count = r.readLen()
        val ops = ArrayList<PatchOp<T>>(count)
        for (i in 0 until count) {
            val at = r.position
            when (val tag = r.readU8().toInt()) {
                OP_INSERT -> {
                    val index = r.readU32()
                    ops.add(PatchOp.Insert(index, itemCodec.decode(r)))
                }
                OP_REMOVE -> ops.add(PatchOp.Remove(r.readU32()))
                OP_UPDATE -> {
                    val index = r.readU32()
                    ops.add(PatchOp.Update(index, itemCodec.decode(r)))
                }
                OP_MOVE -> {
                    val from = r.readU32()
                    ops.add(PatchOp.Move(from, r.readU32()))
                }
                OP_CLEAR -> ops.add(PatchOp.Clear)
                else -> throw WireException.InvalidTag(tag.toUInt(), at, "PatchOp")
            }
        }
        return ops
    }

    /**
     * Decodes a patch that spans all of [bytes].
     *
     * @throws WireException.TrailingBytes if bytes are left after the patch.
     */
    public fun <T> decodePatch(bytes: ByteArray, itemCodec: UndraCodec<T>): List<PatchOp<T>> {
        val r = UndraReader(bytes)
        val ops = decodePatch(r, itemCodec)
        r.finish()
        return ops
    }

    /** Writes [ops] to [w], encoding each item with [itemCodec]. */
    public fun <T> encodePatch(w: UndraWriter, ops: List<PatchOp<T>>, itemCodec: UndraCodec<T>) {
        w.writeLen(ops.size)
        for (op in ops) {
            when (op) {
                is PatchOp.Insert -> {
                    w.writeU8(OP_INSERT.toUByte())
                    w.writeU32(op.index)
                    itemCodec.encode(w, op.item)
                }
                is PatchOp.Remove -> {
                    w.writeU8(OP_REMOVE.toUByte())
                    w.writeU32(op.index)
                }
                is PatchOp.Update -> {
                    w.writeU8(OP_UPDATE.toUByte())
                    w.writeU32(op.index)
                    itemCodec.encode(w, op.item)
                }
                is PatchOp.Move -> {
                    w.writeU8(OP_MOVE.toUByte())
                    w.writeU32(op.from)
                    w.writeU32(op.to)
                }
                PatchOp.Clear -> w.writeU8(OP_CLEAR.toUByte())
            }
        }
    }

    /** Encodes [ops] into a fresh byte array. */
    public fun <T> encodePatch(ops: List<PatchOp<T>>, itemCodec: UndraCodec<T>): ByteArray {
        val w = UndraWriter()
        encodePatch(w, ops, itemCodec)
        return w.toByteArray()
    }

    /**
     * Applies [ops] in order to [list] and returns the result as a **new** list; [list] is not
     * modified. Every index is bounds-checked against the list as it stands at that op:
     *  - `Insert`: `0 <= index <= size`
     *  - `Remove`, `Update`: `0 <= index < size`
     *  - `Move`: `from < size` and `to < size` (the item is removed first, then re-inserted at `to`)
     *  - `Clear`: always valid.
     *
     * @throws WireException.PatchOutOfBounds if an index is out of range, meaning the host's list
     *   has diverged from the core's.
     */
    public fun <T> applyPatch(list: List<T>, ops: List<PatchOp<T>>): List<T> {
        val out = ArrayList<T>(list)
        for (opIndex in ops.indices) {
            when (val op = ops[opIndex]) {
                is PatchOp.Insert -> {
                    if (op.index > out.size.toUInt()) throw outOfBounds(opIndex, "Insert", op.index, out.size)
                    out.add(op.index.toInt(), op.item)
                }
                is PatchOp.Remove -> {
                    if (op.index >= out.size.toUInt()) throw outOfBounds(opIndex, "Remove", op.index, out.size)
                    out.removeAt(op.index.toInt())
                }
                is PatchOp.Update -> {
                    if (op.index >= out.size.toUInt()) throw outOfBounds(opIndex, "Update", op.index, out.size)
                    out[op.index.toInt()] = op.item
                }
                is PatchOp.Move -> {
                    if (op.from >= out.size.toUInt()) throw outOfBounds(opIndex, "Move", op.from, out.size)
                    if (op.to >= out.size.toUInt()) throw outOfBounds(opIndex, "Move", op.to, out.size)
                    val item = out.removeAt(op.from.toInt())
                    out.add(op.to.toInt(), item)
                }
                PatchOp.Clear -> out.clear()
            }
        }
        return out
    }

    private fun outOfBounds(opIndex: Int, op: String, index: UInt, size: Int): WireException =
        WireException.PatchOutOfBounds(opIndex, op, index, size)
}
