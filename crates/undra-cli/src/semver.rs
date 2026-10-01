//! Release versions (`1.2.3`, `1.0.0-rc.1`) and their order, for `undra upgrade`.
//!
//! Only what the CLI needs: parsing `major.minor.patch` with an optional pre-release suffix (a
//! shorter `major.minor` is read as `major.minor.0`) and comparing by the semver rules, where a
//! pre-release sorts before its release and numeric identifiers sort numerically.

use core::cmp::Ordering;
use core::fmt;

/// A version: `major.minor.patch` and, for a pre-release, its dot-separated identifiers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Semver {
    /// The major version.
    pub major: u64,
    /// The minor version.
    pub minor: u64,
    /// The patch version.
    pub patch: u64,
    /// The pre-release identifiers (`rc`, `1` of `1.0.0-rc.1`); empty for a release.
    pub pre: Vec<String>,
}

impl Semver {
    /// A release version.
    #[cfg(test)]
    #[must_use]
    pub fn new(major: u64, minor: u64, patch: u64) -> Semver {
        Semver {
            major,
            minor,
            patch,
            pre: Vec::new(),
        }
    }

    /// Parses `1.2.3`, `v1.2.3`, `1.2` (as `1.2.0`) and `1.2.3-rc.1`; `None` for anything else
    /// (build metadata after `+` is ignored).
    #[must_use]
    pub fn parse(text: &str) -> Option<Semver> {
        let text = text.trim().trim_start_matches('v');
        let text = text.split('+').next()?;
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = match parts.next() {
            Some(p) => p.parse().ok()?,
            None => 0,
        };
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            Some("") => return None,
            Some(p) => p.split('.').map(ToOwned::to_owned).collect(),
            None => Vec::new(),
        };
        Some(Semver {
            major,
            minor,
            patch,
            pre,
        })
    }

    /// `major.minor`: the release line the package registries are asked for.
    #[must_use]
    pub fn line(&self) -> String {
        format!("{}.{}", self.major, self.minor)
    }
}

impl fmt::Display for Semver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

impl PartialOrd for Semver {
    fn partial_cmp(&self, other: &Semver) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Semver {
    fn cmp(&self, other: &Semver) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => compare_pre(&self.pre, &other.pre),
            })
    }
}

/// Pre-release identifiers: numeric ones numerically and before alphanumeric ones, a shorter list
/// before a longer one it is a prefix of.
fn compare_pre(a: &[String], b: &[String]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        let ordering = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => x.cmp(y),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    a.len().cmp(&b.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Semver {
        Semver::parse(text).unwrap_or_else(|| panic!("{text} parses"))
    }

    #[test]
    fn versions_parse_in_the_forms_the_pins_use() {
        assert_eq!(v("1.2.3"), Semver::new(1, 2, 3));
        assert_eq!(v("v0.1.0"), Semver::new(0, 1, 0), "a git tag");
        assert_eq!(v("0.1"), Semver::new(0, 1, 0), "a release line");
        assert_eq!(v("1.0.0-rc.1").pre, ["rc", "1"]);
        assert_eq!(v("1.2.3+build5"), Semver::new(1, 2, 3));
        for bad in ["", "1", "1.x", "1.2.3.4", "1.2.3-", "latest", "^1.2.0"] {
            assert_eq!(Semver::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn versions_print_as_they_were_written_in_full() {
        assert_eq!(v("0.1").to_string(), "0.1.0");
        assert_eq!(v("1.0.0-rc.1").to_string(), "1.0.0-rc.1");
        assert_eq!(v("1.2.3").line(), "1.2");
    }

    #[test]
    fn versions_order_by_the_semver_rules() {
        let ordered = [
            "0.0.9",
            "0.1.0",
            "0.1.1",
            "0.2.0",
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.10.0",
            "2.0.0",
        ];
        for pair in ordered.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        }
        assert_eq!(v("0.1").cmp(&v("0.1.0")), Ordering::Equal);
        assert!(v("1.9.0") < v("1.10.0"), "numerically, not as text");
    }
}
