//! `#[undra::store]`: stores, objects whose fields are signals the platforms mirror.
//!
//! # Signals
//!
//! Fields typed `Signal<T>`, `Computed<T>`, `DerivedList<T>` and `Lazy<T>` are the store's signals,
//! numbered `0..n` in declaration order (other fields are private state, `Ctx` included).
//! `#[undra(key = "id")]` on a `Signal<Vec<T>>` makes it a keyed list that ships patches; a
//! `DerivedList<T>` (ADR-039) must have one, and is described as a read-only keyed list
//! (`computed: true` with a `key`, the schema of a `Vec<T>`);
//! `#[undra(no_coalesce)]` makes every commit of a signal reach the platforms, and is recorded in
//! the signal's schema entry so their mirrors apply every one of them (ADR-031). A `Lazy<T>`
//! (ADR-043) is a list the platforms page through instead of mirroring: its schema type is
//! `Lazy(T)`, it is persisted as the `Vec<T>` of its items (so it restores like a plain signal and
//! takes `#[undra(default)]`), and `#[undra(key = "id")]` names the field that identifies its
//! rows for the platform. `Lazy<T>` is legal only as a store field: inside a `Signal`, `Computed` or
//! `DerivedList` it is E0001.
//!
//! # The hidden cell
//!
//! Every store instance owns one `StoreCell` (the per-instance signal registry the runtime
//! observes). Rust has no way to add state to a struct without the struct having a field
//! for it, so the macro **appends a field**:
//!
//! ```ignore
//! #[doc(hidden)] pub __undra_cell: ::undra::signals::CellSlot
//! ```
//!
//! `CellSlot` is `Default` and creates the cell on first use. To keep the ergonomic
//! `Self { ctx, todos, filter, visible }` working, the `#[undra::api(store)]` impl block of the
//! store rewrites struct literals of `Self`/the type inside its own methods to add
//! `__undra_cell: Default::default()` (see `object.rs`). Struct literals anywhere else must
//! spell the field out.
//!
//! The cell is created and its signals attached the first time `StoreObject::cell()` is
//! called, or explicitly by the constructor's dispatch arm (`__undra_attach_all`), which also
//! gives the cell its handle (`__undra_set_handle`). Attaching can fail (`SignalsError`, for
//! example when a signal already belongs to another store), so `__undra_attach_all` returns
//! the error and the constructor's dispatch arm answers `DispatchResult::BadRequest` with its
//! text instead of publishing a store that cannot deliver. The hidden methods and constants
//! below are how the impl block's generated code talks to the struct without naming its
//! fields.
//!
//! Signals are attached with the `StoreCell` family that fits the field: `attach` for a plain
//! `Signal<T>`, `attach_keyed` (with a typed `fn(&Item) -> u64` that hashes the encoded key
//! field) for `#[undra(key = "..")]`, `attach_computed` for a `Computed<T>`, `attach_derived` (with
//! the same key function) for a `DerivedList<T>`, `attach_lazy` for a `Lazy<T>`, and
//! `set_no_coalesce` after any of them for `#[undra(no_coalesce)]`.
//!
//! # Restore
//!
//! `StoreObject::restore(ctx, r)` reads the store body of a snapshot (SPEC 5.9): `signal_count
//! u32`, then per signal `signal_id u32` and a length-prefixed value. Only non-computed
//! signals are stored. The store is then rebuilt in one of two ways:
//!
//! * automatically, if every non-signal field is a `Ctx` (cloned from the argument), a `WeakCtx`
//!   (downgraded from it; what a store should keep, ADR-034) or `Default`, and there are no
//!   `Computed` or `DerivedList` fields: a struct literal;
//! * through a hook, `#[undra::store(restore = "Self::rebuild")]`, with the signature
//!   `fn(ctx: Ctx, <one Signal<T> (or Lazy<T>) per non-computed signal, in order>) -> Self`. Use it
//!   when the store has computed fields (only your code knows how to derive them), a `Lazy::over`
//!   view (derived data: a snapshot holds an empty list for it, and the hook rebuilds the view from
//!   the list it is derived from) or other state without a `Default`.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Fields, ItemStruct};

use super::attrs::{Site, take};
use super::check::{Checks, on_unimplemented};
use super::common::{check_generics, item_root};
use super::diag::{Diag, Errors, code};
use super::naming::unraw;
use super::paths::Root;
use super::types::{Allow, KType, Pos, map_type, ty_string};

/// The kind of a signal field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SigKind {
    Signal,
    Computed,
    /// `DerivedList<T>` (ADR-039): computed by the core, shipped as keyed patches.
    Derived,
    /// `Lazy<T>` (ADR-043): a list the platforms page through; store state like a plain signal.
    Lazy,
}

impl SigKind {
    /// Evaluated by the core, not written: read-only on the platforms, left out of snapshots,
    /// rebuilt by the restore hook.
    fn is_computed(self) -> bool {
        matches!(self, SigKind::Computed | SigKind::Derived)
    }

    /// A plain value of the store, written by it and persisted: a `Signal<T>` or a `Lazy<T>`.
    fn is_plain(self) -> bool {
        matches!(self, SigKind::Signal | SigKind::Lazy)
    }
}

struct SignalField {
    ident: syn::Ident,
    name: String,
    id: u32,
    kind: SigKind,
    /// The `T` of `Signal<T>` / `Computed<T>`, and `Vec<T>` for a `DerivedList<T>` or a `Lazy<T>`:
    /// the value the platforms see (a `Lazy` is persisted as the `Vec` of its items).
    value_ty: syn::Type,
    /// The item type `T` of a `Lazy<T>`.
    lazy_item: Option<syn::Type>,
    /// The schema type of the signal.
    kty: KType,
    /// `#[undra(key = "..")]`: the key field and the list's item type.
    key: Option<KeyedList>,
    no_coalesce: bool,
    /// `#[undra(default)]` on a `Signal<T>`: a snapshot without the signal restores it with
    /// `T::default()` (ADR-037).
    default: bool,
}

/// How a non-signal field is filled when the store is restored automatically.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CtxField {
    /// Not a context: `Default::default()`.
    No,
    /// A `Ctx`: cloned from the restore context.
    Strong,
    /// A `WeakCtx`: downgraded from the restore context (ADR-034, decision 7).
    Weak,
}

/// `#[undra(key = "..")]` on a `Signal<Vec<Item>>`.
struct KeyedList {
    /// The field of `Item` that identifies it, as written.
    name: String,
    /// The string it was written in, where the error about it is reported.
    lit: syn::LitStr,
    /// `Item`, as written (it may be a `Box<Row>`).
    item_ty: syn::Type,
}

struct StateField {
    ident: syn::Ident,
    ty: syn::Type,
    ctx: CtxField,
}

/// The last path segment of a one-argument generic type (`Signal<T>`), with its `T`.
fn wrapper_of(ty: &syn::Type) -> Option<(String, syn::Type)> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    if path.qself.is_some() {
        return None;
    }
    let seg = path.path.segments.last()?;
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    let mut types = args.args.iter().filter_map(|arg| match arg {
        syn::GenericArgument::Type(t) => Some(t),
        _ => None,
    });
    let value = types.next()?;
    if types.next().is_some() {
        return None;
    }
    Some((seg.ident.to_string(), value.clone()))
}

/// `Signal<T>`, `Computed<T>`, `DerivedList<T>` or `Lazy<T>` (by last path segment) with its `T`.
fn signal_wrapper(ty: &syn::Type) -> Option<(SigKind, syn::Type)> {
    let (name, value) = wrapper_of(ty)?;
    match name.as_str() {
        "Signal" => Some((SigKind::Signal, value)),
        "Computed" => Some((SigKind::Computed, value)),
        "DerivedList" => Some((SigKind::Derived, value)),
        "Lazy" => Some((SigKind::Lazy, value)),
        _ => None,
    }
}

