//! Instantiation of generic parameters in TIR type forms.
//!
//! A [`Substitution`] binds owner-qualified [`GenericParamId`]s, one map per
//! generic sort, and [`Substitution::apply`] is the single fold that replaces
//! them in a symbolic [`ResolvedTypeExpr`]. Every generic instantiation uses
//! it: nominal defaults that name earlier parameters, concrete field types of
//! a nominal application, and the Nat parameters of a generic bound. A
//! binding may itself be symbolic, and an unbound parameter stays symbolic;
//! concreteness is checked by the consumer that needs a concrete type.
//!
//! Replacing a template's static ports (include and DAG-call bindings) is a
//! different operation keyed by declaration identities; it lives in
//! [`super::specialization`].

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::desugared_ast::MulDivOp;
use crate::dimension::{Dimension, Rational};
use crate::generic_param::GenericParamId;
use crate::hir::nominal::NominalGenericParam;
use crate::nat::{NatOverflowError, NatPolyForm};
use crate::registry::declared_type::{DeclaredGenericArg, DeclaredType, IndexTypeRef};
use crate::registry::error::GraphcalError;
use crate::syntax::span::Span;

use super::{ResolvedDimArg, ResolvedDimTerm, ResolvedGenericArg, ResolvedIndex, ResolvedTypeExpr};

/// Bindings of generic parameters, one map per generic sort.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Substitution {
    dims: HashMap<GenericParamId, ResolvedDimArg>,
    indexes: HashMap<GenericParamId, ResolvedIndex>,
    nats: HashMap<GenericParamId, NatPolyForm>,
    types: HashMap<GenericParamId, ResolvedTypeExpr>,
}

/// Arithmetic failure while folding a substituted type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubstitutionError {
    /// A substituted Nat form overflowed.
    NatOverflow { span: Span },
    /// A substituted dimension exponent overflowed.
    DimensionOverflow { span: Span },
    /// A substituted `Fin(...)` cardinality is not a valid finite index.
    InvalidFiniteIndex {
        error: crate::registry::types::IndexCardinalityError,
        span: Span,
    },
    /// A declared type still names a Nat parameter the substitution does not
    /// bind.
    UnboundNat { param: GenericParamId, span: Span },
}

impl SubstitutionError {
    /// Render the failure at its source span.
    #[must_use]
    pub fn into_graphcal(self, src: &NamedSource<Arc<String>>) -> GraphcalError {
        match self {
            Self::NatOverflow { span } => GraphcalError::EvalError {
                message: NatOverflowError.to_string(),
                src: src.clone(),
                span: span.into(),
            },
            Self::DimensionOverflow { span } => GraphcalError::DimensionOverflow {
                src: src.clone(),
                span: span.into(),
            },
            Self::InvalidFiniteIndex { error, span } => GraphcalError::EvalError {
                message: error.describe_finite_index(),
                src: src.clone(),
                span: span.into(),
            },
            Self::UnboundNat { param, span } => GraphcalError::internal_error(
                format!("required Nat binding is missing: {param:?}"),
                src,
                crate::diagnostic_anchor::DiagnosticAnchor::Source(span),
            ),
        }
    }
}

