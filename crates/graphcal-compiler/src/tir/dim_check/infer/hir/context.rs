//! The inference environment, operation-scoped control state, and inference position.

use crate::hir::expr::{ConstRef, Expr, ExprKind, LocalEnv, MatchPattern, visit_expr};
use crate::resolved_name::ResolvedDeclName;
use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::expression_id::ExprId;
use crate::registry::declared_type::IndexTypeRef;
use crate::registry::error::GraphcalError;
use crate::registry::types::FormattingRegistry;
use crate::syntax::module_name::ScopedName;

use crate::tir::dim_check::{DeclaredType, InferredType};

use super::facts::{
    ExpressionFactCollector, TypeDefinitionDependency, TypeDefinitionDependencyCollector,
    TypeDefinitionDependencyTracking,
};

/// Read-only inputs every inference rule consults while checking one DAG body.
#[derive(Clone, Copy)]
pub(in crate::tir::dim_check) struct InferEnv<'a> {
    pub(in crate::tir::dim_check) declared_types: &'a HashMap<ScopedName, DeclaredType>,
    pub(in crate::tir::dim_check) dag: &'a crate::tir::typed::DagTIR,
    pub(in crate::tir::dim_check) tir: &'a crate::tir::typed::TIR,
    pub(in crate::tir::dim_check) registry: &'a FormattingRegistry,
    pub(in crate::tir::dim_check) src: &'a NamedSource<Arc<String>>,
}

impl<'a> InferEnv<'a> {
    /// Resolve a canonical constructor to its owning nominal definition.
    pub(in crate::tir::dim_check) fn resolved_constructor(
        self,
        constructor: &crate::resolved_name::ResolvedConstructorName,
        span: crate::syntax::span::Span,
    ) -> Result<&'a crate::hir::nominal::ResolvedConstructor, GraphcalError> {
        self.tir
            .project_type_store()
            .lookup_constructor(constructor)
            .ok_or_else(|| {
                GraphcalError::internal_error(
                    format!("project type store has no constructor `{constructor}`"),
                    self.src,
                    crate::diagnostic_anchor::DiagnosticAnchor::Source(span),
                )
            })
    }

    /// Infer an expression's type, recording checked facts for every visited node.
    pub(in crate::tir::dim_check) fn infer_with_expression_facts(
        self,
        expr: &Expr,
        owner: Option<&ResolvedDeclName>,
        cancellation: &crate::cancellation::CancellationToken,
        collector: ExpressionFactCollector,
    ) -> Result<InferredType, GraphcalError> {
        let control = InferenceControl {
            cancellation: cancellation.clone(),
            type_definition_dependencies: TypeDefinitionDependencyTracking::Disabled,
            expression_facts: Some((collector, expr.id().clone())),
        };
        Infer::root(self, owner, &control).infer_hir_type(expr)
    }

    /// Collect uses that inspect nominal type definitions while inferring a whole body.
    ///
    /// Running through the ordinary inference recursion preserves every lexical
    /// local environment instead of speculatively inferring detached subexpressions.
    /// Bodies without a definition-observing operation need no inference; this is
    /// important for context-typed leaves such as plot label strings.
    pub(in crate::tir::dim_check) fn collect_type_definition_dependencies(
        self,
        expr: &Expr,
        owner: Option<&ResolvedDeclName>,
        cancellation: &crate::cancellation::CancellationToken,
    ) -> Result<Vec<TypeDefinitionDependency>, GraphcalError> {
        if !contains_type_definition_observation(expr) {
            return Ok(Vec::new());
        }
        let collector = TypeDefinitionDependencyCollector::default();
        let control = InferenceControl {
            cancellation: cancellation.clone(),
            type_definition_dependencies: TypeDefinitionDependencyTracking::Collect(
                collector.clone(),
            ),
            expression_facts: None,
        };
        Infer::root(self, owner, &control).infer_hir_type(expr)?;
        Ok(collector.snapshot())
    }
}

/// Operation-scoped policy shared by every recursive inference step.
///
/// Keeping cancellation, nominal-use tracking, and expression-fact recording in
/// one value that [`Infer`] always carries prevents nested helpers from
/// silently dropping any policy.
pub(super) struct InferenceControl {
    pub(super) cancellation: crate::cancellation::CancellationToken,
    pub(super) type_definition_dependencies: TypeDefinitionDependencyTracking,
    pub(super) expression_facts: Option<(ExpressionFactCollector, ExprId)>,
}

impl InferenceControl {
    pub(super) fn checkpoint(&self) -> Result<(), GraphcalError> {
        self.cancellation.checkpoint().map_err(GraphcalError::from)
    }

    pub(super) fn retain_static_index(
        &self,
        expr: &Expr,
        operand: &Expr,
        axis: &IndexTypeRef,
        position: u64,
        usage: crate::tir::expression_facts::StaticIndexUse,
    ) {
        if let Some((collector, _)) = &self.expression_facts {
            collector
                .static_indexes
                .borrow_mut()
                .entry(expr.id().clone())
                .or_default()
                .push(crate::tir::expression_facts::StaticIndexRequirement {
                    operand: operand.id().clone(),
                    axis: axis.clone(),
                    position,
                    usage,
                });
        }
    }
}

/// One inference position: the DAG environment, the declaration whose body is
/// being inferred (when override reconciliation applies), the lexical locals in
/// scope, and the operation-scoped control state.
#[derive(Clone, Copy)]
pub(super) struct Infer<'a> {
    pub(super) env: InferEnv<'a>,
    pub(super) owner: Option<&'a ResolvedDeclName>,
    pub(super) locals: &'a LocalEnv<'a, InferredType>,
    pub(super) control: &'a InferenceControl,
}

/// The empty lexical scope every inference operation starts from.
const ROOT_LOCALS: &LocalEnv<'static, InferredType> = &LocalEnv::root();

impl<'a> Infer<'a> {
    const fn root(
        env: InferEnv<'a>,
        owner: Option<&'a ResolvedDeclName>,
        control: &'a InferenceControl,
    ) -> Self {
        Self {
            env,
            owner,
            locals: ROOT_LOCALS,
            control,
        }
    }

    /// Continue inference inside a nested lexical scope.
    pub(super) const fn with_locals<'b>(self, locals: &'b LocalEnv<'b, InferredType>) -> Infer<'b>
    where
        'a: 'b,
    {
        Infer {
            env: self.env,
            owner: self.owner,
            locals,
            control: self.control,
        }
    }

    /// Infer a subexpression outside the owning declaration's override checks.
    pub(super) const fn without_owner(self) -> Self {
        Self {
            owner: None,
            ..self
        }
    }
}

fn contains_type_definition_observation(expr: &Expr) -> bool {
    let mut found = false;
    visit_expr(expr, &mut |candidate| {
        if found {
            return;
        }
        found = match candidate.kind() {
            ExprKind::FieldAccess { .. } | ExprKind::ConstructorCall { .. } => true,
            ExprKind::ConstRef(target) => {
                matches!(&target.value, ConstRef::Constructor(_))
            }
            ExprKind::Match { arms, .. } => arms
                .iter()
                .any(|arm| matches!(&arm.pattern, MatchPattern::Constructor { .. })),
            _ => false,
        };
    });
    found
}
