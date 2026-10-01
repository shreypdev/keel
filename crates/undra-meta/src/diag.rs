//! The shape of every Undra diagnostic (constitution R8, SPEC section 12).
//!
//! A diagnostic that is not a compile error of a macro (a schema that fails validation, a message
//! the runtime panics with) reads exactly like one: a code and what is wrong, a note on why the
//! rule exists, a help line with the fix and the docs link of the code.
//!
//! ```text
//! error[undra::E0050]: duplicate type name `Todo` (declared as record and again as error)
//!   = note: a type is identified by its name, so two declarations cannot share one
//!   = help: rename one of the two, or remove the duplicate
//!   = docs: https://shreypdev.github.io/undra/docs/errors.html#E0050
//! ```
//!
//! The macros render the same shape themselves (`undra-macros` cannot depend on this crate); a test
//! in the macros crate reads the catalogue and the goldens of every crate and checks that they
//! agree.

use core::fmt::Display;

/// Where the per-code documentation lives; the link of a diagnostic is this plus `#` and the code.
pub const DOCS_BASE: &str = "https://shreypdev.github.io/undra/docs/errors.html";

/// The four-line message of a diagnostic: `error[undra::<code>]: <what>`, then the note (`why`),
/// the help (`fix`) and the docs link, each on a line of its own.
///
/// ```
/// let text = undra_meta::diag::message("E0050", "what", "why", "fix");
/// assert_eq!(
///     text,
///     "error[undra::E0050]: what\n  = note: why\n  = help: fix\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0050"
/// );
/// ```
#[must_use]
pub fn message(code: &str, what: impl Display, why: impl Display, fix: impl Display) -> String {
    format!(
        "error[undra::{code}]: {what}\n  = note: {why}\n  = help: {fix}\n  = docs: {DOCS_BASE}#{code}"
    )
}
