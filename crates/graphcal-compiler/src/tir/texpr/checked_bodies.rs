//! The checked typed tree of every expression root of one DAG body.
//!
//! A root is published [`Executable`](CheckedBody::Executable) when every
//! node of its tree has a concrete type whose axes all have a known
//! cardinality and every static position it relies on is proven in range;
//! each node that introduces keys or entries of an axis is then given that
//! concrete axis. Evaluation reads only such trees. Otherwise the root is
//! [`Deferred`](CheckedBody::Deferred): its symbolic tree awaits the Static or
//! generic bindings that specialization supplies.

use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use thiserror::Error;

use crate::expression_id::ExprId;
use crate::hir::expr::Expr;
use crate::semantic::checked_type::{CheckedType, Concrete, IndexTypeRef, Symbolic};
use crate::semantic::index_axis::IndexAxis;
use crate::semantic::index_def::IndexCardinality;
use crate::semantic::key_value::KeyValue;
use crate::syntax::index_name::IndexEntryKey;
use crate::syntax::span::Span;
use crate::tir::static_index::{
    AxisDefinition, Readiness, StaticIndexError, UnavailableIndex, check_static_position,
};

use super::assembly::PendingNodes;
use super::call_targets::{CallSlot, CallTargets};
use super::map::{KeyEntry, TypeMap};
use super::model::{StaticPosition, TArg, TBody, TContextual, TExpr, TNodeRef};
use super::nominal::NominalObservation;
use super::nominal::{ConstructorApplication, ConstructorMatch};

/// Why the typed trees of one checking pass cannot be published.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TypedBodiesError {
    #[error("checked root {0:?} has no typed tree")]
    MissingRoot(ExprId),
    #[error("typed node {0:?} belongs to no checked root")]
    Unclaimed(ExprId),
}

/// Why a checked tree cannot be classified.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DischargeError {
    #[error(transparent)]
    UnavailableIndex(#[from] UnavailableIndex),
    #[error(transparent)]
    StaticIndex(#[from] StaticIndexError),
    #[error("a typed node at {0:?} has no axis where its form relies on one")]
    AxisShape(Span),
    #[error("checked expression relies on an axis with a non-finite coordinate: {0}")]
    NonFiniteAxis(Box<IndexTypeRef<Concrete>>),
    #[error("a constant key at {span:?} names no entry of its axis `{axis}`")]
    KeyOutsideAxis {
        axis: Box<IndexTypeRef<Concrete>>,
        span: Span,
    },
}

/// Why a root cannot be evaluated.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExecutableBodyError {
    #[error("missing checked expression: {0:?}")]
    Missing(ExprId),
    #[error("expression has undischarged static obligations: {0:?}")]
    Deferred(ExprId),
    #[error("contextual metadata is not an executable value: {0:?}")]
    Contextual(ExprId),
}

/// The checked tree of one root.
#[derive(Debug, Clone)]
pub enum CheckedBody {
    /// Every obligation of the tree is discharged; evaluation may run it.
    Executable(TBody<Concrete>),
    /// The tree still awaits Static or generic bindings.
    Deferred(TBody<Symbolic>),
}

impl CheckedBody {
    /// Classify a symbolic tree by what evaluation may rely on.
    ///
    /// Every node is inspected, children before their parent, so an
    /// unavailable index or an out-of-range static position is always
    /// reported, even below a node that is already known to wait.
    ///
    /// # Errors
    ///
    /// Returns a [`DischargeError`] for an unavailable index, a static
    /// position outside its now-known axis, or an axis a node relies on that
    /// cannot be given to it.
    pub(crate) fn discharge(
        body: TBody<Symbolic>,
        definition: &AxisDefinition<'_>,
    ) -> Result<Self, DischargeError> {
        Self::discharge_with(body, definition, &mut ToConcrete::new(definition))
    }

    fn discharge_with(
        body: TBody<Symbolic>,
        definition: &AxisDefinition<'_>,
        concrete: &mut ToConcrete<'_, '_>,
    ) -> Result<Self, DischargeError> {
        let expr = match body {
            TBody::Contextual(literal) => return Ok(Self::Executable(TBody::Contextual(literal))),
            TBody::Value(expr) => expr,
        };
        if !ready(&expr, &|index| {
            Ok(definition(index)?.map(|kind| kind.cardinality()))
        })? {
            return Ok(Self::Deferred(TBody::Value(expr)));
        }
        match expr.map_types(concrete) {
            Ok(concrete) => Ok(Self::Executable(TBody::Value(Box::new(concrete)))),
            Err(NotConcrete::Waiting) => Ok(Self::Deferred(TBody::Value(expr))),
            Err(NotConcrete::Discharge(error)) => Err(error),
        }
    }
}

