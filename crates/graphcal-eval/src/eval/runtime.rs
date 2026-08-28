//! Runtime evaluation: converting TIR execution results to Values,
//! running execution plans, and checking asserts.

use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::span::Span;

use crate::decl_key::RuntimeDeclKey;
use crate::eval_expr::{
    EvalContext, HirLocalValueMap, RuntimeValue, RuntimeValueMap, eval_hir_expr,
    eval_hir_expr_with_presentation,
};
use crate::runtime_presentation::PresentationInstanceMap;
use graphcal_compiler::ir::resolve::{DeclCategory, ExpectedFail, ExpectedFailKey};
use graphcal_compiler::plot_shape::PlotLeafKind;
use graphcal_compiler::registry::builtins::{BuiltinFunctions, builtin_functions};
use graphcal_compiler::registry::declared_type::{DeclaredType, IndexTypeRef};
use graphcal_compiler::registry::error::GraphcalError;

use super::display::attach_presentation;
use super::public_projection::EvaluatedValue;
use super::types::{
    AssertResult, AxisMeta, DeclType, EvalResult, NodeError, PlotFieldValue, PlotSpec, Value,
    validate_display_projection,
};

/// Result of running the core eval loop: successfully evaluated values and per-node errors.
pub(super) struct EvalLoopResult {
    pub values: RuntimeValueMap,
    pub presentation_instances: PresentationInstanceMap,
    pub errors: HashMap<RuntimeDeclKey, NodeError>,
    pub presentation_calls: crate::execution_facts::EvaluatedPresentationCalls,
}

/// One completed runtime evaluation before project-level public output assembly.
///
/// Keeping values, contained node errors, and the display-aware root result in
/// one artifact makes the evaluator's direct output available to debugging
/// consumers without running the evaluator a second time.
pub struct RuntimeEvaluation {
    pub(super) result: EvalResult,
    pub(super) values: RuntimeValueMap,
    pub(super) errors: HashMap<RuntimeDeclKey, NodeError>,
}

impl std::fmt::Debug for RuntimeEvaluation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeEvaluation")
            .field("result", &self.result)
            .field("values", &self.values)
            .field("errors", &self.errors)
            .finish()
    }
}

impl RuntimeEvaluation {
    /// Whether evaluation produced a node, assertion, or plot failure.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty() || self.result.has_errors()
    }

    /// Display-aware result for the directly evaluated root DAG.
    #[must_use]
    pub const fn result(&self) -> &EvalResult {
        &self.result
    }
}

/// Core evaluation loop shared by project and prepared-model evaluation.
///
/// Inserts imported and const values, then iterates in topological order.
/// Unfold expressions carry their resolved axis and are evaluated inline.
/// Domain constraints are checked after successful evaluation.
///
/// Returns all computed values and any per-node errors. Internal invariant
/// violations are returned immediately rather than being fault-isolated as
/// ordinary user evaluation failures.
/// Run the core loop with one plan-validated row of runtime parameter bindings.
#[expect(
    clippy::too_many_lines,
    reason = "one dependency-ordered loop keeps value insertion and per-node failure isolation auditable"
)]
pub(super) fn run_eval_loop_with_bindings(
    plan: &crate::exec_plan::ExecPlan,
    bindings: &super::bindings::RuntimeParameterBindings,
    tir: &graphcal_compiler::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    builtin_fns: &BuiltinFunctions,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<EvalLoopResult, GraphcalError> {
    cancellation.checkpoint()?;
    let empty_hir_locals = HirLocalValueMap::root();

    let mut values: RuntimeValueMap = HashMap::new();
    let mut presentation_instances = PresentationInstanceMap::new();
    let mut errors: HashMap<RuntimeDeclKey, NodeError> = HashMap::new();
    let presentation_calls = crate::execution_facts::EvaluatedPresentationCalls::default();

    // Insert imported compile-time constants into the lookup table.
    // They keep their original `ScopedName` qualification.
    for (name, val) in &plan.imported_values {
        values.insert(name.clone(), val.clone());
    }

    // Insert const values into the lookup table.
    for (name, val) in plan.const_values.iter() {
        values.insert(name.clone(), val.clone());
    }

    // Inject supplied params before evaluating defaults or dependent nodes.
    // Bound and defaulted params share the same resolved domain constraints.
    for (name, binding) in bindings {
        if let Some(constraint) = plan.domain_constraints.get(name)
            && let Err(violation) =
                crate::domain_check::check_domain_constraint(&binding.value, constraint)
        {
            errors.insert(
                name.clone(),
                NodeError::EvalFailed {
                    message: violation.message,
                },
            );
            continue;
        }
        values.insert(name.clone(), binding.value.clone());
    }

    // Evaluate in topological order (params first, then nodes that depend on them).
    // Top-level declarations in a single file are always `Local`-form names.
    for name in &plan.topo_order {
        cancellation.checkpoint()?;
        if values.contains_key(name) || errors.contains_key(name) {
            continue;
        }

        // Check if any local runtime dependency has failed. Module-aware TIRs
        // carry canonical dependency identities; use those when present so a
        // qualified imported dependency with the same leaf as a local failure
        // cannot be mistaken for the local declaration.
        let failed_deps = failed_runtime_dependencies(tir.root(), name, &errors);

        if !failed_deps.is_empty() {
            errors.insert(name.clone(), NodeError::DependencyFailed { failed_deps });
            continue;
        }

        let ctx = EvalContext {
            cancellation: cancellation.clone(),
            work_budget: crate::eval_expr::fresh_work_budget(),
            builtin_fns,
            registry: tir.registry(),
            src,
            tir,
            current_dag: tir.root(),
            current_decl: Some(name.as_resolved().clone()),
            root_values: Some(&values),
            root_presentation_instances: Some(&presentation_instances),
            checked_execution_facts: Some(&plan.checked_execution_facts),
            presentation_calls: Some(&presentation_calls),
            struct_field_constraints: Some(&plan.struct_field_constraints),
            generic_nat_bindings: None,
            host_fns: Some(host_fns),
        };

        let result = tir
            .root()
            .runtime_expr(name.as_resolved())
            .ok_or_else(|| {
                GraphcalError::internal_error(
                    format!("TIR runtime declaration missing for `{name}`"),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })
            .and_then(|hir_expr| {
                eval_hir_expr_with_presentation(
                    hir_expr,
                    &values,
                    &presentation_instances,
                    &empty_hir_locals,
                    &ctx,
                )
            });

        match result {
            Ok(evaluated) => {
                let (val, presentation) = evaluated.into_parts();
                // Check domain constraints after successful evaluation.
                if let Some(constraint) = plan.domain_constraints.get(name)
                    && let Err(violation) =
                        crate::domain_check::check_domain_constraint(&val, constraint)
                {
                    errors.insert(
                        name.clone(),
                        NodeError::EvalFailed {
                            message: violation.message,
                        },
                    );
                    continue;
                }
                values.insert(name.clone(), val);
                if !presentation.is_none() {
                    presentation_instances.insert(name.clone(), presentation);
                }
            }
            Err(error @ (GraphcalError::InternalError { .. } | GraphcalError::Cancelled(_))) => {
                return Err(error);
            }
            Err(error) => {
                errors.insert(name.clone(), eval_failed_node_error(&error));
            }
        }
    }

    Ok(EvalLoopResult {
        values,
        presentation_instances,
        errors,
        presentation_calls,
    })
}

/// Convert a runtime `GraphcalError` into a per-node `EvalFailed` error,
/// preferring the bare eval message over the full rendered diagnostic.
fn eval_failed_node_error(e: &GraphcalError) -> NodeError {
    let message = match e {
        GraphcalError::EvalError { message, .. } => message.clone(),
        other => format!("{other}"),
    };
    NodeError::EvalFailed { message }
}

#[expect(
    clippy::option_if_let_else,
    reason = "explicit branches document that a declaration absent from the dependency map has no failed dependencies"
)]
fn failed_runtime_dependencies(
    dag: &graphcal_compiler::tir::typed::DagTIR,
    name: &RuntimeDeclKey,
    errors: &HashMap<RuntimeDeclKey, NodeError>,
) -> Vec<DeclName> {
    match dag
        .semantic()
        .dependencies
        .runtime_deps
        .get(name.as_resolved())
    {
        Some(dependencies) => dependencies
            .iter()
            .filter(|dependency| {
                errors.contains_key(&RuntimeDeclKey::resolved((*dependency).clone()))
            })
            .map(|dependency| DeclName::from_atom(dependency.atom().clone()))
            .collect(),
        None => Vec::new(),
    }
}

