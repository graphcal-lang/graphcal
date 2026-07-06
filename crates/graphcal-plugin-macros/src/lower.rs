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
use crate::parse::{
    DimExprAst, DimTermAst, ExponentAst, MulOp, PluginFnDecl, PluginInput, ResultAst, TypeAst,
};
use crate::rational::Rational;

/// The validated `plugin!` block.
pub struct PluginIr {
    pub functions: Vec<FunctionIr>,
}

impl PluginIr {
    /// Whether any function moves arrays — the shape that needs the
    /// allocator exports and the buffer wrappers.
    pub fn uses_buffers(&self) -> bool {
        self.functions.iter().any(FunctionIr::uses_buffers)
    }
}

/// One validated function: signature plus its untouched Rust body.
pub struct FunctionIr {
    pub docs: Vec<syn::Attribute>,
    pub name: syn::Ident,
    pub dim_vars: Vec<syn::Ident>,
    pub index_vars: Vec<syn::Ident>,
    pub params: Vec<ParamIr>,
    pub result: KindIr,
    pub body: TokenStream,
}

impl FunctionIr {
    /// Whether this function moves values through plugin-memory buffers
    /// (array parameters/results or a struct result).
    pub fn uses_buffers(&self) -> bool {
        self.params
            .iter()
            .map(|param| &param.kind)
            .chain(std::iter::once(&self.result))
            .any(|kind| matches!(kind, KindIr::Array { .. } | KindIr::Struct(_)))
    }
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
    Array {
        element: MonomialIr,
        index: syn::Ident,
    },
    /// A struct-shaped result (never a parameter).
    Struct(Vec<FieldIr>),
}

/// One validated field of a struct-shaped result.
pub struct FieldIr {
    pub name: syn::Ident,
    pub kind: FieldKindIr,
}

