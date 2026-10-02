//! Host callback interfaces in the TypeScript output (ADR-041).
//!
//! `callbacks.ts`, written only when the schema has callbacks, declares per `#[undra::callback]`
//! trait the interface the app implements (`export interface Reporter`), the weak wrapper
//! (`weakReporter(target)`) and the bridge the runtime delivers the core's calls through
//! (`ReporterCallback`, a `CallbackInterface`). A callback parameter is lent for the call
//! (`lending(core, (lend) => ...)`, which gives the reference back when the call never reached
//! the core or was refused).

use undra_meta::{MethodDef, PortDef, TypeRef, ids};

use super::{Ctx, Module, hex, js_string, jsdoc, param_ident};
use crate::GeneratedFile;
use crate::emit::CodeWriter;
use crate::model::{CallbackUse, Ret};
use crate::naming;

/// The names a bridge's decoder writes besides the method's parameters.
const BRIDGE_LOCALS: &[&str] = &["r", "impl", "signal", "error"];

impl Ctx<'_> {
    /// Imports the interface `name` for a type position.
    pub(super) fn use_callback_type(&mut self, name: &str) {
        self.local_type(Module::Callbacks, name);
    }

    /// The name of the bridge of the interface `name`, imported.
    fn use_bridge(&mut self, name: &str) -> String {
        let bridge = format!("{name}Callback");
        self.local_value(Module::Callbacks, &bridge);
        bridge
    }

    /// The statement that writes the callback parameter `value` with `writer`, lent with `lend`
    /// (the function `lending` passes its `send`).
    pub(super) fn write_callback(
        &mut self,
        callback: &CallbackUse<'_>,
        value: &str,
        writer: &str,
        lend: &str,
    ) -> String {
        let bridge = self.use_bridge(callback.name());
        match callback {
            CallbackUse::One(_) => format!("{writer}.writeU64({lend}({value}, {bridge}));"),
            CallbackUse::Optional(_) => {
                let codec = self.handle_codec(true);
                format!(
                    "{codec}.encode({writer}, {value} === null ? null : {lend}({value}, {bridge}));"
                )
            }
        }
    }

    /// The interface, weak wrapper and bridge of one callback trait.
    fn callback_interface(&mut self, w: &mut CodeWriter, p: &PortDef) {
        self.ids();
        let name = &p.name;
        jsdoc(w, &p.docs, &[]);
        w.block(format!("export interface {name}"), |w| {
            for m in &p.methods {
                let mut extra = Vec::new();
                let mut params = self.param_list(&m.params);
                let ret = if m.is_async {
                    match Ret::classify(&m.returns).and_then(|r| r.error()) {
                        Some(err) => {
                            extra.push(format!(
                                "Reject with `{err}` to fail with it; any other failure is reported to `onError`"
                            ));
                            extra.push("and the core hears \"unavailable\".".to_owned());
                        }
                        None => extra.push(
                            "A failure is reported to `onError` and the core hears \"unavailable\"."
                                .to_owned(),
                        ),
                    }
                    extra.push(
                        "@param signal Aborted when the core stops waiting for the answer.".to_owned(),
                    );
                    params.push("signal: AbortSignal".to_owned());
                    format!("Promise<{}>", self.ok_type(m))
                } else {
                    "void".to_owned()
                };
                jsdoc(w, &m.docs, &extra);
                w.call(member(m), &params, format!(": {ret};"), true);
            }
        });
        w.blank();
        self.weak_wrapper(w, p);
        w.blank();
        self.bridge(w, p);
    }

    /// The success type of an `async` callback method.
    fn ok_type(&mut self, m: &MethodDef) -> String {
        match Ret::classify(&m.returns) {
            Some(Ret::Result { ok, .. } | Ret::Plain(ok)) => self.ty(ok),
            _ => "void".to_owned(),
        }
    }

    /// `weak<Name>(target)`: forwards while `target` lives (ADR-041 decision 9).
    fn weak_wrapper(&mut self, w: &mut CodeWriter, p: &PortDef) {
        let name = &p.name;
        let asynchronous: Vec<String> = p
            .methods
            .iter()
            .filter(|m| m.is_async)
            .map(|m| format!("`{}`", member(m)))
            .collect();
        let gone = if asynchronous.is_empty() {
            "its methods do nothing".to_owned()
        } else {
            format!(
                "its methods do nothing ({} {} the core \"unavailable\")",
                asynchronous.join(", "),
                if asynchronous.len() == 1 {
                    "answers"
                } else {
                    "answer"
                }
            )
        };
        let text = format!(
            "{} that holds `target` weakly (ADR-041): it forwards while `target` is alive; once `target` was\ngarbage-collected {gone}.\nPass it instead of `target` when the core must not keep `target` alive.",
            article(name, true)
        );
        jsdoc(w, &text, &[]);
        let fn_name = format!("weak{name}");
        w.block(
            format!("export function {fn_name}(target: {name}): {name}"),
            |w| {
                w.line("const ref = new WeakRef(target);");
                w.block_with("return {", "};", |w| {
                    for m in &p.methods {
                        let mut args: Vec<String> =
                            m.params.iter().map(|a| weak_ident(&a.name)).collect();
                        if m.is_async {
                            args.push("signal".to_owned());
                        }
                        let list = args.join(", ");
                        let head = format!("({list})");
                        let call = format!("ref.deref()?.{}({list})", member(m));
                        if m.is_async {
                            self.rt_value("callbackGone");
                            w.line(format!(
                                "{}: {head} => {call} ?? callbackGone(),",
                                member(m)
                            ));
                        } else {
                            w.line(format!("{}: {head} => {call},", member(m)));
                        }
                    }
                });
            },
        );
    }

    /// `<Name>Callback`: the ids and methods the runtime delivers the core's calls through.
    fn bridge(&mut self, w: &mut CodeWriter, p: &PortDef) {
        let name = &p.name;
        self.rt_type("CallbackInterface");
        jsdoc(
            w,
            &format!(
                "How the core's calls reach {} the app passed in (ADR-041), for the runtime: the ids and the\ndecoding of each method. Generated calls lend through it; an app does not use it.",
                article(name, false)
            ),
            &[],
        );
        let ids = format!("UndraIds.Callbacks.{name}");
        w.block_with(
            format!("export const {name}Callback: CallbackInterface<{name}> = {{"),
            "};",
            |w| {
                w.line(format!("name: {},", js_string(name)));
                w.line(format!("portId: {ids}.portId,"));
                w.line(format!("releaseInstance: {ids}.releaseInstance,"));
                w.line(format!("cancelCall: {ids}.cancelCall,"));
                if p.background {
                    w.line("background: true,");
                }
                w.block_with("methods: {", "},", |w| {
                    for m in &p.methods {
                        self.bridge_method(w, &ids, m);
                    }
                });
            },
        );
    }

    fn bridge_method(&mut self, w: &mut CodeWriter, ids: &str, m: &MethodDef) {
        let member = member(m);
        let idents: Vec<String> = m
            .params
            .iter()
            .map(|a| naming::avoid(&param_ident(&a.name), BRIDGE_LOCALS))
            .collect();
        w.block_with(format!("[{ids}.{member}]: {{"), "},", |w| {
            w.line(format!("name: {},", js_string(&member)));
            if m.coalesce {
                w.line("coalesce: true,");
            }
            let kind = if m.is_async { "call" } else { "notify" };
            let reader = if m.params.is_empty() { "()" } else { "(r)" };
            w.block_with(format!("{kind}: {reader} => {{"), "},", |w| {
                for (a, ident) in m.params.iter().zip(&idents) {
                    let expr = self.read_expr(&a.ty, "r");
                    w.line(format!("const {ident} = {expr};"));
                }
                if !m.is_async {
                    w.line(format!("return (impl) => impl.{member}({});", idents.join(", ")));
                    return;
                }
                let mut args = idents.clone();
                args.push("signal".to_owned());
                let call = format!("await impl.{member}({})", args.join(", "));
                let ret = Ret::classify(&m.returns);
                let (ok, err) = match &ret {
                    Some(Ret::Result { ok, err }) => (*ok, Some(*err)),
                    Some(Ret::Plain(ok)) => (*ok, None),
                    _ => (&TypeRef::Unit, None),
                };
                self.rt_value("encodeValue");
                w.block_with("return async (impl, signal) => {", "};", |w| {
                    let answer = |cx: &mut Self, w: &mut CodeWriter| {
                        if matches!(ok, TypeRef::Unit) {
                            w.line(format!("{call};"));
                            w.line("return new Uint8Array(0);");
                        } else {
                            let codec = cx.codec(ok);
                            w.line(format!("return encodeValue({codec}, {call});"));
                        }
                    };
                    match err {
                        Some(err) => {
                            self.rt_value("UndraPortError");
                            self.use_value(err, err);
                            let err_codec = self.codec(&TypeRef::named(err));
                            w.line("try {");
                            w.indented(|w| answer(self, w));
                            w.line("} catch (error) {");
                            w.indented(|w| {
                                w.line(format!(
                                    "if (error instanceof {err}) throw new UndraPortError(encodeValue({err_codec}, error));"
                                ));
                                w.line("throw error;");
                            });
                            w.line("}");
                        }
                        None => answer(self, w),
                    }
                });
            });
        });
    }
}

