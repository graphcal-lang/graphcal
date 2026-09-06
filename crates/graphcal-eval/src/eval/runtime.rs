//! Runtime evaluation: converting TIR execution results to Values,
//! running execution plans, and checking asserts.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::module_name::{ModuleAliasName, ScopedName};
use graphcal_compiler::syntax::span::Span;

use crate::assertion_eval::evaluate_assert_with_expected_fail;
use crate::decl_key::RuntimeDeclKey;
use crate::eval_expr::{
    EvalContext, HirLocalValueMap, RuntimeValue, RuntimeValueMap, eval_hir_expr,
    eval_hir_expr_with_presentation,
};
use crate::execution_frame::eval_failed_node_error;
use crate::runtime_presentation::PresentationInstanceMap;
use graphcal_compiler::declaration_category::DeclCategory;
use graphcal_compiler::plot_shape::PlotLeafKind;
use graphcal_compiler::registry::builtins::{BuiltinFunctions, builtin_functions};
use graphcal_compiler::registry::declared_type::DeclaredType;
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
    pub presentation_calls: crate::presentation_calls::EvaluatedPresentationCalls,
}

/// One completed runtime evaluation before project-level public output assembly.
///
/// Keeping values, contained node errors, and the display-aware root result in
/// one artifact makes the evaluator's direct output available to debugging
/// consumers without running the evaluator a second time.
fn root_instance_name(
    root: &graphcal_compiler::dag_id::DagId,
    parent: &graphcal_compiler::dag_id::DagId,
    exposed: &ScopedName,
) -> ScopedName {
    let parent_path = parent
        .segments()
        .iter()
        .skip(root.segments().len())
        .map(|segment| ModuleAliasName::expect_valid(segment.as_ref().to_owned()));
    ScopedName::qualified_path(
        parent_path.chain(exposed.qualifier().iter().cloned()),
        exposed.member().clone(),
    )
}

#[derive(Clone, Copy)]
enum OutputExposure {
    Surface,
    Debug,
}

#[derive(Default)]
struct RuntimeResultValueAssembly {
    identities: HashMap<ScopedName, RuntimeDeclKey>,
    all: Vec<(ScopedName, Result<Value, NodeError>, DeclType)>,
    output_surface: std::collections::HashSet<ScopedName>,
}

struct AssembledRuntimeResultValues {
    consts: Vec<(ScopedName, Result<Value, NodeError>)>,
    params: Vec<(ScopedName, Result<Value, NodeError>)>,
    nodes: Vec<(ScopedName, Result<Value, NodeError>)>,
    all: Vec<(ScopedName, Result<Value, NodeError>, DeclType)>,
    output_surface: std::collections::HashSet<ScopedName>,
}

impl RuntimeResultValueAssembly {
    fn insert(
        &mut self,
        key: RuntimeDeclKey,
        name: ScopedName,
        result: Result<Value, NodeError>,
        decl_type: DeclType,
        exposure: OutputExposure,
        src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        if matches!(exposure, OutputExposure::Surface) {
            self.output_surface.insert(name.clone());
        }
        match self.identities.get(&name) {
            Some(existing) if existing == &key => return Ok(()),
            Some(existing) => {
                return Err(GraphcalError::internal_error(
                    format!(
                        "runtime output name `{name}` identifies both `{existing}` and `{key}`"
                    ),
                    src,
                    DiagnosticAnchor::WholeFile,
                ));
            }
            None => {}
        }
        self.identities.insert(name.clone(), key);
        self.all.push((name, result, decl_type));
        Ok(())
    }

    fn finish(self) -> AssembledRuntimeResultValues {
        let category = |expected| {
            self.all
                .iter()
                .filter(|(_, _, decl_type)| *decl_type == expected)
                .map(|(name, result, _)| (name.clone(), result.clone()))
                .collect()
        };
        AssembledRuntimeResultValues {
            consts: category(DeclType::Const),
            params: category(DeclType::Param),
            nodes: category(DeclType::Node),
            all: self.all,
            output_surface: self.output_surface,
        }
    }
}

