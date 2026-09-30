//! Diagnostics with stable codes (SPEC section 12, constitution rule R8).
//!
//! Every macro error has the same shape:
//!
//! ```text
//! error[keel::E0001]: `&str` cannot cross the boundary
//!   = note: references have no wire representation; every value crosses by copy
//!   = help: use an owned `String`
//!   = docs: https://keel.dev/errors/E0001
//! ```
//!
//! rustc prints the message of a `compile_error!` after its own `error:` prefix and
//! underlines the span the error was raised on. The first line is [`MESSAGE_PREFIX`] plus
//! the code and the *what*; the *why*, the *fix* and the docs link follow on their own
//! lines.

use core::fmt::Display;

use proc_macro2::{Span, TokenStream};
use quote::ToTokens;

/// Where the per-code documentation lives.
pub(crate) const DOCS_BASE: &str = "https://keel.dev/errors";

/// The text every diagnostic starts with, before the code.
///
/// `rustc` adds its own `error: ` in front, so the user sees
/// `error: error[keel::E0001]: ...`. Changing this constant changes every message.
pub(crate) const MESSAGE_PREFIX: &str = "error";

/// The diagnostic codes of SPEC section 12, plus the additions made by this crate.
///
/// | Code | Meaning |
/// |---|---|
/// | E0001 | unsupported type in a public position |
/// | E0002 | generic parameter |
/// | E0003 | lifetime in a public signature |
/// | E0004 | trait object, `dyn`, `impl Trait` |
/// | E0005 | `Result` / `Stream` outside return position |
/// | E0006 | map key type not allowed |
/// | E0007 | unsupported item shape (addition) |
/// | E0008 | unknown or misplaced `#[keel(..)]` attribute or macro argument (addition) |
/// | E0010 | invalid `#[keel::error]` variant |
/// | E0011 | store and `#[keel::api(store)]` impl block disagree |
/// | E0012 | trait object in a record field |
/// | E0013 | store cannot be restored automatically (addition) |
/// | E0020 | `&mut self` receiver |
/// | E0021 | `self` by value |
/// | E0022 | non-`Send` future in an async method (raised by `rustc` through a generated assertion) |
/// | E0030 | port method with a non-wire parameter |
/// | E0031 | event port method that is not a plain `fn(..)` returning `()` |
/// | E0032 | invalid port trait shape (addition) |
/// | E0040 | query without `key`, mutation with `stale`, or another invalid query argument |
/// | E0041 | query or mutation function with an invalid signature (addition) |
pub(crate) mod code {
    pub(crate) const E0001: &str = "E0001";
    pub(crate) const E0002: &str = "E0002";
    pub(crate) const E0003: &str = "E0003";
    pub(crate) const E0004: &str = "E0004";
    pub(crate) const E0005: &str = "E0005";
    pub(crate) const E0006: &str = "E0006";
    pub(crate) const E0007: &str = "E0007";
    pub(crate) const E0008: &str = "E0008";
    pub(crate) const E0010: &str = "E0010";
    pub(crate) const E0011: &str = "E0011";
    pub(crate) const E0012: &str = "E0012";
    pub(crate) const E0013: &str = "E0013";
    pub(crate) const E0020: &str = "E0020";
    pub(crate) const E0021: &str = "E0021";
    pub(crate) const E0030: &str = "E0030";
    pub(crate) const E0031: &str = "E0031";
    pub(crate) const E0032: &str = "E0032";
    pub(crate) const E0040: &str = "E0040";
    pub(crate) const E0041: &str = "E0041";
}

/// A diagnostic under construction: everything except the span it is reported on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Diag {
    /// The stable code, for example `"E0001"`.
    pub(crate) code: &'static str,
    /// What is wrong (first line).
    pub(crate) what: String,
    /// Why the rule exists.
    pub(crate) why: String,
    /// How to fix it.
    pub(crate) help: String,
}

impl Diag {
    /// Builds a diagnostic.
    pub(crate) fn new(
        code: &'static str,
        what: impl Display,
        why: impl Display,
        help: impl Display,
    ) -> Diag {
        Diag {
            code,
            what: what.to_string(),
            why: why.to_string(),
            help: help.to_string(),
        }
    }

    /// The full multi-line message text.
    pub(crate) fn message(&self) -> String {
        format!(
            "{MESSAGE_PREFIX}[keel::{code}]: {what}\n  = note: {why}\n  = help: {help}\n  = docs: {DOCS_BASE}/{code}",
            code = self.code,
            what = self.what,
            why = self.why,
            help = self.help,
        )
    }

    /// Reports the diagnostic on a single span.
    pub(crate) fn at(&self, span: Span) -> syn::Error {
        syn::Error::new(span, self.message())
    }

    /// Reports the diagnostic on a syntax node (underlining all of it).
    pub(crate) fn on(&self, node: &impl ToTokens) -> syn::Error {
        syn::Error::new_spanned(node, self.message())
    }
}

/// Collects several errors so that one expansion reports all of them.
#[derive(Default)]
pub(crate) struct Errors(Option<syn::Error>);

impl Errors {
    /// An empty collection.
    pub(crate) fn new() -> Errors {
        Errors(None)
    }

    /// Adds an error.
    pub(crate) fn push(&mut self, error: syn::Error) {
        match &mut self.0 {
            Some(existing) => existing.combine(error),
            None => self.0 = Some(error),
        }
    }

    /// Adds the error of a result and returns the value if there was one.
    pub(crate) fn ok<T>(&mut self, result: syn::Result<T>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.push(error);
                None
            }
        }
    }

    /// Whether any error was recorded.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// `Ok(())` if nothing was recorded, else all errors combined.
    pub(crate) fn finish(self) -> syn::Result<()> {
        match self.0 {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Takes the combined error, if any.
    pub(crate) fn into_error(self) -> Option<syn::Error> {
        self.0
    }
}

/// Turns an error into `compile_error!` tokens.
pub(crate) fn compile_error(error: &syn::Error) -> TokenStream {
    error.to_compile_error()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_has_the_documented_shape() {
        let diag = Diag::new(code::E0001, "`&str` cannot cross the boundary", "why", "fix");
        let message = diag.message();
        let lines: Vec<&str> = message.lines().collect();
        assert_eq!(
            lines,
            [
                "error[keel::E0001]: `&str` cannot cross the boundary",
                "  = note: why",
                "  = help: fix",
                "  = docs: https://keel.dev/errors/E0001",
            ]
        );
    }

    #[test]
    fn errors_combine_and_finish() {
        let mut errors = Errors::new();
        assert!(errors.is_empty());
        errors.push(Diag::new(code::E0001, "a", "b", "c").at(Span::call_site()));
        errors.push(Diag::new(code::E0002, "d", "e", "f").at(Span::call_site()));
        assert!(!errors.is_empty());
        let combined = errors.finish().unwrap_err();
        assert_eq!(combined.into_iter().count(), 2);
    }

    #[test]
    fn ok_collects_errors_and_passes_values() {
        let mut errors = Errors::new();
        assert_eq!(errors.ok::<u8>(Ok(3)), Some(3));
        assert_eq!(
            errors.ok::<u8>(Err(Diag::new(code::E0001, "a", "b", "c").at(Span::call_site()))),
            None
        );
        assert!(errors.into_error().is_some());
    }
}
