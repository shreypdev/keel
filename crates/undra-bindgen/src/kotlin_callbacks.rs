//! Kotlin: host callback interfaces (ADR-041), the `Callbacks.kt` of a schema that has
//! `#[undra::callback]` traits.
//!
//! Per trait: the `interface` the app implements (a `suspend fun` for an `async` method, which
//! throws the method's error class; a `fun` for a fire-and-forget one), its weak wrapper
//! (`UploadListener.weak(target)`), and the bridge object the core entry registers
//! (`UploadListenerBridge`), which decodes the core's calls and hands them to the runtime
//! (`UndraCallbackBridge`); the runtime runs them on the main thread through the mirror's drain,
//! or on a serial executor per instance for a `background` trait.

use undra_meta::{MethodDef, PortDef, TypeRef, ids};

use super::{Ctx, KtGen, hex, ident, kdoc, kt_string};
use crate::emit::CodeWriter;
use crate::model::Ret;
use crate::naming;

impl KtGen<'_> {
    /// `Callbacks.kt`, or `None` when the schema has no callback interface.
    pub(super) fn callbacks_file(&self) -> Option<String> {
        if self.model.callbacks.is_empty() {
            return None;
        }
        let mut cx = Ctx::new(self);
        let mut w = CodeWriter::new("    ");
        for callback in &self.model.callbacks {
            cx.callback_interface(&mut w, callback);
            w.blank();
            cx.weak_callback(&mut w, callback);
            w.blank();
            cx.callback_bridge(&mut w, callback);
            w.blank();
        }
        cx.body = w;
        Some(self.assemble(cx))
    }

    /// `object Callbacks` of `UndraIds`: per interface its port id, its method ids and the two
    /// reserved ones. Nothing when the schema has no callback interface.
    pub(super) fn callback_ids(&self, w: &mut CodeWriter) {
        if self.model.callbacks.is_empty() {
            return;
        }
        w.blank();
        w.block("object Callbacks", |w| {
            for p in &self.model.callbacks {
                w.block(format!("object {}", p.name), |w| {
                    w.line(format!("const val PORT_ID: UInt = {}", hex(p.port_id)));
                    for method in &p.methods {
                        w.line(format!(
                            "const val {}: UInt = {}",
                            naming::upper_snake(&method.name),
                            hex(method.method_id)
                        ));
                    }
                    w.line(format!(
                        "const val RELEASE_INSTANCE: UInt = {}",
                        hex(ids::callback_release_id(&p.name))
                    ));
                    w.line(format!(
                        "const val CANCEL_CALL: UInt = {}",
                        hex(ids::callback_cancel_id(&p.name))
                    ));
                });
            }
        });
    }

    /// The bridges the core entry registers before the core starts.
    pub(super) fn callback_bridges(&self) -> Vec<String> {
        self.model
            .callbacks
            .iter()
            .map(|p| format!("{}Bridge", p.name))
            .collect()
    }
}

/// The `Ok` type of an `async` callback method (`None` for a fire-and-forget one).
fn ask_ok(m: &MethodDef) -> Option<&TypeRef> {
    if !m.is_async {
        return None;
    }
    match Ret::classify(&m.returns) {
        Some(Ret::Result { ok, .. }) => Some(ok),
        Some(Ret::Plain(t)) => Some(t),
        _ => None,
    }
}

