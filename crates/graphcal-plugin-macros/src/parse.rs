//! Parsing the `plugin!` input: graphcal extern-declaration signatures
//! with Rust bodies.
//!
//! The signature grammar deliberately mirrors `grammar.ebnf`'s
//! `extern_fn_decl` (binders, named parameters, `->` result) and
//! `dim_expr` (`*` and `/` between terms, `^` powers with integer or
//! parenthesized-rational exponents), so a declaration can be copied
//! verbatim between a `plugin!` block and the `.gcl` import site. Bodies
//! are opaque Rust blocks, captured as raw tokens and re-emitted by
//! codegen.
//!
//! This module is purely syntactic: names are classified (dimension
//! variable vs. prelude dimension vs. `Bool`/`Int`) and validated in
//! [`crate::lower`], where the binder context is known.

use proc_macro2::{Span, TokenStream};
use syn::Token;
use syn::parse::{Parse, ParseStream};

/// The full `plugin! { ... }` input: one or more function declarations.
pub struct PluginInput {
    pub functions: Vec<PluginFnDecl>,
}

/// One `fn name<Vars>(params) -> Result { body }` declaration.
pub struct PluginFnDecl {
    /// `///` doc comments, re-emitted onto the generated wrapper.
    pub docs: Vec<syn::Attribute>,
    pub name: syn::Ident,
    /// Dimension-variable binders, in declaration order (empty when the
    /// declaration has no `<...>` list).
    pub dim_vars: Vec<syn::Ident>,
    pub params: Vec<ParamDecl>,
    pub result: DimExprAst,
    /// The tokens inside the body's braces.
    pub body: TokenStream,
}

/// One `name: type` parameter.
pub struct ParamDecl {
    pub name: syn::Ident,
    pub ty: DimExprAst,
}

/// A parsed type position: `dim_expr` from the graphcal grammar. A lone
/// `Bool`/`Int` ident is also parsed as this and classified during
/// lowering.
pub struct DimExprAst {
    pub first: DimTermAst,
    pub rest: Vec<(MulOp, DimTermAst)>,
}

/// `*` or `/` between dimension terms.
#[derive(Clone, Copy)]
pub enum MulOp {
    Mul,
    Div,
}

/// One `dim_term`: a named dimension or a parenthesized sub-expression,
/// optionally raised to a power.
pub enum DimTermAst {
    Named {
        name: syn::Ident,
        pow: Option<ExponentAst>,
    },
    Group {
        inner: Box<DimExprAst>,
        pow: Option<ExponentAst>,
    },
}

/// A `^` exponent: `signed_integer` or `(signed_integer / signed_integer)`.
pub struct ExponentAst {
    pub num: i64,
    /// The denominator when the parenthesized rational form was used.
    pub den: Option<i64>,
    pub span: Span,
}

impl Parse for PluginInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut functions = Vec::new();
        while !input.is_empty() {
            functions.push(input.parse()?);
        }
        if functions.is_empty() {
            return Err(syn::Error::new(
                Span::call_site(),
                "plugin! declares no functions; a plugin must export at least one",
            ));
        }
        Ok(Self { functions })
    }
}

impl Parse for PluginFnDecl {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let docs = input.call(syn::Attribute::parse_outer)?;
        for attr in &docs {
            if !attr.path().is_ident("doc") {
                return Err(syn::Error::new_spanned(
                    attr,
                    "only doc comments are supported on plugin! functions",
                ));
            }
        }
        if input.peek(Token![pub]) {
            return Err(input.error(
                "plugin! functions are exported from the wasm module automatically; drop `pub`",
            ));
        }
        input.parse::<Token![fn]>()?;
        let name: syn::Ident = input.parse()?;

        let mut dim_vars = Vec::new();
        if input.peek(Token![<]) {
            let open_span = input.span();
            input.parse::<Token![<]>()?;
            loop {
                if input.peek(Token![>]) {
                    break;
                }
                let var = input.parse::<syn::Ident>()?;
                input.parse::<Token![:]>()?;
                let constraint = input.parse::<syn::Ident>()?;
                if constraint != "Dim" {
                    return Err(syn::Error::new(
                        constraint.span(),
                        format!(
                            "unsupported binder constraint `{constraint}`; extern signatures \
                             support `Dim` (write `{var}: Dim`)"
                        ),
                    ));
                }
                dim_vars.push(var);
                if input.peek(Token![,]) {
                    input.parse::<Token![,]>()?;
                } else {
                    break;
                }
            }
            input.parse::<Token![>]>()?;
            if dim_vars.is_empty() {
                return Err(syn::Error::new(
                    open_span,
                    "empty dimension-variable binder list; drop the `<>` or declare a variable",
                ));
            }
        }