/// The cardinality of an axis, or `None` while it awaits a binding.
type AxisCardinality<'a> =
    dyn Fn(&IndexTypeRef<Symbolic>) -> Result<Option<IndexCardinality>, UnavailableIndex> + 'a;

/// Why a symbolic tree has no concrete form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotConcrete {
    /// Some of its types still mention a `Nat` variable, or an axis it relies
    /// on still awaits its binding: the tree waits.
    Waiting,
    /// An axis it relies on cannot be given to it.
    Discharge(DischargeError),
}

impl From<UnavailableIndex> for NotConcrete {
    fn from(error: UnavailableIndex) -> Self {
        Self::Discharge(error.into())
    }
}

/// Close every type of a symbolic tree that mentions no `Nat` variable, and
/// give each node the concrete axes and keys it relies on.
///
/// Each axis is built once per tree from its definition.
pub struct ToConcrete<'d, 'a> {
    definition: &'d AxisDefinition<'a>,
    axes: HashMap<IndexTypeRef<Concrete>, IndexAxis>,
}

impl<'d, 'a> ToConcrete<'d, 'a> {
    /// Close trees whose axes are defined by `definition`.
    pub(crate) fn new(definition: &'d AxisDefinition<'a>) -> Self {
        Self {
            definition,
            axes: HashMap::new(),
        }
    }

    /// The concrete axis of `index`, a concrete index of a node's type.
    fn concrete_axis(
        &mut self,
        index: Option<&IndexTypeRef<Concrete>>,
        span: Span,
    ) -> Result<IndexAxis, NotConcrete> {
        let index = index.ok_or(NotConcrete::Discharge(DischargeError::AxisShape(span)))?;
        if let Some(axis) = self.axes.get(index) {
            return Ok(axis.clone());
        }
        let kind = (self.definition)(&index.to_symbolic())?.ok_or(NotConcrete::Waiting)?;
        let axis = IndexAxis::from_concrete(index.clone(), kind.into_owned()).ok_or_else(|| {
            NotConcrete::Discharge(DischargeError::NonFiniteAxis(Box::new(index.clone())))
        })?;
        self.axes.insert(index.clone(), axis.clone());
        Ok(axis)
    }
}

impl TypeMap<Symbolic, Concrete> for ToConcrete<'_, '_> {
    type Error = NotConcrete;

    fn node_type(
        &mut self,
        ty: &CheckedType<Symbolic>,
        _span: Span,
    ) -> Result<CheckedType<Concrete>, NotConcrete> {
        ty.to_concrete().ok_or(NotConcrete::Waiting)
    }

    fn application(
        &mut self,
        application: &ConstructorApplication<Symbolic>,
        _ty: &CheckedType<Concrete>,
        _span: Span,
    ) -> Result<ConstructorApplication<Concrete>, NotConcrete> {
        let applied = &application.applied;
        let generic_args = applied
            .generic_args()
            .iter()
            .map(crate::semantic::checked_type::CheckedGenericArg::to_concrete)
            .collect::<Option<_>>()
            .ok_or(NotConcrete::Waiting)?;
        Ok(ConstructorApplication {
            constructor: application.constructor.clone(),
            applied: std::sync::Arc::new(applied.try_map_types(
                applied.runtime_type().clone(),
                generic_args,
                |ty| ty.to_concrete().ok_or(NotConcrete::Waiting),
            )?),
        })
    }

    fn static_position(
        &mut self,
        position: &StaticPosition<Symbolic>,
        _span: Span,
    ) -> Result<StaticPosition<Concrete>, NotConcrete> {
        Ok(StaticPosition {
            axis: position.axis.to_concrete().ok_or(NotConcrete::Waiting)?,
            position: position.position,
            usage: position.usage,
        })
    }

    fn axis(
        &mut self,
        (): &(),
        index: Option<&IndexTypeRef<Concrete>>,
        span: Span,
    ) -> Result<IndexAxis, NotConcrete> {
        self.concrete_axis(index, span)
    }

    fn key(
        &mut self,
        (): &(),
        index: Option<&IndexTypeRef<Concrete>>,
        entry: KeyEntry<'_>,
        span: Span,
    ) -> Result<KeyValue, NotConcrete> {
        let axis = self.concrete_axis(index, span)?;
        let index = axis.index().clone();
        match entry {
            KeyEntry::Position(position) => usize::try_from(position)
                .ok()
                .and_then(|position| KeyValue::at(axis, position)),
            KeyEntry::Label(label) => {
                KeyValue::for_entry(axis, &IndexEntryKey::named(label.clone()))
            }
        }
        .ok_or_else(|| {
            NotConcrete::Discharge(DischargeError::KeyOutsideAxis {
                axis: Box::new(index),
                span,
            })
        })
    }

