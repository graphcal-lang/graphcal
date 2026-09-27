//! `#[derive(PhaseLift)]`: structural `From<T<Source>> for T<Target>`.
//!
//! The pipeline is attribute parsing (`ContainerAttr`, `FieldAttr`,
//! `VariantAttr`) → per-field conversion plan (`FieldLift`) → emission. See
//! the derive's documentation in the crate root for the accepted forms.

use proc_macro2::{Ident, TokenStream, TokenTree};
use quote::{ToTokens, format_ident, quote};
use syn::spanned::Spanned;
use syn::{
    Attribute, Data, DeriveInput, Field, Fields, GenericArgument, GenericParam, PathArguments,
    Type, Variant,
};

const ATTR: &str = "phase_lift";

pub fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let phase = phase_param(input)?;
    let container = ContainerAttr::parse(input)?;
    let name = &input.ident;
    let source_ty = &container.from;
    let target_ty = &container.to;
    let source = format_ident!("__phase_lift_source");

    let body = match &input.data {
        Data::Struct(data) => {
            let shape = FieldsShape::plan(&data.fields, &phase)?;
            let pattern = shape.pattern(quote!(#name));
            let construct = shape.construct(quote!(Self));
            quote! {
                let #pattern = #source;
                #construct
            }
        }
        Data::Enum(data) => {
            let arms = data
                .variants
                .iter()
                .map(|variant| variant_arm(name, variant, &phase))
                .collect::<syn::Result<Vec<_>>>()?;
            quote! {
                match #source {
                    #(#arms)*
                }
            }
        }
        Data::Union(data) => {
            return Err(syn::Error::new(
                data.union_token.span,
                "`PhaseLift` cannot be derived for unions",
            ));
        }
    };

    Ok(quote! {
        #[automatically_derived]
        impl ::core::convert::From<#name<#source_ty>> for #name<#target_ty> {
            fn from(#source: #name<#source_ty>) -> Self {
                #body
            }
        }
    })
}

/// The single type parameter that stands for the AST phase.
fn phase_param(input: &DeriveInput) -> syn::Result<Ident> {
    let mut type_params = Vec::new();
    for param in &input.generics.params {
        match param {
            GenericParam::Type(param) => type_params.push(param.ident.clone()),
            GenericParam::Lifetime(_) | GenericParam::Const(_) => {
                return Err(syn::Error::new(
                    param.span(),
                    "`PhaseLift` supports only a single phase type parameter",
                ));
            }
        }
    }
    match <[Ident; 1]>::try_from(type_params) {
        Ok([phase]) => Ok(phase),
        Err(_) => Err(syn::Error::new(
            input.generics.span(),
            "`PhaseLift` requires exactly one type parameter: the AST phase",
        )),
    }
}

fn phase_lift_attrs(attrs: &[Attribute]) -> impl Iterator<Item = &Attribute> {
    attrs.iter().filter(|attr| attr.path().is_ident(ATTR))
}

/// `#[phase_lift(from = <source phase>, to = <target phase>)]`.
struct ContainerAttr {
    from: Type,
    to: Type,
}

impl ContainerAttr {
    fn parse(input: &DeriveInput) -> syn::Result<Self> {
        let mut from = None;
        let mut to = None;
        for attr in phase_lift_attrs(&input.attrs) {
            attr.parse_nested_meta(|meta| {
                let slot = match meta.path.get_ident() {
                    Some(key) if key == "from" => &mut from,
                    Some(key) if key == "to" => &mut to,
                    _ => {
                        return Err(meta.error(
                            "expected `from = <phase>` or `to = <phase>` on a `PhaseLift` type",
                        ));
                    }
                };
                if slot.is_some() {
                    return Err(meta.error("duplicate `phase_lift` key"));
                }
                *slot = Some(meta.value()?.parse::<Type>()?);
                Ok(())
            })?;
        }
        match (from, to) {
            (Some(from), Some(to)) => Ok(Self { from, to }),
            _ => Err(syn::Error::new(
                input.ident.span(),
                "`#[derive(PhaseLift)]` requires \
                 `#[phase_lift(from = <source phase>, to = <target phase>)]`",
            )),
        }
    }
}

