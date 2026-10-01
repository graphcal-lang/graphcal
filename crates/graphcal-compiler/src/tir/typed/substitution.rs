//! Instantiation of generic parameters in TIR type forms.
//!
//! A [`Substitution`] binds owner-qualified [`GenericParamId`]s, one map per
//! generic sort, and [`Substitution::apply`] is the single fold that replaces
//! them in a symbolic [`ResolvedDeclType`]. Every generic instantiation uses
//! it: nominal defaults that name earlier parameters, concrete field types of
//! a nominal application, and the Nat parameters of a generic bound. A
//! binding may itself be symbolic, and an unbound parameter stays symbolic;
//! concreteness is checked by the consumer that needs a concrete type.
//!
//! Replacing a template's static ports (include and DAG-call bindings) is a
//! different operation keyed by declaration identities; it lives in
//! [`super::specialization`].

use std::collections::HashMap;

use crate::desugar::desugared_ast::MulDivOp;
use crate::dimension::{Dimension, Rational};
use crate::generic_param::GenericParamId;
use crate::graphcal_error::GraphcalError;
use crate::hir::nominal::NominalGenericParam;
use crate::nat::{NatOverflowError, NatPolyForm};
use crate::semantic::checked_type::{CheckedType, IndexTypeRef, InstantiationError, Symbolic};
use crate::semantic_error::dimension::DimensionError;
use crate::source_id::SourceId;
use crate::syntax::span::Span;

use super::{
    ResolvedDeclType, ResolvedDim, ResolvedDimTerm, ResolvedGenericArg, ResolvedIndex,
    ResolvedValueType,
};

/// Bindings of generic parameters, one map per generic sort.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Substitution {
    dims: HashMap<GenericParamId, ResolvedDim>,
    indexes: HashMap<GenericParamId, ResolvedIndex>,
    nats: HashMap<GenericParamId, NatPolyForm>,
    types: HashMap<GenericParamId, ResolvedValueType>,
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
        error: crate::semantic::index_def::IndexCardinalityError,
        span: Span,
    },
    /// A declared type still names a Nat parameter the substitution does not
    /// bind.
    UnboundNat { param: GenericParamId, span: Span },
}