        let params_content;
        syn::parenthesized!(params_content in input);
        let mut params = Vec::new();
        while !params_content.is_empty() {
            let name: syn::Ident = params_content.parse()?;
            params_content.parse::<Token![:]>()?;
            let ty: DimExprAst = params_content.parse()?;
            params.push(ParamDecl { name, ty });
            if params_content.peek(Token![,]) {
                params_content.parse::<Token![,]>()?;
            } else if params_content.is_empty() {
                break;
            } else {
                return Err(params_content.error("expected `,` between parameters"));
            }
        }

        input.parse::<Token![->]>()?;
        let result: DimExprAst = input.parse()?;

        if input.peek(Token![;]) {
            return Err(input.error(
                "plugin! functions need a Rust body; the semicolon-terminated declaration \
                 form belongs in the .gcl import site",
            ));
        }
        let body_content;
        syn::braced!(body_content in input);
        let body: TokenStream = body_content.parse()?;

        Ok(Self {
            docs,
            name,
            dim_vars,
            params,
            result,
            body,
        })
    }
}

impl Parse for DimExprAst {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let first: DimTermAst = input.parse()?;
        let mut rest = Vec::new();
        loop {
            if input.peek(Token![*]) {
                input.parse::<Token![*]>()?;
                rest.push((MulOp::Mul, input.parse()?));
            } else if input.peek(Token![/]) {
                input.parse::<Token![/]>()?;
                rest.push((MulOp::Div, input.parse()?));
            } else {
                break;
            }
        }
        Ok(Self { first, rest })
    }
}

impl Parse for DimTermAst {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.peek(syn::token::Paren) {
            let content;
            syn::parenthesized!(content in input);
            let inner: DimExprAst = content.parse()?;
            if !content.is_empty() {
                return Err(content.error("unexpected tokens in parenthesized dimension"));
            }
            let pow = parse_optional_pow(input)?;
            Ok(Self::Group {
                inner: Box::new(inner),
                pow,
            })
        } else if input.peek(syn::Ident) {
            let name: syn::Ident = input.parse()?;
            let pow = parse_optional_pow(input)?;
            Ok(Self::Named { name, pow })
        } else {
            Err(input.error(
                "expected a type: `Bool`, `Int`, or a dimension expression over dimension \
                 variables and prelude dimensions",
            ))
        }
    }
}

fn parse_optional_pow(input: ParseStream) -> syn::Result<Option<ExponentAst>> {
    if !input.peek(Token![^]) {
        return Ok(None);
    }
    input.parse::<Token![^]>()?;

    if input.peek(syn::token::Paren) {
        let content;
        let paren = syn::parenthesized!(content in input);
        let num = parse_signed_integer(&content)?;
        let den = if content.peek(Token![/]) {
            content.parse::<Token![/]>()?;
            Some(parse_signed_integer(&content)?)
        } else {
            None
        };
        if !content.is_empty() {
            return Err(content.error("expected `)` after the exponent"));
        }
        return Ok(Some(ExponentAst {
            num,
            den,
            span: paren.span.join(),
        }));
    }

    let start = input.span();
    let num = parse_signed_integer(input)?;
    Ok(Some(ExponentAst {
        num,
        den: None,
        span: start,
    }))
}

fn parse_signed_integer(input: ParseStream) -> syn::Result<i64> {
    let negative = if input.peek(Token![-]) {
        input.parse::<Token![-]>()?;
        true
    } else {
        false
    };
    let literal: syn::LitInt = input
        .parse()
        .map_err(|_| input.error("expected an integer exponent, e.g. `^2`, `^-3`, or `^(1/2)`"))?;
    if !literal.suffix().is_empty() {
        return Err(syn::Error::new(
            literal.span(),
            "exponents are plain integers; drop the type suffix",
        ));
    }
    let value: i64 = literal.base10_parse()?;
    Ok(if negative { -value } else { value })
}
