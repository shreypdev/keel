//! Objects as parameters and returns in the TypeScript output (ADR-040).
//!
//! An object crosses as its handle. A parameter is borrowed: the call writes the wrapper's handle
//! after `requireOwn` (which refuses an object of another core before anything is sent). A return
//! is one reference the host now owns: the reply's handle goes through `adoptObject`,
//! `adoptOptional` or `adoptList`, which return the live wrapper of a handle the host already
//! wraps (giving the extra reference back) or a new one, and observe a new store before it is
//! returned. Constructors go through `adopt` too.

use super::{Ctx, Module};
use crate::model::ObjectUse;

/// The hoisted codec of an optional handle (an `Option` of an object or a callback).
const OPTIONAL_HANDLE: (&str, &str) = ("optionalHandle", "codecs.option(codecs.u64)");
/// The hoisted codec of a list of handles.
const HANDLES: (&str, &str) = ("handles", "codecs.vec(codecs.u64)");

impl Ctx<'_> {
    /// The module that declares the class of the object `name`.
    pub(super) fn object_module(&self, name: &str) -> Module {
        let model = self.model();
        if model.stores.iter().any(|o| o.name == name) {
            Module::Stores
        } else if model.query_handles.iter().any(|o| o.name == name) {
            Module::Queries
        } else {
            Module::Objects
        }
    }

    /// Imports the class of the object `name` for a type position.
    pub(super) fn use_object_type(&mut self, name: &str) {
        let module = self.object_module(name);
        self.local_type(module, name);
    }

    /// Imports the class of the object `name` as a value: what `adopt` makes.
    pub(super) fn use_object_class(&mut self, name: &str) {
        let module = self.object_module(name);
        self.local_value(module, name);
    }

    /// A codec of handles, declared once at the end of the file. Handles are `bigint`s whatever
    /// the 64-bit number setting of the generator, so this does not go through `codec`.
    pub(super) fn handle_codec(&mut self, optional: bool) -> String {
        self.rt_value("codecs");
        let (name, expr) = if optional { OPTIONAL_HANDLE } else { HANDLES };
        if !self.hoisted.iter().any(|(n, _)| n == name) {
            self.hoisted.push((name.to_owned(), expr.to_owned()));
        }
        name.to_owned()
    }

    /// The statement that writes the object parameter `value` with `writer`: borrowed by the call
    /// into `core`, which must be the core of the object.
    pub(super) fn write_object(
        &mut self,
        object: &ObjectUse<'_>,
        value: &str,
        writer: &str,
        core: &str,
    ) -> String {
        self.rt_value("requireOwn");
        match object {
            ObjectUse::One(_) => format!("{writer}.writeU64(requireOwn({core}, {value}));"),
            ObjectUse::Optional(_) => {
                let codec = self.handle_codec(true);
                format!(
                    "{codec}.encode({writer}, {value} === null ? null : requireOwn({core}, {value}));"
                )
            }
            ObjectUse::Many(_) => {
                let codec = self.handle_codec(false);
                let item = if value == "item" { "each" } else { "item" };
                format!(
                    "{codec}.encode({writer}, {value}.map(({item}) => requireOwn({core}, {item})));"
                )
            }
        }
    }

    /// The expression (awaited) of what a reply `body` of `core` carries: the object, the optional
    /// object or the list of objects, adopted.
    pub(super) fn adopt_expr(&mut self, object: &ObjectUse<'_>, body: &str, core: &str) -> String {
        let function = match object {
            ObjectUse::One(_) => "adoptObject",
            ObjectUse::Optional(_) => "adoptOptional",
            ObjectUse::Many(_) => "adoptList",
        };
        self.rt_value(function);
        let name = object.name();
        self.use_object_class(name);
        format!("await {function}({core}, {body}, {name})")
    }
}