impl Substitution {
    /// Bind the leading `params` to `args`, pairwise. Each argument's
    /// variant is its sort; sort agreement with the parameter is checked by
    /// the producer of `args`.
    #[must_use]
    pub fn for_params<'a>(
        params: &[NominalGenericParam],
        args: impl IntoIterator<Item = &'a ResolvedGenericArg>,
    ) -> Self {
        let mut substitution = Self::default();
        for (param, arg) in params.iter().zip(args) {
            substitution.bind(param.id().clone(), arg.clone());
        }
        substitution
    }

    /// Bind Nat parameters to concrete values.
    #[must_use]
    pub fn for_nats<'a>(nats: impl IntoIterator<Item = (&'a GenericParamId, &'a u64)>) -> Self {
        Self {
            nats: nats
                .into_iter()
                .map(|(param, value)| (param.clone(), NatPolyForm::from_constant(*value)))
                .collect(),
            ..Self::default()
        }
    }

    /// Bind one parameter; the argument's variant selects the sort.
    pub fn bind(&mut self, param: GenericParamId, arg: ResolvedGenericArg) {
        match arg {
            ResolvedGenericArg::Dim(dim) => {
                self.dims.insert(param, dim);
            }
            ResolvedGenericArg::Index(index) => {
                self.indexes.insert(param, index);
            }
            ResolvedGenericArg::Nat(form, _) => {
                self.nats.insert(param, form);
            }
            ResolvedGenericArg::Type(type_expr) => {
                self.types.insert(param, type_expr);
            }
        }
    }

    /// Replace every bound parameter in `type_expr`.
    ///
    /// # Errors
    ///
    /// Returns a [`SubstitutionError`] when folding the substituted
    /// dimensions or Nat forms overflows.
    pub fn apply(
        &self,
        type_expr: &ResolvedTypeExpr,
    ) -> Result<ResolvedTypeExpr, SubstitutionError> {
        Ok(match type_expr {
            ResolvedTypeExpr::Key { index, span } => ResolvedTypeExpr::Key {
                index: self.apply_index(index)?,
                span: *span,
            },
            ResolvedTypeExpr::GenericDimParam(param, _) => self.dims.get(param).map_or_else(
                || type_expr.clone(),
                |replacement| dim_arg_as_type(replacement.clone()),
            ),
            ResolvedTypeExpr::GenericTypeParam(param, _) => self
                .types
                .get(param)
                .cloned()
                .unwrap_or_else(|| type_expr.clone()),
            ResolvedTypeExpr::GenericDimExpr { terms, span } => {
                dim_arg_as_type(self.apply_dim_terms(terms, *span)?)
            }
            ResolvedTypeExpr::Complex { dimension, span } => ResolvedTypeExpr::Complex {
                dimension: self.apply_dim_arg(dimension)?,
                span: *span,
            },
            ResolvedTypeExpr::GenericStruct {
                name,
                generic_args,
                span,
            } => ResolvedTypeExpr::GenericStruct {
                name: name.clone(),
                generic_args: generic_args
                    .iter()
                    .map(|arg| self.apply_generic_arg(arg))
                    .collect::<Result<_, _>>()?,
                span: *span,
            },
            ResolvedTypeExpr::Indexed { base, indexes } => ResolvedTypeExpr::Indexed {
                base: Box::new(self.apply(base)?),
                indexes: indexes
                    .iter()
                    .map(|index| self.apply_index(index))
                    .collect::<Result<_, _>>()?,
            },
            ResolvedTypeExpr::Dimensionless
            | ResolvedTypeExpr::Bool
            | ResolvedTypeExpr::Int
            | ResolvedTypeExpr::Datetime(_)
            | ResolvedTypeExpr::Quantity(_)
            | ResolvedTypeExpr::Struct(_, _) => type_expr.clone(),
        })
    }

    /// Replace every bound parameter in one generic argument.
    ///
    /// # Errors
    ///
    /// Returns a [`SubstitutionError`] on arithmetic overflow.
    pub fn apply_generic_arg(
        &self,
        arg: &ResolvedGenericArg,
    ) -> Result<ResolvedGenericArg, SubstitutionError> {
        Ok(match arg {
            ResolvedGenericArg::Dim(dim) => ResolvedGenericArg::Dim(self.apply_dim_arg(dim)?),
            ResolvedGenericArg::Index(index) => ResolvedGenericArg::Index(self.apply_index(index)?),
            ResolvedGenericArg::Nat(form, span) => {
                ResolvedGenericArg::Nat(self.apply_nat(form, *span)?, *span)
            }
            ResolvedGenericArg::Type(type_expr) => ResolvedGenericArg::Type(self.apply(type_expr)?),
        })
    }

    /// Close a declared type, whose only symbolic parts are Nat forms
    /// (`Fin(N)` axes and Nat arguments), under this substitution's Nat
    /// bindings.
    ///
    /// # Errors
    ///
    /// Returns a [`SubstitutionError`] on overflow, when a Nat parameter is
    /// unbound, or when a substituted `Fin(...)` cardinality is not a valid
    /// finite index.
    pub fn apply_declared(
        &self,
        declared: &DeclaredType,
        span: Span,
    ) -> Result<DeclaredType, SubstitutionError> {
        Ok(match declared {
            DeclaredType::Key(index) => DeclaredType::Key(self.apply_index_ref(index, span)?),
            DeclaredType::Indexed { element, index } => DeclaredType::Indexed {
                element: Box::new(self.apply_declared(element, span)?),
                index: self.apply_index_ref(index, span)?,
            },
            DeclaredType::Struct(identity, args) => DeclaredType::Struct(
                identity.clone(),
                args.iter()
                    .map(|arg| {
                        Ok(match arg {
                            DeclaredGenericArg::Nat(form) => {
                                DeclaredGenericArg::Nat(self.close_nat(form, span)?)
                            }
                            DeclaredGenericArg::Type(ty) => {
                                DeclaredGenericArg::Type(self.apply_declared(ty, span)?)
                            }
                            DeclaredGenericArg::Index(index) => {
                                DeclaredGenericArg::Index(self.apply_index_ref(index, span)?)
                            }
                            DeclaredGenericArg::Dim(_) => arg.clone(),
                        })
                    })
                    .collect::<Result<_, SubstitutionError>>()?,
            ),
            DeclaredType::Quantity(_)
            | DeclaredType::Complex(_)
            | DeclaredType::Bool
            | DeclaredType::Int
            | DeclaredType::Datetime(_) => declared.clone(),
        })
    }

    fn apply_nat(&self, form: &NatPolyForm, span: Span) -> Result<NatPolyForm, SubstitutionError> {
        form.substitute_forms(&self.nats)
            .map_err(|NatOverflowError| SubstitutionError::NatOverflow { span })
    }

    /// Substitute a Nat form that must become a constant.
    fn close_nat(&self, form: &NatPolyForm, span: Span) -> Result<NatPolyForm, SubstitutionError> {
        let closed = self.apply_nat(form, span)?;
        closed
            .variables()
            .into_iter()
            .next()
            .map_or(Ok(closed), |param| {
                Err(SubstitutionError::UnboundNat { param, span })
            })
    }

    fn apply_index(&self, index: &ResolvedIndex) -> Result<ResolvedIndex, SubstitutionError> {
        Ok(match index {
            ResolvedIndex::GenericParam(param, _) => self
                .indexes
                .get(param)
                .cloned()
                .unwrap_or_else(|| index.clone()),
            ResolvedIndex::Finite(form, span) => {
                ResolvedIndex::Finite(self.apply_nat(form, *span)?, *span)
            }
            ResolvedIndex::Concrete(_, _) => index.clone(),
        })
    }

    /// Close a declared index under this substitution's Nat bindings.
    ///
    /// # Errors
    ///
    /// Returns a [`SubstitutionError`] as [`Self::apply_declared`] does.
    pub fn apply_index_ref(
        &self,
        index: &IndexTypeRef,
        span: Span,
    ) -> Result<IndexTypeRef, SubstitutionError> {
        match index.finite_index_form() {
            Some(form) if !form.is_constant() => {
                IndexTypeRef::from_finite_index_form(self.close_nat(&form, span)?)
                    .map_err(|error| SubstitutionError::InvalidFiniteIndex { error, span })
            }
            Some(_) | None => Ok(index.clone()),
        }
    }

    fn apply_dim_arg(&self, arg: &ResolvedDimArg) -> Result<ResolvedDimArg, SubstitutionError> {
        match arg {
            ResolvedDimArg::GenericParam(param, _) => {
                Ok(self.dims.get(param).cloned().unwrap_or_else(|| arg.clone()))
            }
            ResolvedDimArg::Expr { terms, span } => self.apply_dim_terms(terms, *span),
            ResolvedDimArg::Dimensionless | ResolvedDimArg::Concrete(_) => Ok(arg.clone()),
        }
    }

    /// Substitute a dimension product and collapse it to a concrete
    /// dimension once no generic term remains.
    fn apply_dim_terms(
        &self,
        terms: &[ResolvedDimTerm],
        span: Span,
    ) -> Result<ResolvedDimArg, SubstitutionError> {
        let overflow = |_| SubstitutionError::DimensionOverflow { span };
        let mut substituted = Vec::with_capacity(terms.len());
        for term in terms {
            match term {
                ResolvedDimTerm::GenericParam {
                    name, power, op, ..
                } if self.dims.contains_key(name) => {
                    let replacement = &self.dims[name];
                    substituted.extend(expand_dim_arg(replacement, *power, *op).map_err(overflow)?);
                }
                ResolvedDimTerm::GenericParam { .. } | ResolvedDimTerm::Concrete { .. } => {
                    substituted.push(term.clone());
                }
            }
        }
        collapse_dim_terms(substituted, span).map_err(overflow)
    }
}