fn project_runtime_value(
    runtime: &RuntimeValue,
    declared_type: &DeclaredType,
    presentation: &graphcal_compiler::tir::presentation::PresentationProvenance,
    presentation_instance: Option<&crate::runtime_presentation::PresentationInstance>,
    ctx: &EvalContext<'_>,
    values: &RuntimeValueMap,
) -> Result<Result<Value, NodeError>, GraphcalError> {
    let mut value = EvaluatedValue::new(runtime, declared_type).project(ctx.tir, ctx.src)?;
    if let Err(error) =
        attach_presentation(&mut value, presentation, presentation_instance, ctx, values)
    {
        return match error {
            error @ (GraphcalError::InternalError { .. } | GraphcalError::Cancelled(_)) => {
                Err(error)
            }
            error => Ok(Err(eval_failed_node_error(&error))),
        };
    }
    Ok(validate_display_projection(&value)
        .map(|()| value)
        .map_err(|error| NodeError::EvalFailed {
            message: error.to_string(),
        }))
}

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

/// Execute the root with ordinary failures contained by the shared machine.
pub(super) fn run_eval_loop_with_bindings(
    plan: &crate::execution_plan::ExecPlan,
    bindings: &super::bindings::RuntimeParameterBindings,
    tir: &graphcal_compiler::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    builtin_fns: &BuiltinFunctions,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<EvalLoopResult, GraphcalError> {
    use crate::execution_frame::{ExecutionFrame, FailurePolicy};
    cancellation.checkpoint()?;
    let empty_hir_locals = HirLocalValueMap::root();
    let presentation_calls = crate::presentation_calls::EvaluatedPresentationCalls::default();
    let mut frame =
        ExecutionFrame::new(plan, tir.root_dag_id(), FailurePolicy::Contain).map_err(|error| {
            GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
        })?;
    for (key, binding) in bindings {
        frame.bind(key, binding.value.clone(), src, Span::new(0, 0))?;
    }
    frame.run(tir, src, cancellation, |entry, frame| {
        // Root declarations keep their existing work allowance; nested calls
        // share this context's budget through immutable scope reselection.
        let context = EvalContext::checked(
            tir,
            plan,
            entry.scope.dag().dag_id(),
            entry.scope.facts().source(),
            builtin_fns,
            host_fns,
            cancellation.clone(),
        )?
        .with_presentation_calls(&presentation_calls)
        .with_roots(&frame.values, Some(&frame.presentations))
        .for_decl(entry.key.as_resolved());
        eval_hir_expr_with_presentation(
            entry.expression,
            &frame.values,
            &frame.presentations,
            &empty_hir_locals,
            &context,
        )
    })?;
    Ok(EvalLoopResult {
        values: frame.values,
        presentation_instances: frame.presentations,
        errors: frame.errors,
        presentation_calls,
    })
}

/// Evaluate using immutable TIR plus one plan and validated runtime bindings.
///
/// Runtime errors are contained per-node: if a node fails, independent nodes
/// still evaluate, and dependent nodes receive a `DependencyFailed` error.
/// Internal invariant violations abort evaluation as `X001`.
/// Evaluate a plan with one row of runtime parameter bindings.
pub(super) fn evaluate_plan_with_bindings_and_cancellation(
    tir: &graphcal_compiler::tir::typed::TIR,
    plan: &crate::execution_plan::ExecPlan,
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
    plan: &crate::execution_plan::ExecPlan,
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
    let ctx = EvalContext::checked(
        tir,
        plan,
        tir.root_dag_id(),
        src,
        builtin_fns,
        host_fns,
        cancellation.clone(),
    )?
    .with_roots(&values, Some(&presentation_instances))
    .with_presentation_calls(&presentation_calls);

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
        project_runtime_value(
            runtime,
            declared_type,
            presentation,
            presentation_instances.get(&runtime_key),
            &ctx,
            &values,
        )
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

    let mut result_values = RuntimeResultValueAssembly::default();
    for (name, category) in tir.root().source_order() {
        let decl_type = match category {
            DeclCategory::Const => DeclType::Const,
            DeclCategory::Param => DeclType::Param,
            DeclCategory::Node => DeclType::Node,
            DeclCategory::Assert
            | DeclCategory::Plot
            | DeclCategory::Figure
            | DeclCategory::Layer => continue,
        };
        let key = local_key(name)?;
        let value = match decl_type {
            DeclType::Const => plan.root.const_values.get(&key).map_or_else(
                || {
                    Err(GraphcalError::internal_error(
                        format!("checked source-order constant `{key}` has no runtime value"),
                        src,
                        DiagnosticAnchor::WholeFile,
                    ))
                },
                |runtime| make_value(name, runtime),
            )?,
            DeclType::Param | DeclType::Node => make_result(name)?,
        };
        result_values.insert(
            key,
            name.clone(),
            value,
            decl_type,
            OutputExposure::Surface,
            src,
        )?;
    }
    cancellation.checkpoint()?;

    let debug_scope_counts = tir.root().semantic_instances().iter().fold(
        HashMap::<ModuleAliasName, usize>::new(),
        |mut counts, record| {
            counts
                .entry(record.debug_scope.clone())
                .and_modify(|count| *count = count.saturating_add(1))
                .or_insert(1);
            counts
        },
    );
    for record in tir.root().semantic_instances() {
        let instance_dag = tir
            .dag_registry()
            .get(record.instance.id.owner())
            .ok_or_else(|| {
                GraphcalError::internal_error(
                    format!(
                        "semantic instance `{}` is absent from checked TIR",
                        record.instance.id.owner()
                    ),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
        let instance_src = plan
            .checked_execution_facts
            .for_dag(instance_dag.dag_id())
            .map_or(
                src,
                crate::execution_facts::CheckedDagExecutionFacts::source,
            );
        for projection in &record.output_projections {
            if tir
                .root()
                .source_order()
                .iter()
                .any(|(name, _)| name == &projection.exposed_name)
            {
                continue;
            }
            let declaration = instance_dag.runtime_decl_identity(&projection.target);
            let key = RuntimeDeclKey::resolved(declaration.clone());
            let decl_type = instance_dag
                .source_order()
                .iter()
                .find_map(|(name, category)| {
                    (instance_dag.bound_decl_identity(name) == Some(&declaration)).then_some(
                        match category {
                            DeclCategory::Const => Some(DeclType::Const),
                            DeclCategory::Param => Some(DeclType::Param),
                            DeclCategory::Node => Some(DeclType::Node),
                            DeclCategory::Assert
                            | DeclCategory::Plot
                            | DeclCategory::Figure
                            | DeclCategory::Layer => None,
                        },
                    )
                })
                .flatten()
                .ok_or_else(|| {
                    GraphcalError::internal_error(
                        format!("projected declaration `{declaration}` is not a runtime value"),
                        src,
                        DiagnosticAnchor::WholeFile,
                    )
                })?;
            let value = if let Some(error) = errors.get(&key) {
                Err(error.clone())
            } else {
                let runtime = values.get(&key).ok_or_else(|| {
                    GraphcalError::internal_error(
                        format!("projected declaration `{declaration}` has no runtime value"),
                        src,
                        DiagnosticAnchor::WholeFile,
                    )
                })?;
                let declared_type = tir.runtime_declared_type(&declaration, src)?;
                let presentation = instance_dag
                    .declaration_presentation(&declaration)
                    .ok_or_else(|| {
                        GraphcalError::internal_error(
                            format!(
                                "checked presentation facts are missing for instance declaration `{declaration}`"
                            ),
                            instance_src,
                            DiagnosticAnchor::WholeFile,
                        )
                    })?;
                project_runtime_value(
                    runtime,
                    &declared_type,
                    presentation,
                    presentation_instances.get(&key),
                    &ctx.for_checked_decl(instance_dag, instance_src, &declaration)?,
                    &values,
                )?
            };
            result_values.insert(
                key,
                projection.exposed_name.clone(),
                value,
                decl_type,
                OutputExposure::Surface,
                src,
            )?;
        }
        for (name, category) in instance_dag.source_order() {
            let decl_type = match category {
                DeclCategory::Const => DeclType::Const,
                DeclCategory::Param => DeclType::Param,
                DeclCategory::Node => DeclType::Node,
                DeclCategory::Assert
                | DeclCategory::Plot
                | DeclCategory::Figure
                | DeclCategory::Layer => continue,
            };
            let declaration =
                instance_dag.require_bound_decl_identity(name, src, DiagnosticAnchor::WholeFile)?;
            let key = RuntimeDeclKey::resolved(declaration.clone());
            let value = if let Some(error) = errors.get(&key) {
                Err(error.clone())
            } else {
                let runtime = values.get(&key).ok_or_else(|| {
                    GraphcalError::internal_error(
                        format!("instance declaration `{declaration}` has no runtime value"),
                        src,
                        DiagnosticAnchor::WholeFile,
                    )
                })?;
                let declared_type = tir.runtime_declared_type(&declaration, src)?;
                let presentation = instance_dag
                    .declaration_presentation(&declaration)
                    .ok_or_else(|| {
                        GraphcalError::internal_error(
                            format!(
                                "checked presentation facts are missing for instance declaration `{declaration}`"
                            ),
                            instance_src,
                            DiagnosticAnchor::WholeFile,
                        )
                    })?;
                project_runtime_value(
                    runtime,
                    &declared_type,
                    presentation,
                    presentation_instances.get(&key),
                    &ctx.for_checked_decl(instance_dag, instance_src, &declaration)?,
                    &values,
                )?
            };
            let debug_scope = if debug_scope_counts
                .get(&record.debug_scope)
                .is_some_and(|count| *count > 1)
            {
                ModuleAliasName::expect_valid(record.instance.id.owner().name())
            } else {
                record.debug_scope.clone()
            };
            let debug_name = ScopedName::qualified(debug_scope, name.member().clone());
            result_values.insert(
                key,
                debug_name,
                value,
                decl_type,
                OutputExposure::Debug,
                src,
            )?;
        }
    }
    cancellation.checkpoint()?;
    let AssembledRuntimeResultValues {
        consts,
        params,
        nodes,
        all,
        output_surface,
    } = result_values.finish();

    // Evaluate assertions in source order, applying expected_fail inversion.
    // An assertion whose body references a failed declaration reports the
    // dependency failure (with its root cause) instead of evaluating over a
    // value map where the failed name is simply absent (#814).
    let mut assertions: Vec<(ScopedName, AssertResult, Span)> = tir
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
                    let ef = plan
                        .root
                        .expected_fail
                        .get(&RuntimeDeclKey::resolved(owner.clone()));
                    evaluate_assert_with_expected_fail(&entry.body, ef, &mut |expr| {
                        eval_hir_expr(expr, &values, &empty_hir_locals, &entry_ctx)
                    })
                },
                |message| AssertResult::Error { message },
            );
            Ok((entry.name.clone(), assert_result, entry.span))
        })
        .collect::<Result<_, GraphcalError>>()?;
    let mut semantic_parents = tir
        .local_dags()
        .map(|(_, dag)| dag)
        .filter(|dag| dag.dag_id() == tir.root_dag_id() || dag.is_semantic_instance())
        .collect::<Vec<_>>();
    semantic_parents.sort_by(|left, right| left.dag_id().cmp(right.dag_id()));
    for parent_dag in semantic_parents {
        for record in parent_dag.semantic_instances() {
            let instance_dag = tir
                .dag_registry()
                .get(record.instance.id.owner())
                .ok_or_else(|| {
                    GraphcalError::internal_error(
                        format!(
                            "semantic instance `{}` is absent from checked TIR",
                            record.instance.id.owner()
                        ),
                        src,
                        DiagnosticAnchor::WholeFile,
                    )
                })?;
            for projection in &record.assertion_projections {
                let owner = instance_dag.runtime_decl_identity(&projection.target);
                let entry = instance_dag
                    .asserts()
                    .iter()
                    .find(|entry| instance_dag.bound_decl_identity(&entry.name) == Some(&owner))
                    .ok_or_else(|| {
                        GraphcalError::internal_error(
                            format!(
                                "projected assertion `{owner}` is absent from semantic instance"
                            ),
                            src,
                            DiagnosticAnchor::WholeFile,
                        )
                    })?;
                let expected = projection.expected_fail.as_ref().or_else(|| {
                    plan.root
                        .expected_fail
                        .get(&RuntimeDeclKey::resolved(owner.clone()))
                });
                let assertion_ctx = ctx.for_checked_decl(instance_dag, src, &owner)?;
                let result =
                    evaluate_assert_with_expected_fail(&entry.body, expected, &mut |expr| {
                        eval_hir_expr(expr, &values, &empty_hir_locals, &assertion_ctx)
                    });
                assertions.push((
                    root_instance_name(
                        tir.root_dag_id(),
                        parent_dag.dag_id(),
                        &projection.exposed_name,
                    ),
                    result,
                    entry.span,
                ));
            }
        }
    }
    cancellation.checkpoint()?;

    // Evaluate plot declarations. Evaluation is per-plot best-effort, but a
    // plot that cannot be rendered is reported, never silently dropped
    // (#842).
    let mut plot_errors: Vec<super::types::PlotError> = Vec::new();
    let mut plots = tir
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
    for record in tir.root().semantic_instances() {
        let outer_instance = tir
            .dag_registry()
            .get(record.instance.id.owner())
            .ok_or_else(|| {
                GraphcalError::internal_error(
                    format!(
                        "semantic instance `{}` is absent from checked TIR",
                        record.instance.id.owner()
                    ),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
        for projection in &record.plot_projections {
            let owner = outer_instance.runtime_decl_identity(&projection.target);
            let plot_dag = tir
                .dag_registry()
                .get(owner.owner())
                .unwrap_or(outer_instance);
            let entry = plot_dag
                .plots()
                .iter()
                .find(|entry| entry.name.member().as_str() == owner.atom().as_str())
                .ok_or_else(|| {
                    GraphcalError::internal_error(
                        format!("projected plot `{owner}` is absent from semantic instance"),
                        src,
                        DiagnosticAnchor::WholeFile,
                    )
                })?;
            match evaluate_plot(
                entry,
                &values,
                &presentation_instances,
                &errors,
                &ctx.for_checked_decl(plot_dag, src, &owner)?,
            ) {
                Ok(mut plot) => {
                    plot.name = projection.exposed_name.clone();
                    plot.displayed = !projection.hidden;
                    plots.push(plot);
                }
                Err(PlotEvaluationError::Render(message)) => {
                    plot_errors.push(super::types::PlotError {
                        name: projection.exposed_name.clone(),
                        message,
                    });
                }
                Err(PlotEvaluationError::Fatal(error)) => return Err(error),
            }
        }
    }
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
                .root
                .domain_constraints
                .get(&key)
                .map(|constraint| (name.clone(), constraint.clone())))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?
        .into_iter()
        .flatten()
        .collect();
    cancellation.checkpoint()?;
    let mut source_names_by_key = tir
        .root()
        .source_order()
        .iter()
        .map(|(name, _)| local_key(name).map(|key| (key, name.clone())))
        .collect::<Result<HashMap<_, _>, GraphcalError>>()?;
    for record in tir.root().semantic_instances() {
        let instance_dag = tir
            .dag_registry()
            .get(record.instance.id.owner())
            .ok_or_else(|| {
                GraphcalError::internal_error(
                    format!(
                        "semantic instance `{}` is absent from checked TIR",
                        record.instance.id.owner()
                    ),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
        for projection in &record.output_projections {
            source_names_by_key.insert(
                RuntimeDeclKey::resolved(instance_dag.runtime_decl_identity(&projection.target)),
                projection.exposed_name.clone(),
            );
        }
        for projection in &record.assertion_projections {
            source_names_by_key.insert(
                RuntimeDeclKey::resolved(instance_dag.runtime_decl_identity(&projection.target)),
                projection.exposed_name.clone(),
            );
        }
    }
    let assumes_map = plan
        .root
        .assumes_map
        .iter()
        .map(|(assertion, assumers)| {
            let assertion_name = source_names_by_key.get(assertion).cloned().ok_or_else(|| {
                GraphcalError::internal_error(
                    format!("assertion `{assertion}` is missing from checked source order"),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
            let assumer_names = assumers
                .iter()
                .map(|assumer| {
                    source_names_by_key.get(assumer).cloned().ok_or_else(|| {
                        GraphcalError::internal_error(
                            format!(
                                "assertion assumer `{assumer}` is missing from checked source order"
                            ),
                            src,
                            DiagnosticAnchor::WholeFile,
                        )
                    })
                })
                .collect::<Result<Vec<_>, GraphcalError>>()?;
            Ok((assertion_name, assumer_names))
        })
        .collect::<Result<HashMap<_, _>, GraphcalError>>()?;

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
    if let graphcal_compiler::hir::ExprKind::StringLiteral(s) = expr.kind() {
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
        .map(|(_, expr)| &**expr)
        .chain(lowered.mark_properties.iter().map(|field| &*field.value))
        .chain(lowered.properties.iter().map(|field| &*field.value));
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
        let graphcal_compiler::ir::lower::LoweredPlotProperty::Mark(mark_prop) = &field.property
        else {
            return Err(PlotEvaluationError::Fatal(ctx.internal_error(
                format!(
                    "checked mark property has incompatible classification `{}`",
                    field.property.name()
                ),
                field.value.span,
            )));
        };
        let field_value = eval_plot_property(&field.value, values, ctx)
            .map_err(|error| format!("mark property `{}`: {error}", field.property.name()))?;
        mark_properties.push((*mark_prop, field_value));
    }

    // Evaluate top-level properties (e.g., title, width, height)
    let mut properties = Vec::new();
    for field in &lowered.properties {
        let graphcal_compiler::ir::lower::LoweredPlotProperty::Plot(plot_prop) = &field.property
        else {
            return Err(PlotEvaluationError::Fatal(ctx.internal_error(
                format!(
                    "checked plot property has incompatible classification `{}`",
                    field.property.name()
                ),
                field.value.span,
            )));
        };
        let field_value = eval_plot_property(&field.value, values, ctx)
            .map_err(|error| format!("property `{}`: {error}", plot_prop.name()))?;
        check_positive_property(plot_prop.name(), plot_prop.value_type(), &field_value)?;
        properties.push((*plot_prop, field_value));
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
    if let graphcal_compiler::hir::ExprKind::StringLiteral(value) = expr.kind() {
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
        let graphcal_compiler::ir::lower::LoweredPlotProperty::Composition(comp_prop) =
            &field.property
        else {
            return Err(format!(
                "internal: checked composition property has incompatible classification `{}`",
                field.property.name()
            ));
        };
        if let graphcal_compiler::hir::ExprKind::StringLiteral(s) = field.value.kind() {
            properties.push((*comp_prop, PlotFieldValue::String(s.clone())));
            continue;
        }
        let rv = eval_hir_expr(&field.value, values, &empty_locals, ctx).map_err(|error| {
            format!(
                "property `{}`: {}",
                comp_prop.name(),
                eval_failed_node_error(&error)
            )
        })?;
        let field_value = runtime_to_plot_field_value(&rv)
            .map_err(|error| format!("property `{}`: {error}", comp_prop.name()))?;
        check_positive_property(comp_prop.name(), comp_prop.value_type(), &field_value)?;
        properties.push((*comp_prop, field_value));
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
        RuntimeValue::Quantity(v) => Ok(PlotFieldValue::Number(v.get())),
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
        RuntimeValue::CoordinateLabel { value, .. } => Ok(PlotFieldValue::Number(value.get())),
    }
}