    fn match_target(&mut self, target: &ConstructorMatch) -> ConstructorMatch {
        target.clone()
    }

    fn call_slot(&mut self, slot: CallSlot) -> CallSlot {
        slot
    }
}

/// Whether every node of `expr` has known axis cardinalities and discharged
/// static positions.
fn ready(
    expr: &TExpr<Symbolic>,
    cardinality: &AxisCardinality<'_>,
) -> Result<bool, DischargeError> {
    crate::stack::with_stack_growth(|| {
        let mut children = true;
        let mut failure = None;
        expr.visit_children(&mut |child| {
            if failure.is_some() {
                return;
            }
            if let TNodeRef::Value(child) = child {
                match ready(child, cardinality) {
                    Ok(ready) => children &= ready,
                    Err(error) => failure = Some(error),
                }
            }
        });
        if let Some(error) = failure {
            return Err(error);
        }
        let mut own = cardinalities_known(expr.ty(), cardinality)?;
        for position in static_positions(expr) {
            own &= check_static_position(
                position.position,
                position.usage,
                cardinality(&position.axis)?,
            )? == Readiness::Ready;
        }
        Ok(own && children)
    })
}

/// Whether every index a type mentions has a known cardinality.
///
/// Every index is queried, even after an unknown one, so an unavailable index
/// definition is always reported.
fn cardinalities_known(
    ty: &CheckedType<Symbolic>,
    cardinality: &AxisCardinality<'_>,
) -> Result<bool, UnavailableIndex> {
    ty.indexes().into_iter().try_fold(true, |known, index| {
        Ok(known & cardinality(index)?.is_some())
    })
}

/// The static positions a node proves, in selector order.
fn static_positions<V: crate::semantic::checked_type::Concreteness>(
    expr: &TExpr<V>,
) -> Vec<&StaticPosition<V>> {
    match expr.kind() {
        super::model::TExprKind::Index { args, .. } => args
            .iter()
            .filter_map(|arg| match arg {
                super::model::TIndexArg::Position { position, .. } => Some(position),
                super::model::TIndexArg::Key(_)
                | super::model::TIndexArg::Variant(_)
                | super::model::TIndexArg::Var(_) => None,
            })
            .collect(),
        super::model::TExprKind::Key {
            form: super::model::TKeyForm::Static { position, .. },
            ..
        } => vec![position],
        _ => Vec::new(),
    }
}

/// The checked tree of each expression root of one body, keyed by the root's
/// occurrence and kept in publication order.
///
/// Each root keeps the nominal uses checking observed in it, and every call
/// node of these trees is numbered by one shared table of call targets.
/// Clones share one immutable publication.
#[derive(Debug, Clone)]
pub struct CheckedBodies {
    roots: Arc<IndexMap<ExprId, CheckedBody>>,
    nominal_uses: Arc<HashMap<ExprId, Arc<[NominalObservation]>>>,
    calls: Arc<CallTargets>,
}

impl CheckedBodies {
    /// Classify the typed tree of each claimed root, in order. Nominal uses
    /// are kept for the published roots only.
    ///
    /// # Errors
    ///
    /// Returns the first [`DischargeError`] in root order.
    pub(crate) fn discharge(
        claimed: ClaimedRoots,
        mut nominal_uses: HashMap<ExprId, Arc<[NominalObservation]>>,
        definition: &AxisDefinition<'_>,
    ) -> Result<Self, DischargeError> {
        let ClaimedRoots { roots, calls } = claimed;
        let mut published = IndexMap::with_capacity(roots.len());
        let mut concrete = ToConcrete::new(definition);
        for (id, body) in roots {
            let body = CheckedBody::discharge_with(body, definition, &mut concrete)?;
            published.insert(id, body);
        }
        nominal_uses.retain(|root, _| published.contains_key(root));
        Ok(Self {
            roots: Arc::new(published),
            nominal_uses: Arc::new(nominal_uses),
            calls: Arc::new(calls),
        })
    }

    /// The DAGs the call nodes of these trees target, by slot.
    #[must_use]
    pub fn calls(&self) -> &CallTargets {
        &self.calls
    }

