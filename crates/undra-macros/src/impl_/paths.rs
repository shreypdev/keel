//! The crate root that generated code names its dependencies through (SPEC 16.3).
//!
//! Generated code never says `undra_wire::Encode`; it says `::undra::wire::Encode`, so users
//! depend on the `undra` facade only. `#[undra(crate = "path")]` (or `crate = "path"` in the
//! macro arguments) overrides the root; workspace crates that cannot name the facade use
//! it to point at the crates directly.

use proc_macro2::TokenStream;
use quote::quote;

use super::diag::{Diag, code};

/// The path that `wire`, `meta`, `runtime`, `signals` and `query` hang off.
#[derive(Clone, Debug)]
pub(crate) struct Root {
    path: syn::Path,
}

impl Default for Root {
    fn default() -> Root {
        Root {
            path: syn::parse_quote!(::undra),
        }
    }
}

impl Root {
    /// Parses the string of `crate = "..."`.
    pub(crate) fn from_lit(lit: &syn::LitStr) -> syn::Result<Root> {
        match lit.parse::<syn::Path>() {
            Ok(path) => Ok(Root { path }),
            Err(_) => Err(Diag::new(
                code::E0008,
                format!("`{}` is not a valid path for `crate = \"..\"`", lit.value()),
                "the value names the crate that generated code refers to, for example `::undra` or `::undra_runtime`",
                "write a path such as `crate = \"::undra\"`",
            )
            .at(lit.span())),
        }
    }

    /// The path as written (`::undra`), for a macro that hands it to another expansion.
    pub(crate) fn path_string(&self) -> String {
        let path = &self.path;
        quote!(#path).to_string().replace(' ', "")
    }

    /// `#root::__instantiate`: the hidden macro that the template of a generic type calls.
    pub(crate) fn instantiate(&self) -> TokenStream {
        let path = &self.path;
        quote!(#path::__instantiate)
    }

    /// `#root::wire`.
    pub(crate) fn wire(&self) -> TokenStream {
        let path = &self.path;
        quote!(#path::wire)
    }

    /// `#root::meta`.
    pub(crate) fn meta(&self) -> TokenStream {
        let path = &self.path;
        quote!(#path::meta)
    }

    /// `#root::runtime`.
    pub(crate) fn runtime(&self) -> TokenStream {
        let path = &self.path;
        quote!(#path::runtime)
    }

    /// `#root::signals`.
    pub(crate) fn signals(&self) -> TokenStream {
        let path = &self.path;
        quote!(#path::signals)
    }

    /// `#root::query`.
    pub(crate) fn query(&self) -> TokenStream {
        let path = &self.path;
        quote!(#path::query)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(tokens: TokenStream) -> String {
        tokens.to_string().replace(' ', "")
    }

    #[test]
    fn default_root_is_the_facade() {
        let root = Root::default();
        assert_eq!(render(root.wire()), "::undra::wire");
        assert_eq!(render(root.meta()), "::undra::meta");
        assert_eq!(render(root.runtime()), "::undra::runtime");
        assert_eq!(render(root.signals()), "::undra::signals");
        assert_eq!(render(root.query()), "::undra::query");
        assert_eq!(render(root.instantiate()), "::undra::__instantiate");
        assert_eq!(root.path_string(), "::undra");
    }

    #[test]
    fn override_is_parsed_from_a_string() {
        let lit: syn::LitStr = syn::parse_quote!("::undra_runtime");
        let root = Root::from_lit(&lit).unwrap();
        assert_eq!(render(root.runtime()), "::undra_runtime::runtime");
    }

    #[test]
    fn invalid_override_is_a_diagnostic() {
        let lit: syn::LitStr = syn::parse_quote!("not a path!");
        let err = Root::from_lit(&lit).unwrap_err();
        assert!(err.to_string().contains("error[undra::E0008]"));
    }
}
