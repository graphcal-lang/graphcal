//! Lowering the parsed `plugin!` input into a validated signature IR.
//!
//! This is where names are classified against the binder list and the
//! dimension vocabulary, monomials are folded into canonical form
//! (duplicate factors merged, cancelled factors dropped), and the
//! compiler's signature invariants are enforced with source spans:
//!
//! - dimension variables are distinct, do not shadow prelude vocabulary,
//!   and every one is *bound* by a parameter that is exactly that bare
//!   variable before (or at) its first compound use — the same
//!   use-before-binding discipline `FunctionSignature::try_new` enforces
//!   in the compiler, reproduced here so violations surface at plugin
//!   build time with a span instead of at graphcal load time;
//! - `Bool`/`Int` are value kinds, not dimension factors;
//! - exponents are non-zero (writing `^0` would erase its term — the
//!   compiler's P016) and their arithmetic must not overflow.

use std::collections::HashSet;

use proc_macro2::{Span, TokenStream};

use crate::dims;
use crate::parse::{DimExprAst, DimTermAst, ExponentAst, MulOp, PluginFnDecl, PluginInput};
use crate::rational::Rational;

/// The validated `plugin!` block.
pub struct PluginIr {
    pub functions: Vec<FunctionIr>,
}

/// One validated function: signature plus its untouched Rust body.
pub struct FunctionIr {
    pub docs: Vec<syn::Attribute>,
    pub name: syn::Ident,
    pub dim_vars: Vec<syn::Ident>,
    pub params: Vec<ParamIr>,
    pub result: KindIr,
    pub body: TokenStream,
}

/// One validated parameter.
pub struct ParamIr {
    pub name: syn::Ident,
    pub kind: KindIr,
}

/// A validated value kind.
pub enum KindIr {
    Bool,
    Int,
    Scalar(MonomialIr),
}

/// A folded dimension monomial: dimension-variable powers in
/// first-occurrence order plus fixed exponents over the eight prelude base
/// dimensions (index-aligned with [`dims::BASE_DIMENSION_NAMES`]).
pub struct MonomialIr {
    pub vars: Vec<VarFactor>,
    pub fixed: [Rational; 8],
}

/// One dimension-variable factor of a monomial.
pub struct VarFactor {
    pub name: String,
    pub power: Rational,
    /// Span of the variable's first mention inside this monomial, for
    /// discipline diagnostics.
    pub span: Span,
}

impl MonomialIr {
    /// The bound-occurrence shape: exactly one variable to the first power
    /// and no fixed factors.
    pub fn as_bare_var(&self) -> Option<&str> {
        match self.vars.as_slice() {
            [factor]
                if factor.power == Rational::ONE
                    && self.fixed.iter().all(|power| power.is_zero()) =>
            {
                Some(&factor.name)
            }
            _ => None,
        }
    }
}

/// Validate the parsed block and produce the signature IR.
pub fn lower(input: &PluginInput) -> syn::Result<PluginIr> {
    let mut seen_functions: HashSet<String> = HashSet::new();
    let functions = input
        .functions
        .iter()
        .map(|decl| {
            if !seen_functions.insert(decl.name.to_string()) {
                return Err(syn::Error::new(
                    decl.name.span(),
                    format!("function `{}` is declared more than once", decl.name),
                ));
            }
            lower_function(decl)
        })
        .collect::<syn::Result<Vec<_>>>()?;
    Ok(PluginIr { functions })
}

