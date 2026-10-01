//! Canonical resolved type annotations.
//!
//! TIR's semantic counterpart of HIR's [`DeclType`](crate::hir::types::DeclType) /
//! [`ValueType`](crate::hir::types::ValueType) split: every named dimension has been
//! folded to a [`Dimension`], while generic parameters stay symbolic. Each
//! type has exactly one spelling — `Dimensionless` is the concrete
//! dimensionless quantity, a non-generic struct is a struct application with
//! no arguments, and a lone generic dimension parameter is a one-term
//! symbolic dimension. Indexing is a declaration-level shape, never a value
//! type, so an indexed type cannot nest or appear as a `Type` argument.

use crate::desugar::desugared_ast::MulDivOp;
use crate::dimension::{Dimension, Rational};
use crate::display::formatting_registry::FormattingRegistry;
use crate::generic_param::GenericParamId;
use crate::nat::NatPolyForm;
use crate::resolved_name::{ResolvedIndexName, ResolvedStructTypeName};
use crate::semantic::checked_type::{
    CheckedGenericArg, CheckedType, IndexDisplayName, IndexTypeRef, StructTypeRef,
};
use crate::semantic::index_def::FiniteIndex;
use crate::semantic::time_scale::TimeScale;
use crate::semantic_error::SemanticError;
use crate::semantic_error::evaluation::EvaluationError;
use crate::source_id::SourceId;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::Span;

/// A resolved dimension: fully concrete, or symbolic in at least one generic
/// dimension parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedDim {
    /// A concrete dimension, including `Dimensionless`.
    Concrete(Dimension),
    /// A dimension expression mentioning at least one generic parameter,
    /// e.g. `D`, `D^2`, or `Length / D`.
    Symbolic {
        terms: Vec<ResolvedDimTerm>,
        span: Span,
    },
}

impl ResolvedDim {
    /// The concrete dimensionless dimension.
    #[must_use]
    pub const fn dimensionless() -> Self {
        Self::Concrete(Dimension::dimensionless())
    }

    /// The parameter and its span when this is exactly one generic parameter
    /// (`D`, not `D^2` or `Length * D`).
    #[must_use]
    pub(crate) fn lone_generic_param(&self) -> Option<(&GenericParamId, Span)> {
        match self {
            Self::Symbolic { terms, .. } => match terms.as_slice() {
                [
                    ResolvedDimTerm::GenericParam {
                        name,
                        power,
                        op: MulDivOp::Mul,
                        span,
                    },
                ] if *power == Rational::ONE => Some((name, *span)),
                _ => None,
            },
            Self::Concrete(_) => None,
        }
    }

    /// Format as a source-like dimension, e.g. `"Length / Time^2"`, `"D^2"`.
    #[must_use]
    pub(crate) fn format(&self, registry: &FormattingRegistry) -> String {
        match self {
            Self::Concrete(dim) => {
                let formatted = registry.dimensions.format_dimension(dim);
                if formatted.is_empty() {
                    "Dimensionless".to_string()
                } else {
                    formatted
                }
            }
            Self::Symbolic { terms, .. } => terms
                .iter()
                .map(|term| term.format(registry))
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

/// A single term in a resolved dimension expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedDimTerm {
    /// A concrete dimension with power and combining operator.
    Concrete {
        dim: Dimension,
        power: Rational,
        op: MulDivOp,
    },
    /// A generic dimension parameter with power and combining operator.
    GenericParam {
        name: GenericParamId,
        power: Rational,
        op: MulDivOp,
        span: Span,
    },
}

impl ResolvedDimTerm {
    /// Format this term as a human-readable string, e.g. `"Length"`, `"/ Time^2"`, `"D^2"`.
    #[must_use]
    fn format(&self, registry: &FormattingRegistry) -> String {
        let (name, power, op) = match self {
            Self::Concrete { dim, power, op } => {
                (registry.dimensions.format_dimension(dim), *power, *op)
            }
            Self::GenericParam {
                name, power, op, ..
            } => (name.to_string(), *power, *op),
        };
        let prefix = match op {
            MulDivOp::Mul => "",
            MulDivOp::Div => "/ ",
        };
        if power == Rational::ONE {
            format!("{prefix}{name}")
        } else {
            format!(
                "{prefix}{name}{}",
                power.fmt_exponent(crate::ratio::ExponentStyle::Source)
            )
        }
    }
}

/// A generic argument resolved according to its declared sort.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedGenericArg {
    Dim(ResolvedDim),
    Index(ResolvedIndex),
    Nat(NatPolyForm, Span),
    /// A `Type`-sorted argument: always a (non-indexed) value type.
    Type(ResolvedValueType),
}

impl ResolvedGenericArg {
    #[must_use]
    pub(crate) fn format(&self, registry: &FormattingRegistry) -> String {
        match self {
            Self::Dim(dim) => dim.format(registry),
            Self::Index(index) => index.to_string(),
            Self::Nat(form, _) => form.format(),
            Self::Type(value_type) => value_type.format(registry),
        }
    }
}

/// A resolved index in an indexed type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedIndex {
    /// A concrete index name, e.g. `Maneuver`.
    Concrete(ResolvedIndexName, Span),
    /// A generic index parameter, e.g. `I`
    GenericParam(GenericParamId, Span),
    /// A structural finite index `Fin(N)` carrying a normalized Nat cardinality.
    Finite(NatPolyForm, Span),
}