/// Evaluate using immutable TIR plus one plan and validated runtime bindings.
///
/// Runtime errors are contained per-node: if a node fails, independent nodes
/// still evaluate, and dependent nodes receive a `DependencyFailed` error.
/// Internal invariant violations abort evaluation as `X001`.
/// Evaluate a plan with one row of runtime parameter bindings.
pub(super) fn evaluate_plan_with_bindings_and_cancellation(
    tir: &graphcal_compiler::tir::typed::TIR,
    plan: &crate::exec_plan::ExecPlan,
    bindings: &super::bindings::RuntimeParameterBindings,
    declared_types: &HashMap<ScopedName, graphcal_compiler::registry::declared_type::DeclaredType>,
    src: &NamedSource<Arc<String>>,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<EvalResult, GraphcalError> {
    evaluate_plan_with_values_and_bindings_and_cancellation(
        tir,
        plan,
        bindings,
        declared_types,
        src,
        host_fns,
        cancellation,
    )
    .map(|outcome| outcome.result)
}

#[expect(
    clippy::too_many_lines,
    reason = "linear evaluation pipeline is clearest as a single function"
)]
pub(super) fn evaluate_plan_with_values_and_bindings_and_cancellation(
    tir: &graphcal_compiler::tir::typed::TIR,
    plan: &crate::exec_plan::ExecPlan,
    bindings: &super::bindings::RuntimeParameterBindings,
    declared_types: &HashMap<ScopedName, graphcal_compiler::registry::declared_type::DeclaredType>,
    src: &NamedSource<Arc<String>>,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<RuntimeEvaluation, GraphcalError> {
    cancellation.checkpoint()?;
    let builtin_fns = builtin_functions();
    let empty_hir_locals = HirLocalValueMap::root();

    let EvalLoopResult {
        values,
        presentation_instances,
        errors,
        presentation_calls,
    } = run_eval_loop_with_bindings(
        plan,
        bindings,
        tir,
        src,
        builtin_fns,
        host_fns,
        cancellation,
    )?;

    cancellation.checkpoint()?;
    let ctx = EvalContext {
        cancellation: cancellation.clone(),
        work_budget: crate::eval_expr::fresh_work_budget(),
        builtin_fns,
        registry: tir.registry(),
        src,
        tir,
        current_dag: tir.root(),
        current_decl: None,
        root_values: Some(&values),
        root_presentation_instances: Some(&presentation_instances),
        checked_execution_facts: Some(&plan.checked_execution_facts),
        presentation_calls: Some(&presentation_calls),
        struct_field_constraints: Some(&plan.struct_field_constraints),
        generic_nat_bindings: None,
        host_fns: Some(host_fns),
    };

    let local_key = |name: &ScopedName| {
        tir.root()
            .require_bound_decl_identity(name, src, DiagnosticAnchor::WholeFile)
            .map(RuntimeDeclKey::resolved)
    };

    let make_value = |name: &ScopedName,
                      runtime: &RuntimeValue|
     -> Result<Result<Value, NodeError>, GraphcalError> {
        let runtime_key = local_key(name)?;
        let declaration = runtime_key.as_resolved();
        let presentation = bindings
            .get(&runtime_key)
            .map(|binding| &binding.presentation)
            .or_else(|| tir.root().declaration_presentation(declaration))
            .ok_or_else(|| {
                GraphcalError::internal_error(
                    format!(
                        "checked presentation facts are missing for declaration `{declaration}`"
                    ),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
        let declared_type = declared_types.get(name).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("checked declared type is missing for public declaration `{declaration}`"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        let mut value = EvaluatedValue::new(runtime, declared_type).project(tir, src)?;
        // Authored display metadata either applies successfully or makes this
        // declaration fail; structural projection failures remain X001.
        if let Err(error) = attach_presentation(
            &mut value,
            presentation,
            presentation_instances.get(&runtime_key),
            &ctx,
            &values,
        ) {
            return match error {
                error @ (GraphcalError::InternalError { .. } | GraphcalError::Cancelled(_)) => {
                    Err(error)
                }
                error => Ok(Err(eval_failed_node_error(&error))),
            };
        }
        match validate_display_projection(&value) {
            Ok(()) => Ok(Ok(value)),
            Err(error) => Ok(Err(NodeError::EvalFailed {
                message: error.to_string(),
            })),
        }
    };

    let make_result = |name: &ScopedName| -> Result<Result<Value, NodeError>, GraphcalError> {
        let key = local_key(name)?;
        errors.get(&key).map_or_else(
            || {
                values.get(&key).map_or_else(
                    || {
                        Err(GraphcalError::internal_error(
                            format!("successful declaration `{key}` has no runtime value"),
                            src,
                            DiagnosticAnchor::WholeFile,
                        ))
                    },
                    |runtime| make_value(name, runtime),
                )
            },
            |error| Ok(Err(error.clone())),
        )
    };

    let consts = tir
        .root()
        .consts()
        .iter()
        .map(|entry| {
            let key = local_key(&entry.name)?;
            let runtime =
                plan.const_values
                    .get(&key)
                    .ok_or_else(|| GraphcalError::InternalError {
                        message: format!("checked constant `{key}` has no runtime value"),
                        src: src.clone(),
                        span: entry.span.into(),
                    })?;
            make_value(&entry.name, runtime).map(|value| (entry.name.clone(), value))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let params = tir
        .root()
        .params()
        .iter()
        .map(|entry| make_result(&entry.name).map(|value| (entry.name.clone(), value)))
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let nodes = tir
        .root()
        .nodes()
        .iter()
        .map(|entry| make_result(&entry.name).map(|value| (entry.name.clone(), value)))
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    cancellation.checkpoint()?;

    let all: Vec<(ScopedName, Result<Value, NodeError>, DeclType)> = tir
        .root()
        .source_order()
        .iter()
        .filter_map(|(name, category)| {
            let decl_type = match category {
                DeclCategory::Const => DeclType::Const,
                DeclCategory::Param => DeclType::Param,
                DeclCategory::Node => DeclType::Node,
                DeclCategory::Assert
                | DeclCategory::Plot
                | DeclCategory::Figure
                | DeclCategory::Layer => return None,
            };
            Some((name, decl_type))
        })
        .map(|(name, decl_type)| match decl_type {
            DeclType::Const => {
                let key = local_key(name)?;
                plan.const_values.get(&key).map_or_else(
                    || {
                        Err(GraphcalError::internal_error(
                            format!("checked source-order constant `{key}` has no runtime value"),
                            src,
                            DiagnosticAnchor::WholeFile,
                        ))
                    },
                    |runtime| {
                        make_value(name, runtime).map(|value| (name.clone(), value, decl_type))
                    },
                )
            }
            DeclType::Param | DeclType::Node => {
                make_result(name).map(|value| (name.clone(), value, decl_type))
            }
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    cancellation.checkpoint()?;
    // A directly evaluated DAG is its own entry surface. Project evaluation
    // replaces this set with include-aware classification after assembling the
    // root result.
    let output_surface = all.iter().map(|(name, _, _)| name.clone()).collect();

    // Evaluate assertions in source order, applying expected_fail inversion.
    // An assertion whose body references a failed declaration reports the
    // dependency failure (with its root cause) instead of evaluating over a
    // value map where the failed name is simply absent (#814).
    let assertions: Vec<(ScopedName, AssertResult, Span)> = tir
        .root()
        .asserts()
        .iter()
        .map(|entry| {
            let owner = tir.root().require_bound_decl_identity(
                &entry.name,
                src,
                DiagnosticAnchor::Source(entry.span),
            )?;
            let entry_ctx = ctx.for_decl(&owner);
            let assert_result = assert_dependency_failure(&entry.body, &errors).map_or_else(
                || {
                    let ef = plan.expected_fail.get(&entry.name);
                    evaluate_assert_with_expected_fail(
                        &entry.body,
                        ef,
                        &values,
                        &empty_hir_locals,
                        &entry_ctx,
                    )
                },
                |message| AssertResult::Error { message },
            );
            Ok((entry.name.clone(), assert_result, entry.span))
        })
        .collect::<Result<_, GraphcalError>>()?;
    cancellation.checkpoint()?;

    // Evaluate plot declarations. Evaluation is per-plot best-effort, but a
    // plot that cannot be rendered is reported, never silently dropped
    // (#842).
    let mut plot_errors: Vec<super::types::PlotError> = Vec::new();
    let plots = tir
        .root()
        .plots()
        .iter()
        .try_fold(Vec::new(), |mut plots, entry| {
            let owner = tir.root().require_bound_decl_identity(
                &entry.name,
                src,
                DiagnosticAnchor::WholeFile,
            )?;
            match evaluate_plot(
                entry,
                &values,
                &presentation_instances,
                &errors,
                &ctx.for_decl(&owner),
            ) {
                Ok(plot) => plots.push(plot),
                Err(PlotEvaluationError::Render(message)) => {
                    plot_errors.push(super::types::PlotError {
                        name: entry.name.clone(),
                        message,
                    });
                }
                Err(PlotEvaluationError::Fatal(error)) => return Err(error),
            }
            Ok(plots)
        })?;
    cancellation.checkpoint()?;

    // Evaluate figure declarations; a failing field reports the figure
    // instead of silently dropping the property (#845).
    let figures: Vec<super::types::FigureSpec> = tir
        .root()
        .figures()
        .iter()
        .map(|entry| {
            let owner = tir.root().require_bound_decl_identity(
                &entry.name,
                src,
                DiagnosticAnchor::WholeFile,
            )?;
            Ok(
                match eval_composition_fields(
                    &entry.fields,
                    &entry.plot_names,
                    &values,
                    &ctx.for_decl(&owner),
                ) {
                    Ok(evaluated) => Some(super::types::FigureSpec {
                        name: entry.name.clone(),
                        plot_names: evaluated.plot_names,
                        properties: evaluated.properties,
                    }),
                    Err(message) => {
                        plot_errors.push(super::types::PlotError {
                            name: entry.name.clone(),
                            message,
                        });
                        None
                    }
                },
            )
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?
        .into_iter()
        .flatten()
        .collect();

    // Evaluate layer declarations
    let layers: Vec<super::types::LayerSpec> = tir
        .root()
        .layers()
        .iter()
        .map(|entry| {
            let owner = tir.root().require_bound_decl_identity(
                &entry.name,
                src,
                DiagnosticAnchor::WholeFile,
            )?;
            Ok(
                match eval_composition_fields(
                    &entry.fields,
                    &entry.plot_names,
                    &values,
                    &ctx.for_decl(&owner),
                ) {
                    Ok(evaluated) => Some(super::types::LayerSpec {
                        name: entry.name.clone(),
                        plot_names: evaluated.plot_names,
                        properties: evaluated.properties,
                    }),
                    Err(message) => {
                        plot_errors.push(super::types::PlotError {
                            name: entry.name.clone(),
                            message,
                        });
                        None
                    }
                },
            )
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?
        .into_iter()
        .flatten()
        .collect();
    cancellation.checkpoint()?;

    // Re-key domain constraints from runtime identities back to the
    // source-order `ScopedName`s using the same key derivation the value
    // maps use, so output entries keep their alias qualification (#813).
    let domain_constraints: HashMap<ScopedName, _> = tir
        .root()
        .source_order()
        .iter()
        .map(|(name, _)| {
            let key = local_key(name)?;
            Ok(plan
                .domain_constraints
                .get(&key)
                .map(|constraint| (name.clone(), constraint.clone())))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?
        .into_iter()
        .flatten()
        .collect();
    cancellation.checkpoint()?;
    let assumes_map: HashMap<ScopedName, Vec<ScopedName>> = plan.assumes_map.clone();

    let result = EvalResult {
        consts,
        params,
        nodes,
        all,
        output_surface,
        assertions,
        plots,
        plot_errors,
        figures,
        layers,
        assumes_map,
        base_dim_symbols: tir.registry().dimensions.base_dim_symbols().clone(),
        domain_constraints,
    };
    Ok(RuntimeEvaluation {
        result,
        values,
        errors,
    })
}

/// If any declaration referenced by an assertion body failed to evaluate,
/// render the dependency-failure message the assertion should report (#814).
///
/// Mirrors the node path's `DependencyFailed` contract: a reference to a
/// failed declaration is not "undefined", it is unevaluable. Direct
/// evaluation failures carry their root cause inline; transitive failures
/// list only the dependency's name (its own failure is reported on that
/// declaration).
fn assert_dependency_failure(
    body: &graphcal_compiler::hir::AssertBody,
    errors: &HashMap<RuntimeDeclKey, NodeError>,
) -> Option<String> {
    let body_exprs: Vec<&graphcal_compiler::hir::Expr> = match body {
        graphcal_compiler::hir::AssertBody::Expr(expr) => vec![expr],
        graphcal_compiler::hir::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
            ..
        } => vec![actual, expected, tolerance],
    };
    dependency_failure_message(body_exprs, errors)
}

/// If any declaration referenced by the given expressions failed to
/// evaluate, render a `dependency failed: ...` message naming each failed
/// dependency (direct failures carry their root cause inline).
///
/// Shared by assertions (#814) and plots (#842): a reference to a failed
/// declaration is not "undefined", it is unevaluable, and the report must
/// point at the root cause.
fn dependency_failure_message<'a>(
    exprs: impl IntoIterator<Item = &'a graphcal_compiler::hir::Expr>,
    errors: &HashMap<RuntimeDeclKey, NodeError>,
) -> Option<String> {
    if errors.is_empty() {
        return None;
    }
    let deps: std::collections::BTreeSet<_> = exprs
        .into_iter()
        .flat_map(|expr| {
            graphcal_compiler::hir::collect_expr_dependencies(expr)
                .graph_refs
                .into_iter()
        })
        .collect();
    let failed: Vec<String> = deps
        .iter()
        .filter_map(|dep| {
            errors
                .get(&RuntimeDeclKey::resolved(dep.clone()))
                .map(|err| {
                    let leaf = DeclName::from_atom(dep.atom().clone());
                    match err {
                        NodeError::EvalFailed { message } => format!("{leaf} ({message})"),
                        NodeError::DependencyFailed { .. } => leaf.to_string(),
                    }
                })
        })
        .collect();
    (!failed.is_empty()).then(|| format!("dependency failed: {}", failed.join(", ")))
}

/// Evaluate an assertion body with optional `#[expected_fail]` handling.
///
/// For `None` (no `expected_fail`): evaluate and return the result as-is.
/// For `Some(ExpectedFail::All)`: invert the final result (Pass↔Fail).
/// For `Some(ExpectedFail::Variants(keys))`: evaluate the expression to get
/// the raw indexed `RuntimeValue`, invert only the matching variant entries,
/// then aggregate.
pub fn evaluate_assert_with_expected_fail(
    body: &graphcal_compiler::hir::AssertBody,
    ef: Option<&ExpectedFail>,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> AssertResult {
    match ef {
        None => evaluate_assert_body(body, values, local_values, ctx),
        Some(ExpectedFail::All) => {
            let result = evaluate_assert_body(body, values, local_values, ctx);
            match result {
                AssertResult::Pass => AssertResult::Fail {
                    message: "assertion passed but was marked #[expected_fail]".to_string(),
                },
                AssertResult::Fail { .. } => AssertResult::Pass,
                AssertResult::Error { .. } => result,
            }
        }
        Some(ExpectedFail::Variants(keys)) => {
            // Per-variant: we need the raw per-key Bool tree to invert
            // specific entries. For `Expr` bodies that is the evaluated
            // expression; for tolerance bodies it is the element-wise
            // pass/fail tree (#809).
            let bool_tree = match body {
                graphcal_compiler::hir::AssertBody::Expr(body_expr) => {
                    match eval_hir_expr(body_expr, values, local_values, ctx) {
                        Ok(value) => value,
                        Err(e) => {
                            return AssertResult::Error {
                                message: format!("{e}"),
                            };
                        }
                    }
                }
                graphcal_compiler::hir::AssertBody::Tolerance {
                    actual,
                    expected,
                    tolerance,
                } => {
                    let operands = eval_tolerance_operands(
                        actual,
                        expected,
                        tolerance,
                        values,
                        local_values,
                        ctx,
                    );
                    let (actual_val, expected_val, tolerance_val) = match operands {
                        Ok(operands) => operands,
                        Err(result) => return result,
                    };
                    match eval_tolerance_tree(&actual_val, &expected_val, &tolerance_val) {
                        Ok((tree, _)) => tree,
                        Err(message) => return AssertResult::Error { message },
                    }
                }
            };
            match bool_tree {
                RuntimeValue::Indexed {
                    index_name,
                    entries,
                } => {
                    let inverted = invert_indexed_variants(&index_name, entries, keys);
                    check_indexed_assert_with_expected_fail(&inverted.0, &inverted.1, keys)
                }
                RuntimeValue::Bool(_) => AssertResult::Error {
                    message:
                        "invalid compiled plan: per-variant #[expected_fail(...)] on a non-indexed assertion"
                            .to_string(),
                },
                other => AssertResult::Error {
                    message: format!("expected Bool or Indexed, got {other:?}"),
                },
            }
        }
    }
}

fn expected_fail_key_matches_path(
    path: &[(IndexTypeRef, IndexEntryKey)],
    key: &ExpectedFailKey,
) -> bool {
    path.len() == key.len()
        && path
            .iter()
            .zip(key.iter())
            .all(|((actual_index, actual_variant), expected)| {
                expected.matches_entry(actual_index, actual_variant)
            })
}

/// Invert specific variant entries in an indexed `RuntimeValue`.
///
/// For each entry in the indexed value, if the variant key matches one of the
/// expected-fail keys, flip `Bool(true)` → `Bool(false)` and vice versa.
/// For nested indexed values (multi-index), recurse.
fn invert_indexed_variants(
    index_name: &IndexTypeRef,
    entries: IndexMap<IndexEntryKey, RuntimeValue>,
    keys: &[ExpectedFailKey],
) -> (IndexTypeRef, IndexMap<IndexEntryKey, RuntimeValue>) {
    let inverted_entries = entries
        .into_iter()
        .map(|(variant, value)| {
            let new_value = match value {
                RuntimeValue::Bool(b) => {
                    // Single-index: check if this variant is in any key
                    let should_invert = keys
                        .iter()
                        .any(|key| key.len() == 1 && key[0].matches_entry(index_name, &variant));
                    if should_invert {
                        RuntimeValue::Bool(!b)
                    } else {
                        RuntimeValue::Bool(b)
                    }
                }
                RuntimeValue::Indexed {
                    index_name: inner_index,
                    entries: inner_entries,
                } => {
                    // Multi-index: filter keys that match the current variant at position 0,
                    // then strip the first element and recurse.
                    let sub_keys: Vec<ExpectedFailKey> = keys
                        .iter()
                        .filter(|key| key.len() >= 2 && key[0].matches_entry(index_name, &variant))
                        .map(|key| key[1..].to_vec())
                        .collect();
                    if sub_keys.is_empty() {
                        // No expected-fail keys apply to this subtree — leave as-is
                        RuntimeValue::Indexed {
                            index_name: inner_index,
                            entries: inner_entries,
                        }
                    } else {
                        let (idx, ents) =
                            invert_indexed_variants(&inner_index, inner_entries, &sub_keys);
                        RuntimeValue::Indexed {
                            index_name: idx,
                            entries: ents,
                        }
                    }
                }
                other => other,
            };
            (variant, new_value)
        })
        .collect();
    (index_name.clone(), inverted_entries)
}

/// Format a list of indexed paths for assertion failure messages.
///
/// Each path is a slice of index/variant pairs from outermost to innermost.
/// For single-index paths, formats as `Mode#Boost, Mode#Cruise`.
/// For multi-index paths, formats as `(Phase#Launch, Maneuver#Correction), (Phase#Cruise, Maneuver#Insertion)`.
fn format_indexed_path_part(index: &IndexTypeRef, key: &IndexEntryKey) -> String {
    match key {
        IndexEntryKey::Named(variant) => format!("{}#{variant}", index.display_name()),
        IndexEntryKey::Position(_) => key.to_string(),
    }
}

fn format_indexed_paths(
    paths: &[&[(IndexTypeRef, IndexEntryKey)]],
    is_multi_index: bool,
) -> String {
    let formatted: Vec<String> = if is_multi_index {
        paths
            .iter()
            .map(|p| {
                format!(
                    "({})",
                    p.iter()
                        .map(|(index, key)| format_indexed_path_part(index, key))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .collect()
    } else {
        paths
            .iter()
            .map(|p| format_indexed_path_part(&p[0].0, &p[0].1))
            .collect()
    };
    formatted.join(", ")
}

/// Check an indexed assertion with expected-fail variant awareness.
///
/// After inversion, the semantics are:
/// - A variant matching an expected-fail key that is `true` (was `false` before inversion)
///   means "expected failure occurred" → good.
/// - A variant matching an expected-fail key that is `false` (was `true` before inversion)
///   means "unexpected pass" → report as failure.
/// - A variant NOT matching any key behaves normally (`true` = pass, `false` = fail).
///
/// We reuse `collect_failing_paths` on the inverted entries, then classify each
/// failing path as either "unexpected pass" or "unexpected fail".
fn check_indexed_assert_with_expected_fail(
    index_name: &IndexTypeRef,
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
    keys: &[ExpectedFailKey],
) -> AssertResult {
    match collect_failing_paths(index_name, entries) {
        Ok(paths) if paths.is_empty() => AssertResult::Pass,
        Ok(paths) => {
            // Classify each failing path
            let mut unexpected_passes = Vec::new();
            let mut unexpected_fails = Vec::new();

            for path in &paths {
                let is_expected_fail_key = keys
                    .iter()
                    .any(|key| expected_fail_key_matches_path(path, key));
                if is_expected_fail_key {
                    // This was an expected-fail key but the value is false after inversion,
                    // meaning the original was true → unexpected pass
                    unexpected_passes.push(path.as_slice());
                } else {
                    unexpected_fails.push(path.as_slice());
                }
            }

            let is_multi_index = paths.iter().any(|p| p.len() > 1);
            let mut parts = Vec::new();

            if !unexpected_passes.is_empty() {
                parts.push(format!(
                    "unexpected pass at {}",
                    format_indexed_paths(&unexpected_passes, is_multi_index)
                ));
            }

            if !unexpected_fails.is_empty() {
                parts.push(format!(
                    "failed at {}",
                    format_indexed_paths(&unexpected_fails, is_multi_index)
                ));
            }

            AssertResult::Fail {
                message: parts.join("; "),
            }
        }
        Err(msg) => AssertResult::Error { message: msg },
    }
}

/// Recursively check an indexed assertion value (possibly multi-dimensional).
///
/// For single-index: `Bool[Mode]` — entries are `Bool` values.
/// For multi-index: `Bool[Phase, Maneuver]` — entries are nested `Indexed` values.
///
/// Single-index failure message example:
///   `failed at Mode#Boost`
/// Multi-index failure message example:
///   `failed at (Phase#Launch, Maneuver#Correction), (Phase#Cruise, Maneuver#Insertion)`
fn check_indexed_assert(
    index_name: &IndexTypeRef,
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
) -> AssertResult {
    match collect_failing_paths(index_name, entries) {
        Ok(paths) if paths.is_empty() => AssertResult::Pass,
        Ok(paths) => {
            let is_multi_index = paths.iter().any(|p| p.len() > 1);
            AssertResult::Fail {
                message: format!(
                    "failed at {}",
                    format_indexed_paths(
                        &paths.iter().map(Vec::as_slice).collect::<Vec<_>>(),
                        is_multi_index,
                    )
                ),
            }
        }
        Err(msg) => AssertResult::Error { message: msg },
    }
}

/// Recursively collect failing variant paths from an indexed assertion value.
///
/// Each path is a `Vec<(IndexTypeRef, VariantName)>` of index/variant pairs from outermost to innermost.
/// For example, `vec![(IndexTypeRef::with_owner(owner, IndexName::expect_valid("Phase")), VariantName::new("Launch")), ...]` for a 2D failure.
fn collect_failing_paths(
    index_name: &IndexTypeRef,
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
) -> Result<Vec<Vec<(IndexTypeRef, IndexEntryKey)>>, String> {
    let mut paths = Vec::new();
    for (variant, value) in entries {
        let key = (index_name.clone(), variant.clone());
        match value {
            RuntimeValue::Bool(true) => {}
            RuntimeValue::Bool(false) => {
                paths.push(vec![key]);
            }
            RuntimeValue::Indexed {
                index_name: inner_index,
                entries: inner_entries,
            } => {
                // Recurse into nested dimension, prepending current variant to each path
                for mut inner_path in collect_failing_paths(inner_index, inner_entries)? {
                    inner_path.insert(0, key.clone());
                    paths.push(inner_path);
                }
            }
            other => {
                return Err(format!(
                    "expected Bool for {}::{variant}, got {other:?}",
                    index_name.display_name()
                ));
            }
        }
    }
    Ok(paths)
}

/// Evaluate a single assert body and return an `AssertResult`.
fn evaluate_assert_body(
    body: &graphcal_compiler::hir::AssertBody,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> AssertResult {
    match body {
        graphcal_compiler::hir::AssertBody::Expr(body_expr) => {
            match eval_hir_expr(body_expr, values, local_values, ctx) {
                Ok(RuntimeValue::Bool(true)) => AssertResult::Pass,
                Ok(RuntimeValue::Bool(false)) => AssertResult::Fail {
                    message: "assertion evaluated to false".to_string(),
                },
                Ok(RuntimeValue::Indexed {
                    index_name,
                    entries,
                }) => check_indexed_assert(&index_name, &entries),
                Ok(other) => AssertResult::Error {
                    message: format!("expected Bool, got {other:?}"),
                },
                Err(e) => AssertResult::Error {
                    message: format!("{e}"),
                },
            }
        }
        graphcal_compiler::hir::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
        } => evaluate_tolerance_assert(actual, expected, tolerance, values, local_values, ctx),
    }
}

/// Evaluate a tolerance assertion body (`actual ~= expected +/- tolerance`).
///
/// Indexed operands broadcast element-wise (#809): the assertion's shape
/// comes from `actual`; `expected` and `tolerance` are each unindexed (applied
/// to every key) or indexed by the same axes. Failures report each failing
/// key with its actual/expected/delta detail.
fn evaluate_tolerance_assert(
    actual: &graphcal_compiler::hir::Expr,
    expected: &graphcal_compiler::hir::Expr,
    tolerance: &graphcal_compiler::hir::Expr,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> AssertResult {
    let (actual_val, expected_val, tolerance_val) =
        match eval_tolerance_operands(actual, expected, tolerance, values, local_values, ctx) {
            Ok(operands) => operands,
            Err(result) => return result,
        };
    match eval_tolerance_tree(&actual_val, &expected_val, &tolerance_val) {
        Err(message) => AssertResult::Error { message },
        Ok((_, failures)) if failures.is_empty() => AssertResult::Pass,
        Ok((_, failures)) => AssertResult::Fail {
            message: format_tolerance_failures(&failures),
        },
    }
}

/// Evaluate the three operand expressions of a tolerance assertion.
///
/// Returns the raw runtime values (any shape — shape checking happens in
/// [`eval_tolerance_tree`]), or the `AssertResult::Error` to report.
fn eval_tolerance_operands(
    actual: &graphcal_compiler::hir::Expr,
    expected: &graphcal_compiler::hir::Expr,
    tolerance: &graphcal_compiler::hir::Expr,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<(RuntimeValue, RuntimeValue, RuntimeValue), AssertResult> {
    let eval = |expr: &graphcal_compiler::hir::Expr| {
        eval_hir_expr(expr, values, local_values, ctx).map_err(|e| AssertResult::Error {
            message: format!("{e}"),
        })
    };
    Ok((eval(actual)?, eval(expected)?, eval(tolerance)?))
}

/// A failing key of a tolerance assertion, with its numeric detail.
struct ToleranceFailure {
    /// Index path from outermost to innermost axis; empty for an unindexed
    /// assertion.
    path: Vec<(IndexTypeRef, IndexEntryKey)>,
    /// `actual X, expected Y +/- T, off by D`.
    detail: String,
}

/// Walk a tolerance assertion's operands element-wise, producing the per-key
/// `Bool` tree (mirroring `actual`'s index structure) plus the detail for
/// every failing key. A shape/sign/type problem aborts with `Err` —
/// reported as an assertion ERROR.
fn eval_tolerance_tree(
    actual: &RuntimeValue,
    expected: &RuntimeValue,
    tolerance: &RuntimeValue,
) -> Result<(RuntimeValue, Vec<ToleranceFailure>), String> {
    let mut failures = Vec::new();
    let mut path = Vec::new();
    let tree = tolerance_tree_inner(actual, expected, tolerance, &mut path, &mut failures)?;
    Ok((tree, failures))
}

fn tolerance_tree_inner(
    actual: &RuntimeValue,
    expected: &RuntimeValue,
    tolerance: &RuntimeValue,
    path: &mut Vec<(IndexTypeRef, IndexEntryKey)>,
    failures: &mut Vec<ToleranceFailure>,
) -> Result<RuntimeValue, String> {
    if let RuntimeValue::Indexed {
        index_name,
        entries,
    } = actual
    {
        let checked_entries = entries
            .iter()
            .map(|(variant, actual_entry)| {
                let expected_entry = tolerance_entry_or_broadcast(expected, index_name, variant)?;
                let tolerance_entry = tolerance_entry_or_broadcast(tolerance, index_name, variant)?;
                path.push((index_name.clone(), variant.clone()));
                let result = tolerance_tree_inner(
                    actual_entry,
                    expected_entry,
                    tolerance_entry,
                    path,
                    failures,
                );
                path.pop();
                Ok((variant.clone(), result?))
            })
            .collect::<Result<_, String>>()?;
        return Ok(RuntimeValue::Indexed {
            index_name: index_name.clone(),
            entries: checked_entries,
        });
    }

    let actual_val = tolerance_quantity_operand(actual, "actual")?;
    let expected_val = tolerance_quantity_operand(expected, "expected")?;
    let tolerance_val = tolerance_quantity_operand(tolerance, "tolerance")?;
    let tol_display = format!("{tolerance_val}");

    // A negative tolerance makes the assertion unsatisfiable (even an
    // exact match fails). Statically-known negatives are rejected at
    // check time (#815); this guards tolerances computed at runtime.
    if tolerance_val < 0.0 {
        return Err(format!("tolerance must be non-negative, got {tol_display}"));
    }

    let delta = (actual_val - expected_val).abs();
    let ok = delta <= tolerance_val;
    if !ok {
        failures.push(ToleranceFailure {
            path: path.clone(),
            detail: format!(
                "actual {actual_val}, expected {expected_val} +/- {tol_display}, off by {delta}"
            ),
        });
    }
    Ok(RuntimeValue::Bool(ok))
}

/// Select the entry of a broadcastable tolerance operand for one key of
/// `actual`'s axis: indexed operands index per key (axes were checked
/// statically; mismatches here are evaluation errors), unindexed operands
/// broadcast unchanged.
fn tolerance_entry_or_broadcast<'a>(
    operand: &'a RuntimeValue,
    axis: &IndexTypeRef,
    variant: &IndexEntryKey,
) -> Result<&'a RuntimeValue, String> {
    match operand {
        RuntimeValue::Indexed {
            index_name,
            entries,
        } => {
            if !index_name.matches_ref(axis) {
                return Err(format!(
                    "tolerance assertion operand has mismatched index axes: `{}` vs `{}`",
                    axis.display_name(),
                    index_name.display_name()
                ));
            }
            entries.get(variant).ok_or_else(|| {
                format!(
                    "tolerance assertion operand is missing entry `{}`",
                    format_indexed_path_part(index_name, variant)
                )
            })
        }
        other => Ok(other),
    }
}

fn tolerance_quantity_operand(value: &RuntimeValue, role: &str) -> Result<f64, String> {
    match value {
        RuntimeValue::Quantity(v) => Ok(*v),
        other => Err(format!("expected quantity {role}, got {other:?}")),
    }
}

/// Render tolerance failures: an unindexed assertion reports its detail bare
/// (`actual X, expected Y +/- T, off by D`); indexed assertions report each
/// failing key with its detail.
fn format_tolerance_failures(failures: &[ToleranceFailure]) -> String {
    if let [failure] = failures
        && failure.path.is_empty()
    {
        return failure.detail.clone();
    }
    let is_multi_index = failures.iter().any(|f| f.path.len() > 1);
    let formatted: Vec<String> = failures
        .iter()
        .map(|f| {
            let key = format_indexed_paths(&[f.path.as_slice()], is_multi_index);
            format!("failed at {key} ({})", f.detail)
        })
        .collect();
    formatted.join("; ")
}

/// Evaluate one plot property expression to a `PlotFieldValue`. String
/// literals are passed through directly (Graphcal has no runtime String
/// value); any other expression is evaluated and converted from a
/// `RuntimeValue`. An evaluation failure aborts the whole plot; the error
/// message is reported on the plot (#842).
fn eval_plot_property(
    expr: &graphcal_compiler::hir::Expr,
    values: &RuntimeValueMap,
    ctx: &EvalContext<'_>,
) -> Result<PlotFieldValue, String> {
    if let graphcal_compiler::hir::ExprKind::StringLiteral(s) = &expr.kind {
        return Ok(PlotFieldValue::String(s.clone()));
    }
    let empty_locals = HirLocalValueMap::root();
    eval_hir_expr(expr, values, &empty_locals, ctx)
        .map_err(|e| eval_failed_node_error(&e).to_string())
        .and_then(|rv| runtime_to_plot_field_value(&rv))
}

#[derive(Debug)]
enum PlotEvaluationError {
    Render(String),
    Fatal(GraphcalError),
}

impl From<String> for PlotEvaluationError {
    fn from(message: String) -> Self {
        Self::Render(message)
    }
}

/// Evaluate a plot declaration, producing a `PlotSpec`.
///
/// The authoritative TIR plot record carries both its lowered HIR body and
/// mark metadata. String literals are handled directly (they are
/// not runtime values in Graphcal).
///
/// Ordinary expression/display failures remain attached to this plot, while
/// cancellation and structural checked/runtime invariant failures abort the
/// enclosing evaluation.
fn evaluate_plot(
    entry: &graphcal_compiler::ir::lower::PlotEntry,
    values: &RuntimeValueMap,
    presentation_values: &PresentationInstanceMap,
    errors: &HashMap<RuntimeDeclKey, NodeError>,
    ctx: &EvalContext<'_>,
) -> Result<PlotSpec, PlotEvaluationError> {
    // A reference to a failed declaration must report the root cause, not a
    // generic lookup failure on the missing value.
    let lowered = &entry.body;
    let body_exprs = lowered
        .encodings
        .iter()
        .map(|(_, expr)| expr)
        .chain(lowered.mark_properties.iter().map(|f| &f.value))
        .chain(lowered.properties.iter().map(|f| &f.value));
    if let Some(message) = dependency_failure_message(body_exprs, errors) {
        return Err(PlotEvaluationError::Render(message));
    }

    let owner = ctx.current_decl.as_ref().ok_or_else(|| {
        PlotEvaluationError::Fatal(ctx.internal_error(
            "plot evaluation has no canonical declaration owner",
            DiagnosticAnchor::WholeFile,
        ))
    })?;
    let channel_facts = ctx
        .current_dag
        .plot_channel_presentations(owner)
        .ok_or_else(|| {
            PlotEvaluationError::Fatal(ctx.internal_error(
                format!("checked presentation facts are missing for plot `{owner}`"),
                DiagnosticAnchor::WholeFile,
            ))
        })?;
    let mut encoding_meta = Vec::new();

    // Evaluate channels and apply their checked structured presentation before
    // row alignment. Numeric projection and axis labels consume the same fact.
    let empty_locals = HirLocalValueMap::root();
    let mut channel_data = Vec::new();
    for (channel, expr) in &lowered.encodings {
        let fact = channel_facts.get(channel).ok_or_else(|| {
            PlotEvaluationError::Fatal(ctx.internal_error(
                format!("checked presentation is missing channel `{channel}`"),
                expr.span,
            ))
        })?;
        let (data, unit_label) = evaluate_plot_channel(
            *channel,
            expr,
            fact,
            values,
            presentation_values,
            &empty_locals,
            ctx,
        )?;

        let dimension_label = fact.dimension().and_then(|dimension| {
            (!dimension.is_dimensionless())
                .then(|| ctx.registry.dimensions.format_dimension(dimension))
        });
        encoding_meta.push((
            *channel,
            AxisMeta {
                dimension_label,
                unit_label,
            },
        ));
        channel_data.push((*channel, data));
    }
    let encodings = super::plot_data::align_encoding_channels(&channel_data)?;

    // Evaluate mark properties (e.g., stroke_width, opacity). Unknown names
    // are rejected at check time (#845); one that still reaches evaluation
    // is an internal inconsistency.
    let mut mark_properties = Vec::new();
    for field in &lowered.mark_properties {
        let Some(mark_prop) = super::types::MarkProperty::from_name(field.name.as_str()) else {
            return Err(PlotEvaluationError::Fatal(ctx.internal_error(
                format!("unknown checked mark property `{}`", field.name),
                field.value.span,
            )));
        };
        let field_value = eval_plot_property(&field.value, values, ctx)
            .map_err(|e| format!("mark property `{}`: {e}", field.name))?;
        mark_properties.push((mark_prop, field_value));
    }

    // Evaluate top-level properties (e.g., title, width, height)
    let mut properties = Vec::new();
    for field in &lowered.properties {
        let Some(plot_prop) = super::types::PlotProperty::from_name(field.name.as_str()) else {
            return Err(PlotEvaluationError::Fatal(ctx.internal_error(
                format!("unknown checked plot property `{}`", field.name),
                field.value.span,
            )));
        };
        let field_value = eval_plot_property(&field.value, values, ctx)
            .map_err(|e| format!("property `{}`: {e}", field.name))?;
        check_positive_property(plot_prop.name(), plot_prop.value_type(), &field_value)?;
        properties.push((plot_prop, field_value));
    }

    Ok(PlotSpec {
        name: entry.name.clone(),
        mark_type: entry.mark_type,
        encodings,
        encoding_meta,
        mark_properties,
        properties,
        displayed: entry.displayed,
    })
}

fn evaluate_plot_channel(
    channel: graphcal_compiler::syntax::ast::EncodingChannel,
    expr: &graphcal_compiler::hir::Expr,
    fact: &graphcal_compiler::tir::presentation::PlotChannelPresentation,
    values: &RuntimeValueMap,
    presentation_values: &PresentationInstanceMap,
    locals: &HirLocalValueMap<'_>,
    ctx: &EvalContext<'_>,
) -> Result<(super::plot_data::ChannelData, Option<String>), PlotEvaluationError> {
    if let graphcal_compiler::hir::ExprKind::StringLiteral(value) = &expr.kind {
        return Ok((
            super::plot_data::ChannelData::unindexed_label(value.clone()),
            None,
        ));
    }
    let evaluated = eval_hir_expr_with_presentation(expr, values, presentation_values, locals, ctx)
        .map_err(|error| classify_plot_channel_error(channel, error))?;
    let (runtime, presentation_instance) = evaluated.into_parts();
    let declared_type =
        plot_declared_type(fact.shape(), ctx, expr.span).map_err(PlotEvaluationError::Fatal)?;
    let mut presented = EvaluatedValue::new(&runtime, &declared_type)
        .project(ctx.tir, ctx.src)
        .map_err(PlotEvaluationError::Fatal)?;
    attach_presentation(
        &mut presented,
        fact.provenance(),
        Some(&presentation_instance),
        ctx,
        values,
    )
    .map_err(|error| classify_plot_channel_error(channel, error))?;
    validate_display_projection(&presented)
        .map_err(|error| format!("encoding channel `{channel}`: {error}"))?;
    let unit_label = super::plot_data::uniform_quantity_unit_label(&presented)
        .map_err(|error| format!("encoding channel `{channel}`: {error}"))?;
    let data = super::plot_data::channel_data_from_presented_value(&runtime, &presented)
        .map_err(|error| format!("encoding channel `{channel}`: {error}"))?;
    Ok((data, unit_label))
}

fn classify_plot_channel_error(
    channel: graphcal_compiler::syntax::ast::EncodingChannel,
    error: GraphcalError,
) -> PlotEvaluationError {
    match error {
        error @ (GraphcalError::InternalError { .. } | GraphcalError::Cancelled(_)) => {
            PlotEvaluationError::Fatal(error)
        }
        error => PlotEvaluationError::Render(format!(
            "encoding channel `{channel}`: {}",
            eval_failed_node_error(&error)
        )),
    }
}

/// Convert the retained checked plot shape into the public projection type.
fn plot_declared_type(
    shape: &graphcal_compiler::plot_shape::PlotChannelShape,
    ctx: &EvalContext<'_>,
    span: Span,
) -> Result<DeclaredType, GraphcalError> {
    let leaf = match shape.leaf() {
        PlotLeafKind::Quantity(dimension) => DeclaredType::Quantity(dimension.clone()),
        PlotLeafKind::Int => DeclaredType::Int,
        PlotLeafKind::Bool => DeclaredType::Bool,
        PlotLeafKind::Datetime(scale) => DeclaredType::Datetime(*scale),
        PlotLeafKind::Key(index) => DeclaredType::Key(index.clone()),
        PlotLeafKind::ContextualString => {
            return Err(ctx.internal_error(
                "contextual string plot channel reached runtime projection",
                span,
            ));
        }
    };
    Ok(shape
        .axes()
        .iter()
        .rev()
        .fold(leaf, |element, index| DeclaredType::Indexed {
            element: Box::new(element),
            index: index.clone(),
        }))
}

/// Evaluated fields of a figure/layer declaration.
struct CompositionFields {
    properties: Vec<(super::types::CompositionProperty, PlotFieldValue)>,
    plot_names: Vec<ScopedName>,
}

/// Evaluate composition fields (properties and plot names) shared by figures and layers.
fn eval_composition_fields(
    fields: &[graphcal_compiler::tir::typed::LoweredPlotField],
    plot_name_spans: &[graphcal_compiler::syntax::span::Spanned<ScopedName>],
    values: &RuntimeValueMap,
    ctx: &EvalContext<'_>,
) -> Result<CompositionFields, String> {
    let empty_locals = HirLocalValueMap::root();
    let mut properties = Vec::new();
    for field in fields {
        let Some(comp_prop) = super::types::CompositionProperty::from_name(field.name.as_str())
        else {
            // Unknown names are rejected at check time (#845); an entry that
            // still reaches evaluation is an internal inconsistency.
            return Err(format!("internal: unknown property `{}`", field.name));
        };
        if let graphcal_compiler::hir::ExprKind::StringLiteral(s) = &field.value.kind {
            properties.push((comp_prop, PlotFieldValue::String(s.clone())));
            continue;
        }
        let rv = eval_hir_expr(&field.value, values, &empty_locals, ctx)
            .map_err(|e| format!("property `{}`: {}", field.name, eval_failed_node_error(&e)))?;
        let field_value = runtime_to_plot_field_value(&rv)
            .map_err(|e| format!("property `{}`: {e}", field.name))?;
        check_positive_property(comp_prop.name(), comp_prop.value_type(), &field_value)?;
        properties.push((comp_prop, field_value));
    }
    let plot_names = plot_name_spans.iter().map(|p| p.value.clone()).collect();
    Ok(CompositionFields {
        properties,
        plot_names,
    })
}

/// Enforce strictly positive values for `PositiveNumber` properties
/// (`width`, `height`) — value-dependent, so checked at evaluation time
/// (#845).
fn check_positive_property(
    property: &'static str,
    value_type: super::types::PlotPropertyType,
    value: &PlotFieldValue,
) -> Result<(), String> {
    if value_type != super::types::PlotPropertyType::PositiveNumber {
        return Ok(());
    }
    match value {
        PlotFieldValue::Number(n) if n.is_finite() && *n > 0.0 => Ok(()),
        PlotFieldValue::Number(n) => Err(format!(
            "property `{property}` must be a positive number, got {n}"
        )),
        _ => Err(format!("property `{property}` must be a positive number")),
    }
}

/// Convert a `RuntimeValue` to a `PlotFieldValue`.
///
/// A value that cannot be represented in a plot (a struct, or an indexed
/// value mixing kinds) is an error — never silently replaced by a
/// placeholder or by index variant names (#840).
fn runtime_to_plot_field_value(rv: &RuntimeValue) -> Result<PlotFieldValue, String> {
    match rv {
        RuntimeValue::Quantity(v) => Ok(PlotFieldValue::Number(*v)),
        RuntimeValue::Complex(_) => Err(
            "Complex values cannot be plotted directly; use re(), im(), abs(), or phase()"
                .to_string(),
        ),
        RuntimeValue::Int(i) => crate::eval_expr::numeric::exact_i64_to_f64(*i)
            .map(PlotFieldValue::Number)
            .map_err(|_| {
                format!(
                    "Int value {i} cannot be plotted exactly; convert it explicitly with to_float()"
                )
            }),
        RuntimeValue::Bool(b) => Ok(PlotFieldValue::String(b.to_string())),
        RuntimeValue::Label { variant, .. } => Ok(PlotFieldValue::String(variant.to_string())),
        RuntimeValue::Indexed { .. } => super::plot_data::flatten_to_field_value(rv),
        RuntimeValue::Struct { .. } => Err(format!("{} cannot be plotted", rv.kind())),
        RuntimeValue::Datetime(epoch) => super::types::epoch_to_rfc3339(epoch)
            .map(PlotFieldValue::Datetime)
            .map_err(|error| error.to_string()),
        RuntimeValue::CoordinateLabel { value, .. } => Ok(PlotFieldValue::Number(*value)),
    }
}
