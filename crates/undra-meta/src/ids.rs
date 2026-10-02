//! Stable identifiers (SPEC §1.1).
//!
//! Every id is derived from a name with FNV-1a, so the macros can compute them
//! at compile time and every platform agrees without a registry lookup. All
//! functions here are `const fn`, so they can be used to initialise `static`
//! metadata:
//!
//! ```
//! use undra_meta::ids;
//!
//! const ADD: u32 = ids::method_id("Calculator", "add");
//! assert_eq!(ADD, 2_353_348_832);
//! assert_eq!(ids::fnv1a32("Calculator.add"), ADD);
//! ```
//!
//! Composite names (`"<Type>.<method>"`, `"port.<Trait>"`, ...) are hashed
//! part by part, so no string is ever allocated or concatenated.

/// FNV-1a 32-bit offset basis.
const FNV32_OFFSET: u32 = 0x811c_9dc5;
/// FNV-1a 32-bit prime.
const FNV32_PRIME: u32 = 0x0100_0193;
/// FNV-1a 64-bit offset basis.
const FNV64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64-bit prime.
const FNV64_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The `signal_id` that addresses every signal of a store at once (SPEC §1.1).
pub const ALL_SIGNALS: u32 = u32::MAX;

/// Folds `bytes` into a running 32-bit FNV-1a state.
const fn fold32(mut hash: u32, bytes: &[u8]) -> u32 {
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u32;
        hash = hash.wrapping_mul(FNV32_PRIME);
        i += 1;
    }
    hash
}

/// Folds `bytes` into a running 64-bit FNV-1a state.
const fn fold64(mut hash: u64, bytes: &[u8]) -> u64 {
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(FNV64_PRIME);
        i += 1;
    }
    hash
}

/// FNV-1a (32-bit) over the UTF-8 bytes of `s`.
///
/// ```
/// assert_eq!(undra_meta::ids::fnv1a32("Calculator.add"), 2_353_348_832);
/// assert_eq!(undra_meta::ids::fnv1a32(""), 0x811c_9dc5);
/// ```
#[must_use]
pub const fn fnv1a32(s: &str) -> u32 {
    fold32(FNV32_OFFSET, s.as_bytes())
}

/// FNV-1a (64-bit) over raw bytes. The schema hash is `fnv1a64` of the
/// canonical schema JSON.
///
/// ```
/// assert_eq!(undra_meta::ids::fnv1a64(b"undra"), 12_206_477_163_874_244_763);
/// assert_eq!(undra_meta::ids::fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
/// ```
#[must_use]
pub const fn fnv1a64(bytes: &[u8]) -> u64 {
    fold64(FNV64_OFFSET, bytes)
}

/// FNV-1a (64-bit) over the UTF-8 bytes of `s`; convenience over [`fnv1a64`].
#[must_use]
pub const fn fnv1a64_str(s: &str) -> u64 {
    fnv1a64(s.as_bytes())
}

/// The id of a record, enum, error or object: `fnv1a32("<TypeName>")`.
///
/// `name` is the Rust identifier without a module path.
#[must_use]
pub const fn type_id(name: &str) -> u32 {
    fnv1a32(name)
}

/// The id of an object method or constructor: `fnv1a32("<TypeName>.<method>")`.
///
/// ```
/// assert_eq!(undra_meta::ids::method_id("Calculator", "add"), 2_353_348_832);
/// ```
#[must_use]
pub const fn method_id(type_name: &str, method: &str) -> u32 {
    let hash = fold32(FNV32_OFFSET, type_name.as_bytes());
    let hash = fold32(hash, b".");
    fold32(hash, method.as_bytes())
}

/// The id of a free function: `fnv1a32("fn.<name>")`.
#[must_use]
pub const fn function_id(name: &str) -> u32 {
    let hash = fold32(FNV32_OFFSET, b"fn.");
    fold32(hash, name.as_bytes())
}

/// The id of a port: `fnv1a32("port.<TraitName>")`.
#[must_use]
pub const fn port_id(trait_name: &str) -> u32 {
    let hash = fold32(FNV32_OFFSET, b"port.");
    fold32(hash, trait_name.as_bytes())
}

/// The id of a port method: `fnv1a32("<TraitName>.<method>")`.
///
/// This is deliberately the same formula as [`method_id`]; ports and objects
/// live in separate id spaces (a port call carries a `port_id` alongside).
#[must_use]
pub const fn port_method_id(trait_name: &str, method: &str) -> u32 {
    method_id(trait_name, method)
}