/// Renders the source-facing spelling: the declared leaf name, the generic
/// parameter, or `Fin(<Nat form>)` through [`IndexDisplayName`].
impl std::fmt::Display for ResolvedIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Concrete(name, _) => f.write_str(name.as_str()),
            Self::GenericParam(name, _) => name.fmt(f),
            Self::Finite(form, _) => IndexDisplayName::Finite(form.clone()).fmt(f),
        }
    }
}

/// A resolved value type: the type of one scalar (non-indexed) value.
///
/// Index arguments are never value types; they live only in
/// [`ResolvedGenericArg::Index`], `Key<I>`, and the axes of
/// [`ResolvedDeclType::Indexed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedValueType {
    /// `Bool`
    Bool,
    /// `Int`
    Int,
    /// A datetime instant in a specific time scale (e.g., `Datetime` = UTC, `Datetime<TT>`).
    Datetime(TimeScale),
    /// A quantity type, e.g. `Length * Time^-2`, `Dimensionless`, or `D^2`.
    Quantity(ResolvedDim),
    /// A dimension-aware complex quantity type, e.g. `Complex<Length>`.
    Complex { dimension: ResolvedDim, span: Span },
    /// An index-key type, e.g. `Key<Maneuver>` or `Key<Fin(3)>`.
    Key { index: ResolvedIndex, span: Span },
    /// A nominal type with its sort-aware arguments, e.g. `TransferResult`,
    /// `Vec3<Length, ECI>`, or `FixedVec<3>`. A non-generic type has none.
    Struct {
        name: ResolvedStructTypeName,
        generic_args: Vec<ResolvedGenericArg>,
        span: Span,
    },
    /// A generic type parameter, e.g. `F: Type`.
    GenericTypeParam(GenericParamId, Span),
}

impl ResolvedValueType {
    /// Format as a human-readable string, e.g. `"Length / Time^2"`, `"Bool"`, `"Vec3<Length, ECI>"`.
    #[must_use]
    pub fn format(&self, registry: &FormattingRegistry) -> String {
        match self {
            Self::Bool => "Bool".to_string(),
            Self::Int => "Int".to_string(),
            Self::Datetime(scale) => {
                if scale.is_utc() {
                    "Datetime".to_string()
                } else {
                    format!("Datetime<{scale}>")
                }
            }
            Self::Quantity(dimension) => dimension.format(registry),
            Self::Complex { dimension, .. } => {
                format!("Complex<{}>", dimension.format(registry))
            }
            Self::Key { index, .. } => format!("Key<{index}>"),
            Self::Struct {
                name, generic_args, ..
            } => {
                if generic_args.is_empty() {
                    name.as_str().to_string()
                } else {
                    let args: Vec<String> = generic_args
                        .iter()
                        .map(|arg| arg.format(registry))
                        .collect();
                    format!("{}<{}>", name.as_str(), args.join(", "))
                }
            }
            Self::GenericTypeParam(name, _) => name.to_string(),
        }
    }

