//! Retained expression checking results. Identity and diagnostics are separate.
//!
//! Publication checks structural coverage, not expression typing. Producers must
//! supply successful inference results; a scalar row is not a missing shape row.

pub use crate::tir::static_index::{
    Readiness, StaticIndexError, StaticIndexRequirement, StaticIndexUse,
};

#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::body_revision::BodyRevision;
use crate::dag_id::DagId;
use crate::expression_id::ExprId;
use crate::expression_source::{ExpressionSourceError, ExpressionSourceMap};
use crate::hir::expr::{ConstRef, Expr, ExprKind, FunctionRef, visit_expr_children};
use crate::hir::nominal::ResolvedConstructor;
use crate::registry::checked_type::{
    CheckedGenericArg, CheckedType, Concrete, Concreteness, IndexTypeRef, Symbolic,
};
use crate::resolved_name::ResolvedStructTypeName;
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorName, FieldName};
pub(crate) use crate::tir::static_index::AxisCardinality;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstructorApplication<V: Concreteness = Concrete> {
    /// The applied constructor and its owning definition. The definition's
    /// field annotations are the application's field constraints.
    pub constructor: ResolvedConstructor,
    pub runtime_type: ResolvedStructTypeName,
    pub generic_args: Vec<CheckedGenericArg<V>>,
}

impl<V: Concreteness> ConstructorApplication<V> {
    /// Definition identity used by field contracts, distinct from runtime owner.
    #[must_use]
    pub fn definition(&self) -> &ResolvedStructTypeName {
        self.constructor.owning_type()
    }
}

/// The checked type of a value expression and, for a constructor, its
/// nominal application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueFact<V: Concreteness = Concrete> {
    pub checked_type: CheckedType<V>,
    pub constructor: Option<Box<ConstructorApplication<V>>>,
}

impl ValueFact<Concrete> {
    /// View a concrete fact at the symbolic level.
    #[must_use]
    pub fn to_symbolic(&self) -> ValueFact<Symbolic> {
        ValueFact {
            checked_type: self.checked_type.to_symbolic(),
            constructor: self.constructor.as_ref().map(|application| {
                Box::new(ConstructorApplication {
                    runtime_type: application.runtime_type.clone(),
                    constructor: application.constructor.clone(),
                    generic_args: application
                        .generic_args
                        .iter()
                        .map(CheckedGenericArg::to_symbolic)
                        .collect(),
                })
            }),
        }
    }
}

impl ValueFact<Symbolic> {
    /// The concrete fact, when neither the type nor the constructor
    /// arguments mention a `Nat` variable.
    #[must_use]
    pub fn to_concrete(&self) -> Option<ValueFact<Concrete>> {
        Some(ValueFact {
            checked_type: self.checked_type.to_concrete()?,
            constructor: match &self.constructor {
                None => None,
                Some(application) => Some(Box::new(ConstructorApplication {
                    runtime_type: application.runtime_type.clone(),
                    constructor: application.constructor.clone(),
                    generic_args: application
                        .generic_args
                        .iter()
                        .map(CheckedGenericArg::to_concrete)
                        .collect::<Option<_>>()?,
                })),
            },
        })
    }
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

/// The checking result of one expression.
///
/// A value is recorded [`Symbolic`](ExpressionFact::Symbolic) by inference;
/// publication classifies it by what evaluation may rely on, so readiness is
/// a property of the fact itself rather than of a side table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpressionFact {
    /// A value whose type and every obligation of its subtree are discharged.
    Executable(ValueFact),
    /// A concrete-typed value whose axis cardinalities, static index proofs,
    /// or operands still await Static bindings.
    Pending(ValueFact),
    /// A value whose type mentions unbound `Nat` parameters.
    Symbolic(ValueFact<Symbolic>),
    /// Contextual metadata, never an executable value.
    Contextual(ContextualOperand),
}

impl ExpressionFact {
    /// The concrete-typed value fact, executable or pending.
    #[must_use]
    pub const fn concrete_value(&self) -> Option<&ValueFact> {
        match self {
            Self::Executable(value) | Self::Pending(value) => Some(value),
            Self::Symbolic(_) | Self::Contextual(_) => None,
        }
    }

