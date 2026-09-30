// FNV-1a hashes used for stable identifiers (docs/SPEC.md section 1.1).
//
// Generated code embeds precomputed ids; these functions exist so the runtime and its tests can
// recompute them (for example `fnv1a32("Calculator.add")`) and cross-check the generator.

/// FNV-1a over the UTF-8 bytes of `s`, 32-bit (offset basis `0x811c9dc5`, prime `0x01000193`).
///
/// ```swift
/// fnv1a32("Calculator.add")   // 2353348832
/// ```
public func fnv1a32(_ s: String) -> UInt32 {
    var hash: UInt32 = 0x811c_9dc5
    for byte in s.utf8 {
        hash ^= UInt32(byte)
        hash = hash &* 0x0100_0193
    }
    return hash
}

/// FNV-1a over the UTF-8 bytes of `s`, 64-bit (offset basis `0xcbf29ce484222325`,
/// prime `0x100000001b3`).
///
/// ```swift
/// fnv1a64("keel")             // 6367360722358687308
/// ```
public func fnv1a64(_ s: String) -> UInt64 {
    var hash: UInt64 = 0xcbf2_9ce4_8422_2325
    for byte in s.utf8 {
        hash ^= UInt64(byte)
        hash = hash &* 0x0000_0100_0000_01b3
    }
    return hash
}
