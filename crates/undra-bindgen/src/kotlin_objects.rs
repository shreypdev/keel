//! Kotlin: objects as parameters and returns (ADR-040) and callback arguments at call sites
//! (ADR-041), for the generated methods, constructors and free functions of `kotlin.rs`.
//!
//! * A returned object is adopted, never wrapped from a raw handle: `core.adoptObject(body,
//!   ::Mailbox)` (and `adoptOptional`, `adoptList`) returns the wrapper the core already has for
//!   the handle, or makes one; a constructor's handle goes through `core.adopt(handle, ::Account)`.
//! * An object argument is checked first (`core.requireOwn(target)`: an object of another core is
//!   refused before anything is sent), written as its handle, and kept reachable until the call
//!   is sent (`reachabilityFence(target)`).
//! * A callback argument is lent to the core (`core.callbacks.lend(listener)`, the instance handle
//!   that is written); a call that fails before the core took it gives it back
//!   (`core.callbacks.giveBackIfRefused(e, listenerInstance)`).

use undra_meta::{ParamDef, TypeRef};

use super::{Ctx, ident};
use crate::emit::CodeWriter;
use crate::model::{CallbackUse, ObjectUse};
use crate::naming;

/// A callback argument of a call: the parameter and the local holding its instance handle.
pub(super) struct Lent {
    /// The Kotlin name of the parameter.
    param: String,
    /// The local that holds the instance handle (`listenerInstance`).
    pub(super) var: String,
    /// `Option<Arc<dyn Trait>>`.
    optional: bool,
}

/// An object argument of a call.
pub(super) struct ObjectArg {
    /// The Kotlin name of the parameter.
    param: String,
}

/// The callback arguments of `params`, with locals that do not collide with `taken`.
pub(super) fn lent(params: &[ParamDef], taken: &[&str]) -> Vec<Lent> {
    params
        .iter()
        .filter_map(|p| {
            let callback = CallbackUse::of(&p.ty)?;
            Some(Lent {
                param: ident(&p.name),
                var: naming::avoid(&format!("{}Instance", naming::camel(&p.name)), taken),
                optional: matches!(callback, CallbackUse::Optional(_)),
            })
        })
        .collect()
}

/// The object arguments of `params`.
pub(super) fn object_args(params: &[ParamDef]) -> Vec<ObjectArg> {
    params
        .iter()
        .filter(|p| ObjectUse::of(&p.ty).is_some())
        .map(|p| ObjectArg {
            param: ident(&p.name),
        })
        .collect()
}

/// `core.requireOwn(target)` for every object argument: one of another core is refused with
/// `UndraCallError.Refused` before anything is sent.
pub(super) fn require_own(w: &mut CodeWriter, core: &str, objects: &[ObjectArg]) {
    for o in objects {
        w.line(format!("{core}.requireOwn({})", o.param));
    }
}

/// Lends every callback argument: `val listenerInstance = core.callbacks.lend(listener)`.
pub(super) fn lend(w: &mut CodeWriter, core: &str, lent: &[Lent]) {
    for l in lent {
        if l.optional {
            w.line(format!(
                "val {} = {}?.let {{ {core}.callbacks.lend(it) }}",
                l.var, l.param
            ));
        } else {
            w.line(format!("val {} = {core}.callbacks.lend({})", l.var, l.param));
        }
    }
}

/// In the `catch` of a call: gives every lent reference back unless the core took it.
pub(super) fn give_back(w: &mut CodeWriter, core: &str, failure: &str, lent: &[Lent]) {
    for l in lent {
        w.line(format!(
            "{core}.callbacks.giveBackIfRefused({failure}, {})",
            l.var
        ));
    }
}

/// Keeps every object argument reachable until here, after the call was sent.
pub(super) fn fence(cx: &mut Ctx<'_>, w: &mut CodeWriter, objects: &[ObjectArg]) {
    if objects.is_empty() {
        return;
    }
    cx.import("dev.undra.runtime.reachabilityFence");
    for o in objects {
        w.line(format!("reachabilityFence({})", o.param));
    }
}

impl Ctx<'_> {
    /// The statement writing parameter `p` with `writer`: an object as its handle, a callback as
    /// the instance handle it was lent as, anything else by its codec.
    pub(super) fn write_param(&mut self, p: &ParamDef, writer: &str, lent: &[Lent]) -> String {
        let name = ident(&p.name);
        if let Some(object) = ObjectUse::of(&p.ty) {
            return match object {
                ObjectUse::One(_) => format!("{writer}.writeI64({name}.handle)"),
                ObjectUse::Optional(_) => {
                    let codec = self.codec(&TypeRef::option(TypeRef::I64), &Default::default());
                    format!("{codec}.encode({writer}, {name}?.handle)")
                }
                ObjectUse::Many(_) => {
                    let codec = self.codec(&TypeRef::vec(TypeRef::I64), &Default::default());
                    format!("{codec}.encode({writer}, {name}.map {{ it.handle }})")
                }
            };
        }
        if let Some(callback) = CallbackUse::of(&p.ty) {
            let var = lent
                .iter()
                .find(|l| l.param == name)
                .map_or_else(|| name.clone(), |l| l.var.clone());
            return match callback {
                CallbackUse::One(_) => format!("{writer}.writeU64({var})"),
                CallbackUse::Optional(_) => {
                    let codec = self.codec(&TypeRef::option(TypeRef::U64), &Default::default());
                    format!("{codec}.encode({writer}, {var})")
                }
            };
        }
        self.write_stmt(&p.ty, &name, writer, &Default::default())
    }

    /// The expression turning reply `body` into the returned object(s) of type `ok` through the
    /// core's identity map, or `None` when `ok` holds no object.
    pub(super) fn adopt_expr(&mut self, ok: &TypeRef, core: &str, body: &str) -> Option<String> {
        let object = ObjectUse::of(ok)?;
        let class = self.named(object.name(), &Default::default());
        let function = match object {
            ObjectUse::One(_) => "adoptObject",
            ObjectUse::Optional(_) => "adoptOptional",
            ObjectUse::Many(_) => "adoptList",
        };
        Some(format!("{core}.{function}({body}, ::{class})"))
    }
}
