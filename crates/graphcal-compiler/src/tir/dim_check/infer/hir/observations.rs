//! The single observation sink of one body-checking pass.
//!
//! Inference records everything it learns about a body exactly once, while it
//! checks that body: each expression's checked fact, the static index proofs an
//! expression relies on, and the nominal uses a checked root makes. Consumers
//! (fact publication, template-closure validation, override-dependency
//! summaries) read these observations instead of inferring the body again.

use crate::hir::expr::{ConstRef, Expr, ExprKind, MatchPattern};
use crate::resolved_name::{ResolvedConstructorName, ResolvedStructTypeName};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::expression_id::ExprId;
use crate::registry::error::GraphcalError;
use crate::syntax::span::Span;
use crate::tir::expression_facts::{
    CheckedExpressionRecord, ConstructorApplication, ConstructorMatch, ExpressionFact,
    NominalObservation, StaticIndexRequirement, ValueFact,
};
use crate::tir::texpr::{AssemblyError, NodeFacts, PendingNodes};

use crate::registry::checked_type::{CheckedType, Symbolic};

/// One executable use that observes a nominal type's concrete definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tir::dim_check) struct TypeDefinitionDependency {
    identity: ResolvedStructTypeName,
    span: Span,
}

impl TypeDefinitionDependency {
    pub(in crate::tir::dim_check) const fn identity(&self) -> &ResolvedStructTypeName {
        &self.identity
    }

    pub(in crate::tir::dim_check) const fn span(&self) -> Span {
        self.span
    }
}

/// One nominal use observed while checking a root, with the source span of a
/// use that inspects the nominal type's concrete definition.
#[derive(Debug, Clone)]
struct NominalUse {
    observation: NominalObservation,
    definition_span: Option<Span>,
}

impl NominalUse {
    fn type_definition_dependency(&self) -> Option<TypeDefinitionDependency> {
        let span = self.definition_span?;
        match &self.observation {
            NominalObservation::Field { identity, .. }
            | NominalObservation::Constructor { identity, .. } => Some(TypeDefinitionDependency {
                identity: identity.clone(),
                span,
            }),
            NominalObservation::TypeArgument(_)
            | NominalObservation::IndexLabel { .. }
            | NominalObservation::IndexArgument(_) => None,
        }
    }
}

/// Everything one checking pass observed about the bodies of one DAG.
///
/// Every expression is recorded at most once, after its children: a second
/// record for the same expression is an internal error, because each body is
/// inferred exactly once. Each record is kept twice while the evaluator still
/// reads expression facts: as the retained fact, and as a node of the body's
/// typed tree ([`crate::tir::texpr`]).
pub(in crate::tir::dim_check) struct BodyObservations {
    environment: Arc<crate::tir::expression_facts::CheckingEnvironment>,
    records: RefCell<HashMap<ExprId, Box<CheckedExpressionRecord>>>,
    typed: RefCell<PendingNodes>,
    nominal_uses: RefCell<HashMap<ExprId, Vec<NominalUse>>>,
    static_indexes: RefCell<HashMap<ExprId, Vec<StaticIndexRequirement>>>,
}

/// The result of one checking pass: the retained facts and the typed trees.
pub(in crate::tir::dim_check) struct FinishedObservations {
    pub(in crate::tir::dim_check) records: HashMap<ExprId, Box<CheckedExpressionRecord>>,
    pub(in crate::tir::dim_check) typed: PendingNodes,
}

