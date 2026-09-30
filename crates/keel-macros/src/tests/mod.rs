//! Unit tests that run the expansion functions on fixture inputs.

/// Removes all whitespace, so token-stream strings can be compared without caring about
/// the spacing `proc_macro2` prints between tokens.
pub(crate) fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Whether `haystack` contains `needle`, ignoring whitespace in both.
pub(crate) fn has(haystack: &str, needle: &str) -> bool {
    squash(haystack).contains(&squash(needle))
}