fn lower_function(decl: &PluginFnDecl) -> syn::Result<FunctionIr> {
    let mut binders: HashSet<String> = HashSet::new();
    for var in &decl.dim_vars {
        let name = var.to_string();
        if is_vocabulary_name(&name) {
            return Err(syn::Error::new(
                var.span(),
                format!(
                    "dimension variable `{name}` shadows the prelude name `{name}`; \
                     rename the variable"
                ),
            ));
        }
        if !binders.insert(name) {
            return Err(syn::Error::new(
                var.span(),
                format!("dimension variable `{var}` is declared more than once"),
            ));
        }
    }

    let mut seen_params: HashSet<String> = HashSet::new();
    let mut bound: HashSet<String> = HashSet::new();
    let mut params = Vec::new();
    for param in &decl.params {
        if !seen_params.insert(param.name.to_string()) {
            return Err(syn::Error::new(
                param.name.span(),
                format!("parameter `{}` is declared more than once", param.name),
            ));
        }
        let kind = lower_type(&param.ty, &binders)?;
        if let KindIr::Scalar(monomial) = &kind {
            match monomial.as_bare_var() {
                Some(var) => {
                    bound.insert(var.to_string());
                }
                None => {
                    for factor in &monomial.vars {
                        if !bound.contains(&factor.name) {
                            return Err(syn::Error::new(
                                factor.span,
                                format!(
                                    "dimension variable `{}` is used in a compound type before \
                                     it is bound; bind it first with a parameter of type \
                                     exactly `{}`",
                                    factor.name, factor.name
                                ),
                            ));
                        }
                    }
                }
            }
        }
        params.push(ParamIr {
            name: param.name.clone(),
            kind,
        });
    }

    let result = lower_type(&decl.result, &binders)?;
    if let KindIr::Scalar(monomial) = &result {
        for factor in &monomial.vars {
            if !bound.contains(&factor.name) {
                return Err(syn::Error::new(
                    factor.span,
                    format!(
                        "the result uses dimension variable `{}`, which no parameter binds; \
                         add a parameter of type exactly `{}`",
                        factor.name, factor.name
                    ),
                ));
            }
        }
    }

    for var in &decl.dim_vars {
        if !bound.contains(&var.to_string()) {
            return Err(syn::Error::new(
                var.span(),
                format!(
                    "dimension variable `{var}` is never bound; every declared variable needs \
                     a parameter of type exactly `{var}`"
                ),
            ));
        }
    }

    Ok(FunctionIr {
        docs: decl.docs.clone(),
        name: decl.name.clone(),
        dim_vars: decl.dim_vars.clone(),
        params,
        result,
        body: decl.body.clone(),
    })
}

/// Lower one type position: a lone `Bool`/`Int`, or a dimension monomial.
fn lower_type(expr: &DimExprAst, binders: &HashSet<String>) -> syn::Result<KindIr> {
    if let (DimTermAst::Named { name, pow: None }, true) = (&expr.first, expr.rest.is_empty()) {
        match name.to_string().as_str() {
            "Bool" => return Ok(KindIr::Bool),
            "Int" => return Ok(KindIr::Int),
            _ => {}
        }
    }

    let mut acc = MonomialAcc::default();
    lower_expr(expr, binders, Rational::ONE, &mut acc)?;
    Ok(KindIr::Scalar(acc.into_monomial()))
}

/// Accumulator for monomial folding; zero-power variables are removed at
/// the end so `D * D^-1` cancels instead of emitting a zero factor.
#[derive(Default)]
struct MonomialAcc {
    vars: Vec<VarFactor>,
    fixed: [Rational; 8],
}

impl MonomialAcc {
    fn mul_var(&mut self, name: &syn::Ident, power: Rational) -> syn::Result<()> {
        let key = name.to_string();
        match self.vars.iter_mut().find(|factor| factor.name == key) {
            Some(factor) => {
                factor.power = factor
                    .power
                    .checked_add(power)
                    .ok_or_else(|| overflow_error(name.span()))?;
            }
            None => self.vars.push(VarFactor {
                name: key,
                power,
                span: name.span(),
            }),
        }
        Ok(())
    }

    fn mul_base(&mut self, index: usize, power: Rational, span: Span) -> syn::Result<()> {
        self.fixed[index] = self.fixed[index]
            .checked_add(power)
            .ok_or_else(|| overflow_error(span))?;
        Ok(())
    }

    fn into_monomial(self) -> MonomialIr {
        MonomialIr {
            vars: self
                .vars
                .into_iter()
                .filter(|factor| !factor.power.is_zero())
                .collect(),
            fixed: self.fixed,
        }
    }
}