    /// The value fact viewed at the symbolic level, whatever its classification.
    #[must_use]
    pub fn symbolic_value(&self) -> Option<Cow<'_, ValueFact<Symbolic>>> {
        match self {
            Self::Executable(value) | Self::Pending(value) => Some(Cow::Owned(value.to_symbolic())),
            Self::Symbolic(value) => Some(Cow::Borrowed(value)),
            Self::Contextual(_) => None,
        }
    }
}

/// Direct requirements compose over `children`; no quadratic transitive sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpressionOperation {
    Literal,
    Contextual(ContextualOperand),
    GraphReference(crate::hir::expr::LocalDecl),
    Constant(ConstRef),
    Local(crate::hir::expr::LocalId),
    Binary(crate::syntax::ast::BinOp),
    Unary(crate::syntax::ast::UnaryOp),
    BuiltinCall(crate::builtin::ScaleFreeBuiltin),
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

    fn from_expr(expr: &Expr) -> Self {
        match expr.kind() {
            ExprKind::Error(no_error) => no_error.absurd(),
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
        }
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
        constructor: crate::resolved_name::ResolvedConstructorName,
    },
    TypeArgument(ResolvedStructTypeName),
    IndexLabel {
        identity: IndexTypeRef<Symbolic>,
        variant: crate::syntax::index_name::IndexVariantName,
    },
    IndexArgument(IndexTypeRef<Symbolic>),
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
    pub(crate) unit_dependencies: Option<std::sync::Arc<[crate::resolved_name::ResolvedUnitName]>>,
    pub constructor_matches:
        HashMap<crate::resolved_name::ResolvedConstructorName, ConstructorMatch>,
    /// Binder-aware nominal observations for a checked root (V005).
    pub(crate) nominal_observations: Option<std::sync::Arc<[NominalObservation]>>,
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
    pub fn unit_dependencies(&self) -> &[crate::resolved_name::ResolvedUnitName] {
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
            matched &= children.next() == Some(child.id());
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
    ) -> Box<Self> {
        let mut children = Vec::new();
        visit_expr_children(expr, &mut |child| children.push(child.id().clone()));
        Box::new(Self {
            environment,
            fact,
            operation: std::sync::Arc::new(ExpressionOperation::from_expr(expr)),
            children: share_nonempty(children),
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
            nominal_observations: None,
        })
    }
}

/// Whether every index a type mentions has a known cardinality.
///
/// Every index is queried, even after an unknown one, so an unavailable index
/// definition is always reported.
fn cardinalities_known(
    ty: &CheckedType<Symbolic>,
    cardinality: &AxisCardinality<'_>,
) -> Result<bool, ExpressionFactsError> {
    ty.indexes().into_iter().try_fold(true, |known, index| {
        Ok(known & cardinality(index)?.is_some())
    })
}

/// Whether a value may execute: its type and static index proofs are
/// discharged and no operand is still waiting.
fn value_is_ready(
    record: &CheckedExpressionRecord,
    value: &ValueFact<Symbolic>,
    waiting: &HashSet<ExprId>,
    cardinality: &AxisCardinality<'_>,
) -> Result<bool, ExpressionFactsError> {
    let mut ready = cardinalities_known(&value.checked_type, cardinality)?;
    for requirement in &record.static_indexes {
        ready &= requirement.check(cardinality(&requirement.axis)?)? == Readiness::Ready;
    }
    Ok(ready
        && record
            .children()
            .iter()
            .all(|child| !waiting.contains(child)))
}