    /// The checked type of a concrete value type.
    ///
    /// # Errors
    ///
    /// Returns a [`SemanticError`] if the type still mentions a generic
    /// parameter.
    pub fn to_checked_type(&self, src: SourceId) -> Result<CheckedType, SemanticError> {
        match self {
            Self::Bool => Ok(CheckedType::Bool),
            Self::Int => Ok(CheckedType::Int),
            Self::Datetime(scale) => Ok(CheckedType::Datetime(*scale)),
            Self::Quantity(ResolvedDim::Concrete(dim)) => Ok(CheckedType::Quantity(dim.clone())),
            Self::Quantity(symbolic @ ResolvedDim::Symbolic { span, .. }) => {
                let message = symbolic.lone_generic_param().map_or_else(
                    || "cannot use generic dimension expression as a concrete type".to_string(),
                    |(name, _)| {
                        format!(
                            "cannot use generic dimension parameter `{name}` as a concrete type"
                        )
                    },
                );
                let span = symbolic
                    .lone_generic_param()
                    .map_or(*span, |(_, span)| span);
                Err(eval_error(message, src, span))
            }
            Self::Complex { dimension, span } => match dimension {
                ResolvedDim::Concrete(dimension) => Ok(CheckedType::Complex(dimension.clone())),
                symbolic @ ResolvedDim::Symbolic { .. } => {
                    let message = symbolic.lone_generic_param().map_or_else(
                        || "complex dimension expression is not concrete".to_string(),
                        |(name, _)| format!("complex dimension parameter `{name}` is not bound"),
                    );
                    Err(eval_error(message, src, *span))
                }
            },
            Self::Key { index, .. } => index_type_ref(index, src).map(CheckedType::Key),
            Self::Struct {
                name, generic_args, ..
            } => Ok(CheckedType::Struct(
                StructTypeRef::from_resolved(name.clone()),
                generic_args
                    .iter()
                    .map(|arg| resolved_generic_arg_to_declared(arg, src))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            Self::GenericTypeParam(name, span) => Err(eval_error(
                format!("cannot use generic type parameter `{name}` as a concrete type"),
                src,
                *span,
            )),
        }
    }
}

/// The type of a declaration or nominal field: a value type, optionally
/// indexed by one or more axes (`T[I, J]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedDeclType {
    /// A scalar (non-indexed) value type.
    Value(ResolvedValueType),
    /// A value type indexed by one or more axes, outermost first.
    Indexed {
        element: ResolvedValueType,
        indexes: NonEmpty<ResolvedIndex>,
    },
}

impl ResolvedDeclType {
    /// The value type of each element (the type itself when not indexed).
    #[must_use]
    pub const fn element(&self) -> &ResolvedValueType {
        match self {
            Self::Value(value_type)
            | Self::Indexed {
                element: value_type,
                ..
            } => value_type,
        }
    }

    /// The index axes, outermost first; empty when not indexed.
    #[must_use]
    pub fn indexes(&self) -> &[ResolvedIndex] {
        match self {
            Self::Value(_) => &[],
            Self::Indexed { indexes, .. } => indexes.as_slice(),
        }
    }

    /// Format as a human-readable string, e.g. `"Velocity[Maneuver]"`.
    #[must_use]
    pub fn format(&self, registry: &FormattingRegistry) -> String {
        match self {
            Self::Value(value_type) => value_type.format(registry),
            Self::Indexed { element, indexes } => {
                let idx_strs: Vec<String> = indexes.iter().map(ToString::to_string).collect();
                format!("{}[{}]", element.format(registry), idx_strs.join(", "))
            }
        }
    }

