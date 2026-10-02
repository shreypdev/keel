//! Objects and callbacks at call sites (ADR-040, ADR-041): what the Swift generator writes for a
//! method, constructor or free function that takes an object or a callback, or returns objects.
//!
//! * A returned object is adopted (`core.adoptObject(body) { Mailbox(adopting: $0, core: $1) }`),
//!   so one handle has one wrapper; `adoptOptional` and `adoptList` for `T?` and `[T]`.
//! * An object argument is checked first (`core.requireOwn(target)`: an object of another core is
//!   refused before anything is sent), kept alive until the call is sent, and written as its handle.
//! * A callback argument is lent to the core (`core.callbacks.lend(listener)`) and its instance
//!   handle written; the call names the instances it carries (`lending:`) so the runtime gives them
//!   back when the call is refused or never sent.
//!
//! All of it runs inside the call's `do`, so a command reports a refusal like any other failure.

use undra_meta::{ParamDef, TypeRef};

use super::{SwiftGen, id};
use crate::emit::CodeWriter;
use crate::model::{CallbackUse, ObjectUse};
use crate::naming;

/// What a call that hands objects or callbacks over wrote before sending.
pub(super) struct Handover {
    /// The expression of the encoded arguments.
    pub args: String,
    /// The `lending:` list, when the call carries callbacks.
    pub lending: Option<String>,
}

/// Whether `params` hand an object or a callback to the core.
pub(super) fn hands_over(params: &[ParamDef]) -> bool {
    params
        .iter()
        .any(|p| ObjectUse::of(&p.ty).is_some() || CallbackUse::of(&p.ty).is_some())
}

impl SwiftGen<'_> {
    /// The local holding the instance handle lent for the callback parameter `p`.
    fn instance_name(p: &ParamDef, params: &[ParamDef]) -> String {
        let taken: Vec<String> = params.iter().map(|q| id(&q.name)).collect();
        let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
        let base = format!("{}Instance", naming::camel(&p.name));
        naming::avoid(&base, &taken)
    }

    /// Writes, inside the call's `do`, the checks of the object arguments, what keeps them alive until
    /// the call is sent, the lending of the callbacks and the encoding of every argument. `checks` is
    /// off where the generated function cannot throw (a stream method).
    pub(super) fn handover(
        &self,
        w: &mut CodeWriter,
        params: &[ParamDef],
        writer: &str,
        core: &str,
        checks: bool,
    ) -> Handover {
        let objects: Vec<String> = params
            .iter()
            .filter(|p| ObjectUse::of(&p.ty).is_some())
            .map(|p| id(&p.name))
            .collect();
        if checks {
            for object in &objects {
                w.line(format!("try {core}.requireOwn({object})"));
            }
            match objects.as_slice() {
                [] => {}
                [one] => w.line(format!("defer {{ withExtendedLifetime({one}) {{}} }}")),
                many => w.line(format!(
                    "defer {{ withExtendedLifetime(({})) {{}} }}",
                    many.join(", ")
                )),
            }
        }
        let mut lent = Vec::new();
        for p in params {
            let Some(callback) = CallbackUse::of(&p.ty) else {
                continue;
            };
            let instance = Self::instance_name(p, params);
            let name = id(&p.name);
            match callback {
                CallbackUse::One(_) => {
                    w.line(format!("let {instance} = {core}.callbacks.lend({name})"));
                }
                CallbackUse::Optional(_) => w.line(format!(
                    "let {instance} = {name}.map {{ {core}.callbacks.lend($0) }}"
                )),
            }
            lent.push(instance);
        }
        let t = self.types();
        let args = if params.is_empty() {
            "[]".to_owned()
        } else {
            w.line(format!("var {writer} = UndraWriter()"));
            for p in params {
                let value = if CallbackUse::of(&p.ty).is_some() {
                    Self::instance_name(p, params)
                } else {
                    id(&p.name)
                };
                w.line(t.write_stmt(&p.ty, &value, writer));
            }
            format!("{writer}.finish()")
        };
        Handover {
            args,
            lending: (!lent.is_empty()).then(|| format!("[{}]", lent.join(", "))),
        }
    }

    /// The statement that returns the result `ok` read from the reply `body`: an object is adopted
    /// through `core`, anything else decoded.
    pub(super) fn return_result(&self, ok: &TypeRef, body: &str, core: &str) -> String {
        let adopt = |helper: &str, name: &str| {
            format!("return try {core}.{helper}({body}) {{ {name}(adopting: $0, core: $1) }}")
        };
        match ObjectUse::of(ok) {
            Some(ObjectUse::One(name)) => adopt("adoptObject", name),
            Some(ObjectUse::Optional(name)) => adopt("adoptOptional", name),
            Some(ObjectUse::Many(name)) => adopt("adoptList", name),
            None => format!("return try {}", self.types().decode_all(ok, body)),
        }
    }

    /// Whether `ok` is a store, or holds stores: the method that returns it makes main-actor
    /// wrappers, so it is `@MainActor` (ADR-040).
    pub(super) fn returns_store(&self, ok: &TypeRef) -> bool {
        ObjectUse::of(ok).is_some_and(|use_| {
            self.model
                .stores
                .iter()
                .any(|store| store.name == use_.name())
        })
    }

    /// Whether `name` is a store (its methods are main-actor already).
    pub(super) fn is_store(&self, name: &str) -> bool {
        self.model.stores.iter().any(|store| store.name == name)
    }
}
