//! Expression-fact and nominal-dependency recording for HIR inference.

use crate::hir::expr::{ConstRef, Expr, ExprKind, MatchPattern, visit_expr};
use crate::hir::nominal::NominalGenericParam;
use crate::resolved_name::{ResolvedConstructorName, ResolvedStructTypeName};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use miette::NamedSource;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::expression_id::ExprId;
use crate::registry::error::GraphcalError;
use crate::syntax::span::Span;
use crate::tir::expression_facts::{
    CheckedExpressionRecord, ConstructorApplication, ContextualOperand, ExpressionFact,
    NominalObservation,
};

use crate::tir::dim_check::{DeclaredType, InferredType};

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

#[derive(Clone, Default)]
pub(super) struct TypeDefinitionDependencyCollector {
    dependencies: Rc<RefCell<Vec<TypeDefinitionDependency>>>,
}

impl TypeDefinitionDependencyCollector {
    fn record(&self, identity: &ResolvedStructTypeName, span: Span) {
        self.dependencies
            .borrow_mut()
            .push(TypeDefinitionDependency {
                identity: identity.clone(),
                span,
            });
    }

    pub(super) fn snapshot(&self) -> Vec<TypeDefinitionDependency> {
        self.dependencies.borrow().clone()
    }
}

#[derive(Clone, Default)]
pub(super) enum TypeDefinitionDependencyTracking {
    #[default]
    Disabled,
    Collect(TypeDefinitionDependencyCollector),
}

impl TypeDefinitionDependencyTracking {
    pub(super) fn record(&self, identity: &ResolvedStructTypeName, span: Span) {
        match self {
            Self::Disabled => {}
            Self::Collect(collector) => collector.record(identity, span),
        }
    }
}

#[cfg(test)]
thread_local! {
    pub(in crate::tir::dim_check) static CONTEXTUAL_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Clone)]
pub(in crate::tir::dim_check) struct ExpressionFactCollector {
    environment: Arc<crate::tir::expression_facts::CheckingEnvironment>,
    records: Rc<RefCell<HashMap<ExprId, Box<CheckedExpressionRecord>>>>,
    observations: Rc<RefCell<HashMap<ExprId, Vec<NominalObservation>>>>,
    pub(super) static_indexes:
        Rc<RefCell<HashMap<ExprId, Vec<crate::tir::expression_facts::StaticIndexRequirement>>>>,
}

impl ExpressionFactCollector {
    pub(in crate::tir::dim_check) fn new(dag: &crate::tir::typed::DagTIR) -> Self {
        Self {
            environment: crate::tir::expression_facts::CheckingEnvironment::new(
                dag.dag_id().clone(),
                dag.body_revision().clone(),
            ),
            records: Rc::default(),
            observations: Rc::default(),
            static_indexes: Rc::default(),
        }
    }

    pub(in crate::tir::dim_check) fn finish(self) -> HashMap<ExprId, Box<CheckedExpressionRecord>> {
        let mut records = self.records.take();
        for (id, observations) in self.observations.take() {
            if let Some(record) = records.get_mut(&id) {
                record.nominal_observations = Some(observations.into());
            }
        }
        records
    }

    pub(super) fn observe(&self, root: &ExprId, observation: NominalObservation) {
        self.observations
            .borrow_mut()
            .entry(root.clone())
            .or_default()
            .push(observation);
    }