    /// The checked type of a concrete declaration type.
    ///
    /// # Errors
    ///
    /// Returns a [`SemanticError`] if the type still mentions a generic
    /// parameter or names an invalid finite index.
    pub fn to_checked_type(&self, src: SourceId) -> Result<CheckedType, SemanticError> {
        let element = self.element().to_checked_type(src)?;
        self.indexes()
            .iter()
            .rev()
            .try_fold(element, |result, index| {
                let index = match index {
                    ResolvedIndex::Concrete(name, _) => IndexTypeRef::from_resolved(name.clone()),
                    ResolvedIndex::Finite(form, span) => {
                        IndexTypeRef::Finite(finite_index(form, src, *span)?)
                    }
                    ResolvedIndex::GenericParam(name, span) => {
                        return Err(eval_error(
                            format!(
                                "cannot use generic index parameter `{name}` as a concrete type"
                            ),
                            src,
                            *span,
                        ));
                    }
                };
                Ok(CheckedType::Indexed {
                    element: Box::new(result),
                    index,
                })
            })
    }
}

fn eval_error(message: String, src: SourceId, span: Span) -> SemanticError {
    SemanticError::located(src, span, EvaluationError::Failed { message })
}

pub fn resolved_generic_arg_to_declared(
    resolved: &ResolvedGenericArg,
    src: SourceId,
) -> Result<CheckedGenericArg, SemanticError> {
    match resolved {
        ResolvedGenericArg::Dim(ResolvedDim::Concrete(dim)) => {
            Ok(CheckedGenericArg::Dim(dim.clone()))
        }
        ResolvedGenericArg::Dim(symbolic @ ResolvedDim::Symbolic { span, .. }) => {
            Err(match symbolic.lone_generic_param() {
                Some((name, span)) => eval_error(
                    format!("generic dimension parameter `{name}` is not bound"),
                    src,
                    span,
                ),
                None => eval_error(
                    "generic dimension expression is not concrete".to_string(),
                    src,
                    *span,
                ),
            })
        }
        ResolvedGenericArg::Index(index) => {
            index_type_ref(index, src).map(CheckedGenericArg::Index)
        }
        ResolvedGenericArg::Nat(form, span) => form
            .constant_value()
            .map(CheckedGenericArg::Nat)
            .ok_or_else(|| {
                eval_error(
                    format!("generic Nat argument `{}` is not concrete", form.format()),
                    src,
                    *span,
                )
            }),
        ResolvedGenericArg::Type(value_type) => {
            value_type.to_checked_type(src).map(CheckedGenericArg::Type)
        }
    }
}

/// The validated structural axis of a constant `Fin(N)` cardinality.
fn finite_index(
    form: &NatPolyForm,
    src: SourceId,
    span: Span,
) -> Result<FiniteIndex, SemanticError> {
    let size = form.constant_value().ok_or_else(|| {
        eval_error(
            format!(
                "cannot use generic nat expression `{}` as a concrete type",
                form.format()
            ),
            src,
            span,
        )
    })?;
    FiniteIndex::try_from_u64(size)
        .map_err(|err| eval_error(err.describe_finite_index(), src, span))
}

fn index_type_ref(index: &ResolvedIndex, src: SourceId) -> Result<IndexTypeRef, SemanticError> {
    match index {
        ResolvedIndex::Concrete(name, _) => Ok(IndexTypeRef::from_resolved(name.clone())),
        ResolvedIndex::Finite(form, span) => {
            finite_index(form, src, *span).map(IndexTypeRef::Finite)
        }
        ResolvedIndex::GenericParam(name, span) => Err(eval_error(
            format!("generic index parameter `{name}` is not bound"),
            src,
            *span,
        )),
    }
}

/// Embed a concrete generic argument into the symbolic form.
///
/// This is the inverse of [`resolved_generic_arg_to_declared`]; `span`
/// locates the embedded nodes. An indexed type is not a value type, so it has
/// no `Type`-sorted embedding.
#[must_use]
pub fn declared_to_resolved_generic_arg(
    arg: &CheckedGenericArg,
    span: Span,
) -> Option<ResolvedGenericArg> {
    Some(match arg {
        CheckedGenericArg::Dim(dim) => ResolvedGenericArg::Dim(ResolvedDim::Concrete(dim.clone())),
        CheckedGenericArg::Index(index) => {
            ResolvedGenericArg::Index(index_ref_to_resolved(index, span))
        }
        CheckedGenericArg::Nat(value) => {
            ResolvedGenericArg::Nat(NatPolyForm::from_constant(*value), span)
        }
        CheckedGenericArg::Type(ty) => {
            ResolvedGenericArg::Type(declared_to_resolved_type(ty, span)?)
        }
    })
}

/// Embed a concrete value type into the symbolic form; `None` for an indexed
/// type.
fn declared_to_resolved_type(declared: &CheckedType, span: Span) -> Option<ResolvedValueType> {
    Some(match declared {
        CheckedType::Quantity(dim) => {
            ResolvedValueType::Quantity(ResolvedDim::Concrete(dim.clone()))
        }
        CheckedType::Complex(dim) => ResolvedValueType::Complex {
            dimension: ResolvedDim::Concrete(dim.clone()),
            span,
        },
        CheckedType::Bool => ResolvedValueType::Bool,
        CheckedType::Int => ResolvedValueType::Int,
        CheckedType::Datetime(scale) => ResolvedValueType::Datetime(*scale),
        CheckedType::Key(index) => ResolvedValueType::Key {
            index: index_ref_to_resolved(index, span),
            span,
        },
        CheckedType::Struct(name, args) => ResolvedValueType::Struct {
            name: name.resolved().clone(),
            generic_args: args
                .iter()
                .map(|arg| declared_to_resolved_generic_arg(arg, span))
                .collect::<Option<_>>()?,
            span,
        },
        CheckedType::Indexed { .. } => return None,
    })
}

fn index_ref_to_resolved(index: &IndexTypeRef, span: Span) -> ResolvedIndex {
    match index {
        IndexTypeRef::Declared(reference) => {
            ResolvedIndex::Concrete(reference.resolved().clone(), span)
        }
        IndexTypeRef::Finite(finite) => {
            ResolvedIndex::Finite(NatPolyForm::from_constant(finite.size_u64()), span)
        }
    }
}

#[cfg(test)]
mod tests;