    /// Whether these are the trees of exactly `roots`.
    pub fn cover<'a>(&self, roots: impl IntoIterator<Item = &'a Expr>) -> bool {
        let roots = roots
            .into_iter()
            .map(Expr::id)
            .collect::<std::collections::HashSet<_>>();
        roots.len() == self.roots.len() && roots.into_iter().all(|id| self.roots.contains_key(id))
    }

    /// The nominal uses checking observed in one root, in inference order.
    #[must_use]
    pub fn nominal_uses(&self, root: &ExprId) -> &[NominalObservation] {
        self.nominal_uses.get(root).map_or(&[], AsRef::as_ref)
    }

    /// The shared nominal uses of one root, to publish with a tree derived
    /// from it.
    pub(crate) fn shared_nominal_uses(&self, root: &ExprId) -> Option<Arc<[NominalObservation]>> {
        self.nominal_uses.get(root).cloned()
    }

    /// The checked tree of one root.
    #[must_use]
    pub fn get(&self, root: &ExprId) -> Option<&CheckedBody> {
        self.roots.get(root)
    }

    /// Every root's checked tree, in publication order.
    pub fn roots(&self) -> impl Iterator<Item = (&ExprId, &CheckedBody)> {
        self.roots.iter()
    }

    /// The executable tree of a value root.
    ///
    /// # Errors
    ///
    /// Returns an [`ExecutableBodyError`] when the root is unknown, still
    /// awaits bindings, or is a contextual literal.
    pub fn executable_value(&self, root: &ExprId) -> Result<&TExpr<Concrete>, ExecutableBodyError> {
        match self.roots.get(root) {
            Some(CheckedBody::Executable(TBody::Value(expr))) => Ok(expr),
            Some(CheckedBody::Executable(TBody::Contextual(_))) => {
                Err(ExecutableBodyError::Contextual(root.clone()))
            }
            Some(CheckedBody::Deferred(_)) => Err(ExecutableBodyError::Deferred(root.clone())),
            None => Err(ExecutableBodyError::Missing(root.clone())),
        }
    }

    /// The contextual literal a root consists of, when it is one.
    #[must_use]
    pub fn contextual(&self, root: &ExprId) -> Option<&TContextual> {
        match self.roots.get(root)? {
            CheckedBody::Executable(TBody::Contextual(literal)) => Some(literal),
            CheckedBody::Executable(TBody::Value(_)) | CheckedBody::Deferred(_) => None,
        }
    }
}

/// The typed tree of each claimed root, in root order, with the call targets
/// their call nodes are numbered by.
#[derive(Debug)]
pub struct ClaimedRoots {
    pub(crate) roots: Vec<(ExprId, TBody<Symbolic>)>,
    pub(crate) calls: CallTargets,
}

/// Claim the typed tree of every root, in root order; every recorded node
/// must belong to one.
///
/// # Errors
///
/// Returns a [`TypedBodiesError`] for a root without a tree or a node that
/// no root claims.
pub fn claim_roots(
    roots: &[&Expr],
    mut pending: PendingNodes,
) -> Result<ClaimedRoots, TypedBodiesError> {
    let mut claimed = Vec::with_capacity(roots.len());
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        let id = root.id();
        if !seen.insert(id) {
            continue;
        }
        let body = match pending.take_root(id) {
            Some(TArg::Value(value)) => TBody::Value(value),
            Some(TArg::Contextual(literal)) => TBody::Contextual(literal),
            None => return Err(TypedBodiesError::MissingRoot(id.clone())),
        };
        claimed.push((id.clone(), body));
    }
    if let Some(unclaimed) = pending.unclaimed() {
        return Err(TypedBodiesError::Unclaimed(unclaimed.clone()));
    }
    Ok(ClaimedRoots {
        roots: claimed,
        calls: pending.into_calls(),
    })
}

impl CheckedBodies {
    /// Every concrete constructor application these trees make, in root and
    /// pre-order: each of an executable tree, and each concretely typed one
    /// of a tree that still awaits bindings (only such an application is one
    /// a value can have).
    #[must_use]
    pub fn concrete_applications(
        &self,
    ) -> Vec<(
        &crate::resolved_name::ResolvedStructTypeName,
        Vec<crate::semantic::checked_type::CheckedGenericArg>,
    )> {
        let mut applications = Vec::new();
        for body in self.roots.values() {
            match body {
                CheckedBody::Executable(body) => {
                    super::model::visit_tnodes(body.as_node(), &mut |node| {
                        if let TNodeRef::Value(expr) = node
                            && let Some(application) = expr.application()
                        {
                            applications.push((
                                application.definition(),
                                application.generic_args().to_vec(),
                            ));
                        }
                    });
                }
                CheckedBody::Deferred(body) => {
                    super::model::visit_tnodes(body.as_node(), &mut |node| {
                        let TNodeRef::Value(expr) = node else {
                            return;
                        };
                        let Some(application) = expr.application() else {
                            return;
                        };
                        let generic_args = application
                            .generic_args()
                            .iter()
                            .map(crate::semantic::checked_type::CheckedGenericArg::to_concrete)
                            .collect::<Option<Vec<_>>>();
                        if let (Some(_), Some(generic_args)) =
                            (expr.ty().to_concrete(), generic_args)
                        {
                            applications.push((application.definition(), generic_args));
                        }
                    });
                }
            }
        }
        applications
    }
}
