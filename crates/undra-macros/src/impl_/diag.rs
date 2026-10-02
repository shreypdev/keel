//! Diagnostics with stable codes (SPEC section 12, constitution rule R8).
//!
//! Every macro error has the same shape:
//!
//! ```text
//! error[undra::E0001]: `&str` cannot cross the boundary
//!   = note: references have no wire representation; every value crosses by copy
//!   = help: use an owned `String`
//!   = docs: https://shreypdev.github.io/undra/docs/errors.html#E0001
//! ```
//!
//! rustc prints the message of a `compile_error!` after its own `error:` prefix and
//! underlines the span the error was raised on. The first line is [`MESSAGE_PREFIX`] plus
//! the code and the *what*; the *why*, the *fix* and the docs link follow on their own
//! lines.

use core::fmt::Display;

use proc_macro2::Span;
use quote::ToTokens;

/// Where the per-code documentation lives.
pub(crate) const DOCS_BASE: &str = "https://shreypdev.github.io/undra/docs/errors.html";

/// The text every diagnostic starts with, before the code.
///
/// `rustc` adds its own `error: ` in front, so the user sees
/// `error: error[undra::E0001]: ...`. Changing this constant changes every message.
pub(crate) const MESSAGE_PREFIX: &str = "error";

/// The diagnostic codes of SPEC section 12 that this crate knows, with the short meaning the
/// error-codes page of the site shows for each. `catalogue.rs` (an integration test) checks this
/// table against SPEC section 12, the constants below, the emitting sites and the goldens.
///
/// The codes E0050 to E0052 are raised by schema validation (`undra-meta`, `undra-bindgen`), which
/// this crate cannot depend on; they have rows here, and no constant, because nothing here emits
/// them.
///
/// | Code | Meaning |
/// |---|---|
/// | E0001 | unsupported type in a public position |
/// | E0002 | generic parameter, or a generic type spelled with its arguments |
/// | E0003 | lifetime in a public signature |
/// | E0004 | a trait object, `dyn` or `impl Trait` that is not a callback parameter, or a callback where one may not stand (ADR-041) |
/// | E0005 | `Result` / `Stream` outside return position |
/// | E0006 | map key type not allowed |
/// | E0007 | unsupported item shape or placement of an Undra attribute (addition) |
/// | E0008 | unknown or misplaced `#[undra(..)]` attribute or macro argument (addition) |
/// | E0010 | invalid `#[undra::error]` variant |
/// | E0011 | store and `#[undra::api(store)]` impl block disagree |
/// | E0012 | trait object in a record field |
/// | E0013 | store cannot be restored automatically (addition) |
/// | E0020 | `&mut self` receiver |
/// | E0021 | `self` by value |
/// | E0022 | non-`Send` future in an async method: `rustc`'s own error, pointed at the method by an assertion named after this code |
/// | E0030 | port method with a non-wire parameter |
/// | E0031 | event port method that is not a plain `fn(..)` returning `()` |
/// | E0032 | invalid port trait shape (addition) |
/// | E0033 | the error type of a port method has no `From<PortError>` (addition) |
/// | E0040 | query without `key`, mutation with `stale`, or another invalid query argument |
/// | E0041 | query or mutation function with an invalid signature (addition) |
/// | E0042 | query whose success value is `()` or an `Option` (addition) |
/// | E0050 | duplicate type name, id or variant index (schema validation; addition) |
/// | E0051 | a name that collides after case conversion or is not an identifier (schema validation; addition) |
/// | E0052 | an item named like a standard library item, with another id (schema validation; addition) |
/// | E0060 | a spelling that looks like a built-in Undra type is another type (addition) |
/// | E0061 | the name written is not the declared name of the type, or the type is not declared with `#[undra::api]` (addition) |
/// | E0062 | a port call could not be answered and its method has no error channel: a runtime message (addition) |
/// | E0063 | nested `Option<Option<T>>` (addition) |
/// | E0064 | an object (`#[undra::api] impl`) used where a value is expected, or a value used as an object, or an object where objects may not stand (ADR-040) |
/// | E0065 | a signal of a store written off its owning runtime's core (ADR-035): a runtime message |
/// | E0066 | a `#[undra::migrate]` hook with a wrong target or shape (ADR-037; addition) |
/// | E0070 | a named instantiation of a generic data type that is declared twice or outside the crate of its template (addition) |
/// | E0071 | a method of a `#[undra::callback]` trait that is neither fire-and-forget nor `async` with a `Result`, or whose name starts with `__` (ADR-041) |
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
    pub(crate) const E0022: &str = "E0022";
    pub(crate) const E0030: &str = "E0030";
    pub(crate) const E0031: &str = "E0031";
    pub(crate) const E0032: &str = "E0032";
    pub(crate) const E0033: &str = "E0033";
    pub(crate) const E0040: &str = "E0040";
    pub(crate) const E0041: &str = "E0041";
    pub(crate) const E0042: &str = "E0042";
    pub(crate) const E0060: &str = "E0060";
    pub(crate) const E0061: &str = "E0061";
    pub(crate) const E0062: &str = "E0062";
    pub(crate) const E0063: &str = "E0063";
    pub(crate) const E0064: &str = "E0064";
    pub(crate) const E0066: &str = "E0066";
    pub(crate) const E0070: &str = "E0070";
    pub(crate) const E0071: &str = "E0071";
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
            "{MESSAGE_PREFIX}[undra::{code}]: {what}\n  = note: {why}\n  = help: {help}\n  = docs: {DOCS_BASE}#{code}",
            code = self.code,
            what = self.what,
            why = self.why,
            help = self.help,
        )
    }

    /// The shape of [`Diag::message`] as a `format!` template with three `{}`: what, why and fix.
    ///
    /// For a message that is finished at run time (E0062 names the port and method of the call
    /// that failed), so a runtime error reads exactly like a compile error: same code, same four
    /// lines, same docs link.
    pub(crate) fn runtime_template(code: &str) -> String {
        format!(
            "{MESSAGE_PREFIX}[undra::{code}]: {{}}\n  = note: {{}}\n  = help: {{}}\n  = docs: {DOCS_BASE}#{code}"
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
    #[cfg(test)]
    pub(crate) fn into_error(self) -> Option<syn::Error> {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_has_the_documented_shape() {
        let diag = Diag::new(
            code::E0001,
            "`&str` cannot cross the boundary",
            "why",
            "fix",
        );
        let message = diag.message();
        let lines: Vec<&str> = message.lines().collect();
        assert_eq!(
            lines,
            [
                "error[undra::E0001]: `&str` cannot cross the boundary",
                "  = note: why",
                "  = help: fix",
                "  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0001",
            ]
        );
    }

    #[test]
    fn the_runtime_template_is_the_message_with_holes() {
        let template = Diag::runtime_template(code::E0062);
        let filled = template
            .replacen("{}", "what", 1)
            .replacen("{}", "why", 1)
            .replacen("{}", "fix", 1);
        assert_eq!(
            filled,
            Diag::new(code::E0062, "what", "why", "fix").message()
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
}