/// `name` spelled without exactly one type argument (`DerivedList`, `Lazy`).
fn is_bare(ty: &syn::Type, name: &str) -> bool {
    let syn::Type::Path(path) = ty else {
        return false;
    };
    path.qself.is_none()
        && path
            .path
            .segments
            .last()
            .is_some_and(|seg| seg.ident == name)
        && wrapper_of(ty).is_none()
}

/// `DerivedList` spelled without exactly one type argument.
fn is_bare_derived(ty: &syn::Type) -> bool {
    is_bare(ty, "DerivedList")
}

/// `Lazy` spelled without exactly one type argument.
fn is_bare_lazy(ty: &syn::Type) -> bool {
    is_bare(ty, "Lazy")
}

/// `Lazy<T>` (by last path segment).
fn is_lazy(ty: &syn::Type) -> bool {
    wrapper_of(ty).is_some_and(|(name, _)| name == "Lazy")
}

/// `Vec<Item>` -> `Item`.
fn vec_item(ty: &syn::Type) -> Option<syn::Type> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    let seg = path.path.segments.last()?;
    if seg.ident != "Vec" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    match args.args.first()? {
        syn::GenericArgument::Type(item) => Some(item.clone()),
        _ => None,
    }
}

fn ctx_field(ty: &syn::Type) -> CtxField {
    let syn::Type::Path(path) = ty else {
        return CtxField::No;
    };
    if path.qself.is_some() {
        return CtxField::No;
    }
    match path.path.segments.last() {
        Some(seg) if seg.arguments.is_none() && seg.ident == "Ctx" => CtxField::Strong,
        Some(seg) if seg.arguments.is_none() && seg.ident == "WeakCtx" => CtxField::Weak,
        _ => CtxField::No,
    }
}

/// `Box<Box<Row>>` -> (`Row`, 2).
fn peel_box(ty: &syn::Type) -> (&syn::Type, usize) {
    let mut ty = ty;
    let mut depth = 0;
    while let syn::Type::Path(path) = ty {
        let Some(seg) = path.path.segments.last() else {
            break;
        };
        let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
            break;
        };
        match (seg.ident == "Box", args.args.first()) {
            (true, Some(syn::GenericArgument::Type(inner))) if args.args.len() == 1 => {
                ty = inner;
                depth += 1;
            }
            _ => break,
        }
    }
    (ty, depth)
}

/// The key as the field it reads: `id`, `r#type` for a keyword, and for a key that is not an
/// identifier at all a name no field has (the constant check of [`key_function`] has already
/// failed for it, so the access is never type-checked against a real type).
fn key_field(name: &str, span: Span) -> syn::Ident {
    let mut ident = syn::parse_str::<syn::Ident>(name)
        .or_else(|_| syn::parse_str::<syn::Ident>(&format!("r#{name}")))
        .unwrap_or_else(|_| format_ident!("__undra_not_a_field_name"));
    ident.set_span(span);
    ident
}

