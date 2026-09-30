// Keyed list patches (docs/SPEC.md section 3.8).
//
// Wire layout:
//   count  u32
//   ops    count x { op u8, ... }
//      0 Insert  { index u32, item T }
//      1 Remove  { index u32 }
//      2 Update  { index u32, item T }
//      3 Move    { from u32, to u32 }
//      4 Clear   { }
//
// Ops apply sequentially to the host's current list; indices refer to the list as it is after
// the previous op.

/// One edit of a keyed list patch.
public enum PatchOp<T> {
    /// Inserts `item` so that it ends up at `index` (`index == count` appends).
    case insert(index: UInt32, item: T)
    /// Removes the element at `index`.
    case remove(index: UInt32)
    /// Replaces the element at `index`.
    case update(index: UInt32, item: T)
    /// Removes the element at `from` and re-inserts it so that it ends up at `to`
    /// (an index into the list after the move).
    case move(from: UInt32, to: UInt32)
    /// Removes every element.
    case clear
}

extension PatchOp: Sendable where T: Sendable {}

extension PatchOp: Equatable where T: Equatable {}

extension PatchOp: Hashable where T: Hashable {}

extension PatchOp: KeelCodec where T: KeelCodec {
    public static func keelDecode(_ r: inout KeelReader) throws -> PatchOp<T> {
        let at = r.position
        let tag = try r.readU8()
        switch tag {
        case 0:
            let index = try r.readU32()
            let item = try T.keelDecode(&r)
            return .insert(index: index, item: item)
        case 1:
            let index = try r.readU32()
            return .remove(index: index)
        case 2:
            let index = try r.readU32()
            let item = try T.keelDecode(&r)
            return .update(index: index, item: item)
        case 3:
            let from = try r.readU32()
            let to = try r.readU32()
            return .move(from: from, to: to)
        case 4:
            return .clear
        default:
            throw WireError.invalidTag(tag: UInt32(tag), at: at, type: "PatchOp")
        }
    }

    public func keelEncode(_ w: inout KeelWriter) {
        switch self {
        case .insert(let index, let item):
            w.writeU8(0)
            w.writeU32(index)
            item.keelEncode(&w)
        case .remove(let index):
            w.writeU8(1)
            w.writeU32(index)
        case .update(let index, let item):
            w.writeU8(2)
            w.writeU32(index)
            item.keelEncode(&w)
        case .move(let from, let to):
            w.writeU8(3)
            w.writeU32(from)
            w.writeU32(to)
        case .clear:
            w.writeU8(4)
        }
    }
}

/// A patch op that cannot be applied to the list it was applied to.
public enum PatchError: Error, Sendable, Equatable, CustomStringConvertible {
    /// Op number `opIndex` (zero-based, in patch order) refers to `index`, which is out of
    /// range for a list of `count` elements at that point of the patch.
    case indexOutOfBounds(opIndex: Int, index: UInt32, count: Int)

    public var description: String {
        switch self {
        case .indexOutOfBounds(let opIndex, let index, let count):
            return "patch op \(opIndex) refers to index \(index), out of range for a list of \(count) element(s)"
        }
    }
}

/// Decodes a keyed patch (`count u32` followed by the ops) from `r`.
///
/// The element type is inferred from the context:
///
/// ```swift
/// let ops: [PatchOp<Todo>] = try decodePatch(&reader)
/// ```
public func decodePatch<T: KeelCodec>(_ r: inout KeelReader) throws -> [PatchOp<T>] {
    let count = try r.readLen()
    var ops: [PatchOp<T>] = []
    ops.reserveCapacity(Swift.min(count, keelMaxPreallocatedElements))
    var index = 0
    while index < count {
        let op = try PatchOp<T>.keelDecode(&r)
        ops.append(op)
        index += 1
    }
    return ops
}

/// Encodes a keyed patch: `count u32` followed by the ops.
public func encodePatch<T: KeelCodec>(_ ops: [PatchOp<T>], into w: inout KeelWriter) {
    w.writeLen(ops.count)
    for op in ops {
        op.keelEncode(&w)
    }
}

/// Applies `ops` to `list` in order (docs/SPEC.md section 3.8).
///
/// Every op is bounds-checked against the list as it will be at that point. Validation runs over
/// the whole patch before anything is modified, so if this throws `PatchError` the list is
/// unchanged. The caller should then treat its mirror as out of sync and request a full value.
public func applyPatch<T>(_ ops: [PatchOp<T>], to list: inout [T]) throws {
    // Pass 1: bounds depend only on the list length, so simulate the length.
    var length = list.count
    var opIndex = 0
    for op in ops {
        switch op {
        case .insert(let index, _):
            if UInt64(index) > UInt64(length) {
                throw PatchError.indexOutOfBounds(opIndex: opIndex, index: index, count: length)
            }
            length += 1
        case .remove(let index):
            if UInt64(index) >= UInt64(length) {
                throw PatchError.indexOutOfBounds(opIndex: opIndex, index: index, count: length)
            }
            length -= 1
        case .update(let index, _):
            if UInt64(index) >= UInt64(length) {
                throw PatchError.indexOutOfBounds(opIndex: opIndex, index: index, count: length)
            }
        case .move(let from, let to):
            if UInt64(from) >= UInt64(length) {
                throw PatchError.indexOutOfBounds(opIndex: opIndex, index: from, count: length)
            }
            if UInt64(to) >= UInt64(length) {
                throw PatchError.indexOutOfBounds(opIndex: opIndex, index: to, count: length)
            }
        case .clear:
            length = 0
        }
        opIndex += 1
    }
    // Pass 2: every index is now known to be in range, so the conversions and mutations are safe.
    for op in ops {
        switch op {
        case .insert(let index, let item):
            list.insert(item, at: Int(index))
        case .remove(let index):
            list.remove(at: Int(index))
        case .update(let index, let item):
            list[Int(index)] = item
        case .move(let from, let to):
            let item = list.remove(at: Int(from))
            list.insert(item, at: Int(to))
        case .clear:
            list.removeAll()
        }
    }
}