/// Field attribute: `#[phase_lift(map)]` or `#[phase_lift(map = <method>)]`.
enum FieldAttr {
    Structural,
    Map(Ident),
}

impl FieldAttr {
    fn parse(field: &Field) -> syn::Result<Self> {
        let mut parsed = None;
        for attr in phase_lift_attrs(&field.attrs) {
            attr.parse_nested_meta(|meta| {
                if !meta.path.is_ident("map") {
                    return Err(meta.error("expected `map` or `map = <method>` on a field"));
                }
                if parsed.is_some() {
                    return Err(meta.error("duplicate `phase_lift` key"));
                }
                let method = if meta.input.peek(syn::Token![=]) {
                    meta.value()?.parse::<Ident>()?
                } else {
                    format_ident!("map")
                };
                parsed = Some(Self::Map(method));
                Ok(())
            })?;
        }
        Ok(parsed.unwrap_or(Self::Structural))
    }
}

/// Variant attribute: `#[phase_lift(from_payload)]`.
enum VariantAttr {
    Structural,
    FromPayload,
}

impl VariantAttr {
    fn parse(variant: &Variant) -> syn::Result<Self> {
        let mut parsed = Self::Structural;
        for attr in phase_lift_attrs(&variant.attrs) {
            attr.parse_nested_meta(|meta| {
                if !meta.path.is_ident("from_payload") {
                    return Err(meta.error("expected `from_payload` on an enum variant"));
                }
                match parsed {
                    Self::FromPayload => Err(meta.error("duplicate `phase_lift` key")),
                    Self::Structural => {
                        parsed = Self::FromPayload;
                        Ok(())
                    }
                }
            })?;
        }
        Ok(parsed)
    }
}

/// How one field is carried from the source phase to the target phase.
enum FieldLift<'a> {
    /// The field type does not mention the phase: moved unchanged.
    Move,
    /// Rebuilt by the field type's shape (`Box` / `Vec` / `Option` / `From`).
    Structural(&'a Type),
    /// `field.<method>(From::from)`.
    Map(Ident),
}

impl<'a> FieldLift<'a> {
    fn plan(field: &'a Field, phase: &Ident) -> syn::Result<Self> {
        let mentions_phase = mentions_phase(field.ty.to_token_stream(), phase);
        match (FieldAttr::parse(field)?, mentions_phase) {
            (FieldAttr::Structural, false) => Ok(Self::Move),
            (FieldAttr::Structural, true) => Ok(Self::Structural(&field.ty)),
            (FieldAttr::Map(method), true) => Ok(Self::Map(method)),
            (FieldAttr::Map(_), false) => Err(syn::Error::new(
                field.ty.span(),
                "`#[phase_lift(map)]` on a field whose type does not mention the phase \
                 parameter; such fields are moved unchanged",
            )),
        }
    }