/// The key function of a keyed list: `fn(&Item) -> u64`, the FNV-1a hash of the encoded key field.
///
/// The macro cannot see the fields of `Item`, so the lookup of the key's name happens in constants
/// in the user's crate: `Item::__UNDRA_FIELDS` (every `#[undra::api]` record has it) is searched
/// by `undra_meta::keys::index_of`, and a key that names no field panics with a diagnostic that
/// lists the fields there are (E0008), reported on the string where the key was written. A type
/// that is not a record has no such constant: the trait declared in the function gives it an empty
/// one, and the same diagnostic says it has no fields.
///
/// The field is then read by name, through a reference whose type only exists when the check
/// passed (`<__UndraGate<{ ok }> as __UndraPass<Item>>::Out`, which is `Item`): when the check
/// fails the reference has no type, so `rustc` adds no "no field `idd`" error, with its own
/// suggestion, after the diagnostic. Records therefore carry a constant and nothing else.
fn key_function(root: &Root, signal: &SignalField, keyed: &KeyedList) -> TokenStream {
    let meta = root.meta();
    let wire = root.wire();
    let fn_name = format_ident!("__undra_key_{}", signal.ident);
    let item_ty = &keyed.item_ty;
    let (core_ty, depth) = peel_box(item_ty);
    let row = if depth == 0 {
        quote!(__item)
    } else {
        let derefs = (0..=depth).map(|_| quote!(*));
        quote!((&#(#derefs)* __item))
    };

    // The message, with the list of fields left for the constant to fill in.
    const HOLE: &str = "\u{0}";
    let diag = Diag::new(
        code::E0008,
        format!(
            "`#[undra(key = \"{}\")]` on `{}` names no field of `{}`",
            keyed.name,
            signal.name,
            ty_string(core_ty)
        ),
        format!(
            "`key` names the field of the list's items that identifies them, and `{}` has {HOLE}",
            ty_string(core_ty)
        ),
        "write the name of one of those fields as the key",
    );
    let message = diag.message();
    let (before, after) = message.split_once(HOLE).unwrap_or((&message, ""));

    let span = keyed.lit.span();
    let key_name = &keyed.name;
    let field = key_field(key_name, span);
    let check = quote_spanned! {span=>
        const __UNDRA_FIELDS: &[&str] = <#core_ty>::__UNDRA_FIELDS;
        const __UNDRA_INDEX: usize = #meta::keys::index_of(__UNDRA_FIELDS, #key_name);
        const __UNDRA_MESSAGE_LEN: usize = #meta::keys::message_len(#before, __UNDRA_FIELDS, #after);
        const __UNDRA_MESSAGE: [u8; __UNDRA_MESSAGE_LEN] =
            #meta::keys::message::<__UNDRA_MESSAGE_LEN>(#before, __UNDRA_FIELDS, #after);
        const __UNDRA_MESSAGE_TEXT: &str = #meta::keys::as_str(&__UNDRA_MESSAGE);
        const __UNDRA_KEY_IS_A_FIELD: bool = if __UNDRA_INDEX == usize::MAX {
            ::core::panic!("{}", __UNDRA_MESSAGE_TEXT)
        } else {
            true
        };
    };
    // The statements that use the constant have the string's span too, so rustc's "erroneous
    // constant encountered" note lands where the error does and not on the list's item type.
    let encode = quote_spanned! {span=>
        let __row: &<__UndraGate<{ __UNDRA_KEY_IS_A_FIELD }> as __UndraPass<#core_ty>>::Out = #row;
        #wire::Encode::encode(&__row.#field, &mut __buf);
    };
    quote_spanned! {item_ty.span()=>
        #[allow(non_camel_case_types, dead_code)]
        fn #fn_name(__item: &#item_ty) -> u64 {
            trait __UndraKeyed {
                const __UNDRA_FIELDS: &'static [&'static str] = &[];
            }
            impl<__T: ?::core::marker::Sized> __UndraKeyed for __T {}
            // `Out` is `T` only for `__UndraGate<true>`: see the doc of `key_function`.
            struct __UndraGate<const __OK: bool>;
            trait __UndraPass<__T: ?::core::marker::Sized> {
                type Out: ?::core::marker::Sized;
            }
            impl<__T: ?::core::marker::Sized> __UndraPass<__T> for __UndraGate<true> {
                type Out = __T;
            }
            #check
            ::std::thread_local! {
                static __UNDRA_KEY_BUF: ::core::cell::RefCell<#wire::Writer> =
                    ::core::cell::RefCell::new(#wire::Writer::new());
            }
            __UNDRA_KEY_BUF.with(|__buf| {
                let mut __buf = __buf.borrow_mut();
                __buf.clear();
                #encode
                #meta::ids::fnv1a64(__buf.as_slice())
            })
        }
    }
}

/// The diagnostic for a `Lazy<T>` written inside a `Signal`, `Computed` or `DerivedList` (E0001): it
/// is a store field of its own, so a computed or derived value cannot be one.
fn lazy_in_a_signal(outer: &syn::Type, lazy: &syn::Type, kind: SigKind) -> Diag {
    let outer_name = match kind {
        SigKind::Computed => "a `Computed`",
        SigKind::Derived => "a `DerivedList`",
        SigKind::Signal | SigKind::Lazy => "a `Signal`",
    };
    let item = wrapper_of(lazy).map_or_else(|| "Row".to_owned(), |(_, item)| ty_string(&item));
    Diag::new(
        code::E0001,
        format!(
            "`{}` cannot hold a `Lazy`: it is a store field, not a value",
            ty_string(outer)
        ),
        format!(
            "a `Lazy<T>` is a list the core owns and the platforms page through, so it stands as a store field of its own; it is never the value of {outer_name}, which is evaluated, compared and sent as a value"
        ),
        format!(
            "write the field as `Lazy<{item}>`; to page a computed or filtered list, derive it (`source.derive().filter(..).build()`) and build the field with `Lazy::over(&derived)`"
        ),
    )
}

/// Expands `#[undra::store]` on a struct.
pub(crate) fn expand_store(
    args_root: Option<Root>,
    restore_hook: Option<syn::Path>,
    mut item: ItemStruct,
) -> syn::Result<TokenStream> {
    let mut errors = Errors::new();
    let root = item_root(&mut item.attrs, args_root, &mut errors);
    check_generics(&item.generics, &item.ident.to_string(), &mut errors);
    let name = item.ident.clone();
    let name_str = unraw(&name);
    let struct_docs = super::attrs::docs(&item.attrs);

    let mut signals: Vec<SignalField> = Vec::new();
    let mut state: Vec<StateField> = Vec::new();
    match &mut item.fields {
        Fields::Named(named) => {
            for field in &mut named.named {
                let ident = field.ident.clone().expect("named field");
                if ident == "__undra_cell" {
                    errors.push(
                        Diag::new(
                            code::E0007,
                            "the field name `__undra_cell` is reserved",
                            "`#[undra::store]` adds a field with that name to hold the store's signal cell",
                            "rename the field",
                        )
                        .on(&ident),
                    );
                    continue;
                }
                if is_bare_lazy(&field.ty) {
                    take(&mut field.attrs, Site::SIGNAL, &mut errors);
                    errors.push(
                        Diag::new(
                            code::E0001,
                            format!("`{}` needs the type of its rows", ty_string(&field.ty)),
                            "a lazy list is paged by the platforms, which decode each row, so the schema needs the row type, written as the one type argument",
                            format!("write `Lazy<Row>` for `{ident}`, with the type of the list's items"),
                        )
                        .on(&field.ty),
                    );
                    continue;
                }
                if is_bare_derived(&field.ty) {
                    take(&mut field.attrs, Site::SIGNAL, &mut errors);
                    errors.push(
                        Diag::new(
                            code::E0001,
                            format!("`{}` needs the type of its rows", ty_string(&field.ty)),
                            "a derived list is a `Vec` of rows the platforms mirror, so the schema needs the row type, written as the one type argument",
                            format!("write `DerivedList<Row>` for `{ident}`, with the type the pipeline's `build()` produces"),
                        )
                        .on(&field.ty),
                    );
                    continue;
                }
                match signal_wrapper(&field.ty) {
                    Some((kind, item_or_value)) => {
                        // A signal wrapped in a `Signal`, `Computed` or `DerivedList` that is a `Lazy`:
                        // `Lazy<T>` is a field of its own, never a value (E0001, with where it may stand).
                        if kind != SigKind::Lazy && is_lazy(&item_or_value) {
                            take(&mut field.attrs, Site::SIGNAL, &mut errors);
                            errors.push(lazy_in_a_signal(&field.ty, &item_or_value, kind).on(&field.ty));
                            continue;
                        }
                        // A derived list of `T` is a `Vec<T>` to the schema and the type checks; so
                        // is a lazy list, which is persisted as the `Vec<T>` of its items.
                        let (value_ty, derived_item) = if matches!(kind, SigKind::Derived | SigKind::Lazy) {
                            (
                                syn::parse_quote!(::std::vec::Vec<#item_or_value>),
                                Some(item_or_value),
                            )
                        } else {
                            (item_or_value, None)
                        };
                        let attr = take(&mut field.attrs, Site::SIGNAL, &mut errors);
                        if attr.default && kind == SigKind::Computed {
                            errors.push(
                                Diag::new(
                                    code::E0008,
                                    format!("`#[undra(default)]` on `{ident}` needs a `Signal<T>`, not a `Computed<T>`"),
                                    "a computed signal is not persisted: a restore recomputes it, so there is nothing to default",
                                    "remove `#[undra(default)]`, or put it on the plain signal the computed one is derived from",
                                )
                                .on(&field.ty),
                            );
                        }
                        let kty = match map_type(&value_ty, Pos::Signal, Allow::NONE) {
                            Ok(kty) => kty,
                            Err(err) => {
                                errors.push(err.into_error());
                                KType::Unit
                            }
                        };
                        let had_key = attr.key.is_some();
                        let key = attr.key.and_then(|lit| {
                            let key_name = lit.value();
                            let item_ty = match kind {
                                SigKind::Signal => vec_item(&value_ty),
                                SigKind::Derived | SigKind::Lazy => derived_item.clone(),
                                SigKind::Computed => None,
                            };
                            let Some(item_ty) = item_ty else {
                                let what = format!(
                                    "`#[undra(key = \"{key_name}\")]` on `{ident}` needs a `Signal<Vec<T>>`, a `DerivedList<T>` or a `Lazy<T>`"
                                );
                                let diag = if kind == SigKind::Computed {
                                    Diag::new(
                                        code::E0008,
                                        what,
                                        "a computed list is sent whole: the core recomputes it and keeps no record of which rows changed",
                                        "to send it as keyed patches, build it as a `DerivedList<T>` (`source.derive().filter(..).build()`) and key that; otherwise remove `key`",
                                    )
                                } else {
                                    Diag::new(
                                        code::E0008,
                                        what,
                                        "only lists of records can be keyed: the key identifies an item across updates so changes ship as patches",
                                        "use a `Signal<Vec<T>>` field, or remove `key`",
                                    )
                                };
                                errors.push(diag.at(lit.span()));
                                return None;
                            };
                            // Whether `key_name` is a field of the item is checked in the user's
                            // crate, where the item is visible: see `key_function`.
                            Some(KeyedList {
                                name: key_name,
                                lit,
                                item_ty,
                            })
                        });
                        if kind == SigKind::Derived && !had_key {
                            let row = derived_item
                                .as_ref()
                                .map_or_else(|| "T".to_owned(), ty_string);
                            errors.push(
                                Diag::new(
                                    code::E0008,
                                    format!(
                                        "`{ident}` is a `DerivedList<{row}>` without `#[undra(key = \"..\")]`"
                                    ),
                                    "a derived list reaches the platforms as keyed patches; the key names the field that identifies a row",
                                    format!("add `#[undra(key = \"id\")]` naming a field of `{row}`"),
                                )
                                .on(&field.ty),
                            );
                        }
                        let lazy_item = if kind == SigKind::Lazy {
                            derived_item.clone()
                        } else {
                            None
                        };
                        signals.push(SignalField {
                            name: unraw(&ident),
                            ident,
                            id: u32::try_from(signals.len()).unwrap_or(u32::MAX),
                            kind,
                            value_ty,
                            lazy_item,
                            kty,
                            key,
                            no_coalesce: attr.no_coalesce,
                            default: attr.default && kind.is_plain(),
                        });
                    }
                    None => {
                        take(&mut field.attrs, Site::STATE_FIELD, &mut errors);
                        state.push(StateField {
                            ctx: ctx_field(&field.ty),
                            ty: field.ty.clone(),
                            ident,
                        });
                    }
                }
            }
        }
        other => errors.push(
            Diag::new(
                code::E0007,
                format!("store `{name_str}` must be a struct with named fields"),
                "the signals are the struct's named fields; tuple and unit structs have none to describe",
                "write `struct Name { ctx: Ctx, items: Signal<Vec<Item>> }`",
            )
            .on(other),
        ),
    }

    let derived_signals = signals
        .iter()
        .filter(|s| s.kind.is_computed())
        .map(|s| s.name.clone())
        .collect::<Vec<_>>();
    if restore_hook.is_none() && !derived_signals.is_empty() {
        // On the first computed field: it is what makes the store unrestorable.
        let first = signals
            .iter()
            .find(|s| s.kind.is_computed())
            .map_or_else(|| name.clone(), |s| s.ident.clone());
        errors.push(
            Diag::new(
                code::E0013,
                format!(
                    "store `{name_str}` cannot be restored automatically: `{}` is computed",
                    derived_signals.join("`, `")
                ),
                "restoring a snapshot decodes the plain signals and rebuilds the store, but only your code knows how to derive computed signals from them",
                "add `#[undra::store(restore = \"Self::rebuild\")]` with `fn rebuild(ctx: Ctx, <one Signal<T> per plain signal, in order>) -> Self`, the same code `new` uses to build the store",
            )
            .on(&first),
        );
    }
    errors.finish()?;

    let mut checks = Checks::new();
    for signal in &signals {
        checks.ty(&signal.value_ty, &signal.kty);
    }
    let checks = checks.emit(&root);

    // Hidden cell field.
    let signals_path = root.signals();
    if let Fields::Named(named) = &mut item.fields {
        named.named.push(syn::parse_quote! {
            #[doc(hidden)]
            pub __undra_cell: #signals_path::CellSlot
        });
    }

    let wire = root.wire();
    let meta = root.meta();
    let runtime = root.runtime();

    // --- the signal table -------------------------------------------------------------------
    let signal_metas = signals.iter().map(|s| {
        let sname = &s.name;
        let id = s.id;
        // A `Lazy<T>` is `Lazy(T)` in the schema; the platforms page it (ADR-043).
        let ty = match (s.kind, &s.kty) {
            (SigKind::Lazy, KType::Vec(item)) => {
                let item = item.meta(&meta);
                quote!(#meta::TypeRefMeta::Lazy(&#item))
            }
            _ => s.kty.meta(&meta),
        };
        // A derived list is a computed with a key (ADR-039): no new schema field.
        let computed = s.kind.is_computed();
        let key = match &s.key {
            Some(keyed) => {
                let key_name = &keyed.name;
                quote!(::core::option::Option::Some(#key_name))
            }
            None => quote!(::core::option::Option::None),
        };
        // Recorded in the schema so the platform mirrors apply every entry of the signal (ADR-031).
        let no_coalesce = s.no_coalesce;
        // Recorded so a restore knows the signal may be missing from an older snapshot (ADR-037).
        let default = s.default;
        quote! {
            #meta::SignalMeta { name: #sname, signal_id: #id, ty: #ty, computed: #computed, key: #key, no_coalesce: #no_coalesce, default: #default }
        }
    });

    // The key of a keyed list: `fn(&Item) -> u64`, the FNV-1a hash of the encoded key field
    // (what `undra_signals::KeyFn` asks for). The encoding goes through a per-thread scratch
    // buffer, because the function runs for every item of an observed list at every commit.
    let key_fns = signals
        .iter()
        .filter_map(|s| Some(key_function(&root, s, s.key.as_ref()?)));

    // One attach per signal, in declaration order; any failure aborts the whole build.
    let attach_stmts = signals.iter().map(|s| {
        let ident = &s.ident;
        let id = s.id;
        let attach = match (s.kind, &s.key) {
            (SigKind::Signal, Some(_)) => {
                let fn_name = format_ident!("__undra_key_{}", s.ident);
                quote!(__cell.attach_keyed(&self.#ident, #id, #fn_name)?;)
            }
            (SigKind::Signal, None) => quote!(__cell.attach(&self.#ident, #id)?;),
            (SigKind::Computed, _) => quote!(__cell.attach_computed(&self.#ident, #id)?;),
            (SigKind::Derived, Some(_)) => {
                let fn_name = format_ident!("__undra_key_{}", s.ident);
                quote!(__cell.attach_derived(&self.#ident, #id, #fn_name)?;)
            }
            // Refused above (E0008): the build has already failed.
            (SigKind::Derived, None) => TokenStream::new(),
            // The core never diffs a lazy list: the key only names the field that identifies a row
            // for the platforms, and its function is referenced so that a key that names no field
            // is the same E0008 as everywhere else.
            (SigKind::Lazy, Some(_)) => {
                let fn_name = format_ident!("__undra_key_{}", s.ident);
                quote!(let _ = #fn_name; __cell.attach_lazy(&self.#ident, #id)?;)
            }
            (SigKind::Lazy, None) => quote!(__cell.attach_lazy(&self.#ident, #id)?;),
        };
        let coalesce = if s.no_coalesce {
            quote!(__cell.set_no_coalesce(#id)?;)
        } else {
            TokenStream::new()
        };
        quote! { #attach #coalesce }
    });

    // --- restore ----------------------------------------------------------------------------
    let plain: Vec<&SignalField> = signals.iter().filter(|s| s.kind.is_plain()).collect();
    let slots: Vec<syn::Ident> = plain
        .iter()
        .map(|s| format_ident!("__slot_{}", s.ident))
        .collect();
    let values: Vec<syn::Ident> = plain
        .iter()
        .map(|s| format_ident!("__value_{}", s.ident))
        .collect();
    let value_tys: Vec<&syn::Type> = plain.iter().map(|s| &s.value_ty).collect();
    let ids: Vec<u32> = plain.iter().map(|s| s.id).collect();
    // What a signal the snapshot lacks becomes: `T::default()` for a `#[undra(default)]` signal
    // (ADR-037), else a decode error naming it.
    let fallbacks: Vec<TokenStream> = plain
        .iter()
        .map(|s| {
            let value_ty = &s.value_ty;
            if s.default {
                quote_spanned!(value_ty.span()=> <#value_ty as ::core::default::Default>::default())
            } else {
                let id = s.id;
                let missing = format!(
                    "snapshot of store {name_str} is missing signal {} ({})",
                    s.id, s.name
                );
                quote! {
                    return ::core::result::Result::Err(#wire::WireError::InvalidTag {
                        tag: #id,
                        at: __r.position(),
                        ty: #missing,
                    })
                }
            }
        })
        .collect();

    let default_trait = format_ident!("__UndraRestoreDefault_{}", name_str);
    let needs_default = restore_hook.is_none() && state.iter().any(|f| f.ctx == CtxField::No);
    let default_attr = on_unimplemented(
        &Diag::new(
            code::E0013,
            format!(
                "store `{name_str}` cannot be restored automatically: its field of type `{{Self}}` has no `Default`"
            ),
            "restoring a snapshot rebuilds the store from its plain signals and fills every other field with `Default::default()` (a `Ctx` is cloned from the argument, a `WeakCtx` downgraded from it)",
            "implement `Default` for the type, or add `#[undra::store(restore = \"Self::rebuild\")]` with `fn rebuild(ctx: Ctx, <one Signal<T> per plain signal, in order>) -> Self`, the same code `new` uses to build the store",
        ),
        "this field type has no `Default`",
    );
    let default_items = if needs_default {
        quote! {
            #[doc(hidden)]
            #[allow(non_camel_case_types, dead_code)]
            #default_attr
            trait #default_trait: ::core::marker::Sized {
                fn __undra_default() -> Self;
            }
            impl<__UndraT: ::core::default::Default> #default_trait for __UndraT {
                fn __undra_default() -> Self {
                    <__UndraT as ::core::default::Default>::default()
                }
            }
        }
    } else {
        TokenStream::new()
    };

    // A store is constructed by the constructors of its `#[undra::api(store)] impl` block, which
    // also implements `UndraObject`. Without the block `rustc` only says `UndraObject` is missing
    // (and, from `StoreObject`'s supertrait, says so once more); this bound says what to write.
    let impl_attr = on_unimplemented(
        &Diag::new(
            code::E0011,
            format!("store `{name_str}` has no `#[undra::api(store)]` impl block"),
            "the constructors of the impl block create the store and wire its signals to the platforms; without the block the store can never be instantiated",
            format!(
                "add an `impl {name_str}` block marked `#[undra::api(store)]` with a constructor such as `pub fn new(ctx: Ctx) -> Self`"
            ),
        ),
        "this store has no `#[undra::api(store)]` impl block",
    );

    let build = match &restore_hook {
        Some(hook) => {
            let wrapped = plain.iter().zip(&values).map(|(s, value)| {
                let value_ty = &s.value_ty;
                match &s.lazy_item {
                    Some(item) => {
                        quote_spanned!(value_ty.span()=> #signals_path::Lazy::<#item>::from_vec(#value))
                    }
                    None => {
                        quote_spanned!(value_ty.span()=> #signals_path::Signal::<#value_ty>::new(#value))
                    }
                }
            });
            quote!(#hook(__ctx, #(#wrapped),*))
        }
        None => {
            // A field that is not a `Ctx` is filled with `Default::default()`: through a trait of
            // the expansion, so a field type without `Default` gets the branded E0013 at the
            // field instead of `rustc`'s bound error at the attribute.
            let state_inits = state.iter().map(|f| {
                let ident = &f.ident;
                let ty = &f.ty;
                match f.ctx {
                    CtxField::Strong => quote!(#ident: ::core::clone::Clone::clone(&__ctx)),
                    CtxField::Weak => quote!(#ident: __ctx.downgrade()),
                    CtxField::No => quote_spanned!(ty.span()=> #ident: <#ty as #default_trait>::__undra_default()),
                }
            });
            let signal_inits = plain.iter().zip(&values).map(|(s, value)| {
                let ident = &s.ident;
                let value_ty = &s.value_ty;
                match &s.lazy_item {
                    Some(item) => {
                        quote_spanned!(value_ty.span()=> #ident: #signals_path::Lazy::<#item>::from_vec(#value))
                    }
                    None => {
                        quote_spanned!(value_ty.span()=> #ident: #signals_path::Signal::<#value_ty>::new(#value))
                    }
                }
            });
            quote! {{
                let _ = &__ctx;
                Self {
                    #(#state_inits,)*
                    #(#signal_inits,)*
                    __undra_cell: ::core::default::Default::default(),
                }
            }}
        }
    };

    let attach_failed = format!(
        "restored store {name_str} could not attach its signals (a signal is already attached to another store)"
    );
    let restorer_fn = format_ident!("__undra_restore_erased_{}", name_str);
    let cell_fn = format_ident!("__undra_cell_erased_{}", name_str);
    let restorer = registration(&root, &name_str, &restorer_fn, &cell_fn);

    // The work lives in inherent `#[doc(hidden)]` members. `impl StoreObject` (which needs
    // `UndraObject`) is written by the `#[undra::api(store)] impl` block next to `impl UndraObject`
    // and only forwards to them, so a store whose impl block is missing gets the branded E0011
    // and not also an unsatisfied `UndraObject` bound.
    Ok(quote! {
        #item

        impl #name {
            #[doc(hidden)]
            pub const __UNDRA_IS_STORE: bool = true;

            #[doc(hidden)]
            pub const __UNDRA_STORE_META: #meta::StoreMeta = #meta::StoreMeta {
                signals: &[ #(#signal_metas),* ],
            };

            /// The struct's own documentation, which the impl block's object docs start with.
            #[doc(hidden)]
            pub const __UNDRA_DOCS: &'static str = #struct_docs;

            /// Builds the signal cell and attaches every signal, in declaration order.
            #[doc(hidden)]
            fn __undra_build_cell(&self) -> ::core::result::Result<
                ::std::sync::Arc<#signals_path::StoreCell>,
                #signals_path::SignalsError
            > {
                #(#key_fns)*
                let __cell = #signals_path::StoreCell::new(#meta::ids::type_id(#name_str));
                #(#attach_stmts)*
                ::core::result::Result::Ok(__cell)
            }

            /// Creates the signal cell and attaches every signal (idempotent). Fails when a
            /// signal cannot be attached, for example because it already belongs to another
            /// store; the constructor's dispatch arm turns that into a bad request.
            #[doc(hidden)]
            pub fn __undra_attach_all(&self) -> ::core::result::Result<(), #signals_path::SignalsError> {
                self.__undra_cell
                    .get_or_try_init(|| self.__undra_build_cell())
                    .map(|_| ())
            }

            /// The store's cell.
            #[doc(hidden)]
            pub fn __undra_cell_ref(&self) -> &::std::sync::Arc<#signals_path::StoreCell> {
                // Every path that publishes a store calls `__undra_attach_all` first and reports
                // its error, so this only fails for a store that was never published. It then
                // gets an empty cell (it delivers nothing) rather than a panic.
                self.__undra_cell.get_or_init(|| {
                    match self.__undra_build_cell() {
                        ::core::result::Result::Ok(__cell) => __cell,
                        ::core::result::Result::Err(_) => {
                            #signals_path::StoreCell::new(#meta::ids::type_id(#name_str))
                        }
                    }
                })
            }

            /// Records the handle the object table issued.
            #[doc(hidden)]
            pub fn __undra_set_handle(&self, __handle: u64) {
                self.__undra_cell_ref().set_handle(__handle);
            }

            /// Rebuilds the store from the body of its snapshot record.
            #[doc(hidden)]
            #[allow(unused_mut, unused_variables)]
            pub fn __undra_restore(
                __ctx: #runtime::Ctx,
                __r: &mut #wire::Reader<'_>,
            ) -> ::core::result::Result<Self, #wire::WireError> {
                let __count = __r.read_u32()?;
                #( let mut #slots: ::core::option::Option<#value_tys> = ::core::option::Option::None; )*
                for _ in 0..__count {
                    let __id = __r.read_u32()?;
                    let __bytes = __r.read_bytes()?;
                    match __id {
                        #(
                            #ids => {
                                #slots = ::core::option::Option::Some(
                                    <#value_tys as #wire::Decode>::decode_exact(__bytes)?,
                                );
                            }
                        )*
                        _ => {}
                    }
                }
                #(
                    let #values = match #slots {
                        ::core::option::Option::Some(__v) => __v,
                        ::core::option::Option::None => #fallbacks,
                    };
                )*
                let __value = #build;
                if __value.__undra_attach_all().is_err() {
                    return ::core::result::Result::Err(#wire::WireError::InvalidTag {
                        tag: 0,
                        at: __r.position(),
                        ty: #attach_failed,
                    });
                }
                ::core::result::Result::Ok(__value)
            }
        }

        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #restorer_fn(
            __ctx: #runtime::Ctx,
            __handle: u64,
            __r: &mut #wire::Reader<'_>,
        ) -> ::core::result::Result<
            ::std::sync::Arc<dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync>,
            #wire::WireError,
        > {
            let __value = <#name>::__undra_restore(__ctx, __r)?;
            // The snapshot re-issues the store's old handle; the cell must know it.
            __value.__undra_set_handle(__handle);
            ::core::result::Result::Ok(::std::sync::Arc::new(__value)
                as ::std::sync::Arc<dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync>)
        }

        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #cell_fn(
            __any: &(dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync),
        ) -> ::core::option::Option<&::std::sync::Arc<#signals_path::StoreCell>> {
            __any
                .downcast_ref::<#name>()
                .map(<#name>::__undra_cell_ref)
        }
        #restorer

        #default_items

        #[doc(hidden)]
        #[allow(non_camel_case_types, dead_code, unused)]
        const _: () = {
            #impl_attr
            trait __UndraStoreNeedsImpl {}
            impl<__UndraT: #runtime::UndraObject> __UndraStoreNeedsImpl for __UndraT {}
            fn __undra_need_impl<__UndraT: __UndraStoreNeedsImpl>() {}
            fn __undra_check_impl() {
                __undra_need_impl::<#name>();
            }
        };

        #checks
    })
}

/// What a store struct that failed to expand still needs, so the error is the only one.
///
/// The impl block `#[undra::api(store)] impl Type` is expanded on its own and talks to the struct
/// through members `#[undra::store]` defines: the hidden cell field (its struct literals get it
/// added), `__UNDRA_IS_STORE`, `__UNDRA_STORE_META`, `__undra_attach_all` and `__undra_set_handle`.
/// Without them a bad store produces a false E0011 and "no field named `__undra_cell`" next to the
/// real error. This adds the field and empty versions of the members to the original struct.
pub(crate) fn recover(args_root: Option<Root>, item: &mut syn::Item) -> TokenStream {
    let syn::Item::Struct(item) = item else {
        return TokenStream::new();
    };
    let mut attrs = item.attrs.clone();
    let root = item_root(&mut attrs, args_root, &mut Errors::new());
    let signals = root.signals();
    let meta = root.meta();
    let runtime = root.runtime();
    let wire = root.wire();
    if let Fields::Named(named) = &mut item.fields {
        // A `DerivedList` without its row type has been reported (E0001); give it one so that
        // `rustc` does not report the missing generic argument as well.
        for field in &mut named.named {
            if is_bare_derived(&field.ty) {
                field.ty = syn::parse_quote!(#signals::DerivedList<()>);
            }
            if is_bare_lazy(&field.ty) {
                field.ty = syn::parse_quote!(#signals::Lazy<()>);
            }
        }
        let has_cell = named.named.iter().any(|field| {
            field
                .ident
                .as_ref()
                .is_some_and(|ident| ident == "__undra_cell")
        });
        if !has_cell {
            named.named.push(syn::parse_quote! {
                #[doc(hidden)]
                pub __undra_cell: #signals::CellSlot
            });
        }
    }
    let name = &item.ident;
    let (impl_generics, type_generics, where_clause) = item.generics.split_for_impl();
    quote! {
        #[allow(dead_code)]
        impl #impl_generics #name #type_generics #where_clause {
            #[doc(hidden)]
            pub const __UNDRA_IS_STORE: bool = true;
            #[doc(hidden)]
            pub const __UNDRA_STORE_META: #meta::StoreMeta = #meta::StoreMeta { signals: &[] };
            #[doc(hidden)]
            pub const __UNDRA_DOCS: &'static str = "";
            #[doc(hidden)]
            pub fn __undra_attach_all(&self) -> ::core::result::Result<(), #signals::SignalsError> {
                ::core::result::Result::Ok(())
            }
            #[doc(hidden)]
            pub fn __undra_set_handle(&self, __handle: u64) {}
            #[doc(hidden)]
            pub fn __undra_cell_ref(&self) -> &::std::sync::Arc<#signals::StoreCell> {
                ::core::unreachable!("the store failed to expand; the build stops first")
            }
            #[doc(hidden)]
            pub fn __undra_restore(
                _ctx: #runtime::Ctx,
                _r: &mut #wire::Reader<'_>,
            ) -> ::core::result::Result<Self, #wire::WireError> {
                ::core::unreachable!("the store failed to expand; the build stops first")
            }
        }
    }
}

/// The `inventory::submit!` of the store's erased restore function and cell accessor.
fn registration(
    root: &Root,
    name: &str,
    restorer_fn: &syn::Ident,
    cell_fn: &syn::Ident,
) -> TokenStream {
    let meta = root.meta();
    let runtime = root.runtime();
    quote! {
        #meta::inventory::submit! {
            #runtime::StoreRestorer {
                type_id: #meta::ids::type_id(#name),
                restore: #restorer_fn,
                cell: #cell_fn,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::has;

    fn expand(src: &str) -> Result<String, String> {
        let item: ItemStruct = syn::parse_str(src).unwrap();
        expand_store(None, None, item)
            .map(|t| t.to_string())
            .map_err(|e| e.to_string())
    }

    fn expand_with_hook(src: &str) -> Result<String, String> {
        let item: ItemStruct = syn::parse_str(src).unwrap();
        expand_store(None, Some(syn::parse_quote!(Self::rebuild)), item)
            .map(|t| t.to_string())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn signals_are_numbered_in_declaration_order_skipping_state() {
        let out =
            expand("struct S { ctx: Ctx, a: Signal<i32>, name: String, b: Signal<Vec<Row>> }")
                .unwrap();
        assert!(
            has(&out, "SignalMeta { name: \"a\", signal_id: 0u32"),
            "{out}"
        );
        assert!(
            has(&out, "SignalMeta { name: \"b\", signal_id: 1u32"),
            "{out}"
        );
        assert!(
            has(&out, "pub __undra_cell: ::undra::signals::CellSlot"),
            "{out}"
        );
        assert!(
            !has(&out, "impl ::undra::runtime::StoreObject for S"),
            "the impl block writes StoreObject: {out}"
        );
        assert!(has(&out, "::undra::runtime::StoreRestorer"), "{out}");
    }

    #[test]
    fn computed_needs_a_restore_hook() {
        let message = expand("struct S { a: Signal<i32>, b: Computed<i32> }").unwrap_err();
        assert!(
            message.starts_with(
                "error[undra::E0013]: store `S` cannot be restored automatically: `b`"
            ),
            "{message}"
        );
        let out = expand_with_hook("struct S { a: Signal<i32>, b: Computed<i32> }").unwrap();
        assert!(has(&out, "computed: true"), "{out}");
        assert!(
            has(
                &out,
                "Self::rebuild(__ctx, ::undra::signals::Signal::<i32>::new(__value_a))"
            ),
            "{out}"
        );
        assert!(has(&out, "attach_computed(&self.b, 1u32)?"), "{out}");
    }

    #[test]
    fn a_lazy_list_is_a_plain_signal_the_platforms_page() {
        let out = expand(
            "struct S { a: Signal<i32>, #[undra(key = \"id\")] books: Lazy<Row>, #[undra(default)] tail: Lazy<String> }",
        )
        .unwrap();
        // The schema type is `Lazy(T)`, persisted (not computed), keyed for the platform.
        assert!(
            has(
                &out,
                "SignalMeta { name: \"books\", signal_id: 1u32, ty: ::undra::meta::TypeRefMeta::Lazy(&::undra::meta::TypeRefMeta::Named(\"Row\")), computed: false, key: ::core::option::Option::Some(\"id\"), no_coalesce: false, default: false }"
            ),
            "{out}"
        );
        assert!(
            has(
                &out,
                "SignalMeta { name: \"tail\", signal_id: 2u32, ty: ::undra::meta::TypeRefMeta::Lazy(&::undra::meta::TypeRefMeta::String), computed: false, key: ::core::option::Option::None, no_coalesce: false, default: true }"
            ),
            "{out}"
        );
        // Attached with `attach_lazy`; a key is checked (E0008) through its function and the core
        // never uses it.
        assert!(
            has(
                &out,
                "let _ = __undra_key_books; __cell.attach_lazy(&self.books, 1u32)?;"
            ),
            "{out}"
        );
        assert!(has(&out, "__cell.attach_lazy(&self.tail, 2u32)?;"), "{out}");
        // Restored from the `Vec` of its items; a missing `#[undra(default)]` one is empty.
        assert!(
            has(
                &out,
                "books: ::undra::signals::Lazy::<Row>::from_vec(__value_books)"
            ),
            "{out}"
        );
        assert!(
            has(
                &out,
                "<::std::vec::Vec<String> as ::core::default::Default>::default()"
            ),
            "{out}"
        );
        assert!(
            has(
                &out,
                "<::std::vec::Vec<Row> as ::undra::wire::Decode>::decode_exact(__bytes)?"
            ),
            "{out}"
        );
    }

    #[test]
    fn a_lazy_list_goes_through_a_restore_hook_as_a_lazy() {
        let out = expand_with_hook("struct S { a: Signal<i32>, l: Lazy<Row>, d: Computed<i32> }")
            .unwrap();
        assert!(
            has(
                &out,
                "Self::rebuild(__ctx, ::undra::signals::Signal::<i32>::new(__value_a), ::undra::signals::Lazy::<Row>::from_vec(__value_l))"
            ),
            "{out}"
        );
    }

    #[test]
    fn a_lazy_list_is_not_a_computed_so_a_store_of_lazies_restores_automatically() {
        expand("struct S { ctx: Ctx, l: Lazy<Row> }").unwrap();
    }

    #[test]
    fn a_lazy_needs_its_row_type() {
        let message = expand("struct S { l: Lazy }").unwrap_err();
        assert!(
            message.starts_with("error[undra::E0001]: `Lazy` needs the type of its rows"),
            "{message}"
        );
        assert!(message.contains("write `Lazy<Row>` for `l`"), "{message}");
    }

    #[test]
    fn a_lazy_inside_a_signal_computed_or_derived_list_says_where_it_may_stand() {
        for (src, outer) in [
            ("struct S { l: Signal<Lazy<Row>> }", "a `Signal`"),
            ("struct S { l: Computed<Lazy<Row>> }", "a `Computed`"),
            (
                "struct S { #[undra(key = \"id\")] l: DerivedList<Lazy<Row>> }",
                "a `DerivedList`",
            ),
        ] {
            let message = expand_with_hook(src).unwrap_err();
            assert!(message.starts_with("error[undra::E0001]: `"), "{message}");
            assert!(
                message.contains("cannot hold a `Lazy`: it is a store field, not a value"),
                "{message}"
            );
            assert!(message.contains(outer), "{message}");
            assert!(
                message.contains("write the field as `Lazy<Row>`")
                    && message.contains("Lazy::over(&derived)"),
                "{message}"
            );
        }
    }

    #[test]
    fn key_on_a_lazy_needs_nothing_else_and_default_works_on_it() {
        let message = expand("struct S { #[undra(key = \"id\")] n: Signal<i32> }").unwrap_err();
        assert!(
            message.contains("needs a `Signal<Vec<T>>`, a `DerivedList<T>` or a `Lazy<T>`"),
            "{message}"
        );
        expand("struct S { #[undra(default, key = \"id\")] l: Lazy<Row> }").unwrap();
    }

    #[test]
    fn signals_attach_with_the_call_that_fits_their_kind() {
        let out = expand_with_hook(
            "struct S { plain: Signal<i32>, #[undra(key = \"id\")] rows: Signal<Vec<Row>>, derived: Computed<i32> }",
        )
        .unwrap();
        assert!(has(&out, "__cell.attach(&self.plain, 0u32)?;"), "{out}");
        assert!(
            has(
                &out,
                "__cell.attach_keyed(&self.rows, 1u32, __undra_key_rows)?;"
            ),
            "{out}"
        );
        assert!(
            has(&out, "__cell.attach_computed(&self.derived, 2u32)?;"),
            "{out}"
        );
        assert!(!has(&out, "attach_lazy"), "{out}");
        assert!(!has(&out, "set_no_coalesce"), "{out}");
    }

    #[test]
    fn attach_errors_are_returned_not_swallowed() {
        let out = expand("struct S { a: Signal<i32> }").unwrap();
        assert!(
            has(
                &out,
                "pub fn __undra_attach_all(&self) -> ::core::result::Result<(), ::undra::signals::SignalsError>"
            ),
            "{out}"
        );
        assert!(
            has(&out, "get_or_try_init(|| self.__undra_build_cell())"),
            "{out}"
        );
        assert!(
            has(&out, "if __value.__undra_attach_all().is_err()"),
            "{out}"
        );
    }

    #[test]
    fn keyed_lists_and_no_coalesce() {
        let out = expand("struct S { #[undra(key = \"id\", no_coalesce)] rows: Signal<Vec<Row>> }")
            .unwrap();
        assert!(
            has(&out, "fn __undra_key_rows(__item: &Row) -> u64"),
            "{out}"
        );
        // The key field is found by name in a constant (E0008 when there is none), and read
        // through a reference typed by that check, so a missing field is not also `rustc`'s error.
        assert!(
            has(
                &out,
                "const __UNDRA_INDEX: usize = ::undra::meta::keys::index_of(__UNDRA_FIELDS, \"id\");"
            ),
            "{out}"
        );
        assert!(has(&out, "<Row>::__UNDRA_FIELDS"), "{out}");
        assert!(
            has(
                &out,
                "let __row: &<__UndraGate<{ __UNDRA_KEY_IS_A_FIELD }> as __UndraPass<Row>>::Out = __item;"
            ),
            "{out}"
        );
        assert!(
            has(
                &out,
                "::undra::wire::Encode::encode(&__row.id, &mut __buf);"
            ),
            "{out}"
        );
        assert!(!has(&out, "__item.id"), "{out}");
        assert!(
            has(
                &out,
                "error[undra::E0008]: `#[undra(key = \\\"id\\\")]` on `rows` names no field of `Row`"
            ),
            "{out}"
        );
        assert!(has(&out, "fnv1a64(__buf.as_slice())"), "{out}");
        assert!(
            has(
                &out,
                "__cell.attach_keyed(&self.rows, 0u32, __undra_key_rows)?;"
            ),
            "{out}"
        );
        assert!(has(&out, "__cell.set_no_coalesce(0u32)?;"), "{out}");
        assert!(
            has(&out, "key: ::core::option::Option::Some(\"id\")"),
            "{out}"
        );
        // The schema records it, so the platform mirrors apply every entry (ADR-031).
        assert!(has(&out, "no_coalesce: true"), "{out}");
    }

    #[test]
    fn a_coalesced_signal_says_so_in_the_schema() {
        let out = expand("struct S { a: Signal<i32> }").unwrap();
        assert!(has(&out, "no_coalesce: false"), "{out}");
        assert!(!has(&out, "set_no_coalesce"), "{out}");
    }

    #[test]
    fn no_coalesce_needs_a_signal_field() {
        // ADR-031 puts the flag in the schema, so a misplaced one must teach (R8), not vanish.
        let message = expand("struct S { a: Signal<i32>, #[undra(no_coalesce)] cache: Vec<u8> }")
            .unwrap_err();
        assert!(message.contains("error[undra::E0008]"), "{message}");
        assert!(
            message.contains("`#[undra(no_coalesce)]` is not valid on a non-signal store field"),
            "{message}"
        );
        assert!(
            message.contains("makes a store signal deliver every commit"),
            "{message}"
        );
        assert!(
            message
                .contains("move it to a signal field of a `#[undra::store]` struct, or remove it"),
            "{message}"
        );
        assert!(message.contains("= docs: "), "{message}");
    }

    #[test]
    fn key_needs_a_vec_signal() {
        for src in [
            "struct S { #[undra(key = \"id\")] a: Signal<i32> }",
            "struct S { #[undra(key = \"id\")] a: Computed<Vec<Row>> }",
        ] {
            let message = expand_with_hook(src).unwrap_err();
            assert!(message.contains("error[undra::E0008]"), "{message}");
            assert!(
                message.contains("needs a `Signal<Vec<T>>`, a `DerivedList<T>` or a `Lazy<T>`"),
                "{message}"
            );
        }
    }

    #[test]
    fn a_keyed_computed_list_is_told_to_become_a_derived_list() {
        let message = expand_with_hook("struct S { #[undra(key = \"id\")] a: Computed<Vec<Row>> }")
            .unwrap_err();
        assert!(
            message.contains("a computed list is sent whole"),
            "{message}"
        );
        assert!(
            message.contains(
                "build it as a `DerivedList<T>` (`source.derive().filter(..).build()`) and key that"
            ),
            "{message}"
        );
        // A plain signal keeps the old advice.
        let message =
            expand_with_hook("struct S { #[undra(key = \"id\")] a: Signal<i32> }").unwrap_err();
        assert!(
            message.contains("use a `Signal<Vec<T>>` field"),
            "{message}"
        );
    }

    #[test]
    fn a_derived_list_is_a_keyed_computed_list() {
        let out = expand_with_hook(
            "struct S { #[undra(key = \"id\")] rows: Signal<Vec<Row>>, filter: Signal<u8>, #[undra(key = \"id\", no_coalesce)] visible: DerivedList<Row> }",
        )
        .unwrap();
        // The schema: a `Vec<Row>`, computed, keyed. No new field.
        assert!(
            has(
                &out,
                "SignalMeta { name: \"visible\", signal_id: 2u32, ty: ::undra::meta::TypeRefMeta::Vec(&::undra::meta::TypeRefMeta::Named(\"Row\")), computed: true, key: ::core::option::Option::Some(\"id\"), no_coalesce: true, default: false }"
            ),
            "{out}"
        );
        // Attached with the key function of its rows.
        assert!(
            has(&out, "fn __undra_key_visible(__item: &Row) -> u64"),
            "{out}"
        );
        assert!(
            has(
                &out,
                "__cell.attach_derived(&self.visible, 2u32, __undra_key_visible)?;"
            ),
            "{out}"
        );
        assert!(has(&out, "__cell.set_no_coalesce(2u32)?;"), "{out}");
        // Left out of the snapshot and of the restore hook's parameters, like a computed.
        assert!(
            has(
                &out,
                "Self::rebuild(__ctx, ::undra::signals::Signal::<Vec<Row>>::new(__value_rows), ::undra::signals::Signal::<u8>::new(__value_filter))"
            ),
            "{out}"
        );
        assert!(!has(&out, "__value_visible"), "{out}");
    }

    #[test]
    fn a_derived_list_needs_a_key() {
        let message =
            expand_with_hook("struct S { rows: Signal<Vec<Todo>>, visible: DerivedList<Todo> }")
                .unwrap_err();
        assert!(
            message.starts_with(
                "error[undra::E0008]: `visible` is a `DerivedList<Todo>` without `#[undra(key = \"..\")]`"
            ),
            "{message}"
        );
        assert!(
            message.contains("a derived list reaches the platforms as keyed patches; the key names the field that identifies a row"),
            "{message}"
        );
        assert!(
            message.contains("add `#[undra(key = \"id\")]` naming a field of `Todo`"),
            "{message}"
        );
    }

    #[test]
    fn a_derived_list_needs_its_row_type() {
        for src in [
            "struct S { #[undra(key = \"id\")] visible: DerivedList }",
            "struct S { visible: DerivedList<Row, u8> }",
        ] {
            let message = expand_with_hook(src).unwrap_err();
            assert!(
                message.starts_with("error[undra::E0001]: `DerivedList"),
                "{message}"
            );
            assert!(message.contains("needs the type of its rows"), "{message}");
            assert!(
                message.contains("write `DerivedList<Row>` for `visible`"),
                "{message}"
            );
        }
    }

    #[test]
    fn a_derived_list_needs_a_restore_hook() {
        let message = expand(
            "struct S { #[undra(key = \"id\")] rows: Signal<Vec<Row>>, #[undra(key = \"id\")] visible: DerivedList<Row> }",
        )
        .unwrap_err();
        assert!(
            message.starts_with(
                "error[undra::E0013]: store `S` cannot be restored automatically: `visible` is computed"
            ),
            "{message}"
        );
    }

    #[test]
    fn a_boxed_item_is_looked_through() {
        let out =
            expand("struct S { #[undra(key = \"id\")] rows: Signal<Vec<Box<Row>>> }").unwrap();
        assert!(
            has(&out, "fn __undra_key_rows(__item: &Box<Row>) -> u64"),
            "{out}"
        );
        assert!(has(&out, "<Row>::__UNDRA_FIELDS"), "{out}");
        assert!(
            has(&out, "as __UndraPass<Row>>::Out = (&** __item);"),
            "{out}"
        );
        let out =
            expand("struct S { #[undra(key = \"id\")] rows: Signal<Vec<Box<Box<Row>>>> }").unwrap();
        assert!(
            has(&out, "as __UndraPass<Row>>::Out = (&*** __item);"),
            "{out}"
        );
    }

    #[test]
    fn a_key_is_read_as_the_field_it_names() {
        let read = |key: &str| {
            let src = format!("struct S {{ #[undra(key = \"{key}\")] rows: Signal<Vec<Row>> }}");
            expand(&src).unwrap()
        };
        assert!(has(&read("id"), "&__row.id,"), "a plain name");
        assert!(
            has(&read("type"), "&__row.r#type,"),
            "a keyword is a raw identifier"
        );
        // Not an identifier: no field has the name, so the constant check fails first and the
        // access never meets a real type.
        for key in ["not a name", "self", "1st", ""] {
            let out = read(key);
            assert!(
                has(&out, "&__row.__undra_not_a_field_name,"),
                "{key}: {out}"
            );
        }
    }

    #[test]
    fn signal_types_are_checked() {
        assert!(
            expand("struct S { a: Signal<&str> }")
                .unwrap_err()
                .contains("E0001")
        );
        assert!(
            expand("struct S { a: Signal<Result<u8, E>> }")
                .unwrap_err()
                .contains("E0005")
        );
        assert!(
            expand("struct S { a: Signal<()> }")
                .unwrap_err()
                .contains("E0001")
        );
    }

    #[test]
    fn shapes_and_reserved_names_are_rejected() {
        assert!(
            expand("struct S(Signal<i32>);")
                .unwrap_err()
                .contains("E0007")
        );
        assert!(expand("struct S;").unwrap_err().contains("E0007"));
        assert!(
            expand("struct S<T> { a: Signal<i32> }")
                .unwrap_err()
                .contains("E0002")
        );
        assert!(
            expand("struct S { __undra_cell: u8 }")
                .unwrap_err()
                .contains("reserved")
        );
    }

    #[test]
    fn restore_fills_state_from_ctx_and_default() {
        let out = expand("struct S { ctx: Ctx, extra: Vec<u8>, a: Signal<i32> }").unwrap();
        assert!(
            has(&out, "ctx: ::core::clone::Clone::clone(&__ctx)"),
            "{out}"
        );
        assert!(
            has(
                &out,
                "extra: <Vec<u8> as __UndraRestoreDefault_S>::__undra_default()"
            ),
            "{out}"
        );
        assert!(
            has(&out, "a: ::undra::signals::Signal::<i32>::new(__value_a)"),
            "{out}"
        );
        assert!(
            has(&out, "snapshot of store S is missing signal 0 (a)"),
            "{out}"
        );
    }
}
