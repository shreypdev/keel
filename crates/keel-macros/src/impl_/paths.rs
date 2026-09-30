//! The crate root that generated code names its dependencies through (SPEC 16.3).
//!
//! Generated code never says `keel_wire::Encode`; it says `::keel::wire::Encode`, so users
//! depend on the `keel` facade only. `#[keel(crate = "path")]` (or `crate = "path"` in the
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
            path: syn::parse_quote!(::keel),
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
                "the value names the crate that generated code refers to, for example `::keel` or `::keel_runtime`",
                "write a path such as `crate = \"::keel\"`",
            )
            .at(lit.span())),
        }
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
        assert_eq!(render(root.wire()), "::keel::wire");
        assert_eq!(render(root.meta()), "::keel::meta");
        assert_eq!(render(root.runtime()), "::keel::runtime");
        assert_eq!(render(root.signals()), "::keel::signals");
        assert_eq!(render(root.query()), "::keel::query");
    }

    #[test]
    fn override_is_parsed_from_a_string() {
        let lit: syn::LitStr = syn::parse_quote!("::keel_runtime");
        let root = Root::from_lit(&lit).unwrap();
        assert_eq!(render(root.runtime()), "::keel_runtime::runtime");
    }

    #[test]
    fn invalid_override_is_a_diagnostic() {
        let lit: syn::LitStr = syn::parse_quote!("not a path!");
        let err = Root::from_lit(&lit).unwrap_err();
        assert!(err.to_string().contains("error[keel::E0008]"));
    }
}