/// A struct field's kind: concrete scalars only (no dimension variables).
pub enum FieldKindIr {
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

/// Validate the binder lists: no prelude shadowing, no duplicate idents
/// across the (shared) dim/index namespaces.
fn validate_binders(decl: &PluginFnDecl) -> syn::Result<(HashSet<String>, HashSet<String>)> {
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
    // Binder idents share one lexical namespace regardless of constraint,
    // so index binders are checked against the dim binders too.
    let mut index_binders: HashSet<String> = HashSet::new();
    for var in &decl.index_vars {
        let name = var.to_string();
        if is_vocabulary_name(&name) {
            return Err(syn::Error::new(
                var.span(),
                format!(
                    "index variable `{name}` shadows the prelude name `{name}`; \
                     rename the variable"
                ),
            ));
        }
        if binders.contains(&name) || !index_binders.insert(name) {
            return Err(syn::Error::new(
                var.span(),
                format!("generic binder `{var}` is declared more than once"),
            ));
        }
    }
    Ok((binders, index_binders))
}

/// Lowered parameters plus the binding facts later checks need.
struct LoweredParams {
    params: Vec<ParamIr>,
    /// Dimension variables bound by a bare parameter or bare array element.
    bound: HashSet<String>,
    /// Index variables used by at least one array parameter.
    used_indexes: HashSet<String>,
}

fn lower_params(
    decl: &PluginFnDecl,
    binders: &HashSet<String>,
    index_binders: &HashSet<String>,
) -> syn::Result<LoweredParams> {
    let mut seen_params: HashSet<String> = HashSet::new();
    let mut bound: HashSet<String> = HashSet::new();
    let mut used_indexes: HashSet<String> = HashSet::new();
    let mut params = Vec::new();
    for param in &decl.params {
        if !seen_params.insert(param.name.to_string()) {
            return Err(syn::Error::new(
                param.name.span(),
                format!("parameter `{}` is declared more than once", param.name),
            ));
        }
        let kind = lower_type(&param.ty, binders, index_binders)?;
        let monomial = match &kind {
            KindIr::Scalar(monomial) => Some(monomial),
            KindIr::Array { element, index } => {
                used_indexes.insert(index.to_string());
                Some(element)
            }
            // The parser only accepts struct shapes in result position.
            KindIr::Bool | KindIr::Int | KindIr::Struct(_) => None,
        };
        if let Some(monomial) = monomial {
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
                                     exactly `{}` (or an array of it)",
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
    Ok(LoweredParams {
        params,
        bound,
        used_indexes,
    })
}

fn lower_function(decl: &PluginFnDecl) -> syn::Result<FunctionIr> {
    let (binders, index_binders) = validate_binders(decl)?;
    let LoweredParams {
        params,
        bound,
        used_indexes,
    } = lower_params(decl, &binders, &index_binders)?;

    let result = lower_result(&decl.result, &binders, &index_binders)?;
    let result_monomial = match &result {
        KindIr::Scalar(monomial) => Some(monomial),
        KindIr::Array { element, index } => {
            if !used_indexes.contains(&index.to_string()) {
                return Err(syn::Error::new(
                    index.span(),
                    format!(
                        "the result array is indexed by `{index}`, which no array parameter \
                         uses; a plugin cannot invent its output length"
                    ),
                ));
            }
            Some(element)
        }
        KindIr::Bool | KindIr::Int | KindIr::Struct(_) => None,
    };
    if let Some(monomial) = result_monomial {
        for factor in &monomial.vars {
            if !bound.contains(&factor.name) {
                return Err(syn::Error::new(
                    factor.span,
                    format!(
                        "the result uses dimension variable `{}`, which no parameter binds; \
                         add a parameter of type exactly `{}` (or an array of it)",
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
                     a parameter of type exactly `{var}` (or an array of it)"
                ),
            ));
        }
    }
    for var in &decl.index_vars {
        if !used_indexes.contains(&var.to_string()) {
            return Err(syn::Error::new(
                var.span(),
                format!("index variable `{var}` is declared but indexes no array parameter"),
            ));
        }
    }

    Ok(FunctionIr {
        docs: decl.docs.clone(),
        name: decl.name.clone(),
        dim_vars: decl.dim_vars.clone(),
        index_vars: decl.index_vars.clone(),
        params,
        result,
        body: decl.body.clone(),
    })
}

/// Lower the result position: a value type, or a braced struct shape.
fn lower_result(
    result: &ResultAst,
    binders: &HashSet<String>,
    index_binders: &HashSet<String>,
) -> syn::Result<KindIr> {
    match result {
        ResultAst::Value(ty) => lower_type(ty, binders, index_binders),
        ResultAst::Struct(fields) => {
            if fields.is_empty() {
                return Err(syn::Error::new(
                    Span::call_site(),
                    "a struct result must declare at least one field",
                ));
            }
            let mut seen: HashSet<String> = HashSet::new();
            let lowered = fields
                .iter()
                .map(|field| {
                    if !seen.insert(field.name.to_string()) {
                        return Err(syn::Error::new(
                            field.name.span(),
                            format!("struct field `{}` is declared more than once", field.name),
                        ));
                    }
                    // Fields lower with no binders in scope: struct-result
                    // fields must have concrete dimensions, so a dimension
                    // variable here reads as an unknown dimension.
                    let empty = HashSet::new();
                    let kind = match lower_type(
                        &TypeAst {
                            element: clone_dim_expr(&field.ty),
                            index: None,
                        },
                        &empty,
                        &empty,
                    )? {
                        KindIr::Bool => FieldKindIr::Bool,
                        KindIr::Int => FieldKindIr::Int,
                        KindIr::Scalar(monomial) => FieldKindIr::Scalar(monomial),
                        KindIr::Array { .. } | KindIr::Struct(_) => {
                            return Err(syn::Error::new(
                                field.name.span(),
                                "struct fields must be Bool, Int, or scalar dimension types",
                            ));
                        }
                    };
                    Ok(FieldIr {
                        name: field.name.clone(),
                        kind,
                    })
                })
                .collect::<syn::Result<Vec<_>>>()?;
            Ok(KindIr::Struct(lowered))
        }
    }
}

/// Clone a parsed dimension expression (the parse AST is not `Clone`; the
/// struct-field path re-lowers through the shared `TypeAst` shape).
fn clone_dim_expr(expr: &DimExprAst) -> DimExprAst {
    DimExprAst {
        first: clone_dim_term(&expr.first),
        rest: expr
            .rest
            .iter()
            .map(|(op, term)| (*op, clone_dim_term(term)))
            .collect(),
    }
}

fn clone_dim_term(term: &DimTermAst) -> DimTermAst {
    match term {
        DimTermAst::Named { name, pow } => DimTermAst::Named {
            name: name.clone(),
            pow: pow.as_ref().map(clone_exponent),
        },
        DimTermAst::Group { inner, pow } => DimTermAst::Group {
            inner: Box::new(clone_dim_expr(inner)),
            pow: pow.as_ref().map(clone_exponent),
        },
    }
}

const fn clone_exponent(exponent: &ExponentAst) -> ExponentAst {
    ExponentAst {
        num: exponent.num,
        den: exponent.den,
        span: exponent.span,
    }
}

/// Lower one type position: a lone `Bool`/`Int`, a dimension monomial, or
/// an array of scalars over a declared index variable.
fn lower_type(
    ty: &TypeAst,
    binders: &HashSet<String>,
    index_binders: &HashSet<String>,
) -> syn::Result<KindIr> {
    let expr = &ty.element;
    if let (DimTermAst::Named { name, pow: None }, true) = (&expr.first, expr.rest.is_empty()) {
        match name.to_string().as_str() {
            "Bool" | "Int" if ty.index.is_some() => {
                return Err(syn::Error::new(
                    name.span(),
                    "array elements must be scalars in this phase",
                ));
            }
            "Bool" => return Ok(KindIr::Bool),
            "Int" => return Ok(KindIr::Int),
            _ => {}
        }
    }

    let mut acc = MonomialAcc::default();
    lower_expr(expr, binders, Rational::ONE, &mut acc)?;
    let element = acc.into_monomial();
    match &ty.index {
        Some(index) => {
            if !index_binders.contains(&index.to_string()) {
                return Err(syn::Error::new(
                    index.span(),
                    format!(
                        "unknown index variable `{index}`; declare it in the binder list as \
                         `{index}: Index`"
                    ),
                ));
            }
            Ok(KindIr::Array {
                element,
                index: index.clone(),
            })
        }
        None => Ok(KindIr::Scalar(element)),
    }
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
