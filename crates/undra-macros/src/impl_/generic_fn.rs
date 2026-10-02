//! Generic functions and methods (ADR-058 decision 1).
//!
//! The schema describes concrete types and the platforms call a function by its id, so a generic
//! function crosses the boundary once for each type its author **lists**:
//!
//! ```ignore
//! #[undra::api(generic(T = [Todo, Note]))]
//! pub fn newest<T: Row>(rows: Vec<T>) -> Option<T> { .. }
//!
//! #[undra::api]
//! impl Library {
//!     #[undra(generic(T = [Todo, Note]))]
//!     pub fn pinned<T: Row>(&self) -> Vec<T> { .. }
//! }
//! ```
//!
//! The function is emitted as written. For each listed type the macro substitutes the type for the
//! parameter in the signature (type positions only, wrapped in a `None`-delimited group so the
//! identity checks of an instantiation look at exactly the substituted type) and runs the
//! **ordinary function or method expansion on the concrete signature**: the dispatcher, which calls
//! `newest::<Todo>(..)`, the `FunctionMeta` or `MethodMeta` with its [`Label`], the registration and
//! the identity checks. Every rule of a hand-written function applies to every instantiation by
//! construction. The signature is also analysed once with the parameter left as it is (the
//! *template pass*), so a mistake that does not depend on the type is reported once, at the
//! function, and not once for each instantiation.
//!
//! This module holds what is particular to the list: reading it, refusing what it may not say
//! (E0072), the substitution and the label.

use proc_macro2::TokenStream;
use quote::quote;
use syn::visit_mut::{self, VisitMut};
use syn::{FnArg, GenericParam, Signature, Type};

use super::attrs::GenericList;
use super::common::GenericOn;
use super::diag::{Diag, Errors, code};
use super::naming::unraw;
use super::types::{Allow, KType, Pos, map_type, ty_string};

/// One instantiation of a generic function or method: one entry of its list.
#[derive(Clone, Debug)]
pub(crate) struct Instance {
    /// The type as the author wrote it (`Todo`, `crate::model::Todo`).
    pub(crate) ty: Type,
    /// The schema name of the type, its last path segment: `Todo`.
    pub(crate) arg_name: String,
    /// The name of the definition, `newest<Todo>`: what the schema, the id and every message use.
    pub(crate) name: String,
    /// What the items generated for this instantiation are named after: `newest_of_Todo`.
    pub(crate) suffix: String,
    /// `::<Todo>`, the generic arguments of the call the dispatcher makes.
    pub(crate) turbofish: TokenStream,
}

/// A generic function or method with its list read and checked.
#[derive(Clone, Debug)]
pub(crate) struct Plan {
    /// The type parameter, `T`.
    pub(crate) param: syn::Ident,
    /// The function's own name, `newest`.
    pub(crate) fn_name: String,
    /// One entry per listed type, in the order they were listed.
    pub(crate) instances: Vec<Instance>,
}

/// What `FunctionMeta::generic` / `MethodMeta::generic` say about one instantiation (the schema's
/// `GenericOf`).
#[derive(Clone, Debug)]
pub(crate) struct Label {
    /// The function's own name, `newest`.
    pub(crate) of: String,
    /// The type parameter, `T`.
    pub(crate) param: String,
    /// The type it is instantiated with, `Todo`.
    pub(crate) arg: String,
    /// Whether the parameter stands in the type of a parameter of the function, so a caller's
    /// arguments fix it.
    pub(crate) inferred: bool,
}

impl Label {
    /// The `Option<&'static GenericOfMeta>` expression of an instantiation.
    pub(crate) fn meta(label: Option<&Label>, meta: &TokenStream) -> TokenStream {
        let Some(label) = label else {
            return quote!(::core::option::Option::None);
        };
        let Label {
            of,
            param,
            arg,
            inferred,
        } = label;
        quote! {
            ::core::option::Option::Some(&#meta::GenericOfMeta {
                of: #of,
                args: &[#meta::GenericArgMeta {
                    param: #param,
                    ty: #meta::TypeRefMeta::Named(#arg),
                    inferred: #inferred,
                }],
            })
        }
    }
}

/// What the generics of a function or method and its `generic(..)` lists say, checked.
///
/// `Some` when the function has a type parameter and a list for it that the macro can expand.
/// Every other case is reported in `errors` and is `None` (a function that has neither a type
/// parameter nor a list is not this module's business: the caller does not ask).
pub(crate) fn plan(
    sig: &Signature,
    on: GenericOn,
    lists: &[GenericList],
    sink: &mut Errors,
) -> Option<Plan> {
    let mut local = Errors::new();
    let planned = plan_into(sig, on, lists, &mut local);
    let clean = local.is_empty();
    sink.absorb(local);
    planned.filter(|_| clean)
}