/// Classify a published value by what evaluation may rely on.
fn classify(value: ValueFact<Symbolic>, ready: bool) -> ExpressionFact {
    match value.to_concrete() {
        Some(concrete) if ready => ExpressionFact::Executable(concrete),
        Some(concrete) => ExpressionFact::Pending(concrete),
        None => ExpressionFact::Symbolic(value),
    }
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

fn value_type(
    records: &HashMap<ExprId, Box<CheckedExpressionRecord>>,
    expr: &Expr,
) -> Result<CheckedType<Symbolic>, ExpressionFactsError> {
    let id = expr.id();
    records
        .get(id)
        .ok_or_else(|| ExpressionFactsError::Missing(id.clone()))?
        .fact
        .symbolic_value()
        .map(|value| value.checked_type.clone())
        .ok_or_else(|| ExpressionFactsError::Incompatible(id.clone()))
}

/// Inventory required static checks by operand identity, never by replaying
/// constant arithmetic. Numeric proof values come only from the checker.
fn static_requirement_coverage(
    expr: &Expr,
    record: &CheckedExpressionRecord,
    records: &HashMap<ExprId, Box<CheckedExpressionRecord>>,
) -> Result<bool, ExpressionFactsError> {
    let mut requirements = record.static_indexes.iter();
    let mut consume_requirement = |operand: &ExprId, axis: &IndexTypeRef<Symbolic>, usage| {
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
            let CheckedType::Key(axis) = value_type(records, expr)? else {
                return Ok(false);
            };
            consume_requirement(arg.id(), &axis, StaticIndexUse::Key)
        }
        ExprKind::IndexAccess { expr: inner, args } => {
            let inner = value_type(records, inner)?;
            let mut ty = &inner;
            for arg in args {
                let CheckedType::Indexed { element, index } = ty else {
                    return Ok(false);
                };
                if let crate::hir::expr::IndexArg::Expr(operand) = arg
                    && matches!(value_type(records, operand)?, CheckedType::Int)
                    && !consume_requirement(operand.id(), index, StaticIndexUse::Selection)
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
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExpressionFactsError {
    #[error(transparent)]
    Source(#[from] ExpressionSourceError),
    #[error("expression facts belong to another semantic environment or revision")]
    WrongEnvironment,
    #[error("missing checked expression: {0:?}")]
    Missing(ExprId),
    #[error(transparent)]
    UnavailableIndex(#[from] crate::tir::static_index::UnavailableIndex),
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
        mut records: HashMap<ExprId, Box<CheckedExpressionRecord>>,
        cardinality: &AxisCardinality<'_>,
    ) -> Result<Self, ExpressionFactsError> {
        let mut inventory = HashMap::new();
        // Values still waiting on a binding, consulted by their parents while
        // this publication classifies each fact bottom-up.
        let mut waiting = HashSet::new();
        let mut classified = Vec::new();
        let mut error = Ok(());
        for root in roots {
            crate::hir::expr::visit_expr_postorder(root, &mut |expr| {
                if error.is_err() {
                    return;
                }
                error = (|| {
                    let id = expr.id().clone();
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
                    let value = record.fact.symbolic_value();
                    let compatible = match (&record.fact, &value, record.operation.as_ref()) {
                        (
                            ExpressionFact::Contextual(actual),
                            _,
                            ExpressionOperation::Contextual(expected),
                        ) => actual == expected,
                        (ExpressionFact::Contextual(_), _, _)
                        | (_, _, ExpressionOperation::Contextual(_))
                        | (_, None, _) => false,
                        (_, Some(value), operation) => {
                            let constructor_matches = matches!(
                                operation,
                                ExpressionOperation::Constructor
                                    | ExpressionOperation::Constant(ConstRef::Constructor(_))
                            ) == value.constructor.is_some();
                            let application_matches =
                                value.constructor.as_ref().is_none_or(|application| {
                                    match &value.checked_type {
                                        CheckedType::Struct(identity, args) => {
                                            identity.resolved() == &application.runtime_type
                                                && args == &application.generic_args
                                        }
                                        _ => false,
                                    }
                                });
                            constructor_matches && application_matches
                        }
                    };
                    if !compatible {
                        return Err(ExpressionFactsError::Incompatible(id));
                    }
                    if let Some(value) = value {
                        let ready = value_is_ready(record, &value, &waiting, cardinality)?;
                        let fact = classify(value.into_owned(), ready);
                        if !matches!(fact, ExpressionFact::Executable(_)) {
                            waiting.insert(id.clone());
                        }
                        classified.push((id.clone(), fact));
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
        for (id, fact) in classified {
            if let Some(record) = records.get_mut(&id) {
                record.fact = fact;
            }
        }
        Ok(Self {
            data: std::sync::Arc::new(PublishedExpressionFacts {
                owner,
                revision,
                records,
                source: ExpressionSourceMap::try_new(inventory)?,
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
        match record.fact {
            ExpressionFact::Executable(_) => Ok(record),
            ExpressionFact::Pending(_) | ExpressionFact::Symbolic(_) => {
                Err(ExpressionFactsError::Deferred(id.clone()))
            }
            ExpressionFact::Contextual(_) => Err(ExpressionFactsError::Contextual(id.clone())),
        }
    }
}