/// "a `Name`" or "an `Name`", capitalised for the start of a sentence when `capital`.
fn article(name: &str, capital: bool) -> String {
    let vowel = name
        .chars()
        .next()
        .is_some_and(|c| matches!(c.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u'));
    let word = match (vowel, capital) {
        (true, true) => "An",
        (true, false) => "an",
        (false, true) => "A",
        (false, false) => "a",
    };
    format!("{word} `{name}`")
}

/// The TypeScript name of a callback method.
fn member(m: &MethodDef) -> String {
    naming::ts_member(&naming::camel(&m.name))
}

/// A parameter of a weak wrapper's arrow function, clear of its locals.
fn weak_ident(name: &str) -> String {
    naming::avoid(&param_ident(name), &["ref", "target", "signal"])
}

impl super::TsGen<'_> {
    /// `callbacks.ts`: one interface, weak wrapper and bridge per callback trait.
    pub(super) fn callbacks_file(&self) -> GeneratedFile {
        let mut cx = Ctx::new(self, Module::Callbacks);
        let mut w = CodeWriter::new("  ");
        for p in &self.model.callbacks {
            cx.callback_interface(&mut w, p);
            w.blank();
        }
        cx.body = w;
        self.assemble("src/callbacks.ts", cx)
    }

    /// The `Callbacks` entries of `UndraIds`: per interface its port id, method ids and the two
    /// reserved methods.
    pub(super) fn callback_ids(&self, w: &mut CodeWriter) {
        if self.model.callbacks.is_empty() {
            return;
        }
        w.block_with("Callbacks: {", "},", |w| {
            for p in &self.model.callbacks {
                w.block_with(format!("{}: {{", p.name), "},", |w| {
                    w.line(format!("portId: {},", hex(p.port_id)));
                    for m in &p.methods {
                        w.line(format!("{}: {},", member(m), hex(m.method_id)));
                    }
                    w.line(format!(
                        "releaseInstance: {},",
                        hex(ids::callback_release_id(&p.name))
                    ));
                    w.line(format!(
                        "cancelCall: {},",
                        hex(ids::callback_cancel_id(&p.name))
                    ));
                });
            }
        });
    }
}