    pub(in crate::tir::dim_check) fn retain_nat_scope(
        &self,
        root: &Expr,
        parameters: &[NominalGenericParam],
    ) {
        let scope: HashMap<_, _> = parameters
            .iter()
            .map(|parameter| (parameter.name().clone(), parameter.id().clone()))
            .collect();
        let scope = (!scope.is_empty()).then(|| Arc::new(scope));
        visit_expr(root, &mut |expr| {
            if let Some(record) = self.records.borrow_mut().get_mut(expr.id()) {
                record.nat_parameters.clone_from(&scope);
            }
        });
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
        let diagnostic = |error: String| {
            GraphcalError::internal_error(error, src, DiagnosticAnchor::Source(expr.span))
        };
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
            std::collections::hash_map::Entry::Occupied(entry) if entry.get() == &record => Ok(()),
            std::collections::hash_map::Entry::Occupied(_) => Err(diagnostic(
                "expression inferred with inconsistent facts".into(),
            )),
        }
    }

    pub(in crate::tir::dim_check) fn record_contextual(
        &self,
        root: &Expr,
        src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        let mut result = Ok(());
        visit_expr(root, &mut |expr| {
            #[cfg(test)]
            CONTEXTUAL_VISITS.with(|visits| {
                visits.set(
                    visits
                        .get()
                        .checked_add(1)
                        .expect("contextual visit counter overflow"),
                );
            });
            if result.is_err() || self.records.borrow().contains_key(expr.id()) {
                return;
            }
            let kind = match expr.kind() {
                ExprKind::StringLiteral(_) => ContextualOperand::String,
                ExprKind::OffsetDateTimeLiteral(_) => ContextualOperand::OffsetDateTime,
                ExprKind::CivilDateTimeLiteral(_) => ContextualOperand::CivilDateTime,
                ExprKind::ZonedDateTimeLiteral(_) => ContextualOperand::ZonedDateTime,
                ExprKind::IanaTimeZoneLiteral(_) => ContextualOperand::TimeZone,
                ExprKind::TypeSystemRef(_) => ContextualOperand::TypeSystem,
                _ => return,
            };
            result = self.insert(expr, ExpressionFact::Contextual(kind), HashMap::new(), src);
        });
        result
    }

    pub(in crate::tir::dim_check) fn record(
        &self,
        expr: &Expr,
        inferred: &InferredType,
        dag: &crate::tir::typed::DagTIR,
        tir: &crate::tir::typed::TIR,
        src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        let checked_type = DeclaredType::from(inferred);
        let shape = crate::tir::dim_check::expression_axes::checked_expression_shape(
            &checked_type,
            tir,
            src,
            expr.span,
        )?;
        let constructor = match expr.kind() {
            ExprKind::ConstructorCall { callee, .. } => Some(&callee.value),
            ExprKind::ConstRef(target) => match &target.value {
                ConstRef::Constructor(name) => Some(name),
                _ => None,
            },
            _ => None,
        }
        .map(|name| {
            let target = dag
                .semantic
                .constructor_refs
                .constructor_defs
                .get(name)
                .ok_or_else(|| {
                    GraphcalError::internal_error(
                        format!("checked constructor `{name}` has no definition"),
                        src,
                        DiagnosticAnchor::Source(expr.span),
                    )
                })?;
            let DeclaredType::Struct(_, args) = &checked_type else {
                return Err(GraphcalError::internal_error(
                    "constructor inferred a non-nominal type",
                    src,
                    DiagnosticAnchor::Source(expr.span),
                ));
            };
            Ok(ConstructorApplication {
                definition: target.owning_type.clone(),
                runtime_type: dag.runtime_struct_type_identity(&target.owning_type),
                constructor: target.variant.name(),
                generic_args: args.clone(),
                required_constraints: target
                    .variant
                    .fields()
                    .iter()
                    .filter(|field| !field.type_annotation().domain_bounds.is_empty())
                    .map(|field| field.name().clone())
                    .collect(),
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
                    let target = dag
                        .semantic
                        .constructor_refs
                        .constructor_defs
                        .get(name)
                        .ok_or_else(|| {
                            GraphcalError::internal_error(
                                format!("checked match constructor `{name}` has no definition"),
                                src,
                                DiagnosticAnchor::Source(expr.span),
                            )
                        })?;
                    Ok((
                        name.clone(),
                        crate::tir::expression_facts::ConstructorMatch {
                            definition: target.owning_type.clone(),
                            runtime_type: dag.runtime_struct_type_identity(&target.owning_type),
                            constructor: target.variant.name(),
                        },
                    ))
                })
                .collect::<Result<_, GraphcalError>>()?,
            _ => HashMap::new(),
        };
        self.insert(
            expr,
            ExpressionFact::Value {
                checked_type,
                shape,
                constructor: constructor.map(Box::new),
            },
            constructor_matches,
            src,
        )
    }
}
