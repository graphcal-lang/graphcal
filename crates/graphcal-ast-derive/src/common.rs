//! Parsing helpers shared by the AST derives.

use proc_macro2::Ident;
use syn::spanned::Spanned;
use syn::{Attribute, DeriveInput, GenericParam};

/// The single type parameter that stands for the AST phase.
///
/// `derive` names the derive in error messages.
pub fn phase_param(input: &DeriveInput, derive: &str) -> syn::Result<Ident> {
    let mut type_params = Vec::new();
    for param in &input.generics.params {
        match param {
            GenericParam::Type(param) => type_params.push(param.ident.clone()),
            GenericParam::Lifetime(_) | GenericParam::Const(_) => {
                return Err(syn::Error::new(
                    param.span(),
                    format!("`{derive}` supports only a single phase type parameter"),
                ));
            }
        }
    }
    match <[Ident; 1]>::try_from(type_params) {
        Ok([phase]) => Ok(phase),
        Err(_) => Err(syn::Error::new(
            input.generics.span(),
            format!("`{derive}` requires exactly one type parameter: the AST phase"),
        )),
    }
}

/// The attributes in the derive's own namespace (`#[<name>(..)]`).
pub fn attrs_named<'a>(
    attrs: &'a [Attribute],
    name: &'static str,
) -> impl Iterator<Item = &'a Attribute> {
    attrs.iter().filter(move |attr| attr.path().is_ident(name))
}
