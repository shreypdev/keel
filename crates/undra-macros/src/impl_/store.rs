//! `#[undra::store]`: stores, objects whose fields are signals the platforms mirror.
//!
//! # Signals
//!
//! Fields typed `Signal<T>` and `Computed<T>` are the store's signals, numbered `0..n` in
//! declaration order (other fields are private state, `Ctx` included).
//! `#[undra(key = "id")]` on a `Signal<Vec<T>>` makes it a keyed list that ships patches;
//! `#[undra(no_coalesce)]` makes every commit of a signal reach the platforms, and is recorded in
//! the signal's schema entry so their mirrors apply every one of them (ADR-031). `Lazy<T>` fields
//! are rejected (E0001): lazily paged lists are not available in v1.
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
//! field) for `#[undra(key = "..")]`, `attach_computed` for a `Computed<T>`, and
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
//!   `Computed` fields: a struct literal;
//! * through a hook, `#[undra::store(restore = "Self::rebuild")]`, with the signature
//!   `fn(ctx: Ctx, <one Signal<T> per non-computed signal, in order>) -> Self`. Use it when
//!   the store has computed fields (only your code knows how to derive them) or other
//!   state without a `Default`.

use proc_macro2::TokenStream;
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
}

struct SignalField {
    ident: syn::Ident,
    name: String,
    id: u32,
    kind: SigKind,
    /// The `T` of `Signal<T>` / `Computed<T>` / `Lazy<T>`.
    value_ty: syn::Type,
    /// The schema type of the signal.
    kty: KType,
    /// `#[undra(key = "..")]`: the key field and the list's item type.
    key: Option<(String, syn::Ident, syn::Type)>,
    no_coalesce: bool,
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

/// `Signal<T>` or `Computed<T>` (by last path segment) with its `T`.
fn signal_wrapper(ty: &syn::Type) -> Option<(SigKind, syn::Type)> {
    let (name, value) = wrapper_of(ty)?;
    match name.as_str() {
        "Signal" => Some((SigKind::Signal, value)),
        "Computed" => Some((SigKind::Computed, value)),
        _ => None,
    }
}

/// `Lazy<T>`: recognised only to be rejected with a teaching diagnostic (not in v1).
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
                if is_lazy(&field.ty) {
                    take(&mut field.attrs, Site::SIGNAL, &mut errors);
                    errors.push(
                        Diag::new(
                            code::E0001,
                            format!(
                                "`{}` is not available in v1: lazy lists cannot be mirrored yet",
                                ty_string(&field.ty)
                            ),
                            "a `Lazy<T>` signal is a list the platform pages through on demand; the platform runtimes have no API for it yet (SPEC section 17), so no language could observe it",
                            "expose the items as a `Signal<Vec<T>>` (keyed with `#[undra(key = \"..\")]` if they have an id) or as a paged method that takes an offset and a limit",
                        )
                        .on(&field.ty),
                    );
                    continue;
                }
                match signal_wrapper(&field.ty) {
                    Some((kind, value_ty)) => {
                        let attr = take(&mut field.attrs, Site::SIGNAL, &mut errors);
                        let kty = match map_type(&value_ty, Pos::Signal, Allow::NONE) {
                            Ok(kty) => kty,
                            Err(err) => {
                                errors.push(err.into_error());
                                KType::Unit
                            }
                        };
                        let key = attr.key.and_then(|lit| {
                            let key_name = lit.value();
                            let Some(item_ty) = (kind == SigKind::Signal)
                                .then(|| vec_item(&value_ty))
                                .flatten()
                            else {
                                errors.push(
                                    Diag::new(
                                        code::E0008,
                                        format!("`#[undra(key = \"{key_name}\")]` on `{ident}` needs a `Signal<Vec<T>>`"),
                                        "only lists of records can be keyed: the key identifies an item across updates so changes ship as patches",
                                        "use a `Signal<Vec<T>>` field, or remove `key`",
                                    )
                                    .at(lit.span()),
                                );
                                return None;
                            };
                            match syn::parse_str::<syn::Ident>(&key_name) {
                                Ok(mut key_ident) => {
                                    // A key naming no field is then reported on the literal.
                                    key_ident.set_span(lit.span());
                                    Some((key_name, key_ident, item_ty))
                                }
                                Err(_) => {
                                    errors.push(
                                        Diag::new(
                                            code::E0008,
                                            format!("`{key_name}` is not a field name"),
                                            "`key` names the field of the list's items that identifies them",
                                            "write the name of a field of the item type, for example `key = \"id\"`",
                                        )
                                        .at(lit.span()),
                                    );
                                    None
                                }
                            }
                        });
                        signals.push(SignalField {
                            name: unraw(&ident),
                            ident,
                            id: u32::try_from(signals.len()).unwrap_or(u32::MAX),
                            kind,
                            value_ty,
                            kty,
                            key,
                            no_coalesce: attr.no_coalesce,
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
        .filter(|s| s.kind == SigKind::Computed)
        .map(|s| s.name.clone())
        .collect::<Vec<_>>();
    if restore_hook.is_none() && !derived_signals.is_empty() {
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
            .on(&name),
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
        let ty = s.kty.meta(&meta);
        let computed = s.kind == SigKind::Computed;
        let key = match &s.key {
            Some((key_name, _, _)) => quote!(::core::option::Option::Some(#key_name)),
            None => quote!(::core::option::Option::None),
        };
        // Recorded in the schema so the platform mirrors apply every entry of the signal (ADR-031).
        let no_coalesce = s.no_coalesce;
        quote! {
            #meta::SignalMeta { name: #sname, signal_id: #id, ty: #ty, computed: #computed, key: #key, no_coalesce: #no_coalesce }
        }
    });

    // The key of a keyed list: `fn(&Item) -> u64`, the FNV-1a hash of the encoded key field
    // (what `undra_signals::KeyFn` asks for). The encoding goes through a per-thread scratch
    // buffer, because the function runs for every item of an observed list at every commit.
    let key_fns = signals.iter().filter_map(|s| {
        let (_, key_ident, item_ty) = s.key.as_ref()?;
        let fn_name = format_ident!("__undra_key_{}", s.ident);
        Some(quote_spanned! {item_ty.span()=>
            fn #fn_name(__item: &#item_ty) -> u64 {
                ::std::thread_local! {
                    static __UNDRA_KEY_BUF: ::core::cell::RefCell<#wire::Writer> =
                        ::core::cell::RefCell::new(#wire::Writer::new());
                }
                __UNDRA_KEY_BUF.with(|__buf| {
                    let mut __buf = __buf.borrow_mut();
                    __buf.clear();
                    #wire::Encode::encode(&__item.#key_ident, &mut __buf);
                    #meta::ids::fnv1a64(__buf.as_slice())
                })
            }
        })
    });

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
        };
        let coalesce = if s.no_coalesce {
            quote!(__cell.set_no_coalesce(#id)?;)
        } else {
            TokenStream::new()
        };
        quote! { #attach #coalesce }
    });

    // --- restore ----------------------------------------------------------------------------
    let plain: Vec<&SignalField> = signals
        .iter()
        .filter(|s| s.kind == SigKind::Signal)
        .collect();
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
    let missing: Vec<String> = plain
        .iter()
        .map(|s| {
            format!(
                "snapshot of store {name_str} is missing signal {} ({})",
                s.id, s.name
            )
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
                quote_spanned!(value_ty.span()=> #signals_path::Signal::<#value_ty>::new(#value))
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
                quote_spanned!(value_ty.span()=> #ident: #signals_path::Signal::<#value_ty>::new(#value))
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
                        ::core::option::Option::None => {
                            return ::core::result::Result::Err(#wire::WireError::InvalidTag {
                                tag: #ids,
                                at: __r.position(),
                                ty: #missing,
                            });
                        }
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
    fn lazy_lists_are_not_available_in_v1() {
        for src in [
            "struct S { a: Signal<i32>, l: Lazy<Row> }",
            "struct S { #[undra(no_coalesce)] l: Lazy<Row> }",
        ] {
            for message in [expand(src).unwrap_err(), expand_with_hook(src).unwrap_err()] {
                assert!(
                    message.starts_with("error[undra::E0001]: `Lazy<Row>` is not available in v1"),
                    "{message}"
                );
                assert!(
                    message.contains("lazy lists cannot be mirrored yet"),
                    "{message}"
                );
                assert!(message.contains("Signal<Vec<T>>"), "{message}");
            }
        }
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
        assert!(has(&out, "Encode::encode(&__item.id, &mut __buf)"), "{out}");
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
        assert!(message.contains("remove the option"), "{message}");
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
            assert!(message.contains("needs a `Signal<Vec<T>>`"), "{message}");
        }
        let message =
            expand("struct S { #[undra(key = \"not a name\")] a: Signal<Vec<Row>> }").unwrap_err();
        assert!(message.contains("is not a field name"), "{message}");
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
