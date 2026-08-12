mod aggregations;
mod arithmetic;
mod builtin_call;
mod complex;
mod conversions;
mod datetime;
mod hir_eval;
mod linear_algebra;
mod linear_algebra_lu;
pub mod numeric;
mod unit_scale;
mod work_budget;

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::syntax::type_name::{ConstructorName, FieldName, GenericParamName};

use graphcal_compiler::hir::{NominalField, NominalTypeDef};
use graphcal_compiler::registry::builtins::BuiltinFunctions;
use graphcal_compiler::registry::declared_type::{DeclaredGenericArg, IndexTypeRef, StructTypeRef};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::types::SemanticRegistry;
use graphcal_compiler::tir::typed::{DagTIR, StructFieldConstraintKey, TIR};

use crate::decl_key::RuntimeDeclKey;
use crate::domain_check::ResolvedDomainConstraint;

pub use crate::execution_facts::RuntimeValueMap;
pub use graphcal_compiler::registry::runtime_value::RuntimeValue;
pub(crate) use hir_eval::eval_hir_expr_with_presentation;
pub use hir_eval::{HirLocalValueMap, eval_hir_expr};
pub use unit_scale::resolve_unit_scale;
pub(in crate::eval_expr) use unit_scale::{checked_finite_quantity, checked_unit_scaled_value};

pub fn fresh_work_budget() -> work_budget::WorkBudget {
    work_budget::WorkBudget::default()
}

/// Immutable evaluation environment shared across all expression evaluations.
///
/// Bundles built-in functions, the type/unit registry, and source information
/// for diagnostics. Built-in constants carry their values in their typed HIR
/// identity and need no runtime string-keyed environment.
pub struct EvalContext<'a> {
    /// Cooperative cancellation for recursive expression evaluation.
    pub cancellation: graphcal_compiler::cancellation::CancellationToken,
    pub(super) work_budget: work_budget::WorkBudget,
    pub builtin_fns: &'a BuiltinFunctions,
    pub registry: &'a SemanticRegistry,
    pub src: &'a NamedSource<Arc<String>>,
    /// The enclosing file's full TIR.
    ///
    /// Used by [`eval_inline_dag_call`] to reach the file's flat per-DAG body
    /// map after semantic call metadata has selected a canonical DAG id.
    pub tir: &'a TIR,
    /// DAG whose expression is currently being evaluated.
    ///
    /// Every evaluation has an explicit semantic scope. Root expressions use
    /// [`TIR::root`], while inline calls use their concrete [`DagTIR`].
    pub current_dag: &'a DagTIR,
    /// Canonical declaration whose expression is currently being evaluated.
    /// Together with the expression span, this selects checked materialized-shape facts.
    pub current_decl: Option<graphcal_compiler::syntax::decl_name::ResolvedDeclName>,
    /// Root-file values visible to nested inline DAG calls. This lets DAG-body
    /// self-imports route by their canonical source `DagId` rather than by a
    /// same-leaf name in the immediate caller's local value map.
    pub root_values: Option<&'a RuntimeValueMap>,
    /// Presentation instances paired with `root_values` for references that
    /// cross an inline-DAG boundary back into the entry DAG.
    pub root_presentation_instances:
        Option<&'a crate::runtime_presentation::PresentationInstanceMap>,
    /// Checked facts for every runtime-callable DAG. Compile-time expression
    /// contexts use `None` because DAG calls are statically forbidden there.
    pub checked_execution_facts: Option<&'a crate::execution_facts::CheckedExecutionFacts>,
    /// Evaluated inline-call environments required only by checked presentation facts.
    pub presentation_calls: Option<&'a crate::execution_facts::EvaluatedPresentationCalls>,
    /// Resolved domain constraints declared on struct/union member fields,
    /// keyed by owner-qualified struct/constructor/field identity. Looked up at
    /// every `ExprKind::ConstructorCall` to validate field values immediately.
    /// `None` means "skip the check" (used by paths that haven't resolved
    /// constraints yet, e.g. const-bound evaluation inside `exec_plan`).
    pub struct_field_constraints:
        Option<&'a HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>>,
    /// Concrete Nat bindings while evaluating one instantiated generic field bound.
    pub generic_nat_bindings: Option<&'a HashMap<GenericParamName, u64>>,
    /// Embedder-injected host functions backing extern (plugin) calls.
    ///
    /// `None` in contexts that must not reach extern calls (const
    /// evaluation, display-unit resolution); dimension checking rejects
    /// extern calls in those positions, so hitting an extern call with
    /// `None` here surfaces as an evaluation error naming the function.
    pub host_fns: Option<&'a crate::host_fns::HostFunctionRegistry>,
}

pub fn index_ref_matches_resolved(
    actual: &IndexTypeRef,
    expected: &graphcal_compiler::syntax::index_name::ResolvedIndexName,
) -> bool {
    actual.declared_resolved() == Some(expected)
}

fn runtime_struct_type_def<'a>(
    type_name: &StructTypeRef,
    ctx: &'a EvalContext<'_>,
) -> Option<&'a NominalTypeDef> {
    ctx.tir.struct_type_def(type_name.resolved())
}

