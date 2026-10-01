//! Name helpers: case conversion and the FNV hash used for compile-time collision checks.
//!
//! The stable ids themselves are computed by the *generated code* through
//! `undra_meta::ids` (so there is a single implementation of the formulas); the macro
//! only needs the hash to detect two methods of one type that collide.

/// FNV-1a 32-bit over the UTF-8 bytes of `s` (SPEC 1.1). Mirrors `undra_meta::ids::fnv1a32`.
pub(crate) fn fnv1a32(s: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in s.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// `SecureStore` -> `secure_store`, `HTTPClient` -> `http_client`, `Http` -> `http`.
pub(crate) fn snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let prev = i.checked_sub(1).map(|p| chars[p]);
            let next = chars.get(i + 1).copied();
            let boundary = match prev {
                Some(p) if p.is_lowercase() || p.is_ascii_digit() => true,
                Some(p) if p.is_uppercase() => next.is_some_and(char::is_lowercase),
                _ => false,
            };
            if boundary && !out.ends_with('_') {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// `add_todo` -> `AddTodo`, `todos` -> `Todos`, `get2fa` -> `Get2fa`.
pub(crate) fn pascal_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for part in name.split('_').filter(|p| !p.is_empty()) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// Strips a raw-identifier prefix: `r#type` -> `type`.
pub(crate) fn unraw(ident: &syn::Ident) -> String {
    let s = ident.to_string();
    match s.strip_prefix("r#") {
        Some(rest) => rest.to_owned(),
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_matches_reference_vectors() {
        assert_eq!(fnv1a32(""), 0x811c_9dc5);
        assert_eq!(fnv1a32("a"), 0xe40c_292c);
        assert_eq!(fnv1a32("foobar"), 0xbf9c_f968);
        // The shared wire vector from undra-meta.
        assert_eq!(fnv1a32("Calculator.add"), 2_353_348_832);
    }

    #[test]
    fn snake_case_handles_acronyms_and_digits() {
        assert_eq!(snake_case("Http"), "http");
        assert_eq!(snake_case("SecureStore"), "secure_store");
        assert_eq!(snake_case("HTTPClient"), "http_client");
        assert_eq!(snake_case("Kv"), "kv");
        assert_eq!(snake_case("Base64Codec"), "base64_codec");
        assert_eq!(snake_case("already_snake"), "already_snake");
        assert_eq!(snake_case("URL"), "url");
    }

    #[test]
    fn pascal_case_joins_parts() {
        assert_eq!(pascal_case("todos"), "Todos");
        assert_eq!(pascal_case("add_todo"), "AddTodo");
        assert_eq!(pascal_case("get_user_2"), "GetUser2");
        assert_eq!(pascal_case("_leading"), "Leading");
        assert_eq!(pascal_case("Already"), "Already");
    }

    #[test]
    fn unraw_strips_prefix() {
        let ident: syn::Ident = syn::parse_str("r#type").unwrap();
        assert_eq!(unraw(&ident), "type");
        let ident: syn::Ident = syn::parse_str("plain").unwrap();
        assert_eq!(unraw(&ident), "plain");
    }
}