fn plan_into(
    sig: &Signature,
    on: GenericOn,
    lists: &[GenericList],
    errors: &mut Errors,
) -> Option<Plan> {
    let fn_name = unraw(&sig.ident);

    // Lifetimes and const parameters are what they are everywhere else.
    let mut others = sig.generics.clone();
    others.params = others
        .params
        .into_iter()
        .filter(|param| !matches!(param, GenericParam::Type(_)))
        .collect();
    others.where_clause = None;
    super::common::check_generics(&others, &fn_name, on, errors);

    let type_params: Vec<&syn::TypeParam> = sig.generics.type_params().collect();
    for list in lists {
        if !type_params.iter().any(|p| p.ident == list.param) {
            let declares = match type_params.as_slice() {
                [] => "declares none".to_owned(),
                [only] => format!("declares `{}`", only.ident),
                many => format!(
                    "declares {}",
                    many.iter()
                        .map(|p| format!("`{}`", p.ident))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            errors.push(
                Diag::new(
                    code::E0072,
                    format!(
                        "`generic({} = [..])` on `{fn_name}`, which has no type parameter `{}`",
                        list.param, list.param
                    ),
                    format!(
                        "the list says which types a type parameter of the function is instantiated with; `{fn_name}` {declares}"
                    ),
                    match type_params.first() {
                        Some(first) => format!("write `generic({} = [Todo, Note])`", first.ident),
                        None => "remove `generic(..)`, or add the type parameter".to_owned(),
                    },
                )
                .on(&list.param),
            );
        }
    }
    if type_params.len() > 1 {
        let names: Vec<String> = type_params
            .iter()
            .map(|p| format!("`{}`", p.ident))
            .collect();
        let listed = match names.split_last() {
            Some((last, rest)) if names.len() == 2 => format!("{} and {last}", rest.join(", ")),
            Some((last, rest)) => format!("{}, and {last}", rest.join(", ")),
            None => String::new(),
        };
        let count = match type_params.len() {
            2 => "two".to_owned(),
            3 => "three".to_owned(),
            n => n.to_string(),
        };
        errors.push(
            Diag::new(
                code::E0072,
                format!("`{fn_name}` has {count} type parameters, {listed}"),
                "a generic function crosses with one type parameter: each listed type is one function, and several parameters would need a list of pairs",
                "keep one parameter generic and write the other type out, or declare one function per pair",
            )
            .on(&sig.generics),
        );
        return None;
    }
    let [param] = type_params.as_slice() else {
        // No type parameter: the lists were reported above.
        return None;
    };
    let param = param.ident.clone();
    let Some(list) = lists.iter().find(|l| l.param == param) else {
        // A type parameter and only lists of others: reported above.
        return None;
    };
    if lists.iter().filter(|l| l.param == param).count() > 1 {
        errors.push(
            Diag::new(
                code::E0072,
                format!("the type parameter `{param}` of `{fn_name}` has two lists"),
                "each listed type becomes one function, so the types of a parameter are one list",
                format!("write one list: `generic({param} = [Todo, Note])`"),
            )
            .on(&param),
        );
    }

    if list.types.is_empty() {
        errors.push(
            Diag::new(
                code::E0072,
                format!("the list of `{param}` on `{fn_name}` is empty"),
                "a generic function crosses once per listed type; with no type it would not cross at all",
                format!("list at least one type, `generic({param} = [Todo])`, or remove `#[undra::api]` if the function is not for the platforms"),
            )
            .on(&list.param),
        );
        return None;
    }

    let mut instances: Vec<Instance> = Vec::new();
    for ty in &list.types {
        let Some(arg_name) = listed_name(ty) else {
            errors.push(
                Diag::new(
                    code::E0072,
                    format!(
                        "`{}` cannot be listed for `{param}` on `{fn_name}`",
                        ty_string(ty)
                    ),
                    "each instantiation is presented under the name of its type (`draft(Todo.self)` in Swift, `draft(Todo::class)` in Kotlin, `draft(\"Todo\")` in TypeScript), so a listed type is a value type declared with `#[undra::api]`: a record, an enum, a newtype or a named instantiation",
                    "list the element type and write `Vec<T>` in the signature, or give the type a name: `#[undra::api] pub struct Todos(pub Vec<Todo>);`",
                )
                .on(ty),
            );
            continue;
        };
        if instances.iter().any(|i| i.arg_name == arg_name) {
            errors.push(
                Diag::new(
                    code::E0072,
                    format!("`{arg_name}` is listed twice for `{param}` on `{fn_name}`"),
                    format!(
                        "each listed type becomes one function, `{fn_name}<{arg_name}>`, and two functions cannot share that name"
                    ),
                    "remove one of them",
                )
                .on(ty),
            );
            continue;
        }
        instances.push(Instance {
            ty: ty.clone(),
            name: format!("{fn_name}<{arg_name}>"),
            suffix: format!("{fn_name}_of_{arg_name}"),
            turbofish: quote!(::<#ty>),
            arg_name,
        });
    }
    Some(Plan {
        param,
        fn_name,
        instances,
    })
}

/// The schema name of a listed type when it is a named value type as far as syntax can tell: a
/// path without arguments that the mapper reads as a name, not as a scalar, a leaf, `String`,
/// a container or a reference. Whether the name is really a record, an enum or a newtype (and not
/// an object) is checked by the compiler (`check.rs`, E0072 and E0061).
fn listed_name(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else {
        return None;
    };
    if path.qself.is_some() || !path.path.segments.last()?.arguments.is_none() {
        return None;
    }
    match map_type(ty, Pos::Field, Allow::NONE) {
        Ok(KType::Named(name)) => Some(name),
        _ => None,
    }
}

/// Whether `ty` mentions the type parameter `param` anywhere in it.
pub(crate) fn mentions(ty: &Type, param: &syn::Ident) -> bool {
    struct Find<'a> {
        param: &'a syn::Ident,
        found: bool,
    }
    impl VisitMut for Find<'_> {
        fn visit_type_path_mut(&mut self, node: &mut syn::TypePath) {
            if node.qself.is_none()
                && node
                    .path
                    .segments
                    .first()
                    .is_some_and(|seg| seg.ident == *self.param && seg.arguments.is_none())
            {
                self.found = true;
            }
            visit_mut::visit_type_path_mut(self, node);
        }
    }
    let mut find = Find {
        param,
        found: false,
    };
    find.visit_type_mut(&mut ty.clone());
    find.found
}

/// Whether the parameter stands in the type of at least one parameter of the function (`ctx` and
/// the receiver are not parameters of the schema): what `inferred` says.
pub(crate) fn inferred(sig: &Signature, param: &syn::Ident) -> bool {
    sig.inputs.iter().any(|input| match input {
        FnArg::Typed(typed) => !super::object::is_ctx(&typed.ty) && mentions(&typed.ty, param),
        FnArg::Receiver(_) => false,
    })
}

/// The signature of one instantiation: no generics, and the type parameter replaced by `ty`
/// (wrapped in a `None`-delimited group, which is what the checks of an instantiation recognise as
/// an argument) in every type of the parameters and of the return type.
pub(crate) fn concrete_signature(sig: &Signature, param: &syn::Ident, ty: &Type) -> Signature {
    struct Replace<'a> {
        param: &'a syn::Ident,
        arg: Type,
    }
    impl VisitMut for Replace<'_> {
        fn visit_type_mut(&mut self, ty: &mut Type) {
            if let Type::Path(path) = ty {
                if path.qself.is_none()
                    && path.path.leading_colon.is_none()
                    && path.path.segments.len() == 1
                    && path.path.segments[0].ident == *self.param
                    && path.path.segments[0].arguments.is_none()
                {
                    *ty = self.arg.clone();
                    return;
                }
            }
            visit_mut::visit_type_mut(self, ty);
        }
    }
    let mut concrete = sig.clone();
    concrete.generics = syn::Generics::default();
    let mut replace = Replace {
        param,
        arg: Type::Group(syn::TypeGroup {
            group_token: syn::token::Group::default(),
            elem: Box::new(ty.clone()),
        }),
    };
    for input in &mut concrete.inputs {
        if let FnArg::Typed(typed) = input {
            replace.visit_type_mut(&mut typed.ty);
        }
    }
    if let syn::ReturnType::Type(_, ty) = &mut concrete.output {
        replace.visit_type_mut(ty);
    }
    concrete
}