/// Embed one dimension argument, with its outer power and operator, into a
/// dimension product.
fn expand_dim_arg(
    arg: &ResolvedDimArg,
    outer_power: Rational,
    outer_op: MulDivOp,
) -> Result<Vec<ResolvedDimTerm>, crate::ratio::RatioError> {
    match arg {
        ResolvedDimArg::Dimensionless => Ok(Vec::new()),
        ResolvedDimArg::Concrete(dim) => Ok(vec![ResolvedDimTerm::Concrete {
            dim: dim.clone(),
            power: outer_power,
            op: outer_op,
        }]),
        ResolvedDimArg::GenericParam(name, span) => Ok(vec![ResolvedDimTerm::GenericParam {
            name: name.clone(),
            power: outer_power,
            op: outer_op,
            span: *span,
        }]),
        ResolvedDimArg::Expr { terms, .. } => terms
            .iter()
            .map(|term| match term {
                ResolvedDimTerm::Concrete { dim, power, op } => Ok(ResolvedDimTerm::Concrete {
                    dim: dim.clone(),
                    power: (*power * outer_power)?,
                    op: combine_dim_ops(outer_op, *op),
                }),
                ResolvedDimTerm::GenericParam {
                    name,
                    power,
                    op,
                    span,
                } => Ok(ResolvedDimTerm::GenericParam {
                    name: name.clone(),
                    power: (*power * outer_power)?,
                    op: combine_dim_ops(outer_op, *op),
                    span: *span,
                }),
            })
            .collect(),
    }
}