    fn convert(&self, value: TokenStream) -> TokenStream {
        match self {
            Self::Move => value,
            Self::Structural(ty) => lift_value(ty, &value),
            Self::Map(method) => quote!(#value.#method(::core::convert::From::from)),
        }
    }
}

/// The fields of a struct or variant together with their binding names.
enum FieldsShape<'a> {
    Named(Vec<(Ident, FieldLift<'a>)>),
    Unnamed(Vec<(Ident, FieldLift<'a>)>),
    Unit,
}

impl<'a> FieldsShape<'a> {
    fn plan(fields: &'a Fields, phase: &Ident) -> syn::Result<Self> {
        match fields {
            Fields::Named(named) => named
                .named
                .iter()
                .map(|field| {
                    let ident = field
                        .ident
                        .clone()
                        .ok_or_else(|| syn::Error::new(field.span(), "named field without name"))?;
                    Ok((ident, FieldLift::plan(field, phase)?))
                })
                .collect::<syn::Result<_>>()
                .map(Self::Named),
            Fields::Unnamed(unnamed) => unnamed
                .unnamed
                .iter()
                .enumerate()
                .map(|(index, field)| {
                    Ok((
                        format_ident!("__field{index}"),
                        FieldLift::plan(field, phase)?,
                    ))
                })
                .collect::<syn::Result<_>>()
                .map(Self::Unnamed),
            Fields::Unit => Ok(Self::Unit),
        }
    }

    fn bindings<'s>(fields: &'s [(Ident, FieldLift<'a>)]) -> impl Iterator<Item = &'s Ident> {
        fields.iter().map(|(ident, _)| ident)
    }

    /// Destructuring pattern that binds every field by value.
    fn pattern(&self, path: TokenStream) -> TokenStream {
        match self {
            Self::Named(fields) => {
                let bindings = Self::bindings(fields);
                quote!(#path { #(#bindings),* })
            }
            Self::Unnamed(fields) => {
                let bindings = Self::bindings(fields);
                quote!(#path ( #(#bindings),* ))
            }
            Self::Unit => path,
        }
    }

    /// Constructor expression from the bindings of [`Self::pattern`].
    fn construct(&self, path: TokenStream) -> TokenStream {
        match self {
            Self::Named(fields) => {
                let inits = fields.iter().map(|(ident, lift)| match lift {
                    FieldLift::Move => quote!(#ident),
                    FieldLift::Structural(_) | FieldLift::Map(_) => {
                        let value = lift.convert(quote!(#ident));
                        quote!(#ident: #value)
                    }
                });
                quote!(#path { #(#inits),* })
            }
            Self::Unnamed(fields) => {
                let values = fields
                    .iter()
                    .map(|(ident, lift)| lift.convert(quote!(#ident)));
                quote!(#path ( #(#values),* ))
            }
            Self::Unit => path,
        }
    }
}

fn variant_arm(enum_name: &Ident, variant: &Variant, phase: &Ident) -> syn::Result<TokenStream> {
    let variant_name = &variant.ident;
    let source_path = quote!(#enum_name::#variant_name);
    match VariantAttr::parse(variant)? {
        VariantAttr::Structural => {
            let shape = FieldsShape::plan(&variant.fields, phase)?;
            let pattern = shape.pattern(source_path);
            let construct = shape.construct(quote!(Self::#variant_name));
            Ok(quote!(#pattern => #construct,))
        }
        VariantAttr::FromPayload => match &variant.fields {
            Fields::Unnamed(unnamed) if unnamed.unnamed.len() == 1 => {
                let payload = format_ident!("__payload");
                Ok(quote!(#source_path(#payload) => ::core::convert::From::from(#payload),))
            }
            Fields::Named(_) | Fields::Unnamed(_) | Fields::Unit => Err(syn::Error::new(
                variant.span(),
                "`#[phase_lift(from_payload)]` requires a tuple variant with exactly one field",
            )),
        },
    }
}

/// Whether a field type depends on the phase: it names the phase parameter
/// or `Self` (the phase-parameterized type itself) anywhere, including inside
/// groups.
fn mentions_phase(tokens: TokenStream, phase: &Ident) -> bool {
    tokens.into_iter().any(|token| match token {
        TokenTree::Ident(candidate) => candidate == *phase || candidate == "Self",
        TokenTree::Group(group) => mentions_phase(group.stream(), phase),
        TokenTree::Punct(_) | TokenTree::Literal(_) => false,
    })
}

/// Standard containers that are rebuilt element-wise.
#[derive(Clone, Copy)]
enum Container {
    Box,
    Vec,
    Option,
}

impl Container {
    /// Accepted spellings: the prelude name or a `std` / `alloc` / `core`
    /// path to the item.
    fn recognize(path: &syn::Path) -> Option<Self> {
        let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
        match segments.as_slice() {
            ["Box"] | ["std" | "alloc", "boxed", "Box"] => Some(Self::Box),
            ["Vec"] | ["std" | "alloc", "vec", "Vec"] => Some(Self::Vec),
            ["Option"] | ["std" | "core", "option", "Option"] => Some(Self::Option),
            _ => None,
        }
    }
}

/// Split `ty` into a recognized container and its element type.
fn container_of(ty: &Type) -> Option<(Container, &Type)> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    if type_path.qself.is_some() {
        return None;
    }
    let path = &type_path.path;
    let last = path.segments.last()?;
    if path
        .segments
        .iter()
        .rev()
        .skip(1)
        .any(|segment| !matches!(segment.arguments, PathArguments::None))
    {
        return None;
    }
    let container = Container::recognize(path)?;
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    match args.args.iter().collect::<Vec<_>>().as_slice() {
        [GenericArgument::Type(element)] => Some((container, element)),
        _ => None,
    }
}

/// Expression converting `value` (of type `ty` in the source phase) into the
/// same type in the target phase.
fn lift_value(ty: &Type, value: &TokenStream) -> TokenStream {
    match container_of(ty) {
        Some((Container::Box, element)) => {
            let inner = lift_value(element, &quote!(*#value));
            quote!(::std::boxed::Box::new(#inner))
        }
        Some((Container::Vec, element)) => {
            let lift = lift_fn(element);
            quote!(#value.into_iter().map(#lift).collect::<::std::vec::Vec<_>>())
        }
        Some((Container::Option, element)) => {
            let lift = lift_fn(element);
            quote!(#value.map(#lift))
        }
        None => quote!(::core::convert::From::from(#value)),
    }
}

/// A callable converting one element of type `ty`.
fn lift_fn(ty: &Type) -> TokenStream {
    if container_of(ty).is_none() {
        return quote!(::core::convert::From::from);
    }
    let element = format_ident!("__element");
    let body = lift_value(ty, &quote!(#element));
    quote!(|#element| #body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    fn expand_str(input: &DeriveInput) -> String {
        expand(input).expect("expansion succeeds").to_string()
    }

    fn error_of(input: &DeriveInput) -> String {
        expand(input).expect_err("expected an error").to_string()
    }

    #[test]
    fn struct_moves_phase_invariant_fields_and_lifts_the_rest() {
        let input: DeriveInput = parse_quote! {
            #[phase_lift(from = Raw, to = Desugared)]
            struct Decl<P: Phase = Raw> {
                name: Name,
                ty: TypeExpr<P>,
                value: Option<Expr<P>>,
                args: Vec<Box<Expr<P>>>,
            }
        };
        let expected = quote! {
            #[automatically_derived]
            impl ::core::convert::From<Decl<Raw>> for Decl<Desugared> {
                fn from(__phase_lift_source: Decl<Raw>) -> Self {
                    let Decl { name, ty, value, args } = __phase_lift_source;
                    Self {
                        name,
                        ty: ::core::convert::From::from(ty),
                        value: value.map(::core::convert::From::from),
                        args: args
                            .into_iter()
                            .map(|__element| ::std::boxed::Box::new(::core::convert::From::from(*__element)))
                            .collect::<::std::vec::Vec<_>>()
                    }
                }
            }
        };
        assert_eq!(expand_str(&input), expected.to_string());
    }

    #[test]
    fn enum_supports_every_variant_shape_and_attributes() {
        let input: DeriveInput = parse_quote! {
            #[phase_lift(from = Raw, to = Desugared)]
            enum Kind<P: Phase> {
                Unit,
                Tuple(Box<Expr<P>>, u8),
                Named {
                    #[phase_lift(map)]
                    args: NonEmpty<Arg<P>>,
                },
                #[phase_lift(from_payload)]
                Sugar(P::Sugar),
            }
        };
        let expected = quote! {
            #[automatically_derived]
            impl ::core::convert::From<Kind<Raw>> for Kind<Desugared> {
                fn from(__phase_lift_source: Kind<Raw>) -> Self {
                    match __phase_lift_source {
                        Kind::Unit => Self::Unit,
                        Kind::Tuple(__field0, __field1) => Self::Tuple(
                            ::std::boxed::Box::new(::core::convert::From::from(*__field0)),
                            __field1
                        ),
                        Kind::Named { args } => Self::Named {
                            args: args.map(::core::convert::From::from)
                        },
                        Kind::Sugar(__payload) => ::core::convert::From::from(__payload),
                    }
                }
            }
        };
        assert_eq!(expand_str(&input), expected.to_string());
    }

    #[test]
    fn map_accepts_a_custom_method() {
        let input: DeriveInput = parse_quote! {
            #[phase_lift(from = Raw, to = Desugared)]
            struct Node<P: Phase> {
                #[phase_lift(map = map_formula)]
                definition: Definition<Expr<P>, Name>,
            }
        };
        assert!(
            expand_str(&input)
                .contains("definition . map_formula (:: core :: convert :: From :: from)")
        );
    }

    #[test]
    fn qualified_std_containers_are_recognized() {
        let input: DeriveInput = parse_quote! {
            #[phase_lift(from = A, to = B)]
            struct S<P> {
                a: std::boxed::Box<X<P>>,
                b: ::std::vec::Vec<X<P>>,
                c: core::option::Option<X<P>>,
            }
        };
        let expanded = expand_str(&input);
        assert!(
            expanded.contains(
                ":: std :: boxed :: Box :: new (:: core :: convert :: From :: from (* a))"
            )
        );
        assert!(expanded.contains("b . into_iter ()"));
        assert!(expanded.contains("c . map (:: core :: convert :: From :: from)"));
    }

    #[test]
    fn missing_container_attribute_is_rejected() {
        let input: DeriveInput = parse_quote! {
            struct S<P> { a: X<P> }
        };
        assert!(error_of(&input).contains("requires `#[phase_lift(from = <source phase>"));
    }

    #[test]
    fn incomplete_container_attribute_is_rejected() {
        let input: DeriveInput = parse_quote! {
            #[phase_lift(from = A)]
            struct S<P> { a: X<P> }
        };
        assert!(error_of(&input).contains("requires `#[phase_lift(from = <source phase>"));
    }

    #[test]
    fn duplicate_container_key_is_rejected() {
        let input: DeriveInput = parse_quote! {
            #[phase_lift(from = A, from = B, to = C)]
            struct S<P> { a: X<P> }
        };
        assert_eq!(error_of(&input), "duplicate `phase_lift` key");
    }

    #[test]
    fn unknown_container_key_is_rejected() {
        let input: DeriveInput = parse_quote! {
            #[phase_lift(from = A, to = B, skip)]
            struct S<P> { a: X<P> }
        };
        assert!(error_of(&input).contains("expected `from = <phase>`"));
    }

    #[test]
    fn phase_parameter_must_be_unique() {
        let none: DeriveInput = parse_quote! {
            #[phase_lift(from = A, to = B)]
            struct S { a: u8 }
        };
        assert!(error_of(&none).contains("exactly one type parameter"));
        let two: DeriveInput = parse_quote! {
            #[phase_lift(from = A, to = B)]
            struct S<P, Q> { a: X<P>, b: Q }
        };
        assert!(error_of(&two).contains("exactly one type parameter"));
        let lifetime: DeriveInput = parse_quote! {
            #[phase_lift(from = A, to = B)]
            struct S<'a, P> { a: &'a X<P> }
        };
        assert!(error_of(&lifetime).contains("single phase type parameter"));
    }

    #[test]
    fn map_on_phase_invariant_field_is_rejected() {
        let input: DeriveInput = parse_quote! {
            #[phase_lift(from = A, to = B)]
            struct S<P> {
                #[phase_lift(map)]
                a: NonEmpty<u8>,
                b: X<P>,
            }
        };
        assert!(error_of(&input).contains("does not mention the phase parameter"));
    }

    #[test]
    fn from_payload_requires_a_single_tuple_field() {
        let input: DeriveInput = parse_quote! {
            #[phase_lift(from = A, to = B)]
            enum E<P> {
                #[phase_lift(from_payload)]
                Sugar { a: X<P> },
            }
        };
        assert!(error_of(&input).contains("exactly one field"));
    }

    #[test]
    fn misplaced_keys_are_rejected() {
        let variant_key_on_field: DeriveInput = parse_quote! {
            #[phase_lift(from = A, to = B)]
            struct S<P> {
                #[phase_lift(from_payload)]
                a: X<P>,
            }
        };
        assert!(error_of(&variant_key_on_field).contains("expected `map`"));
        let field_key_on_variant: DeriveInput = parse_quote! {
            #[phase_lift(from = A, to = B)]
            enum E<P> {
                #[phase_lift(map)]
                V(X<P>),
            }
        };
        assert!(error_of(&field_key_on_variant).contains("expected `from_payload`"));
    }

    #[test]
    fn unions_are_rejected() {
        let input: DeriveInput = parse_quote! {
            #[phase_lift(from = A, to = B)]
            union U<P> { a: X<P> }
        };
        assert!(error_of(&input).contains("cannot be derived for unions"));
    }
}