/// E0001 for an associated type of the type parameter (`T::Item`) in the signature: each
/// instantiation has its own, and the schema records one type per position.
pub(crate) fn check_parameter_use(sig: &Signature, param: &syn::Ident, errors: &mut Errors) {
    struct Assoc<'a> {
        param: &'a syn::Ident,
        found: Vec<Type>,
    }
    impl VisitMut for Assoc<'_> {
        fn visit_type_path_mut(&mut self, node: &mut syn::TypePath) {
            if node.qself.is_none()
                && node.path.segments.len() > 1
                && node.path.segments[0].ident == *self.param
            {
                self.found.push(Type::Path(node.clone()));
            }
            visit_mut::visit_type_path_mut(self, node);
        }
    }
    let mut assoc = Assoc {
        param,
        found: Vec::new(),
    };
    for input in &sig.inputs {
        if let FnArg::Typed(typed) = input {
            assoc.visit_type_mut(&mut (*typed.ty).clone());
        }
    }
    if let syn::ReturnType::Type(_, ty) = &sig.output {
        assoc.visit_type_mut(&mut (**ty).clone());
    }
    for ty in assoc.found {
        errors.push(
            Diag::new(
                code::E0001,
                format!(
                    "`{}` cannot cross the boundary: it is an associated type of a type parameter",
                    ty_string(&ty)
                ),
                "each instantiation has its own associated type, and the schema records one type per position",
                "take the type as a parameter of its own, or write the type out in the signature",
            )
            .on(&ty),
        );
    }
}