/// Why one checking pass's results cannot be published.
#[derive(Debug, thiserror::Error)]
pub(in crate::tir::dim_check) enum PublicationError {
    #[error(transparent)]
    Facts(#[from] crate::tir::expression_facts::ExpressionFactsError),
    #[error(transparent)]
    TypedBodies(#[from] crate::tir::texpr::TypedBodiesError),
    #[error(transparent)]
    Disagreement(#[from] crate::tir::texpr::fact_agreement::FactDisagreement),
}

impl FinishedObservations {
    /// Publish the facts and the typed trees of `roots`, requiring that both
    /// cover exactly these roots and agree on every expression.
    pub(in crate::tir::dim_check) fn publish(
        self,
        owner: crate::dag_id::DagId,
        revision: crate::body_revision::BodyRevision,
        roots: &[&Expr],
        cardinality: &crate::tir::expression_facts::AxisCardinality<'_>,
    ) -> Result<
        (
            crate::tir::expression_facts::CheckedExpressionFacts,
            crate::tir::texpr::TypedBodies,
        ),
        PublicationError,
    > {
        let facts = crate::tir::expression_facts::CheckedExpressionFacts::publish(
            owner,
            revision,
            roots,
            self.records,
            cardinality,
        )?;
        let typed = crate::tir::texpr::TypedBodies::publish(roots, self.typed)?;
        crate::tir::texpr::fact_agreement::check(&typed, &facts)?;
        Ok((facts, typed))
    }
}

impl BodyObservations {
    pub(in crate::tir::dim_check) fn new(dag: &crate::tir::typed::DagTIR) -> Self {
        Self {
            environment: crate::tir::expression_facts::CheckingEnvironment::new(
                dag.dag_id().clone(),
                dag.body_revision().clone(),
            ),
            records: RefCell::default(),
            typed: RefCell::default(),
            nominal_uses: RefCell::default(),
            static_indexes: RefCell::default(),
        }
    }

    /// The checked expression records, each root carrying its nominal uses,
    /// and the typed trees of the checked roots.
    pub(in crate::tir::dim_check) fn finish(self) -> FinishedObservations {
        let mut records = self.records.into_inner();
        for (id, uses) in self.nominal_uses.into_inner() {
            if let Some(record) = records.get_mut(&id) {
                record.nominal_observations = Some(
                    uses.into_iter()
                        .map(|nominal_use| nominal_use.observation)
                        .collect::<Vec<_>>()
                        .into(),
                );
            }
        }
        FinishedObservations {
            records,
            typed: self.typed.into_inner(),
        }
    }

    /// Record one nominal use made while checking `root`.
    pub(super) fn observe_nominal(
        &self,
        root: &ExprId,
        observation: NominalObservation,
        definition_span: Option<Span>,
    ) {
        self.nominal_uses
            .borrow_mut()
            .entry(root.clone())
            .or_default()
            .push(NominalUse {
                observation,
                definition_span,
            });
    }

    /// The uses of `root` that inspect a nominal type's concrete definition,
    /// in the order inference made them.
    pub(in crate::tir::dim_check) fn type_definition_dependencies(
        &self,
        root: &ExprId,
    ) -> Vec<TypeDefinitionDependency> {
        self.nominal_uses
            .borrow()
            .get(root)
            .into_iter()
            .flatten()
            .filter_map(NominalUse::type_definition_dependency)
            .collect()
    }

    /// Retain a static index proof `expr` relies on, attached when `expr` is recorded.
    pub(super) fn retain_static_index(&self, expr: &ExprId, requirement: StaticIndexRequirement) {
        self.static_indexes
            .borrow_mut()
            .entry(expr.clone())
            .or_default()
            .push(requirement);
    }

    /// Record one checked expression: its retained fact and its typed node.
    fn insert(
        &self,
        expr: &Expr,
        node: CheckedNode,
        src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        self.try_insert(expr, node).map_err(|error| {
            GraphcalError::internal_error(
                error.to_string(),
                src,
                DiagnosticAnchor::Source(expr.span),
            )
        })
    }

    fn try_insert(&self, expr: &Expr, node: CheckedNode) -> Result<(), AssemblyError> {
        let id = expr.id();
        if self.records.borrow().contains_key(id) {
            return Err(AssemblyError::CheckedTwice(id.clone()));
        }
        let static_indexes = self
            .static_indexes
            .borrow_mut()
            .remove(id)
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let (fact, constructor_matches) = match node {
            CheckedNode::Contextual => {
                let operand = self.typed.borrow_mut().record_contextual(expr)?;
                (ExpressionFact::Contextual(operand), HashMap::new())
            }
            CheckedNode::Value {
                value,
                constructor_matches,
            } => {
                self.typed.borrow_mut().record_value(
                    expr,
                    value.checked_type.clone(),
                    &NodeFacts {
                        constructor: value.constructor.as_deref(),
                        constructor_matches: &constructor_matches,
                        static_indexes: &static_indexes,
                    },
                )?;
                (ExpressionFact::Symbolic(value), constructor_matches)
            }
        };
        let mut record = CheckedExpressionRecord::new(expr, fact, Arc::clone(&self.environment));
        record.constructor_matches = constructor_matches;
        record.static_indexes = static_indexes;
        self.records.borrow_mut().insert(id.clone(), record);
        Ok(())
    }

    /// Record a contextual literal accepted by the construct that consumes it.
    ///
    /// Contextual literals (plot strings, datetime and timezone literals) are
    /// never values of their own; the consuming rule records them where it
    /// accepts them.
    pub(in crate::tir::dim_check) fn record_contextual(
        &self,
        expr: &Expr,
        src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        self.insert(expr, CheckedNode::Contextual, src)
    }

    pub(in crate::tir::dim_check) fn record(
        &self,
        expr: &Expr,
        inferred: &CheckedType<Symbolic>,
        dag: &crate::tir::typed::DagTIR,
        tir: &crate::tir::typed::UncheckedTir,
        src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        let checked_type = inferred.clone();
        crate::tir::dim_check::expression_axes::check_materializable(
            &checked_type,
            tir,
            src,
            expr.span,
        )?;
        let resolved_constructor = |name| {
            tir.project_type_store()
                .lookup_constructor(name)
                .ok_or_else(|| {
                    GraphcalError::internal_error(
                        format!("checked constructor `{name}` has no definition"),
                        src,
                        DiagnosticAnchor::Source(expr.span),
                    )
                })
        };
        let constructor = match expr.kind() {
            ExprKind::ConstructorCall { callee, .. } => Some(&callee.value),
            ExprKind::ConstRef(target) => match &target.value {
                ConstRef::Constructor(name) => Some(name),
                _ => None,
            },
            _ => None,
        }
        .map(|name| {
            let target = resolved_constructor(name)?;
            let CheckedType::Struct(_, args) = &checked_type else {
                return Err(GraphcalError::internal_error(
                    "constructor inferred a non-nominal type",
                    src,
                    DiagnosticAnchor::Source(expr.span),
                ));
            };
            Ok(ConstructorApplication {
                runtime_type: dag.runtime_struct_type_identity(target.owning_type()),
                constructor: target.clone(),
                generic_args: args.clone(),
            })
        })
        .transpose()?;
        let constructor_matches = match expr.kind() {
            ExprKind::Match { arms, .. } => arms
                .iter()
                .filter_map(|arm| match &arm.pattern {
                    MatchPattern::Constructor { constructor, .. } => Some(&constructor.value),
                    MatchPattern::IndexLabel { .. } => None,
                })
                .map(|name| {
                    let target = resolved_constructor(name)?;
                    Ok((
                        name.clone(),
                        crate::tir::expression_facts::ConstructorMatch {
                            definition: target.owning_type().clone(),
                            runtime_type: dag.runtime_struct_type_identity(target.owning_type()),
                            constructor: target.name(),
                        },
                    ))
                })
                .collect::<Result<_, GraphcalError>>()?,
            _ => HashMap::new(),
        };
        self.insert(
            expr,
            CheckedNode::Value {
                value: ValueFact {
                    checked_type,
                    constructor: constructor.map(Box::new),
                },
                constructor_matches,
            },
            src,
        )
    }
}

/// What checking established about one expression, before it is recorded.
enum CheckedNode {
    Contextual,
    Value {
        value: ValueFact<Symbolic>,
        constructor_matches: HashMap<ResolvedConstructorName, ConstructorMatch>,
    },
}
