//! The inference environment, operation-scoped control state, and inference position.

use crate::hir::expr::{Expr, LocalEnv};
use crate::resolved_name::ResolvedDeclName;
use std::sync::Arc;

use miette::NamedSource;

use crate::expression_id::ExprId;
use crate::registry::checked_type::{IndexTypeRef, Symbolic};
use crate::registry::error::GraphcalError;
use crate::registry::types::FormattingRegistry;

use crate::registry::checked_type::CheckedType;

use super::observations::BodyObservations;

/// Read-only inputs every inference rule consults while checking one DAG body.
#[derive(Clone, Copy)]
pub(in crate::tir::dim_check) struct InferEnv<'a> {
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

    /// Infer the type of the checked root `expr`, recording every
    /// observation of its subtree in `observations`.
    pub(in crate::tir::dim_check) fn infer_root(
        self,
        expr: &Expr,
        owner: Option<&ResolvedDeclName>,
        cancellation: &crate::cancellation::CancellationToken,
        observations: &BodyObservations,
    ) -> Result<CheckedType<Symbolic>, GraphcalError> {
        let control = InferenceControl {
            cancellation,
            observations,
            root: expr.id(),
        };
        Infer::root(self, owner, control).infer_hir_type(expr)
    }
}

/// Operation-scoped policy shared by every recursive inference step.
///
/// Keeping cancellation and the observation sink in one value that [`Infer`]
/// always carries prevents nested helpers from silently dropping either.
#[derive(Clone, Copy)]
pub(super) struct InferenceControl<'a> {
    cancellation: &'a crate::cancellation::CancellationToken,
    observations: &'a BodyObservations,
    /// The checked root whose nominal uses this pass observes.
    root: &'a ExprId,
}

impl InferenceControl<'_> {
    pub(super) fn checkpoint(&self) -> Result<(), GraphcalError> {
        self.cancellation.checkpoint().map_err(GraphcalError::from)
    }

    pub(super) const fn observations(&self) -> &BodyObservations {
        self.observations
    }

    /// Record one nominal use of the checked root.
    pub(super) fn observe_nominal(
        &self,
        observation: crate::tir::expression_facts::NominalObservation,
        definition_span: Option<crate::syntax::span::Span>,
    ) {
        self.observations
            .observe_nominal(self.root, observation, definition_span);
    }

    pub(super) fn retain_static_index(
        &self,
        expr: &Expr,
        operand: &Expr,
        axis: &IndexTypeRef<Symbolic>,
        position: u64,
        usage: crate::tir::expression_facts::StaticIndexUse,
    ) {
        self.observations.retain_static_index(
            expr.id(),
            crate::tir::expression_facts::StaticIndexRequirement {
                operand: operand.id().clone(),
                axis: axis.clone(),
                position,
                usage,
            },
        );
    }
}

/// A call's arguments already inferred with the owning declaration intact.
///
/// When the owning declaration reconciles include overrides, every argument
/// of a function call is checked once with that identity before the call's
/// own signature rule runs; the rule then reuses these types instead of
/// inferring the arguments a second time.
pub(super) struct PrecheckedArgs<'e> {
    args: &'e [Expr],
    types: Vec<CheckedType<Symbolic>>,
}

impl<'e> PrecheckedArgs<'e> {
    pub(super) const fn new(args: &'e [Expr], types: Vec<CheckedType<Symbolic>>) -> Self {
        Self { args, types }
    }

    fn get(&self, arg: &Expr) -> Option<&CheckedType<Symbolic>> {
        self.args
            .iter()
            .zip(&self.types)
            .find_map(|(candidate, ty)| (candidate.id() == arg.id()).then_some(ty))
    }
}

/// One inference position: the DAG environment, the declaration whose body is
/// being inferred (when override reconciliation applies), the lexical locals in
/// scope, and the operation-scoped control state.
#[derive(Clone, Copy)]
pub(super) struct Infer<'a> {
    pub(super) env: InferEnv<'a>,
    pub(super) owner: Option<&'a ResolvedDeclName>,
    pub(super) locals: &'a LocalEnv<'a, CheckedType<Symbolic>>,
    pub(super) control: InferenceControl<'a>,
    /// The enclosing call's already checked arguments, if any.
    prechecked_args: Option<&'a PrecheckedArgs<'a>>,
}

/// The empty lexical scope every inference operation starts from.
const ROOT_LOCALS: &LocalEnv<'static, CheckedType<Symbolic>> = &LocalEnv::root();

impl<'a> Infer<'a> {
    const fn root(
        env: InferEnv<'a>,
        owner: Option<&'a ResolvedDeclName>,
        control: InferenceControl<'a>,
    ) -> Self {
        Self {
            env,
            owner,
            locals: ROOT_LOCALS,
            control,
            prechecked_args: None,
        }
    }

    /// Continue inference inside a nested lexical scope.
    pub(super) const fn with_locals<'b>(
        self,
        locals: &'b LocalEnv<'b, CheckedType<Symbolic>>,
    ) -> Infer<'b>
    where
        'a: 'b,
    {
        Infer {
            env: self.env,
            owner: self.owner,
            locals,
            control: self.control,
            prechecked_args: self.prechecked_args,
        }
    }

    /// Infer a subexpression outside the owning declaration's override checks.
    pub(super) const fn without_owner(self) -> Self {
        Self {
            owner: None,
            ..self
        }
    }

    /// Leave the enclosing call: its prechecked arguments no longer apply.
    pub(super) const fn outside_call(self) -> Self {
        Self {
            prechecked_args: None,
            ..self
        }
    }

    /// Continue with the call arguments `prechecked` already inferred.
    pub(super) const fn with_prechecked_args<'b>(
        self,
        prechecked: &'b PrecheckedArgs<'b>,
    ) -> Infer<'b>
    where
        'a: 'b,
    {
        Infer {
            env: self.env,
            owner: self.owner,
            locals: self.locals,
            control: self.control,
            prechecked_args: Some(prechecked),
        }
    }

    /// The type of an argument of the enclosing call, inferred outside the
    /// owning declaration's override checks unless it was already checked.
    pub(super) fn infer_arg(&self, arg: &Expr) -> Result<CheckedType<Symbolic>, GraphcalError> {
        self.prechecked_args
            .and_then(|prechecked| prechecked.get(arg))
            .map_or_else(
                || self.without_owner().infer_hir_type(arg),
                |checked| Ok(checked.clone()),
            )
    }
}
