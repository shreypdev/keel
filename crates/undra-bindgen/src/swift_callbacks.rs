//! `Callbacks.swift` (ADR-041): for each host callback interface, the protocol the app implements,
//! its weak forwarding wrapper and the bridge the core's entry installs (`UndraCallbackInterface`).
//!
//! A main-thread interface is a `@MainActor` protocol (the runtime delivers through the mirror's
//! drain, after the store changes the core committed before the call); a background one is a
//! `Sendable` protocol with nonisolated requirements (delivered on a serial queue per instance).
//! Both refine `AnyObject` (an instance is interned by identity) and `Sendable` (an implementation
//! is handed to calls made from any isolation). Fire-and-forget methods return nothing and do not
//! throw; asynchronous ones throw their error type, typed like port requirements
//! ([`crate::Generator::swift_typed_throws`]).

use undra_meta::ids::{callback_cancel_id, callback_release_id};
use undra_meta::{MethodDef, PortDef, TypeRef};

use super::{SwiftGen, doc, hex, id, swift_string};
use crate::emit::CodeWriter;
use crate::model::{Ret, doc_lines};
use crate::naming;

impl SwiftGen<'_> {
    /// The file, or `None` for a schema without callback interfaces.
    pub(super) fn callbacks_file(&self) -> Option<String> {
        if self.model.callbacks.is_empty() {
            return None;
        }
        let mut w = self.header(&["Foundation", "UndraRuntime"]);
        for interface in &self.model.callbacks {
            self.callback_protocol(&mut w, interface);
            w.blank();
            self.weak_wrapper(&mut w, interface);
            w.blank();
            self.callback_bridge(&mut w, interface);
            w.blank();
        }
        Some(w.finish())
    }

    /// The global holding the bridge of `interface`: `uploadListenerCallbacks`.
    pub(super) fn bridge_name(interface: &PortDef) -> String {
        format!("{}Callbacks", naming::camel(&interface.name))
    }

    /// The `UndraIds.Callbacks` block.
    pub(super) fn callback_ids(&self, w: &mut CodeWriter) {
        if self.model.callbacks.is_empty() {
            return;
        }
        w.blank();
        w.block("public enum Callbacks", |w| {
            for p in &self.model.callbacks {
                w.block(format!("public enum {}", p.name), |w| {
                    w.line(format!(
                        "public static let portId: UInt32 = {}",
                        hex(p.port_id)
                    ));
                    for method in &p.methods {
                        w.line(format!(
                            "public static let {}: UInt32 = {}",
                            id(&method.name),
                            hex(method.method_id)
                        ));
                    }
                    w.line(format!(
                        "public static let releaseInstance: UInt32 = {}",
                        hex(callback_release_id(&p.name))
                    ));
                    w.line(format!(
                        "public static let cancelCall: UInt32 = {}",
                        hex(callback_cancel_id(&p.name))
                    ));
                });
            }
        });
    }

    /// The declaration of one requirement: `func confirm(question: String) async throws(E) -> Bool`.
    fn requirement(&self, m: &MethodDef) -> String {
        let t = self.types();
        let ret = Ret::classify(&m.returns).unwrap_or(Ret::Plain(&m.returns));
        let params = self.param_decls(&m.params).join(", ");
        let asyncw = if m.is_async { " async" } else { "" };
        let throws = self.port_throws_clause(ret.error());
        let returns = match &ret {
            Ret::Plain(ty) | Ret::Result { ok: ty, .. } if !matches!(ty, TypeRef::Unit) => {
                format!(" -> {}", t.ty(ty))
            }
            _ => String::new(),
        };
        format!("func {}({params}){asyncw}{throws}{returns}", id(&m.name))
    }

    /// The arguments of a call of `m` passing its parameters on, labeled as declared.
    fn forwarded(&self, m: &MethodDef, idents: &[String]) -> String {
        let unlabeled = self.unlabeled(&m.params);
        m.params
            .iter()
            .zip(idents)
            .map(|(p, ident)| {
                if unlabeled {
                    ident.clone()
                } else {
                    format!("{}: {ident}", id(&p.name))
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn callback_protocol(&self, w: &mut CodeWriter, p: &PortDef) {
        let mut extra = Vec::new();
        if !doc_lines(&p.docs).is_empty() {
            extra.push(String::new());
        }
        if p.background {
            extra.push(
                "The core calls an implementation off the main thread, one call at a time per"
                    .to_owned(),
            );
            extra.push("instance, in the order it made them. Pass an implementation to a method that takes one;".to_owned());
            extra.push(format!(
                "the core keeps it until it lets go of what it gave it to (``Weak{}`` keeps it weakly).",
                p.name
            ));
        } else {
            extra.push("The core calls an implementation on the main actor, after the store changes it made".to_owned());
            extra.push("before the call are applied. Pass an implementation to a method that takes one; the core".to_owned());
            extra.push(format!(
                "keeps it until it lets go of what it gave it to (``Weak{}`` keeps it weakly).",
                p.name
            ));
        }
        doc(w, &p.docs, &extra);
        let head = if p.background {
            format!("public protocol {}: AnyObject, Sendable", p.name)
        } else {
            w.line("@MainActor");
            format!("public protocol {}: AnyObject, Sendable", p.name)
        };
        w.block(head, |w| {
            for m in &p.methods {
                let ret = Ret::classify(&m.returns).unwrap_or(Ret::Plain(&m.returns));
                let mut extra = Vec::new();
                if let Some(err) = ret.error() {
                    extra.push(format!("- Throws: ``{err}``."));
                }
                doc(w, &m.docs, &extra);
                w.line(self.requirement(m));
            }
        });
    }

    fn weak_wrapper(&self, w: &mut CodeWriter, p: &PortDef) {
        let name = &p.name;
        let weak = format!("Weak{name}");
        let article = if name.starts_with(['A', 'E', 'I', 'O', 'U']) {
            "An"
        } else {
            "A"
        };
        doc(
            w,
            &format!(
                "{article} ``{name}`` that forwards to `target` while it lives, without keeping it alive: what to\n\
                 pass when the implementation holds (directly or not) the object it is passed to. Once\n\
                 `target` is gone the core's calls do nothing, and its questions are answered as\n\
                 unavailable."
            ),
            &[],
        );
        let head = if p.background {
            format!("public final class {weak}: {name}, UndraWeakCallback, @unchecked Sendable")
        } else {
            w.line("@MainActor");
            format!("public final class {weak}: {name}, UndraWeakMainCallback")
        };
        w.block(head, |w| {
            w.line(format!(
                "/// The ``{name}`` the calls go to, while it lives."
            ));
            w.line(format!("public private(set) weak var target: (any {name})?"));
            w.blank();
            w.line("/// Forwards to `target` without keeping it alive.");
            w.block(format!("public init(_ target: any {name})"), |w| {
                w.line("self.target = target");
            });
            w.blank();
            w.line("/// `target`, for the runtime, which calls it directly.");
            w.block("public var undraTarget: AnyObject?", |w| {
                w.line("return target");
            });
            for m in &p.methods {
                w.blank();
                let taken: Vec<String> = m.params.iter().map(|q| id(&q.name)).collect();
                let target = naming::avoid(
                    "target",
                    &taken.iter().map(String::as_str).collect::<Vec<_>>(),
                );
                let call = format!("{}({})", id(&m.name), self.forwarded(m, &taken));
                w.block(format!("public {}", self.requirement(m)), |w| {
                    if !m.is_async {
                        w.line(format!("target?.{call}"));
                        return;
                    }
                    w.block(format!("guard let {target} = target else"), |w| {
                        if self.typed() {
                            w.line("return await UndraCallbacks.targetGone()");
                        } else {
                            w.line(format!(
                                "throw UndraCallError.unavailable(.closed) // {weak}'s target is gone"
                            ));
                        }
                    });
                    let ret = Ret::classify(&m.returns).unwrap_or(Ret::Plain(&m.returns));
                    let tried = if ret.error().is_some() { "try " } else { "" };
                    w.line(format!("return {tried}await {target}.{call}"));
                });
            }
        });
    }

    fn callback_bridge(&self, w: &mut CodeWriter, p: &PortDef) {
        let ids = format!("UndraIds.Callbacks.{}", p.name);
        doc(
            w,
            &format!(
                "How the core reaches the app's ``{}`` implementations: the bridge the core's entry installs\n\
                 when it loads.",
                p.name
            ),
            &[],
        );
        w.line(format!(
            "let {} = UndraCallbackInterface(",
            Self::bridge_name(p)
        ));
        w.indented(|w| {
            w.line(format!("name: {},", swift_string(&p.name)));
            w.line(format!("portId: {ids}.portId,"));
            w.line(format!("releaseInstance: {ids}.releaseInstance,"));
            w.line(format!("cancelCall: {ids}.cancelCall,"));
            w.line("methods: [");
            w.indented(|w| {
                for m in &p.methods {
                    self.bridge_method(w, p, m, &ids);
                }
            });
            w.line("]");
        });
        w.line(")");
    }

    fn bridge_method(&self, w: &mut CodeWriter, p: &PortDef, m: &MethodDef, ids: &str) {
        let t = self.types();
        let ret = Ret::classify(&m.returns).unwrap_or(Ret::Plain(&m.returns));
        let taken: Vec<String> = m.params.iter().map(|q| id(&q.name)).collect();
        let mut reserved: Vec<&str> = taken.iter().map(String::as_str).collect();
        reserved.extend(["args", "result", "error"]);
        let implementation = naming::avoid(&naming::camel(&p.name), &reserved);
        let idents: Vec<String> = taken
            .iter()
            .map(|n| naming::avoid(n, &["args", "result", "error", implementation.as_str()]))
            .collect();
        let method = id(&m.name);
        let plain = method.trim_matches('`').to_owned();
        let factory = match (p.background, m.is_async) {
            (false, false) if m.coalesce => {
                format!(".notify({}, coalesce: true)", swift_string(&plain))
            }
            (false, false) => format!(".notify({})", swift_string(&plain)),
            (false, true) => format!(".call({})", swift_string(&plain)),
            (true, false) => format!(".notifyInBackground({})", swift_string(&plain)),
            (true, true) => format!(".callInBackground({})", swift_string(&plain)),
        };
        let signature = if m.is_async {
            format!(
                "({implementation}: any {}, args: inout UndraReader) async throws -> [UInt8] in",
                p.name
            )
        } else {
            format!(
                "({implementation}: any {}, args: inout UndraReader) in",
                p.name
            )
        };
        w.line(format!("{ids}.{method}: {factory} {{"));
        w.indented(|w| {
            w.line(signature);
            for (a, ident) in m.params.iter().zip(&idents) {
                w.line(format!("let {ident} = try {}", t.read_expr(&a.ty, "args")));
            }
            w.line("try args.finish()");
            let call = format!("{implementation}.{method}({})", self.forwarded(m, &idents));
            if !m.is_async {
                w.line(call);
                return;
            }
            let ok = match &ret {
                Ret::Plain(ty) | Ret::Result { ok: ty, .. } => *ty,
                Ret::Stream(_) | Ret::ResultStream { .. } => &TypeRef::Unit,
            };
            let finish = |w: &mut CodeWriter| {
                let tried = if ret.error().is_some() { "try " } else { "" };
                if matches!(ok, TypeRef::Unit) {
                    w.line(format!("{tried}await {call}"));
                    w.line("return []");
                } else {
                    w.line(format!("let result = {tried}await {call}"));
                    w.line(format!("return {}", t.encoded(ok, "result")));
                }
            };
            match ret.error() {
                Some(err) => {
                    w.line("do {");
                    w.indented(finish);
                    w.line(format!("}} catch let error as {err} {{"));
                    w.indented(|w| {
                        w.line("throw UndraPortError(body: error.undraEncoded())");
                    });
                    w.line("}");
                }
                None => finish(w),
            }
        });
        w.line("},");
    }
}
