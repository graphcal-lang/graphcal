//! `#[derive(FormatEquivalent)]`: structural equality modulo formatting.
//!
//! The pipeline is attribute parsing (`ContainerAttr`, `FieldAttr`) → impl
//! header (`ImplTarget`) → per-shape comparison (`FieldsShape`). See the
//! derive's documentation in the crate root for the accepted forms.

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{
    Attribute, Data, DeriveInput, Field, Fields, GenericParam, Member, Type, Variant, parse_quote,
};

use crate::common::{attrs_named, phase_param};

const ATTR: &str = "fe";
const DERIVE: &str = "FormatEquivalent";

pub fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let target = ImplTarget::plan(input)?;
    let body = match &input.data {
        Data::Struct(data) => {
            let shape = FieldsShape::plan(&data.fields)?;
            let this = shape.pattern(&quote!(Self), Side::This);
            let other = shape.pattern(&quote!(Self), Side::Other);
            let comparison = shape.comparison();
            quote! {
                let #this = self;
                let #other = other;
                #comparison
            }
        }
        Data::Enum(data) => enum_body(data.variants.iter())?,
        Data::Union(data) => {
            return Err(syn::Error::new(
                data.union_token.span,
                "`FormatEquivalent` cannot be derived for unions",
            ));
        }
    };
    let header = target.header(&input.ident);
    Ok(quote! {
        #[automatically_derived]
        #header {
            fn format_equivalent(&self, other: &Self) -> bool {
                #body
            }
        }
    })
}

fn fe_attrs(attrs: &[Attribute]) -> impl Iterator<Item = &Attribute> {
    attrs_named(attrs, ATTR)
}

/// `#[fe(phase = <phase>)]` on the type.
struct ContainerAttr {
    phase: Option<Type>,
}

impl ContainerAttr {
    fn parse(input: &DeriveInput) -> syn::Result<Self> {
        let mut phase = None;
        for attr in fe_attrs(&input.attrs) {
            attr.parse_nested_meta(|meta| {
                if !meta.path.is_ident("phase") {
                    return Err(
                        meta.error("expected `phase = <phase>` on a `FormatEquivalent` type")
                    );
                }
                if phase.is_some() {
                    return Err(meta.error("duplicate `fe` key"));
                }
                phase = Some(meta.value()?.parse::<Type>()?);
                Ok(())
            })?;
        }
        Ok(Self { phase })
    }
}

/// Which type the generated impl is for.
enum ImplTarget {
    /// `impl FormatEquivalent for Name<Phase>`: the phase parameter is
    /// instantiated with the phase named by `#[fe(phase = ..)]`.
    Phase(Type),
    /// `impl<T: FormatEquivalent, ..> FormatEquivalent for Name<T, ..>`: every
    /// type parameter must itself be format-equivalent.
    Generic(syn::Generics),
}

impl ImplTarget {
    fn plan(input: &DeriveInput) -> syn::Result<Self> {
        let Some(phase) = ContainerAttr::parse(input)?.phase else {
            let mut generics = input.generics.clone();
            for param in &mut generics.params {
                if let GenericParam::Type(param) = param {
                    param.bounds.push(parse_quote!(FormatEquivalent));
                }
            }
            return Ok(Self::Generic(generics));
        };
        phase_param(input, DERIVE)?;
        Ok(Self::Phase(phase))
    }

    fn header(&self, name: &Ident) -> TokenStream {
        match self {
            Self::Phase(phase) => quote!(impl FormatEquivalent for #name<#phase>),
            Self::Generic(generics) => {
                let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
                quote!(impl #impl_generics FormatEquivalent for #name #type_generics #where_clause)
            }
        }
    }
}

/// Field attribute: `#[fe(skip)]`.
#[derive(Clone, Copy)]
enum FieldAttr {
    Compared,
    Skipped,
}

impl FieldAttr {
    fn parse(field: &Field) -> syn::Result<Self> {
        let mut parsed = Self::Compared;
        for attr in fe_attrs(&field.attrs) {
            attr.parse_nested_meta(|meta| {
                if !meta.path.is_ident("skip") {
                    return Err(meta.error("expected `skip` on a field"));
                }
                match parsed {
                    Self::Skipped => Err(meta.error("duplicate `fe` key")),
                    Self::Compared => {
                        parsed = Self::Skipped;
                        Ok(())
                    }
                }
            })?;
        }
        Ok(parsed)
    }
}

/// Which operand of `format_equivalent` a pattern destructures.
#[derive(Clone, Copy)]
enum Side {
    This,
    Other,
}

/// The fields of a struct or variant, in declaration order.
struct FieldsShape(Vec<(Member, FieldAttr)>);

impl FieldsShape {
    fn plan(fields: &Fields) -> syn::Result<Self> {
        fields
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let member = field
                    .ident
                    .clone()
                    .map_or_else(|| Member::from(index), Member::Named);
                Ok((member, FieldAttr::parse(field)?))
            })
            .collect::<syn::Result<_>>()
            .map(Self)
    }

