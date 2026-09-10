//! Retained expression checking results. Identity and diagnostics are separate.
//!
//! Publication checks structural coverage, not expression typing. Producers must
//! supply successful inference results; a scalar row is not a missing shape row.

mod static_index;
pub use static_index::{Readiness, StaticIndexError, StaticIndexRequirement, StaticIndexUse};

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::body_revision::BodyRevision;
use crate::dag_id::DagId;
use crate::expression_id::{ExprId, UnassignedExprId};
use crate::expression_source::{ExpressionSourceError, ExpressionSourceMap};
use crate::hir::expr::{ConstRef, Expr, ExprKind, FunctionRef, visit_expr_children};
use crate::registry::declared_type::{DeclaredGenericArg, DeclaredType, IndexTypeRef};
use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorName, FieldName, ResolvedStructTypeName};
use crate::tir::materialized_shape::MaterializedShape;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpressionShape {
    Scalar,
    Concrete(MaterializedShape),
    /// Axes whose cardinalities require a Static or lexical generic binding.
    Symbolic(Vec<IndexTypeRef>),
}

impl ExpressionShape {
    fn matches_type(
        &self,
        checked_type: &DeclaredType,
        cardinality: &dyn Fn(
            &IndexTypeRef,
        ) -> Result<
            Option<crate::registry::index::IndexCardinality>,
            ExpressionFactsError,
        >,
    ) -> Result<bool, ExpressionFactsError> {
        let mut axes = Vec::new();
        let mut ty = checked_type;
        while let DeclaredType::Indexed { element, index } = ty {
            axes.push(index);
            ty = element;
        }
        let sizes = axes
            .iter()
            .map(|index| cardinality(index))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(match self {
            Self::Scalar => axes.is_empty(),
            Self::Concrete(shape) => {
                shape.rank() == axes.len()
                    && sizes
                        .iter()
                        .zip(shape.axes().iter())
                        .all(|(expected, actual)| *expected == Some(*actual))
            }
            Self::Symbolic(symbolic) => {
                !axes.is_empty()
                    && sizes.iter().any(Option::is_none)
                    && axes.iter().copied().eq(symbolic.iter())
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstructorApplication {
    /// Definition identity used by field contracts, distinct from runtime owner.
    pub definition: ResolvedStructTypeName,
    pub runtime_type: ResolvedStructTypeName,
    pub constructor: ConstructorName,
    pub generic_args: Vec<DeclaredGenericArg>,
    /// Empty means no constraints; missing a listed field's contract is an error.
    pub required_constraints: Vec<FieldName>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstructorMatch {
    pub definition: ResolvedStructTypeName,
    pub runtime_type: ResolvedStructTypeName,
    pub constructor: ConstructorName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextualOperand {
    String,
    OffsetDateTime,
    CivilDateTime,
    ZonedDateTime,
    TimeZone,
    TypeSystem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpressionFact {
    Value {
        checked_type: DeclaredType,
        shape: ExpressionShape,
        constructor: Option<Box<ConstructorApplication>>,
    },
    Contextual(ContextualOperand),
}

/// Direct requirements compose over `children`; no quadratic transitive sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpressionOperation {
    Literal,
    Contextual(ContextualOperand),
    GraphReference(ResolvedDeclName),
    Constant(ConstRef),
    Local(crate::hir::expr::LocalId),
    Binary(crate::syntax::ast::BinOp),
    Unary(crate::syntax::ast::UnaryOp),
    BuiltinCall(crate::builtin::BuiltinFnName),
    EpochCall(crate::registry::time_scale::TimeScale),
    HostCall(crate::plugin_identity::ExternFnKey),
    Conditional,
    Conversion,
    DisplayTimezone,
    Field,
    Constructor,
    Map,
    Comprehension,
    Index,
    Scan,
    Unfold,
    Key,
    Match,
    Variant,
    DagCall(DagId),
}

impl ExpressionOperation {
    /// Validate borrowed structure without manufacturing an owned operation (and
    /// cloning canonical graph/plugin/constant identities) for comparison.
    fn matches_expr(&self, expr: &Expr) -> bool {
        match (self, expr.kind()) {
            (
                Self::Literal,
                ExprKind::Number(_)
                | ExprKind::Integer(_)
                | ExprKind::Bool(_)
                | ExprKind::QuantityLiteral { .. },
            )
            | (Self::Contextual(ContextualOperand::String), ExprKind::StringLiteral(_))
            | (
                Self::Contextual(ContextualOperand::OffsetDateTime),
                ExprKind::OffsetDateTimeLiteral(_),
            )
            | (
                Self::Contextual(ContextualOperand::CivilDateTime),
                ExprKind::CivilDateTimeLiteral(_),
            )
            | (
                Self::Contextual(ContextualOperand::ZonedDateTime),
                ExprKind::ZonedDateTimeLiteral(_),
            )
            | (Self::Contextual(ContextualOperand::TimeZone), ExprKind::IanaTimeZoneLiteral(_))
            | (Self::Contextual(ContextualOperand::TypeSystem), ExprKind::TypeSystemRef(_))
            | (Self::Conditional, ExprKind::If { .. })
            | (Self::Conversion, ExprKind::Convert { .. })
            | (Self::DisplayTimezone, ExprKind::DisplayTimezone { .. })
            | (Self::Field, ExprKind::FieldAccess { .. })
            | (Self::Constructor, ExprKind::ConstructorCall { .. })
            | (Self::Map, ExprKind::MapLiteral { .. })
            | (Self::Comprehension, ExprKind::ForComp { .. })
            | (Self::Index, ExprKind::IndexAccess { .. })
            | (Self::Scan, ExprKind::Scan { .. })
            | (Self::Unfold, ExprKind::Unfold { .. })
            | (Self::Key, ExprKind::KeyForm { .. })
            | (Self::Match, ExprKind::Match { .. })
            | (Self::Variant, ExprKind::VariantLiteral(_)) => true,
            (Self::GraphReference(expected), ExprKind::GraphRef(actual)) => {
                expected == &actual.value
            }
            (Self::Constant(expected), ExprKind::ConstRef(actual)) => expected == &actual.value,
            (Self::Local(expected), ExprKind::LocalRef(actual)) => expected == &actual.value,
            (Self::Binary(expected), ExprKind::BinOp { op, .. }) => expected == op,
            (Self::Unary(expected), ExprKind::UnaryOp { op, .. }) => expected == op,
            (Self::BuiltinCall(expected), ExprKind::FnCall { callee, .. }) => {
                matches!(&callee.value, FunctionRef::Builtin(actual) if actual == expected)
            }
            (Self::EpochCall(expected), ExprKind::FnCall { callee, .. }) => {
                matches!(&callee.value, FunctionRef::Epoch { scale } if &scale.value == expected)
            }
            (Self::HostCall(expected), ExprKind::FnCall { callee, .. }) => {
                matches!(&callee.value, FunctionRef::External(actual) if actual.plugin == expected.plugin && actual.name == expected.name)
            }
            (Self::DagCall(expected), ExprKind::DagCall { target, .. }) => {
                expected == &target.value
            }
            _ => false,
        }
    }

    fn from_expr(expr: &Expr) -> Result<Self, ExpressionFactsError> {
        Ok(match expr.kind() {
            ExprKind::Error { .. } => return Err(ExpressionFactsError::Unresolved),
            ExprKind::Number(_)
            | ExprKind::Integer(_)
            | ExprKind::Bool(_)
            | ExprKind::QuantityLiteral { .. } => Self::Literal,
            ExprKind::StringLiteral(_) => Self::Contextual(ContextualOperand::String),
            ExprKind::OffsetDateTimeLiteral(_) => {
                Self::Contextual(ContextualOperand::OffsetDateTime)
            }
            ExprKind::CivilDateTimeLiteral(_) => Self::Contextual(ContextualOperand::CivilDateTime),
            ExprKind::ZonedDateTimeLiteral(_) => Self::Contextual(ContextualOperand::ZonedDateTime),
            ExprKind::IanaTimeZoneLiteral(_) => Self::Contextual(ContextualOperand::TimeZone),
            ExprKind::TypeSystemRef(_) => Self::Contextual(ContextualOperand::TypeSystem),
            ExprKind::GraphRef(target) => Self::GraphReference(target.value.clone()),
            ExprKind::ConstRef(target) => Self::Constant(target.value.clone()),
            ExprKind::LocalRef(local) => Self::Local(local.value),
            ExprKind::BinOp { op, .. } => Self::Binary(*op),
            ExprKind::UnaryOp { op, .. } => Self::Unary(*op),
            ExprKind::FnCall { callee, .. } => match &callee.value {
                FunctionRef::Builtin(builtin) => Self::BuiltinCall(*builtin),
                FunctionRef::Epoch { scale } => Self::EpochCall(scale.value),
                FunctionRef::External(function) => Self::HostCall(function.key()),
            },
            ExprKind::If { .. } => Self::Conditional,
            ExprKind::Convert { .. } => Self::Conversion,
            ExprKind::DisplayTimezone { .. } => Self::DisplayTimezone,
            ExprKind::FieldAccess { .. } => Self::Field,
            ExprKind::ConstructorCall { .. } => Self::Constructor,
            ExprKind::MapLiteral { .. } => Self::Map,
            ExprKind::ForComp { .. } => Self::Comprehension,
            ExprKind::IndexAccess { .. } => Self::Index,
            ExprKind::Scan { .. } => Self::Scan,
            ExprKind::Unfold { .. } => Self::Unfold,
            ExprKind::KeyForm { .. } => Self::Key,
            ExprKind::Match { .. } => Self::Match,
            ExprKind::VariantLiteral(_) => Self::Variant,
            ExprKind::DagCall { target, .. } => Self::DagCall(target.value.clone()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NominalObservation {
    Field {
        identity: ResolvedStructTypeName,
        field: FieldName,
    },
    Constructor {
        identity: ResolvedStructTypeName,
        constructor: crate::syntax::type_name::ResolvedConstructorName,
    },
    TypeArgument(ResolvedStructTypeName),
    IndexLabel {
        identity: IndexTypeRef,
        variant: crate::syntax::index_name::IndexVariantName,
    },
    IndexArgument(IndexTypeRef),
}

/// One immutable checking environment is shared by all rows of a product.
/// Keeping the owner and semantic revision together prevents independently
/// copied stamps from drifting while avoiding per-expression DAG clones.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CheckingEnvironment {
    owner: DagId,
    revision: BodyRevision,
}

impl CheckingEnvironment {
    pub(crate) fn new(owner: DagId, revision: BodyRevision) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self { owner, revision })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedExpressionRecord {
    pub(crate) environment: std::sync::Arc<CheckingEnvironment>,
    pub fact: ExpressionFact,
    pub operation: std::sync::Arc<ExpressionOperation>,
    pub(crate) children: Option<std::sync::Arc<[ExprId]>>,
    pub static_indexes: Vec<StaticIndexRequirement>,
    /// Canonical unit edges are resolved in the unit body's own fact scope.
    pub(crate) unit_dependencies:
        Option<std::sync::Arc<[crate::syntax::dimension::ResolvedUnitName]>>,
    pub constructor_matches:
        HashMap<crate::syntax::type_name::ResolvedConstructorName, ConstructorMatch>,
    /// Binder-aware nominal observations for a checked root (V005).
    pub(crate) nominal_observations: Option<std::sync::Arc<[NominalObservation]>>,
    pub nat_parameters: Option<
        std::sync::Arc<
            HashMap<crate::syntax::type_name::GenericParamName, crate::hir::types::GenericParamId>,
        >,
    >,
}

fn share_nonempty<T>(values: Vec<T>) -> Option<std::sync::Arc<[T]>> {
    (!values.is_empty()).then(|| values.into())
}

impl CheckedExpressionRecord {
    #[must_use]
    pub fn children(&self) -> &[ExprId] {
        self.children.as_deref().unwrap_or(&[])
    }

    #[must_use]
    pub fn unit_dependencies(&self) -> &[crate::syntax::dimension::ResolvedUnitName] {
        self.unit_dependencies.as_deref().unwrap_or(&[])
    }

    #[must_use]
    pub fn nominal_observations(&self) -> &[NominalObservation] {
        self.nominal_observations.as_deref().unwrap_or(&[])
    }

    fn matches_structure(&self, expr: &Expr) -> bool {
        if !self.operation.matches_expr(expr) {
            return false;
        }
        let mut children = self.children().iter();
        let mut matched = true;
        visit_expr_children(expr, &mut |child| {
            matched &= child.id().is_ok_and(|id| children.next() == Some(id));
        });
        let units_match = match expr.kind() {
            ExprKind::QuantityLiteral { unit, .. } | ExprKind::Convert { target: unit, .. } => unit
                .terms
                .iter()
                .map(|term| term.name.value.resolved())
                .eq(self.unit_dependencies().iter()),
            _ => self.unit_dependencies().is_empty(),
        };
        matched && children.next().is_none() && units_match
    }

    pub(crate) fn new(
        expr: &Expr,
        fact: ExpressionFact,
        environment: std::sync::Arc<CheckingEnvironment>,
    ) -> Result<Box<Self>, ExpressionFactsError> {
        let mut children = Vec::new();
        visit_expr_children(expr, &mut |child| children.push(child.id().cloned()));
        Ok(Box::new(Self {
            environment,
            fact,
            operation: std::sync::Arc::new(ExpressionOperation::from_expr(expr)?),
            children: share_nonempty(children.into_iter().collect::<Result<_, _>>()?),
            static_indexes: Vec::new(),
            unit_dependencies: share_nonempty(match expr.kind() {
                ExprKind::QuantityLiteral { unit, .. } | ExprKind::Convert { target: unit, .. } => {
                    unit.terms
                        .iter()
                        .map(|term| term.name.value.resolved().clone())
                        .collect()
                }
                _ => Vec::new(),
            }),
            constructor_matches: HashMap::new(),
            nat_parameters: None,
            nominal_observations: None,
        }))
    }
}

fn type_is_ready(
    ty: &DeclaredType,
    cardinality: &dyn Fn(
        &IndexTypeRef,
    ) -> Result<
        Option<crate::registry::index::IndexCardinality>,
        ExpressionFactsError,
    >,
) -> Result<bool, ExpressionFactsError> {
    Ok(match ty {
        DeclaredType::Indexed { element, index } => {
            cardinality(index)?.is_some() & type_is_ready(element, cardinality)?
        }
        DeclaredType::Key(index) | DeclaredType::IndexArg(index) => cardinality(index)?.is_some(),
        DeclaredType::Struct(_, args) => args.iter().try_fold(true, |ready, arg| {
            Ok::<_, ExpressionFactsError>(
                ready
                    & match arg {
                        DeclaredGenericArg::Nat(form) => form.is_constant(),
                        DeclaredGenericArg::Type(ty) => type_is_ready(ty, cardinality)?,
                        DeclaredGenericArg::Index(index) => cardinality(index)?.is_some(),
                        DeclaredGenericArg::Dim(_) => true,
                    },
            )
        })?,
        DeclaredType::Quantity(_)
        | DeclaredType::Complex(_)
        | DeclaredType::Bool
        | DeclaredType::Int
        | DeclaredType::Datetime(_) => true,
    })
}

fn record_is_ready(
    record: &CheckedExpressionRecord,
    deferred: &HashSet<ExprId>,
    cardinality: &dyn Fn(
        &IndexTypeRef,
    ) -> Result<
        Option<crate::registry::index::IndexCardinality>,
        ExpressionFactsError,
    >,
) -> Result<bool, ExpressionFactsError> {
    let mut ready = match &record.fact {
        ExpressionFact::Value { checked_type, .. } => type_is_ready(checked_type, cardinality)?,
        ExpressionFact::Contextual(_) => true,
    };
    for requirement in &record.static_indexes {
        ready &= requirement.check(cardinality(&requirement.axis)?)? == Readiness::Ready;
    }
    Ok(ready
        && record
            .children()
            .iter()
            .all(|child| !deferred.contains(child)))
}

fn matches_constructor_targets(expr: &Expr, record: &CheckedExpressionRecord) -> bool {
    let ExprKind::Match { arms, .. } = expr.kind() else {
        return record.constructor_matches.is_empty();
    };
    let expected: HashSet<_> = arms
        .iter()
        .filter_map(|arm| match &arm.pattern {
            crate::hir::expr::MatchPattern::Constructor { constructor, .. } => {
                Some(&constructor.value)
            }
            crate::hir::expr::MatchPattern::IndexLabel { .. } => None,
        })
        .collect();
    expected.len() == record.constructor_matches.len()
        && expected
            .into_iter()
            .all(|id| record.constructor_matches.contains_key(id))
}

fn value_type<'a>(
    records: &'a HashMap<ExprId, Box<CheckedExpressionRecord>>,
    expr: &Expr,
) -> Result<&'a DeclaredType, ExpressionFactsError> {
    let id = expr.id()?;
    match &records
        .get(id)
        .ok_or_else(|| ExpressionFactsError::Missing(id.clone()))?
        .fact
    {
        ExpressionFact::Value { checked_type, .. } => Ok(checked_type),
        ExpressionFact::Contextual(_) => Err(ExpressionFactsError::Incompatible(id.clone())),
    }
}

/// Inventory required static checks by operand identity, never by replaying
/// constant arithmetic. Numeric proof values come only from the checker.
fn static_requirement_coverage(
    expr: &Expr,
    record: &CheckedExpressionRecord,
    records: &HashMap<ExprId, Box<CheckedExpressionRecord>>,
) -> Result<bool, ExpressionFactsError> {
    let mut requirements = record.static_indexes.iter();
    let mut consume_requirement = |operand: &ExprId, axis: &IndexTypeRef, usage| {
        requirements.next().is_some_and(|requirement| {
            *operand == requirement.operand
                && *axis == requirement.axis
                && usage == requirement.usage
        })
    };
    let matched = match expr.kind() {
        ExprKind::KeyForm {
            kind: crate::syntax::ast::KeyFormKind::Static,
            arg,
            ..
        } => {
            let DeclaredType::Key(axis) = value_type(records, expr)? else {
                return Ok(false);
            };
            consume_requirement(arg.id()?, axis, StaticIndexUse::Key)
        }
        ExprKind::IndexAccess { expr: inner, args } => {
            let mut ty = value_type(records, inner)?;
            for arg in args {
                let DeclaredType::Indexed { element, index } = ty else {
                    return Ok(false);
                };
                if let crate::hir::expr::IndexArg::Expr(operand) = arg
                    && matches!(value_type(records, operand)?, DeclaredType::Int)
                    && !consume_requirement(operand.id()?, index, StaticIndexUse::Selection)
                {
                    return Ok(false);
                }
                ty = element;
            }
            true
        }
        _ => true,
    };
    Ok(matched && requirements.next().is_none())
}

/// Clones share one immutable publication; cloning a DAG or input row must not
/// deep-copy its sealed checking result.
#[derive(Debug, Clone)]
pub struct CheckedExpressionFacts {
    data: std::sync::Arc<PublishedExpressionFacts>,
}

#[derive(Debug)]
struct PublishedExpressionFacts {
    owner: DagId,
    revision: BodyRevision,
    records: HashMap<ExprId, Box<CheckedExpressionRecord>>,
    source: ExpressionSourceMap,
    deferred: HashSet<ExprId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExpressionFactsError {
    #[error(transparent)]
    Unassigned(#[from] UnassignedExprId),
    #[error(transparent)]
    Source(#[from] ExpressionSourceError),
    #[error("unresolved expression cannot have checked facts")]
    Unresolved,
    #[error("expression facts belong to another semantic environment or revision")]
    WrongEnvironment,
    #[error("missing checked expression: {0:?}")]
    Missing(ExprId),
    #[error("checked expression references an unavailable index: {0}")]
    MissingIndex(Box<IndexTypeRef>),
    #[error("extra checked expression: {0:?}")]
    Extra(ExprId),
    #[error("incompatible checked expression: {0:?}")]
    Incompatible(ExprId),
    #[error("one expression identity has conflicting diagnostic origins: {0:?}")]
    ConflictingOrigin(ExprId),
    #[error("expression has undischarged static obligations: {0:?}")]
    Deferred(ExprId),
    #[error("contextual metadata is not an executable value: {0:?}")]
    Contextual(ExprId),
    #[error(transparent)]
    StaticIndex(#[from] StaticIndexError),
}

impl CheckedExpressionFacts {
    pub(crate) fn publish(
        owner: DagId,
        revision: BodyRevision,
        roots: &[&Expr],
        records: HashMap<ExprId, Box<CheckedExpressionRecord>>,
        cardinality: &dyn Fn(
            &IndexTypeRef,
        ) -> Result<
            Option<crate::registry::index::IndexCardinality>,
            ExpressionFactsError,
        >,
    ) -> Result<Self, ExpressionFactsError> {
        let mut inventory = HashMap::new();
        let mut deferred = HashSet::new();
        let mut error = Ok(());
        for root in roots {
            crate::hir::expr::visit_expr_postorder(root, &mut |expr| {
                if error.is_err() {
                    return;
                }
                error = (|| {
                    let id = expr.id()?.clone();
                    let record = records
                        .get(&id)
                        .ok_or_else(|| ExpressionFactsError::Missing(id.clone()))?;
                    if record.environment.owner != owner || record.environment.revision != revision
                    {
                        return Err(ExpressionFactsError::WrongEnvironment);
                    }
                    if !record.matches_structure(expr)
                        || !static_requirement_coverage(expr, record, &records)?
                    {
                        return Err(ExpressionFactsError::Incompatible(id));
                    }
                    if !matches_constructor_targets(expr, record) {
                        return Err(ExpressionFactsError::Incompatible(id));
                    }
                    let compatible = match (&record.fact, record.operation.as_ref()) {
                        (
                            ExpressionFact::Contextual(actual),
                            ExpressionOperation::Contextual(expected),
                        ) => actual == expected,
                        (ExpressionFact::Contextual(_), _)
                        | (ExpressionFact::Value { .. }, ExpressionOperation::Contextual(_)) => {
                            false
                        }
                        (
                            ExpressionFact::Value {
                                checked_type,
                                shape,
                                constructor,
                            },
                            operation,
                        ) => {
                            let shape_matches = shape.matches_type(checked_type, cardinality)?;
                            let constructor_matches = matches!(
                                operation,
                                ExpressionOperation::Constructor
                                    | ExpressionOperation::Constant(ConstRef::Constructor(_))
                            ) == constructor.is_some();
                            let application_matches =
                                constructor
                                    .as_ref()
                                    .is_none_or(|application| match checked_type {
                                        DeclaredType::Struct(identity, args) => {
                                            identity.resolved() == &application.runtime_type
                                                && args == &application.generic_args
                                        }
                                        _ => false,
                                    });
                            shape_matches && constructor_matches && application_matches
                        }
                    };
                    if !compatible {
                        return Err(ExpressionFactsError::Incompatible(id));
                    }
                    if !record_is_ready(record, &deferred, cardinality)? {
                        deferred.insert(id.clone());
                    }
                    match inventory.entry(id) {
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(expr.span);
                        }
                        std::collections::hash_map::Entry::Occupied(entry)
                            if *entry.get() == expr.span => {}
                        std::collections::hash_map::Entry::Occupied(entry) => {
                            return Err(ExpressionFactsError::ConflictingOrigin(
                                entry.key().clone(),
                            ));
                        }
                    }
                    Ok(())
                })();
            });
        }
        error?;
        if let Some(extra) = records.keys().find(|id| !inventory.contains_key(*id)) {
            return Err(ExpressionFactsError::Extra(extra.clone()));
        }
        Ok(Self {
            data: std::sync::Arc::new(PublishedExpressionFacts {
                owner,
                revision,
                records,
                source: ExpressionSourceMap::try_new(inventory)?,
                deferred,
            }),
        })
    }

    pub fn validate_environment(
        &self,
        owner: &DagId,
        revision: &BodyRevision,
    ) -> Result<(), ExpressionFactsError> {
        if &self.data.owner == owner && &self.data.revision == revision {
            Ok(())
        } else {
            Err(ExpressionFactsError::WrongEnvironment)
        }
    }

    pub fn get(&self, id: &ExprId) -> Result<&CheckedExpressionRecord, ExpressionFactsError> {
        self.data
            .records
            .get(id)
            .map(AsRef::as_ref)
            .ok_or_else(|| ExpressionFactsError::Missing(id.clone()))
    }

    pub fn span(&self, id: &ExprId) -> Result<Span, ExpressionFactsError> {
        self.data.source.span(id).map_err(Into::into)
    }

    pub fn records(&self) -> impl Iterator<Item = (&ExprId, &CheckedExpressionRecord)> {
        self.data
            .records
            .iter()
            .map(|(id, record)| (id, record.as_ref()))
    }

    /// Require the complete descendant proof before any execution or host work.
    pub fn executable_value(
        &self,
        id: &ExprId,
    ) -> Result<&CheckedExpressionRecord, ExpressionFactsError> {
        let record = self.get(id)?;
        if self.data.deferred.contains(id) {
            return Err(ExpressionFactsError::Deferred(id.clone()));
        }
        match record.fact {
            ExpressionFact::Value { .. } => Ok(record),
            ExpressionFact::Contextual(_) => Err(ExpressionFactsError::Contextual(id.clone())),
        }
    }
}