impl Ctx<'_> {
    fn callback_interface(&mut self, w: &mut CodeWriter, p: &PortDef) {
        let delivery = if p.background {
            "The core calls it off the main thread, one call at a time per instance, in the order\n\
             it made the calls."
        } else {
            "The core calls it on the main thread, in order with the stores' changes: a method sees\n\
             the stores as they were when the core called it."
        };
        let docs = if p.docs.trim().is_empty() {
            String::new()
        } else {
            format!("{}\n\n", p.docs.trim_end())
        };
        kdoc(
            w,
            &format!(
                "{docs}The app implements it and passes it to the core, which keeps it until it lets go of it.\n\
                 {delivery}\n[weak] wraps an implementation without keeping it alive."
            ),
            &[],
        );
        w.block(format!("interface {}", p.name), |w| {
            for (i, m) in p.methods.iter().enumerate() {
                if i > 0 {
                    w.blank();
                }
                let ret = Ret::classify(&m.returns);
                let mut extra = Vec::new();
                if let Some(err) = ret.as_ref().and_then(Ret::error) {
                    extra.push(format!("@throws {err} to answer the core with an error; anything else it throws is"));
                    extra.push(
                        "  reported (`LoadOptions.onError`) and the core is answered unavailable.".to_owned(),
                    );
                }
                kdoc(w, &m.docs, &extra);
                let params = self.param_list(&m.params);
                match ask_ok(m) {
                    Some(ok) => {
                        let suffix = if matches!(ok, TypeRef::Unit) {
                            String::new()
                        } else {
                            format!(": {}", self.ty(ok, &Default::default()))
                        };
                        w.call(
                            format!("suspend fun {}", ident(&m.name)),
                            &params,
                            suffix,
                            true,
                        );
                    }
                    None => w.call(format!("fun {}", ident(&m.name)), &params, "", true),
                }
            }
            w.blank();
            w.block("companion object", |w| {
                kdoc(
                    w,
                    "Forwards to [target] while something else keeps it alive, and holds it only weakly: once it\n\
                     is collected, the methods that return nothing do nothing and the `suspend` ones answer the\n\
                     core unavailable. For an implementation that holds, directly or not, the object it is passed\n\
                     to (a view model that owns the store it listens to), which would otherwise keep itself alive.",
                    &[],
                );
                w.line(format!(
                    "fun weak(target: {0}): {0} = Weak{0}(target)",
                    p.name
                ));
            });
        });
    }

    fn weak_callback(&mut self, w: &mut CodeWriter, p: &PortDef) {
        self.import("java.lang.ref.WeakReference");
        kdoc(w, &format!("What [{}.weak] returns.", p.name), &[]);
        w.block(
            format!("private class Weak{0}(target: {0}) : {0}", p.name),
            |w| {
                w.line("private val target = WeakReference(target)");
                for m in &p.methods {
                    w.blank();
                    let params = self.param_list(&m.params);
                    let args: Vec<String> = m.params.iter().map(|a| ident(&a.name)).collect();
                    let call = format!("{}({})", ident(&m.name), args.join(", "));
                    match ask_ok(m) {
                        Some(ok) => {
                            self.import("dev.undra.runtime.UndraCallbackGoneException");
                            let suffix = if matches!(ok, TypeRef::Unit) {
                                String::new()
                            } else {
                                format!(": {}", self.ty(ok, &Default::default()))
                            };
                            w.call_block(
                                format!("override suspend fun {}", ident(&m.name)),
                                &params,
                                suffix,
                                true,
                                |w| {
                                    w.line(format!(
                                        "val target = target.get() ?: throw UndraCallbackGoneException({})",
                                        kt_string(&p.name)
                                    ));
                                    w.line(format!("return target.{call}"));
                                },
                            );
                        }
                        None => w.call_block(
                            format!("override fun {}", ident(&m.name)),
                            &params,
                            "",
                            true,
                            |w| {
                                w.line(format!("target.get()?.{call}"));
                            },
                        ),
                    }
                }
            },
        );
    }

    fn callback_bridge(&mut self, w: &mut CodeWriter, p: &PortDef) {
        self.import("dev.undra.runtime.UndraCallbackBridge");
        self.import("dev.undra.runtime.UndraCallbackInvocation");
        self.import("dev.undra.runtime.wire.UndraReader");
        let ids = format!("UndraIds.Callbacks.{}", p.name);
        kdoc(
            w,
            &format!(
                "Decodes the core's calls into `{}` implementations for the runtime, which runs\n\
                 them (ADR-041). The core entry registers it.",
                p.name
            ),
            &[],
        );
        let mut args = vec![
            format!("name = {}", kt_string(&p.name)),
            format!("portId = {ids}.PORT_ID"),
            format!("releaseInstance = {ids}.RELEASE_INSTANCE"),
            format!("cancelCall = {ids}.CANCEL_CALL"),
        ];
        if p.background {
            args.push("background = true".to_owned());
        }
        let coalesced: Vec<String> = p
            .methods
            .iter()
            .filter(|m| m.coalesce)
            .map(|m| format!("{ids}.{}", naming::upper_snake(&m.name)))
            .collect();
        if !coalesced.is_empty() {
            args.push(format!("coalesced = setOf({})", coalesced.join(", ")));
        }
        w.call(
            format!("internal object {}Bridge : UndraCallbackBridge<{}>", p.name, p.name),
            &args,
            " {",
            true,
        );
        w.indented(|w| {
            w.line(format!(
                "override fun invocation(methodId: UInt, args: UndraReader): UndraCallbackInvocation<{}>? =",
                p.name
            ));
            w.indented(|w| {
                w.block("when (methodId)", |w| {
                    for m in &p.methods {
                        self.bridge_method(w, m, &ids);
                    }
                    w.line("else -> null");
                });
            });
        });
        w.line("}");
    }

    fn bridge_method(&mut self, w: &mut CodeWriter, m: &MethodDef, ids: &str) {
        let none = Default::default();
        let reserved = ["methodId", "args", "it", "e"];
        let locals: Vec<String> = m
            .params
            .iter()
            .map(|a| naming::avoid(&ident(&a.name), &reserved))
            .collect();
        let name = ident(&m.name);
        let method = kt_string(&naming::camel(&m.name));
        w.block(format!("{ids}.{} ->", naming::upper_snake(&m.name)), |w| {
            for (a, local) in m.params.iter().zip(&locals) {
                let expr = self.read_expr(&a.ty, "args", &none);
                w.line(format!("val {local} = {expr}"));
            }
            w.line("args.finish()");
            let call = format!("it.{name}({})", locals.join(", "));
            let Some(ok) = ask_ok(m) else {
                w.line(format!(
                    "UndraCallbackInvocation.Notify({method}) {{ {call} }}"
                ));
                return;
            };
            let err = Ret::classify(&m.returns)
                .as_ref()
                .and_then(Ret::error)
                .map(str::to_owned);
            let encode = |cx: &mut Ctx<'_>, w: &mut CodeWriter| {
                if matches!(ok, TypeRef::Unit) {
                    w.line(call.clone());
                    w.line("ByteArray(0)");
                } else {
                    cx.import("dev.undra.runtime.wire.encodeToByteArray");
                    let codec = cx.codec(ok, &Default::default());
                    w.line(format!("{codec}.encodeToByteArray({call})"));
                }
            };
            w.block_with(
                format!("UndraCallbackInvocation.Ask({method}) {{"),
                "}",
                |w| match &err {
                    Some(err) => {
                        self.import("dev.undra.runtime.UndraPortException");
                        self.import("dev.undra.runtime.wire.encodeToByteArray");
                        let err = self.err_name(err);
                        w.line("try {");
                        w.indented(|w| encode(self, w));
                        w.line(format!("}} catch (e: {err}) {{"));
                        w.indented(|w| {
                            w.line(format!(
                                "throw UndraPortException({err}.encodeToByteArray(e))"
                            ));
                        });
                        w.line("}");
                    }
                    None => encode(self, w),
                },
            );
        });
    }
}