const fn combine_dim_ops(outer: MulDivOp, inner: MulDivOp) -> MulDivOp {
    if matches!(
        (outer, inner),
        (MulDivOp::Mul, MulDivOp::Mul) | (MulDivOp::Div, MulDivOp::Div)
    ) {
        MulDivOp::Mul
    } else {
        MulDivOp::Div
    }
}

/// Fold a product of only concrete terms to one dimension; keep a product
/// with a generic term symbolic.
fn collapse_dim_terms(
    terms: Vec<ResolvedDimTerm>,
    span: Span,
) -> Result<ResolvedDimArg, crate::ratio::RatioError> {
    let concrete = terms
        .iter()
        .map(|term| match term {
            ResolvedDimTerm::Concrete { dim, power, op } => Some((dim, *power, *op)),
            ResolvedDimTerm::GenericParam { .. } => None,
        })
        .collect::<Option<Vec<_>>>();
    let Some(concrete) = concrete else {
        return Ok(ResolvedDimArg::Expr { terms, span });
    };
    let dimension = concrete.into_iter().try_fold(
        Dimension::dimensionless(),
        |dimension, (dim, power, op)| {
            let powered = dim.pow(power)?;
            match op {
                MulDivOp::Mul => dimension.checked_mul(&powered),
                MulDivOp::Div => dimension.checked_div(&powered),
            }
        },
    )?;
    Ok(if dimension.is_dimensionless() {
        ResolvedDimArg::Dimensionless
    } else {
        ResolvedDimArg::Concrete(dimension)
    })
}

fn dim_arg_as_type(arg: ResolvedDimArg) -> ResolvedTypeExpr {
    match arg {
        ResolvedDimArg::Dimensionless => ResolvedTypeExpr::Dimensionless,
        ResolvedDimArg::Concrete(dim) => ResolvedTypeExpr::Quantity(dim),
        ResolvedDimArg::GenericParam(name, span) => ResolvedTypeExpr::GenericDimParam(name, span),
        ResolvedDimArg::Expr { terms, span } => ResolvedTypeExpr::GenericDimExpr { terms, span },
    }
}

#[cfg(test)]
mod tests;
