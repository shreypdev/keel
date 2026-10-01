//! Compile-time lookup of the key field of a keyed list, for the code `#[undra::store]` emits.
//!
//! `#[undra(key = "id")]` on a `Signal<Vec<Row>>` names a field of `Row`, which a macro cannot
//! see: `Row` is another item. Every `#[undra::api]` record therefore lists its field names
//! (`Row::__UNDRA_FIELDS`, a constant and nothing else); the store asks [`index_of`] for the key
//! in a constant, so a key that names no field is a compile error with a message of Undra's own,
//! listing the fields there are ([`message`]), instead of `rustc`'s "no field `idd` on type
//! `&Row`" (the store reads the field through a type that only exists when the check passed).
//!
//! Everything here is a `const fn`: it runs while the user's crate compiles.
//!
//! ```
//! use undra_meta::keys;
//!
//! const FIELDS: &[&str] = &["id", "title"];
//! assert_eq!(keys::index_of(FIELDS, "title"), 1);
//! assert_eq!(keys::index_of(FIELDS, "idd"), usize::MAX);
//! ```

/// The index of `key` in `fields`, or `usize::MAX` when there is no such field.
#[must_use]
pub const fn index_of(fields: &[&str], key: &str) -> usize {
    let mut index = 0;
    while index < fields.len() {
        if same(fields[index], key) {
            return index;
        }
        index += 1;
    }
    usize::MAX
}

/// Whether two strings are equal (`==` on `&str` is not available in constants).
const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// What a message lists when the type has no fields.
const NO_FIELDS: &str = "no fields (only a record declared with `#[undra::api]` has any)";

/// The list a message shows: ``the fields `id`, `title` `` or [`NO_FIELDS`].
///
/// Written once, as a sequence of pieces, so [`message_len`] and [`message`] agree.
const fn pieces<'a>(fields: &[&'a str], index: usize) -> Piece<'a> {
    if fields.is_empty() {
        return if index == 0 {
            Piece::Text(NO_FIELDS)
        } else {
            Piece::End
        };
    }
    // `the fields ` then `a`, `b`, `c` with ", " between: 1 + 3 pieces per field - 1.
    if index == 0 {
        return Piece::Text("the fields ");
    }
    let slot = index - 1;
    let field = slot / 3;
    if field >= fields.len() {
        return Piece::End;
    }
    match slot % 3 {
        0 => Piece::Text(if field == 0 { "`" } else { ", `" }),
        1 => Piece::Text(fields[field]),
        _ => Piece::Text("`"),
    }
}

/// One piece of the list in a message.
enum Piece<'a> {
    Text(&'a str),
    End,
}

/// The length in bytes of [`message`] for the same arguments.
#[must_use]
pub const fn message_len(before: &str, fields: &[&str], after: &str) -> usize {
    let mut len = before.len() + after.len();
    let mut index = 0;
    loop {
        match pieces(fields, index) {
            Piece::Text(text) => len += text.len(),
            Piece::End => return len,
        }
        index += 1;
    }
}

/// `before`, the fields (``the fields `id`, `title` `` or a note that there are none) and `after`,
/// as `N` bytes (`N` is [`message_len`] of the same arguments).
///
/// A constant cannot format, so the message of a compile error that has to list something is
/// assembled byte by byte, then shown with `panic!("{}", ..)` through [`as_str`].
#[must_use]
pub const fn message<const N: usize>(before: &str, fields: &[&str], after: &str) -> [u8; N] {
    let mut out = [0u8; N];
    let mut at = push(&mut out, 0, before);
    let mut index = 0;
    while let Piece::Text(text) = pieces(fields, index) {
        at = push(&mut out, at, text);
        index += 1;
    }
    let _ = push(&mut out, at, after);
    out
}

/// Copies `text` into `out` at `at` and returns the end.
const fn push<const N: usize>(out: &mut [u8; N], at: usize, text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        out[at + i] = bytes[i];
        i += 1;
    }
    at + bytes.len()
}

/// The bytes of a [`message`] as text (empty if they are not UTF-8, which they cannot be).
#[must_use]
pub const fn as_str(bytes: &[u8]) -> &str {
    match core::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIELDS: &[&str] = &["id", "title", "done"];
    const LEN: usize = message_len("has ", FIELDS, ".");
    const BYTES: [u8; LEN] = message::<LEN>("has ", FIELDS, ".");

    #[test]
    fn index_of_finds_a_field_or_says_there_is_none() {
        assert_eq!(index_of(FIELDS, "id"), 0);
        assert_eq!(index_of(FIELDS, "done"), 2);
        assert_eq!(index_of(FIELDS, "idd"), usize::MAX);
        assert_eq!(index_of(FIELDS, ""), usize::MAX);
        assert_eq!(index_of(&[], "id"), usize::MAX);
        assert_eq!(index_of(FIELDS, "i"), usize::MAX, "a prefix is not a field");
    }

    #[test]
    fn the_message_lists_the_fields_in_order() {
        assert_eq!(as_str(&BYTES), "has the fields `id`, `title`, `done`.");
    }

    #[test]
    fn one_field_and_no_fields() {
        const ONE: &[&str] = &["id"];
        const ONE_LEN: usize = message_len("[", ONE, "]");
        assert_eq!(
            as_str(&message::<ONE_LEN>("[", ONE, "]")),
            "[the fields `id`]"
        );
        const NONE_LEN: usize = message_len("[", &[], "]");
        assert_eq!(
            as_str(&message::<NONE_LEN>("[", &[], "]")),
            "[no fields (only a record declared with `#[undra::api]` has any)]"
        );
    }

    #[test]
    fn the_length_is_the_message_length_for_any_input() {
        for fields in [&["a"][..], &["a", "bb"], &["x", "y", "z", "w"], &[]] {
            let expected: usize = "<>".len()
                + if fields.is_empty() {
                    NO_FIELDS.len()
                } else {
                    "the fields ".len()
                        + fields.iter().map(|f| f.len() + 2).sum::<usize>()
                        + (fields.len() - 1) * 2
                };
            assert_eq!(message_len("<", fields, ">"), expected, "{fields:?}");
        }
    }
}
