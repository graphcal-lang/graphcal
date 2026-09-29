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
    CheckedExpressionRecord, ConstructorApplication, ContextualOperand, ExpressionFact,
    NominalObservation, StaticIndexRequirement, ValueFact,
};

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
/// Every expression is recorded at most once: a second record for the same
/// expression is an internal error, because each body is inferred exactly once.
pub(in crate::tir::dim_check) struct BodyObservations {
    environment: Arc<crate::tir::expression_facts::CheckingEnvironment>,
    records: RefCell<HashMap<ExprId, Box<CheckedExpressionRecord>>>,
    nominal_uses: RefCell<HashMap<ExprId, Vec<NominalUse>>>,
    static_indexes: RefCell<HashMap<ExprId, Vec<StaticIndexRequirement>>>,
}

impl BodyObservations {
    pub(in crate::tir::dim_check) fn new(dag: &crate::tir::typed::DagTIR) -> Self {
        Self {
            environment: crate::tir::expression_facts::CheckingEnvironment::new(
                dag.dag_id().clone(),
                dag.body_revision().clone(),
            ),
            records: RefCell::default(),
            nominal_uses: RefCell::default(),
            static_indexes: RefCell::default(),
        }
    }

    /// The checked expression records, each root carrying its nominal uses.
    pub(in crate::tir::dim_check) fn finish(self) -> HashMap<ExprId, Box<CheckedExpressionRecord>> {
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
        records
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

    fn insert(
        &self,
        expr: &Expr,
        fact: ExpressionFact,
        constructor_matches: HashMap<
            ResolvedConstructorName,
            crate::tir::expression_facts::ConstructorMatch,
        >,
        src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        let id = expr.id().clone();
        let mut record = CheckedExpressionRecord::new(expr, fact, Arc::clone(&self.environment));
        record.constructor_matches = constructor_matches;
        record.static_indexes.extend(
            self.static_indexes
                .borrow_mut()
                .remove(&id)
                .into_iter()
                .flatten(),
        );
        match self.records.borrow_mut().entry(id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(record);
                Ok(())
            }
            std::collections::hash_map::Entry::Occupied(_) => Err(GraphcalError::internal_error(
                "expression checked more than once in one checking pass",
                src,
                DiagnosticAnchor::Source(expr.span),
            )),
        }
    }

    /// Record a contextual literal accepted by the construct that consumes it.
    ///
    /// Contextual literals (plot strings, datetime and timezone literals) are
    /// never values of their own; the consuming rule records them where it
    /// accepts them. Publication rejects an operand that does not match the
    /// literal's kind.
    pub(in crate::tir::dim_check) fn record_contextual(
        &self,
        expr: &Expr,
        operand: ContextualOperand,
        src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        self.insert(
            expr,
            ExpressionFact::Contextual(operand),
            HashMap::new(),
            src,
        )
    }

    pub(in crate::tir::dim_check) fn record(
        &self,
        expr: &Expr,
        inferred: &CheckedType<Symbolic>,
        dag: &crate::tir::typed::DagTIR,
        tir: &crate::tir::typed::TIR,
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
            ExpressionFact::Symbolic(ValueFact {
                checked_type,
                constructor: constructor.map(Box::new),
            }),
            constructor_matches,
            src,
        )
    }
}