    fn binding(index: usize, side: Side) -> Ident {
        match side {
            Side::This => format_ident!("__self_{index}"),
            Side::Other => format_ident!("__other_{index}"),
        }
    }

    /// Exhaustive destructuring pattern: compared fields are bound by
    /// reference, skipped fields are matched by `_`. The brace form
    /// (`Path { 0: x }`) is valid for named, tuple, and unit shapes alike.
    fn pattern(&self, path: &TokenStream, side: Side) -> TokenStream {
        let fields = self
            .0
            .iter()
            .enumerate()
            .map(|(index, (member, attr))| match attr {
                FieldAttr::Compared => {
                    let binding = Self::binding(index, side);
                    quote!(#member: #binding)
                }
                FieldAttr::Skipped => quote!(#member: _),
            });
        quote!(#path { #(#fields),* })
    }

    /// Conjunction of the compared fields' equivalences (`true` when none).
    fn comparison(&self) -> TokenStream {
        let comparisons = self
            .0
            .iter()
            .enumerate()
            .filter(|(_, (_, attr))| matches!(attr, FieldAttr::Compared))
            .map(|(index, _)| {
                let this = Self::binding(index, Side::This);
                let other = Self::binding(index, Side::Other);
                quote!(FormatEquivalent::format_equivalent(#this, #other))
            })
            .collect::<Vec<_>>();
        if comparisons.is_empty() {
            quote!(true)
        } else {
            quote!(#(#comparisons)&&*)
        }
    }
}

fn enum_body<'a>(variants: impl ExactSizeIterator<Item = &'a Variant>) -> syn::Result<TokenStream> {
    let variant_count = variants.len();
    if variant_count == 0 {
        return Ok(quote!(match *self {}));
    }
    let arms = variants
        .map(|variant| {
            if let Some(attr) = fe_attrs(&variant.attrs).next() {
                return Err(syn::Error::new(
                    attr.span(),
                    "`#[fe(..)]` is not supported on enum variants",
                ));
            }
            let name = &variant.ident;
            let path = quote!(Self::#name);
            let shape = FieldsShape::plan(&variant.fields)?;
            let this = shape.pattern(&path, Side::This);
            let other = shape.pattern(&path, Side::Other);
            let comparison = shape.comparison();
            Ok(quote!((#this, #other) => #comparison,))
        })
        .collect::<syn::Result<Vec<_>>>()?;
    // A single-variant enum needs no mismatch arm (it would be unreachable).
    let mismatch = (variant_count > 1).then(|| quote!(_ => false,));
    Ok(quote! {
        match (self, other) {
            #(#arms)*
            #mismatch
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand_str(input: &DeriveInput) -> String {
        expand(input).expect("expansion succeeds").to_string()
    }

    fn error_of(input: &DeriveInput) -> String {
        expand(input).expect_err("expected an error").to_string()
    }

    #[test]
    fn struct_compares_fields_and_skips_marked_ones() {
        let input: DeriveInput = parse_quote! {
            struct Decl {
                name: Name,
                #[fe(skip)]
                span: Span,
                value: Expr,
            }
        };
        let expected = quote! {
            #[automatically_derived]
            impl FormatEquivalent for Decl {
                fn format_equivalent(&self, other: &Self) -> bool {
                    let Self { name: __self_0, span: _, value: __self_2 } = self;
                    let Self { name: __other_0, span: _, value: __other_2 } = other;
                    FormatEquivalent::format_equivalent(__self_0, __other_0)
                        && FormatEquivalent::format_equivalent(__self_2, __other_2)
                }
            }
        };
        assert_eq!(expand_str(&input), expected.to_string());
    }

    #[test]
    fn struct_without_compared_fields_is_always_equivalent() {
        let input: DeriveInput = parse_quote! {
            struct Marker(#[fe(skip)] Span);
        };
        let expected = quote! {
            #[automatically_derived]
            impl FormatEquivalent for Marker {
                fn format_equivalent(&self, other: &Self) -> bool {
                    let Self { 0: _ } = self;
                    let Self { 0: _ } = other;
                    true
                }
            }
        };
        assert_eq!(expand_str(&input), expected.to_string());
    }

    #[test]
    fn enum_matches_pairs_of_variants_of_every_shape() {
        let input: DeriveInput = parse_quote! {
            #[fe(phase = Raw)]
            enum Kind<P: Phase = Raw> {
                Unit,
                Tuple(Box<Expr<P>>, #[fe(skip)] Span),
                Named { op: Op, #[fe(skip)] span: Span },
            }
        };
        let expected = quote! {
            #[automatically_derived]
            impl FormatEquivalent for Kind<Raw> {
                fn format_equivalent(&self, other: &Self) -> bool {
                    match (self, other) {
                        (Self::Unit {}, Self::Unit {}) => true,
                        (Self::Tuple { 0: __self_0, 1: _ }, Self::Tuple { 0: __other_0, 1: _ }) =>
                            FormatEquivalent::format_equivalent(__self_0, __other_0),
                        (Self::Named { op: __self_0, span: _ }, Self::Named { op: __other_0, span: _ }) =>
                            FormatEquivalent::format_equivalent(__self_0, __other_0),
                        _ => false,
                    }
                }
            }
        };
        assert_eq!(expand_str(&input), expected.to_string());
    }

    #[test]
    fn single_variant_enum_has_no_mismatch_arm() {
        let input: DeriveInput = parse_quote! {
            enum Sugar {
                Multi(Decl),
            }
        };
        let expanded = expand_str(&input);
        assert!(
            expanded
                .contains("(Self :: Multi { 0 : __self_0 } , Self :: Multi { 0 : __other_0 }) =>")
        );
        assert!(!expanded.contains("_ => false"));
    }

    #[test]
    fn empty_enum_matches_on_self() {
        let input: DeriveInput = parse_quote! {
            enum Never {}
        };
        assert!(expand_str(&input).contains("match * self { }"));
    }

    #[test]
    fn generic_parameters_are_bounded_by_the_trait() {
        let input: DeriveInput = parse_quote! {
            enum Bindings<'a, B: Clone, C = u8> where B: Default {
                Bare,
                Listed(&'a [B], C),
            }
        };
        let expanded = expand_str(&input);
        assert!(expanded.contains(
            "impl < 'a , B : Clone + FormatEquivalent , C : FormatEquivalent > \
             FormatEquivalent for Bindings < 'a , B , C > where B : Default"
        ));
    }

    #[test]
    fn phase_attribute_requires_a_single_type_parameter() {
        let none: DeriveInput = parse_quote! {
            #[fe(phase = Raw)]
            struct S { a: u8 }
        };
        assert!(error_of(&none).contains("`FormatEquivalent` requires exactly one type parameter"));
        let lifetime: DeriveInput = parse_quote! {
            #[fe(phase = Raw)]
            struct S<'a, P> { a: &'a X<P> }
        };
        assert!(error_of(&lifetime).contains("single phase type parameter"));
    }

    #[test]
    fn unknown_and_duplicate_keys_are_rejected() {
        let unknown_container: DeriveInput = parse_quote! {
            #[fe(skip)]
            struct S { a: u8 }
        };
        assert!(error_of(&unknown_container).contains("expected `phase = <phase>`"));
        let duplicate_container: DeriveInput = parse_quote! {
            #[fe(phase = A, phase = B)]
            struct S<P> { a: X<P> }
        };
        assert_eq!(error_of(&duplicate_container), "duplicate `fe` key");
        let unknown_field: DeriveInput = parse_quote! {
            struct S { #[fe(phase = A)] a: u8 }
        };
        assert!(error_of(&unknown_field).contains("expected `skip` on a field"));
        let duplicate_field: DeriveInput = parse_quote! {
            struct S { #[fe(skip, skip)] a: u8 }
        };
        assert_eq!(error_of(&duplicate_field), "duplicate `fe` key");
    }

    #[test]
    fn variant_attributes_are_rejected() {
        let input: DeriveInput = parse_quote! {
            enum E {
                A,
                #[fe(skip)]
                B(u8),
            }
        };
        assert!(error_of(&input).contains("not supported on enum variants"));
    }

    #[test]
    fn foreign_attributes_are_ignored() {
        let input: DeriveInput = parse_quote! {
            #[phase_lift(from = A, to = B)]
            struct S {
                #[phase_lift(map)]
                a: u8,
            }
        };
        assert!(
            expand_str(&input)
                .contains("FormatEquivalent :: format_equivalent (__self_0 , __other_0)")
        );
    }

    #[test]
    fn unions_are_rejected() {
        let input: DeriveInput = parse_quote! {
            union U { a: u8 }
        };
        assert!(error_of(&input).contains("cannot be derived for unions"));
    }
}