fn constructor_fields_for_runtime_struct<'a>(
    type_def: &'a NominalTypeDef,
    type_name: &StructTypeRef,
) -> Option<&'a [NominalField]> {
    type_def.union_members()?.iter().find_map(|member| {
        (member.name().as_str() == type_name.as_str()).then_some(member.fields())
    })
}

fn find_struct_field_constraint<'a>(
    constraints: &'a HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
    owning_type: Option<&StructTypeRef>,
    generic_args: &[DeclaredGenericArg],
    constructor: &ConstructorName,
    field: &FieldName,
) -> Option<&'a ResolvedDomainConstraint> {
    owning_type.and_then(|owning_type| {
        constraints.get(&StructFieldConstraintKey::for_application(
            owning_type.clone(),
            generic_args.to_vec(),
            constructor.clone(),
            field.clone(),
        ))
    })
}

impl<'a> EvalContext<'a> {
    /// Borrow this evaluation environment with a different diagnostic source.
    #[must_use]
    pub fn with_src<'b>(&'b self, src: &'b NamedSource<Arc<String>>) -> EvalContext<'b>
    where
        'a: 'b,
    {
        EvalContext {
            cancellation: self.cancellation.clone(),
            work_budget: self.work_budget.clone(),
            builtin_fns: self.builtin_fns,
            registry: self.registry,
            src,
            tir: self.tir,
            current_dag: self.current_dag,
            current_decl: self.current_decl.clone(),
            root_values: self.root_values,
            root_presentation_instances: self.root_presentation_instances,
            checked_execution_facts: self.checked_execution_facts,
            presentation_calls: self.presentation_calls,
            struct_field_constraints: self.struct_field_constraints,
            generic_nat_bindings: self.generic_nat_bindings,
            host_fns: self.host_fns,
        }
    }

    /// Borrow this environment in a canonical declaration's checked DAG/source.
    #[must_use]
    pub fn for_checked_decl<'b>(
        &'b self,
        dag: &'b DagTIR,
        src: &'b NamedSource<Arc<String>>,
        declaration: &graphcal_compiler::syntax::decl_name::ResolvedDeclName,
    ) -> EvalContext<'b>
    where
        'a: 'b,
    {
        EvalContext {
            cancellation: self.cancellation.clone(),
            work_budget: self.work_budget.clone(),
            builtin_fns: self.builtin_fns,
            registry: self.registry,
            src,
            tir: self.tir,
            current_dag: dag,
            current_decl: Some(declaration.clone()),
            root_values: self.root_values,
            root_presentation_instances: self.root_presentation_instances,
            checked_execution_facts: self.checked_execution_facts,
            presentation_calls: self.presentation_calls,
            struct_field_constraints: self.struct_field_constraints,
            generic_nat_bindings: self.generic_nat_bindings,
            host_fns: self.host_fns,
        }
    }

    /// Borrow this environment while evaluating one canonical declaration body.
    #[must_use]
    pub fn for_decl(
        &self,
        declaration: &graphcal_compiler::syntax::decl_name::ResolvedDeclName,
    ) -> Self {
        Self {
            cancellation: self.cancellation.clone(),
            work_budget: self.work_budget.clone(),
            builtin_fns: self.builtin_fns,
            registry: self.registry,
            src: self.src,
            tir: self.tir,
            current_dag: self.current_dag,
            current_decl: Some(declaration.clone()),
            root_values: self.root_values,
            root_presentation_instances: self.root_presentation_instances,
            checked_execution_facts: self.checked_execution_facts,
            presentation_calls: self.presentation_calls,
            struct_field_constraints: self.struct_field_constraints,
            generic_nat_bindings: self.generic_nat_bindings,
            host_fns: self.host_fns,
        }
    }

    /// Build a `GraphcalError::EvalError` using this context's source.
    pub fn eval_error(
        &self,
        message: impl Into<String>,
        span: graphcal_compiler::syntax::span::Span,
    ) -> GraphcalError {
        GraphcalError::EvalError {
            message: message.into(),
            src: self.src.clone(),
            span: span.into(),
        }
    }

    /// Build a `GraphcalError::InternalError` using this context's source.
    pub fn internal_error(
        &self,
        message: impl Into<String>,
        anchor: impl Into<graphcal_compiler::diagnostic_anchor::DiagnosticAnchor>,
    ) -> GraphcalError {
        GraphcalError::internal_error(message, self.src, anchor.into())
    }
}

fn dag_decl_runtime_key(
    name: &graphcal_compiler::syntax::decl_name::ResolvedDeclName,
) -> RuntimeDeclKey {
    RuntimeDeclKey::resolved(name.clone())
}

fn imported_binding_value<'a>(
    target: &graphcal_compiler::syntax::decl_name::ResolvedDeclName,
    caller_values: &'a RuntimeValueMap,
    ctx: &'a EvalContext<'_>,
) -> Option<&'a RuntimeValue> {
    let key = RuntimeDeclKey::resolved(target.clone());
    if target.owner() == ctx.current_dag.dag_id() {
        caller_values.get(&key)
    } else if target.owner() == ctx.tir.root_dag_id() {
        ctx.root_values.and_then(|values| values.get(&key))
    } else {
        None
    }
}