fn lower_expr(
    expr: &DimExprAst,
    binders: &HashSet<String>,
    outer: Rational,
    acc: &mut MonomialAcc,
) -> syn::Result<()> {
    lower_term(&expr.first, binders, outer, acc)?;
    for (op, term) in &expr.rest {
        let signed = match op {
            MulOp::Mul => outer,
            MulOp::Div => outer
                .checked_neg()
                .ok_or_else(|| overflow_error(term_span(term)))?,
        };
        lower_term(term, binders, signed, acc)?;
    }
    Ok(())
}

fn lower_term(
    term: &DimTermAst,
    binders: &HashSet<String>,
    outer: Rational,
    acc: &mut MonomialAcc,
) -> syn::Result<()> {
    let power = match term {
        DimTermAst::Named { pow, .. } | DimTermAst::Group { pow, .. } => match pow {
            Some(exponent) => exponent_to_rational(exponent)?,
            None => Rational::ONE,
        },
    };
    let effective = outer
        .checked_mul(power)
        .ok_or_else(|| overflow_error(term_span(term)))?;

    match term {
        DimTermAst::Group { inner, .. } => lower_expr(inner, binders, effective, acc),
        DimTermAst::Named { name, .. } => {
            let key = name.to_string();
            if binders.contains(&key) {
                return acc.mul_var(name, effective);
            }
            if key == "Dimensionless" {
                return Ok(());
            }
            if let Some(index) = dims::base_dimension_index(&key) {
                return acc.mul_base(index, effective, name.span());
            }
            if let Some(factors) = dims::derived_dimension_factors(&key) {
                for (index, exponent) in factors {
                    let power = effective
                        .checked_mul(integer_rational(*exponent, name.span())?)
                        .ok_or_else(|| overflow_error(name.span()))?;
                    acc.mul_base(*index, power, name.span())?;
                }
                return Ok(());
            }
            Err(unknown_name_error(name, &key))
        }
    }
}

fn unknown_name_error(name: &syn::Ident, key: &str) -> syn::Error {
    let message = match key {
        "Bool" | "Int" => format!(
            "`{key}` is a value kind, not a dimension; it cannot appear inside a dimension \
             expression"
        ),
        "Datetime" => {
            "`Datetime` values cannot cross the plugin boundary (plugin ABI v1)".to_string()
        }
        _ => format!(
            "unknown dimension `{key}`; expected a dimension variable declared in `<...>`, a \
             prelude base dimension ({base}), a prelude derived dimension ({derived}), or \
             `Dimensionless`",
            base = dims::BASE_DIMENSION_NAMES.join(", "),
            derived = dims::DERIVED_DIMENSION_NAMES.join(", "),
        ),
    };
    syn::Error::new(name.span(), message)
}

fn exponent_to_rational(exponent: &ExponentAst) -> syn::Result<Rational> {
    if exponent.den == Some(0) {
        return Err(syn::Error::new(
            exponent.span,
            "exponent denominator cannot be zero",
        ));
    }
    let value = Rational::new(exponent.num, exponent.den.unwrap_or(1))
        .ok_or_else(|| overflow_error(exponent.span))?;
    if value.is_zero() {
        return Err(syn::Error::new(
            exponent.span,
            "`^0` would erase its term; drop the factor instead",
        ));
    }
    Ok(value)
}

fn integer_rational(value: i64, span: Span) -> syn::Result<Rational> {
    Rational::new(value, 1).ok_or_else(|| overflow_error(span))
}

fn overflow_error(span: Span) -> syn::Error {
    syn::Error::new(span, "dimension exponent arithmetic overflowed")
}

fn term_span(term: &DimTermAst) -> Span {
    match term {
        DimTermAst::Named { name, .. } => name.span(),
        DimTermAst::Group { pow: Some(exp), .. } => exp.span,
        DimTermAst::Group { pow: None, .. } => Span::call_site(),
    }
}

fn is_vocabulary_name(name: &str) -> bool {
    name == "Dimensionless"
        || name == "Bool"
        || name == "Int"
        || name == "Datetime"
        || dims::base_dimension_index(name).is_some()
        || dims::derived_dimension_factors(name).is_some()
}