impl SubstitutionError {
    /// Render the failure at its source span.
    #[must_use]
    pub fn into_graphcal(self, src: SourceId) -> GraphcalError {
        match self {
            Self::NatOverflow { span } => GraphcalError::EvalError {
                message: NatOverflowError.to_string(),
                src,
                span: span.into(),
            },
            Self::DimensionOverflow { span } => {
                GraphcalError::located(src, span, DimensionError::DimensionOverflow)
            }
            Self::InvalidFiniteIndex { error, span } => GraphcalError::EvalError {
                message: error.describe_finite_index(),
                src,
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

    /// Replace every bound parameter in a declaration or field type.
    ///
    /// # Errors
    ///
    /// Returns a [`SubstitutionError`] when folding the substituted
    /// dimensions or Nat forms overflows.
    pub fn apply(
        &self,
        decl_type: &ResolvedDeclType,
    ) -> Result<ResolvedDeclType, SubstitutionError> {
        Ok(match decl_type {
            ResolvedDeclType::Value(value_type) => {
                ResolvedDeclType::Value(self.apply_value(value_type)?)
            }
            ResolvedDeclType::Indexed { element, indexes } => ResolvedDeclType::Indexed {
                element: self.apply_value(element)?,
                indexes: indexes.try_map_ref(|index| self.apply_index(index))?,
            },
        })
    }

    /// Replace every bound parameter in a value type.
    ///
    /// # Errors
    ///
    /// Returns a [`SubstitutionError`] on arithmetic overflow.
    pub fn apply_value(
        &self,
        value_type: &ResolvedValueType,
    ) -> Result<ResolvedValueType, SubstitutionError> {
        Ok(match value_type {
            ResolvedValueType::Key { index, span } => ResolvedValueType::Key {
                index: self.apply_index(index)?,
                span: *span,
            },
            ResolvedValueType::GenericTypeParam(param, _) => self
                .types
                .get(param)
                .cloned()
                .unwrap_or_else(|| value_type.clone()),
            ResolvedValueType::Quantity(dimension) => {
                ResolvedValueType::Quantity(self.apply_dim_arg(dimension)?)
            }
            ResolvedValueType::Complex { dimension, span } => ResolvedValueType::Complex {
                dimension: self.apply_dim_arg(dimension)?,
                span: *span,
            },
            ResolvedValueType::Struct {
                name,
                generic_args,
                span,
            } => ResolvedValueType::Struct {
                name: name.clone(),
                generic_args: generic_args
                    .iter()
                    .map(|arg| self.apply_generic_arg(arg))
                    .collect::<Result<_, _>>()?,
                span: *span,
            },
            ResolvedValueType::Bool | ResolvedValueType::Int | ResolvedValueType::Datetime(_) => {
                value_type.clone()
            }
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
            ResolvedGenericArg::Type(value_type) => {
                ResolvedGenericArg::Type(self.apply_value(value_type)?)
            }
        })
    }

    /// Instantiate a symbolic checked type, whose only symbolic parts are
    /// Nat forms (`Fin(N)` axes and Nat arguments), under this
    /// substitution's Nat bindings.
    ///
    /// # Errors
    ///
    /// Returns a [`SubstitutionError`] on overflow, when a Nat parameter is
    /// unbound, or when a substituted `Fin(...)` cardinality is not a valid
    /// finite index.
    pub fn instantiate(
        &self,
        ty: &CheckedType<Symbolic>,
        span: Span,
    ) -> Result<CheckedType, SubstitutionError> {
        ty.instantiate(|form| self.close_nat(form, span))
            .map_err(|error| instantiation_error(error, span))
    }

    /// Instantiate a symbolic index under this substitution's Nat bindings.
    ///
    /// # Errors
    ///
    /// Returns a [`SubstitutionError`] as [`Self::instantiate`] does.
    pub fn instantiate_index(
        &self,
        index: &IndexTypeRef<Symbolic>,
        span: Span,
    ) -> Result<IndexTypeRef, SubstitutionError> {
        index
            .instantiate(|form| self.close_nat(form, span))
            .map_err(|error| instantiation_error(error, span))
    }

    fn apply_nat(&self, form: &NatPolyForm, span: Span) -> Result<NatPolyForm, SubstitutionError> {
        form.substitute_forms(&self.nats)
            .map_err(|NatOverflowError| SubstitutionError::NatOverflow { span })
    }

    /// Substitute a Nat form that must become a constant.
    fn close_nat(&self, form: &NatPolyForm, span: Span) -> Result<u64, SubstitutionError> {
        let closed = self.apply_nat(form, span)?;
        closed.variables().into_iter().next().map_or_else(
            || Ok(closed.constant()),
            |param| Err(SubstitutionError::UnboundNat { param, span }),
        )
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

    fn apply_dim_arg(&self, arg: &ResolvedDim) -> Result<ResolvedDim, SubstitutionError> {
        // A lone parameter is replaced verbatim, keeping the replacement's own
        // spelling and span.
        if let Some((param, _)) = arg.lone_generic_param() {
            return Ok(self.dims.get(param).cloned().unwrap_or_else(|| arg.clone()));
        }
        match arg {
            ResolvedDim::Symbolic { terms, span } => self.apply_dim_terms(terms, *span),
            ResolvedDim::Concrete(_) => Ok(arg.clone()),
        }
    }

    /// Substitute a dimension product and collapse it to a concrete
    /// dimension once no generic term remains.
    fn apply_dim_terms(
        &self,
        terms: &[ResolvedDimTerm],
        span: Span,
    ) -> Result<ResolvedDim, SubstitutionError> {
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

/// Report a failed instantiation at `span`.
fn instantiation_error(
    error: InstantiationError<SubstitutionError>,
    span: Span,
) -> SubstitutionError {
    match error {
        InstantiationError::Binding(error) => error,
        InstantiationError::Cardinality(error) => {
            SubstitutionError::InvalidFiniteIndex { error, span }
        }
    }
}

/// Embed one dimension argument, with its outer power and operator, into a
/// dimension product.
fn expand_dim_arg(
    arg: &ResolvedDim,
    outer_power: Rational,
    outer_op: MulDivOp,
) -> Result<Vec<ResolvedDimTerm>, crate::ratio::RatioError> {
    match arg {
        ResolvedDim::Concrete(dim) if dim.is_dimensionless() => Ok(Vec::new()),
        ResolvedDim::Concrete(dim) => Ok(vec![ResolvedDimTerm::Concrete {
            dim: dim.clone(),
            power: outer_power,
            op: outer_op,
        }]),
        ResolvedDim::Symbolic { terms, .. } => terms
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
) -> Result<ResolvedDim, crate::ratio::RatioError> {
    let concrete = terms
        .iter()
        .map(|term| match term {
            ResolvedDimTerm::Concrete { dim, power, op } => Some((dim, *power, *op)),
            ResolvedDimTerm::GenericParam { .. } => None,
        })
        .collect::<Option<Vec<_>>>();
    let Some(concrete) = concrete else {
        return Ok(ResolvedDim::Symbolic { terms, span });
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
    Ok(ResolvedDim::Concrete(dimension))
}

#[cfg(test)]
mod tests;