/// The reserved method every callback port answers to release one instance:
/// `fnv1a32("<Trait>.__release")` (ADR-041). Fire-and-forget, arguments `instance u64`.
pub const CALLBACK_RELEASE: &str = "__release";

/// The reserved method every callback port answers to cancel an async call:
/// `fnv1a32("<Trait>.__cancel")` (ADR-041). Fire-and-forget, arguments `instance u64,
/// port_call_id u32`.
pub const CALLBACK_CANCEL: &str = "__cancel";

/// The id of a callback port's `__release` method.
///
/// ```
/// use undra_meta::ids;
/// assert_eq!(ids::callback_release_id("UploadListener"), ids::fnv1a32("UploadListener.__release"));
/// ```
#[must_use]
pub const fn callback_release_id(trait_name: &str) -> u32 {
    port_method_id(trait_name, CALLBACK_RELEASE)
}

/// The id of a callback port's `__cancel` method.
#[must_use]
pub const fn callback_cancel_id(trait_name: &str) -> u32 {
    port_method_id(trait_name, CALLBACK_CANCEL)
}

/// The id of a query: `fnv1a32("query.<fn_name>")`.
#[must_use]
pub const fn query_id(name: &str) -> u32 {
    let hash = fold32(FNV32_OFFSET, b"query.");
    fold32(hash, name.as_bytes())
}

/// The id of a mutation: `fnv1a32("mutation.<fn_name>")`.
#[must_use]
pub const fn mutation_id(name: &str) -> u32 {
    let hash = fold32(FNV32_OFFSET, b"mutation.");
    fold32(hash, name.as_bytes())
}

// Compile-time proof that the helpers are usable in const contexts and agree
// with the shared wire vectors (contract-tests/wire-vectors.json).
const _: () = {
    assert!(fnv1a32("Calculator.add") == 2_353_348_832);
    assert!(method_id("Calculator", "add") == 2_353_348_832);
    assert!(fnv1a64_str("undra") == 12_206_477_163_874_244_763);
    assert!(function_id("greet") == fnv1a32("fn.greet"));
    assert!(port_id("Clock") == fnv1a32("port.Clock"));
    assert!(query_id("todos") == fnv1a32("query.todos"));
    assert!(mutation_id("add_todo") == fnv1a32("mutation.add_todo"));
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv32_reference_vectors() {
        // Published FNV-1a test vectors (Fowler/Noll/Vo reference suite).
        assert_eq!(fnv1a32(""), 0x811c_9dc5);
        assert_eq!(fnv1a32("a"), 0xe40c_292c);
        assert_eq!(fnv1a32("foobar"), 0xbf9c_f968);
    }

    #[test]
    fn fnv64_reference_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn shared_wire_vectors() {
        assert_eq!(fnv1a32("Calculator.add"), 2_353_348_832);
        assert_eq!(fnv1a64(b"undra"), 12_206_477_163_874_244_763);
        assert_eq!(fnv1a64_str("undra"), fnv1a64(b"undra"));
    }

    #[test]
    fn composite_ids_equal_hash_of_concatenation() {
        assert_eq!(type_id("Todo"), fnv1a32("Todo"));
        assert_eq!(method_id("Todo", "rename"), fnv1a32("Todo.rename"));
        assert_eq!(function_id("greet"), fnv1a32("fn.greet"));
        assert_eq!(port_id("Http"), fnv1a32("port.Http"));
        assert_eq!(port_method_id("Http", "request"), fnv1a32("Http.request"));
        assert_eq!(query_id("todos"), fnv1a32("query.todos"));
        assert_eq!(mutation_id("add_todo"), fnv1a32("mutation.add_todo"));
    }

    #[test]
    fn id_spaces_are_distinct_for_the_same_name() {
        let name = "same";
        let ids = [
            type_id(name),
            function_id(name),
            port_id(name),
            query_id(name),
            mutation_id(name),
        ];
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn hashes_utf8_bytes_not_chars() {
        // "é" is two UTF-8 bytes; make sure we fold bytes, not code points.
        let by_hand = {
            let mut h: u32 = 0x811c_9dc5;
            for b in "é".as_bytes() {
                h ^= u32::from(*b);
                h = h.wrapping_mul(0x0100_0193);
            }
            h
        };
        assert_eq!(fnv1a32("é"), by_hand);
        assert_eq!("é".len(), 2);
    }

    #[test]
    fn usable_in_statics() {
        static ID: u32 = method_id("Unicode\u{1F30A}", "wave");
        assert_eq!(ID, fnv1a32("Unicode\u{1F30A}.wave"));
    }

    #[test]
    fn all_signals_sentinel() {
        assert_eq!(ALL_SIGNALS, u32::MAX);
    }
}
