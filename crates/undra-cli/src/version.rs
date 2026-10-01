//! The version line `undra --version` prints: `undra <semver> (<short sha>)`.
//!
//! The semver is the package's own version. The short SHA is the commit the binary was built
//! from, handed in by the release workflow through the `UNDRA_BUILD_SHA` environment variable at
//! build time; a build without it (`cargo install`, a developer's `cargo build`) says `unknown`.
//! Everything here is evaluated at compile time, so the line is a `&'static str` that clap prints
//! without allocating.

/// The version of this crate, which is the workspace version (`[workspace.package]`).
pub(crate) const SEMVER: &str = env!("CARGO_PKG_VERSION");

/// The first seven characters of `UNDRA_BUILD_SHA`, or `unknown` when it is unset or empty.
pub(crate) const BUILD_SHA: &str = short_sha(option_env!("UNDRA_BUILD_SHA"));

/// `<semver> (<short sha>)`: what follows the program name in the `--version` output.
pub(crate) const LINE: &str = match core::str::from_utf8(&LINE_BYTES) {
    Ok(line) => line,
    Err(_) => SEMVER,
};

const LINE_LEN: usize = SEMVER.len() + " (".len() + BUILD_SHA.len() + ")".len();

// `concat!` takes only literals, so the line is assembled byte by byte in a constant.
const LINE_BYTES: [u8; LINE_LEN] = {
    let mut out = [0u8; LINE_LEN];
    let mut at = copy_into(&mut out, 0, SEMVER);
    at = copy_into(&mut out, at, " (");
    at = copy_into(&mut out, at, BUILD_SHA);
    let _ = copy_into(&mut out, at, ")");
    out
};

/// Writes `text` into `out` from `at` and returns the index after it.
const fn copy_into(out: &mut [u8], at: usize, text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        out[at + i] = bytes[i];
        i += 1;
    }
    at + bytes.len()
}

/// The short form of a commit SHA: at most its first seven bytes, `unknown` when there is none
/// (or when seven bytes would cut a multi-byte character, which a SHA never contains).
pub(crate) const fn short_sha(sha: Option<&'static str>) -> &'static str {
    let Some(sha) = sha else { return "unknown" };
    let bytes = sha.as_bytes();
    if bytes.is_empty() {
        return "unknown";
    }
    let keep = if bytes.len() < 7 { bytes.len() } else { 7 };
    let (head, _) = bytes.split_at(keep);
    match core::str::from_utf8(head) {
        Ok(short) => short,
        Err(_) => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_short_sha_is_the_first_seven_characters() {
        assert_eq!(
            short_sha(Some("0123456789abcdef0123456789abcdef01234567")),
            "0123456"
        );
        assert_eq!(short_sha(Some("abc1234")), "abc1234");
        assert_eq!(short_sha(Some("abc")), "abc");
    }

    #[test]
    fn no_sha_is_unknown() {
        assert_eq!(short_sha(None), "unknown");
        assert_eq!(short_sha(Some("")), "unknown");
        // Seven bytes that end inside a character are not a SHA either.
        assert_eq!(short_sha(Some("abcdef\u{e9}")), "unknown");
    }

    #[test]
    fn the_line_is_the_semver_and_the_sha() {
        assert_eq!(LINE, format!("{SEMVER} ({BUILD_SHA})"));
        assert!(LINE.starts_with(env!("CARGO_PKG_VERSION")));
        assert!(LINE.ends_with(')'));
    }
}
