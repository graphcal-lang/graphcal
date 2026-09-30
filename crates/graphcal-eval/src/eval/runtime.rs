//! Runtime evaluation: converting TIR execution results to Values,
//! running execution plans, and checking asserts.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::module_name::{ModuleAliasName, ScopeSegment, ScopedName};
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::typed::{
    AssertionOperands, CheckedInstance, DeclarationBody, ResolvedProjection, Scoped,
};

use crate::assertion_eval::evaluate_assert_with_expected_fail;
use crate::eval_expr::{
    EvalSession, RuntimeValue, RuntimeValueMap, eval_root, eval_root_with_presentation,
};
use crate::execution_frame::eval_failed_node_error;
use crate::presentation_evidence::{
    LeafPresentationDiagnostic, PresentationDiagnostic, PresentationFailure, PresentationInstance,
    PresentationInstanceMap,
};
use graphcal_compiler::declaration_category::{DeclCategory, ValueDeclCategory};
use graphcal_compiler::plot_shape::PlotLeafKind;
use graphcal_compiler::registry::checked_type::{CheckedType, Symbolic};
use graphcal_compiler::registry::error::GraphcalError;

use super::display::attach_presentation;
use super::public_projection::EvaluatedValue;
use super::types::{
    AssertResult, AxisMeta, EvalResult, NodeUnavailable, PlotFieldValue, PlotSpec, Value,
};

/// Result of running the core eval loop: successfully evaluated values and per-node errors.
pub(super) struct EvalLoopResult {
    pub unfinished_calls: std::cell::RefCell<BTreeSet<ResolvedDeclName>>,
    pub values: RuntimeValueMap,
    pub presentation_instances: PresentationInstanceMap,
    pub errors: HashMap<ResolvedDeclName, NodeUnavailable>,
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
    // A parent outside the root's subtree contributes no qualifier.
    let parent_path = parent.scopes_below(root).into_iter().flatten();
    ScopedName::from_parts(
        graphcal_compiler::syntax::non_empty::NonEmpty::try_from_vec(
            parent_path
                .chain(exposed.qualifier().iter().cloned())
                .collect(),
        )
        .ok(),
        exposed.leaf().clone(),
    )
}

#[derive(Clone, Copy)]
enum OutputExposure {
    Surface,
    Debug,
}

#[derive(Default)]
struct RuntimeResultValueAssembly {
    identities: HashMap<ScopedName, ResolvedDeclName>,
    entries: Vec<(
        ScopedName,
        Result<Value, NodeUnavailable>,
        ValueDeclCategory,
    )>,
    output_surface: std::collections::HashSet<ScopedName>,
}

impl RuntimeResultValueAssembly {
    fn insert(
        &mut self,
        key: ResolvedDeclName,
        name: ScopedName,
        result: Result<Value, NodeUnavailable>,
        decl_type: ValueDeclCategory,
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
        self.entries.push((name, result, decl_type));
        Ok(())
    }
}

fn project_runtime_value(
    runtime: &RuntimeValue,
    declared_type: &CheckedType,
    presentation_instance: Option<&PresentationInstance>,
    ctx: &EvalSession<'_>,
    diagnostics: &std::cell::RefCell<Vec<PresentationDiagnostic>>,
) -> Result<Result<Value, NodeUnavailable>, GraphcalError> {
    let mut value = EvaluatedValue::new(runtime, declared_type).project(ctx.tir, ctx.src)?;
    let notices = attach_presentation(&mut value, presentation_instance)
        .map_err(|error| ctx.internal_error(error.to_string(), DiagnosticAnchor::WholeFile))?;
    let declaration = ctx.current_decl.as_ref().ok_or_else(|| {
        ctx.internal_error(
            "output projection has no declaration",
            DiagnosticAnchor::WholeFile,
        )
    })?;
    diagnostics
        .borrow_mut()
        .extend(notices.into_iter().map(|detail| PresentationDiagnostic {
            declaration: declaration.clone(),
            channel: None,
            detail,
        }));
    Ok(Ok(value))
}

pub struct RuntimeEvaluation {
    pub(super) result: EvalResult,
    pub(super) presentation_instances: PresentationInstanceMap,
    pub(super) values: RuntimeValueMap,
    pub(super) errors: HashMap<ResolvedDeclName, NodeUnavailable>,
}

impl std::fmt::Debug for RuntimeEvaluation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeEvaluation")
            .field("result", &self.result)
            .field("presentation_instances", &self.presentation_instances)
            .field("values", &self.values)
            .field("errors", &self.errors)
            .finish()
    }
}

impl RuntimeEvaluation {
    /// Whether evaluation produced a node, assertion, or plot failure.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.errors.values().any(NodeUnavailable::has_failure) || self.result.has_errors()
    }

    /// Display-aware result for the directly evaluated root DAG.
    #[must_use]
    pub const fn result(&self) -> &EvalResult {
        &self.result
    }
}

/// Execute the root with ordinary failures contained by the shared machine.
pub(super) fn run_eval_loop_with_bindings(
    plan: &crate::execution_plan::ExecPlan<'_>,
    bindings: &super::bindings::RuntimeParameterBindings,
    src: &NamedSource<Arc<String>>,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<EvalLoopResult, GraphcalError> {
    use crate::execution_frame::{ExecutionFrame, FailurePolicy};
    cancellation.checkpoint()?;
    let unfinished_calls = std::cell::RefCell::new(BTreeSet::new());
    let mut frame = ExecutionFrame::new(plan, plan.root(), FailurePolicy::Contain);
    for (key, binding) in bindings {
        frame.bind_argument(
            key,
            crate::runtime_presentation::EvaluatedRuntimeValue::new(
                binding.value.clone(),
                binding.presentation.clone(),
            ),
            src,
            Span::new(0, 0),
        )?;
    }
    frame.run(cancellation, |entry, frame| {
        // Root declarations keep their existing work allowance; nested calls
        // share this context's budget through immutable scope reselection.
        let root = EvalSession::checked(plan, src, host_fns, cancellation.clone())
            .with_roots(frame.values(), Some(frame.presentations()))
            .with_unavailable(frame.errors())
            .with_unfinished_calls(&unfinished_calls);
        let session = root.for_declaration(&entry);
        eval_root_with_presentation(
            entry.body(),
            frame.values(),
            frame.presentations(),
            &session,
        )
    })?;
    let outcome = frame.finish();
    Ok(EvalLoopResult {
        unfinished_calls,
        values: outcome.values,
        presentation_instances: outcome.presentations,
        errors: outcome.errors,
    })
}

/// The checked declared type of a runtime declaration.
fn checked_declared_type<'a>(
    tir: &'a graphcal_compiler::tir::typed::CheckedTir,
    declaration: &ResolvedDeclName,
    src: &NamedSource<Arc<String>>,
) -> Result<&'a CheckedType, GraphcalError> {
    tir.decl_type(declaration)
        .map(graphcal_compiler::tir::typed::CheckedDeclType::declared)
        .ok_or_else(|| {
            GraphcalError::internal_error(
                format!("runtime declaration `{declaration}` is absent from checked TIR"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })
}

/// Evaluate using immutable TIR plus one plan and validated runtime bindings.
///
/// Runtime errors are contained per-node: if a node fails, independent nodes
/// still evaluate, and dependent nodes receive a `DependencyFailed` error.
/// Internal invariant violations abort evaluation as `X001`.
/// Evaluate a plan with one row of runtime parameter bindings.
#[expect(
    clippy::too_many_lines,
    reason = "linear evaluation pipeline is clearest as a single function"
)]
pub(super) fn evaluate_plan_with_values_and_bindings_and_cancellation(
    plan: &crate::execution_plan::ExecPlan<'_>,
    bindings: &super::bindings::RuntimeParameterBindings,
    src: &NamedSource<Arc<String>>,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<RuntimeEvaluation, GraphcalError> {
    cancellation.checkpoint()?;
    let tir = plan.tir();

    let EvalLoopResult {
        unfinished_calls,
        values,
        presentation_instances,
        errors,
    } = run_eval_loop_with_bindings(plan, bindings, src, host_fns, cancellation)?;

    cancellation.checkpoint()?;
    let ctx = EvalSession::checked(plan, src, host_fns, cancellation.clone())
        .with_roots(&values, Some(&presentation_instances))
        .with_unavailable(&errors)
        .with_unfinished_calls(&unfinished_calls);
    let presentation_instances = presentation_instances
        .iter()
        .map(|(key, evidence)| {
            crate::eval_expr::presentation::resolve(evidence.clone(), &values, &ctx)
                .map(|evidence| (key.clone(), evidence))
        })
        .collect::<Result<PresentationInstanceMap, _>>()?;
    let presentation_diagnostics = std::cell::RefCell::new(Vec::new());

    let make_value = |declaration: &ResolvedDeclName,
                      runtime: &RuntimeValue|
     -> Result<Result<Value, NodeUnavailable>, GraphcalError> {
        let declared_type = checked_declared_type(tir, declaration, src)?;
        project_runtime_value(
            runtime,
            declared_type,
            presentation_instances.get(declaration),
            &ctx.for_decl(declaration),
            &presentation_diagnostics,
        )
    };

    let make_result =
        |key: &ResolvedDeclName| -> Result<Result<Value, NodeUnavailable>, GraphcalError> {
            errors.get(key).map_or_else(
                || {
                    values.get(key).map_or_else(
                        || {
                            Err(GraphcalError::internal_error(
                                format!("successful declaration `{key}` has no runtime value"),
                                src,
                                DiagnosticAnchor::WholeFile,
                            ))
                        },
                        |runtime| make_value(key, runtime),
                    )
                },
                |error| Ok(Err(error.clone())),
            )
        };

    let mut result_values = RuntimeResultValueAssembly::default();
    for entry in tir.root().decls().iter() {
        let name = entry.name();
        let decl_type = match entry.category() {
            DeclCategory::Value(decl_type) => decl_type,
            DeclCategory::Assert
            | DeclCategory::Plot
            | DeclCategory::Figure
            | DeclCategory::Layer => continue,
        };
        let key = entry.identity().clone();
        let value = match decl_type {
            ValueDeclCategory::Const => plan.root().scope().const_values().get(&key).map_or_else(
                || {
                    Err(GraphcalError::internal_error(
                        format!("checked source-order constant `{key}` has no runtime value"),
                        src,
                        DiagnosticAnchor::WholeFile,
                    ))
                },
                |runtime| make_value(&key, runtime),
            )?,
            ValueDeclCategory::Param | ValueDeclCategory::Node => make_result(&key)?,
        };
        result_values.insert(
            key,
            ScopedName::local(name.clone()),
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
        let instance = semantic_instance(tir, record, src)?;
        let instance_dag = instance.dag();
        let instance_src = plan
            .program()
            .facts()
            .source(instance_dag.dag_id())
            .unwrap_or(src);
        for ResolvedProjection {
            target: declaration,
            projection,
        } in instance.output_projections()
        {
            // A selected value is declared by the including DAG itself (a
            // projection alias), whose own entry reports it.
            if projection.exposure.selected().is_some() {
                continue;
            }
            let key = declaration.clone();
            let decl_type = instance_dag
                .decls()
                .iter()
                .find_map(|entry| {
                    (entry.identity() == declaration).then_some(match entry.category() {
                        DeclCategory::Value(decl_type) => Some(decl_type),
                        DeclCategory::Assert
                        | DeclCategory::Plot
                        | DeclCategory::Figure
                        | DeclCategory::Layer => None,
                    })
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
                let declared_type = checked_declared_type(tir, &declaration, src)?;
                project_runtime_value(
                    runtime,
                    declared_type,
                    presentation_instances.get(&key),
                    &ctx.with_src(instance_src).for_decl(&declaration),
                    &presentation_diagnostics,
                )?
            };
            result_values.insert(
                key,
                instance.record().instance.exposed_name(projection),
                value,
                decl_type,
                OutputExposure::Surface,
                src,
            )?;
        }
        for entry in instance_dag.decls().iter() {
            let name = entry.name();
            let decl_type = match entry.category() {
                DeclCategory::Value(decl_type) => decl_type,
                DeclCategory::Assert
                | DeclCategory::Plot
                | DeclCategory::Figure
                | DeclCategory::Layer => continue,
            };
            let declaration = entry.identity().clone();
            let key = declaration.clone();
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
                let declared_type = checked_declared_type(tir, &declaration, src)?;
                project_runtime_value(
                    runtime,
                    declared_type,
                    presentation_instances.get(&key),
                    &ctx.with_src(instance_src).for_decl(&declaration),
                    &presentation_diagnostics,
                )?
            };
            let debug_scope = if debug_scope_counts
                .get(&record.debug_scope)
                .is_some_and(|count| *count > 1)
            {
                record.instance.id().scope().clone()
            } else {
                ScopeSegment::Named(record.debug_scope.clone())
            };
            let debug_name = ScopedName::in_scope(debug_scope, name.clone());
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
    let RuntimeResultValueAssembly {
        entries,
        output_surface,
        identities: _,
    } = result_values;

    let assertions = evaluate_assertions(tir, src, &ctx, &values, &errors)?;
    cancellation.checkpoint()?;

    // Evaluate plot declarations. Evaluation is per-plot best-effort, but a
    // plot that cannot be rendered is reported, never silently dropped
    // (#842).
    let mut plot_errors: Vec<super::types::PlotError> = Vec::new();
    let mut plots = tir
        .root()
        .plots()
        .try_fold(Vec::new(), |mut plots, entry| {
            let owner = entry.identity();
            let unit = declaration_body(tir, &owner, src)?;
            let plot = unit.plot().ok_or_else(|| {
                GraphcalError::internal_error(
                    format!("plot `{owner}` has no checked body"),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
            match evaluate_plot(unit, plot, &values, &presentation_instances, &errors, &ctx) {
                Ok(plot) => plots.push(plot),
                Err(PlotEvaluationError::Unavailable(reason)) => {
                    plot_errors.push(super::types::PlotError {
                        name: ScopedName::local(entry.name().clone()),
                        reason,
                    });
                }
                Err(PlotEvaluationError::Fatal(error)) => return Err(error),
            }
            Ok(plots)
        })?;
    for record in tir.root().semantic_instances() {
        for ResolvedProjection {
            target: owner,
            projection,
        } in semantic_instance(tir, record, src)?.plot_projections()
        {
            // The plot runs in the DAG that owns it, which may be an instance
            // nested in this one when the template forwards its own plot.
            let unit = declaration_body(tir, &owner, src)?;
            let plot = unit.plot().ok_or_else(|| {
                GraphcalError::internal_error(
                    format!("projected plot `{owner}` is absent from semantic instance"),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
            match evaluate_plot(unit, plot, &values, &presentation_instances, &errors, &ctx) {
                Ok(mut plot) => {
                    plot.name = ScopedName::local(projection.alias.clone());
                    plot.visibility = projection.visibility;
                    plots.push(plot);
                }
                Err(PlotEvaluationError::Unavailable(reason)) => {
                    plot_errors.push(super::types::PlotError {
                        name: ScopedName::local(projection.alias.clone()),
                        reason,
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
        .map(|entry| {
            let owner = entry.identity();
            let fields = declaration_body(tir, &owner, src)?
                .figure()
                .map(|figure| figure.map(|figure| figure.fields.as_slice()))
                .ok_or_else(|| {
                    GraphcalError::internal_error(
                        format!("figure `{owner}` has no checked body"),
                        src,
                        DiagnosticAnchor::WholeFile,
                    )
                })?;
            Ok(
                match check_plot_dependencies(&entry.plot_names, &plot_errors, tir.root(), &ctx)
                    .and_then(|()| {
                        eval_composition_fields(fields, &entry.plot_names, &values, &ctx)
                    }) {
                    Ok(evaluated) => Some(super::types::FigureSpec {
                        name: ScopedName::local(entry.name().clone()),
                        plot_names: evaluated.plot_names,
                        properties: evaluated.properties,
                    }),
                    Err(PlotEvaluationError::Fatal(error)) => return Err(error),
                    Err(PlotEvaluationError::Unavailable(reason)) => {
                        plot_errors.push(super::types::PlotError {
                            name: ScopedName::local(entry.name().clone()),
                            reason,
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
        .map(|entry| {
            let owner = entry.identity();
            let fields = declaration_body(tir, &owner, src)?
                .layer()
                .map(|layer| layer.map(|layer| layer.fields.as_slice()))
                .ok_or_else(|| {
                    GraphcalError::internal_error(
                        format!("layer `{owner}` has no checked body"),
                        src,
                        DiagnosticAnchor::WholeFile,
                    )
                })?;
            Ok(
                match check_plot_dependencies(&entry.plot_names, &plot_errors, tir.root(), &ctx)
                    .and_then(|()| {
                        eval_composition_fields(fields, &entry.plot_names, &values, &ctx)
                    }) {
                    Ok(evaluated) => Some(super::types::LayerSpec {
                        name: ScopedName::local(entry.name().clone()),
                        plot_names: evaluated.plot_names,
                        properties: evaluated.properties,
                    }),
                    Err(PlotEvaluationError::Fatal(error)) => return Err(error),
                    Err(PlotEvaluationError::Unavailable(reason)) => {
                        plot_errors.push(super::types::PlotError {
                            name: ScopedName::local(entry.name().clone()),
                            reason,
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
        .decls()
        .iter()
        .filter_map(|entry| {
            plan.domain_constraint(&entry.identity())
                .map(|constraint| (ScopedName::local(entry.name().clone()), constraint.clone()))
        })
        .collect();
    cancellation.checkpoint()?;
    let source_names_by_key = root_source_names(tir, src)?
        .into_iter()
        .collect::<HashMap<_, _>>();
    let assumes_map = merge_assumes_maps(
        plan.root()
            .execution_dags()
            .iter()
            .map(|scope| scope.dag().assumes_map()),
    )
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

    presentation_diagnostics.borrow_mut().extend(
        plots
            .iter()
            .flat_map(|plot| plot.presentation_diagnostics.iter().cloned()),
    );
    let result = EvalResult {
        unfinished_calls: unfinished_calls.into_inner().into_iter().collect(),
        entries,
        output_surface,
        assertions,
        plots,
        plot_errors,
        presentation_diagnostics: presentation_diagnostics.into_inner(),
        figures,
        layers,
        assumes_map,
        render: super::types::RenderContext::new(
            tir.registry().dimensions.base_unit_symbols(),
            tir.registry().time_zones.clone(),
        ),
        domain_constraints,
    };
    Ok(RuntimeEvaluation {
        result,
        presentation_instances,
        values,
        errors,
    })
}

/// Merge per-DAG `#[assumes]` tables keyed by runtime identity.
///
/// One assertion can be assumed both inside its semantic instance and by the
/// importer through a projection, so tables of different DAGs share keys.
fn merge_assumes_maps<'a>(
    maps: impl IntoIterator<Item = &'a HashMap<ResolvedDeclName, Vec<ResolvedDeclName>>>,
) -> HashMap<ResolvedDeclName, Vec<ResolvedDeclName>> {
    let mut merged = HashMap::<ResolvedDeclName, Vec<ResolvedDeclName>>::new();
    for (assertion, assumers) in maps.into_iter().flatten() {
        let entry = merged.entry(assertion.clone()).or_default();
        for assumer in assumers {
            if !entry.contains(assumer) {
                entry.push(assumer.clone());
            }
        }
    }
    merged
}

/// One semantic-instance record paired with the checked DAG it materialized.
fn semantic_instance<'tir>(
    tir: &'tir graphcal_compiler::tir::typed::CheckedTir,
    record: &'tir graphcal_compiler::ir::instance::HirInstanceRecord,
    src: &NamedSource<Arc<String>>,
) -> Result<CheckedInstance<'tir>, GraphcalError> {
    tir.dag_registry().semantic_instance(record).ok_or_else(|| {
        GraphcalError::internal_error(
            format!(
                "semantic instance `{}` is absent from checked TIR",
                record.instance.id().owner()
            ),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })
}

/// The source of `declaration` in the scope of its owner.
fn declaration_body<'tir>(
    tir: &'tir graphcal_compiler::tir::typed::CheckedTir,
    declaration: &ResolvedDeclName,
    src: &NamedSource<Arc<String>>,
) -> Result<DeclarationBody<'tir>, GraphcalError> {
    tir.declaration_body(declaration).ok_or_else(|| {
        GraphcalError::internal_error(
            format!("declaration `{declaration}` is absent from its owner's checked body"),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })
}

/// Evaluate every assertion reported for the root DAG: root assertions in
/// source order, then assertions projected from semantic instances. Each
/// result applies its `expected_fail` inversion.
///
/// A root assertion whose body references a failed declaration reports the
/// dependency failure (with its root cause) instead of evaluating over a
/// value map where the failed name is simply absent (#814).
pub(super) fn evaluate_assertions(
    tir: &graphcal_compiler::tir::typed::CheckedTir,
    src: &NamedSource<Arc<String>>,
    ctx: &EvalSession<'_>,
    values: &RuntimeValueMap,
    errors: &HashMap<ResolvedDeclName, NodeUnavailable>,
) -> Result<Vec<(ScopedName, AssertResult, Span)>, GraphcalError> {
    let mut assertions: Vec<(ScopedName, AssertResult, Span)> = tir
        .root()
        .asserts()
        .map(|entry| {
            let owner = entry.identity();
            let unit = declaration_body(tir, &owner, src)?;
            let body = unit
                .assertion()
                .ok_or_else(|| {
                    GraphcalError::internal_error(
                        format!("assertion `{owner}` has no checked body"),
                        src,
                        DiagnosticAnchor::WholeFile,
                    )
                })?
                .map(|entry| &*entry.body);
            let entry_ctx = ctx.for_decl(&owner);
            let assert_result =
                assert_dependency_failure(body, errors, &entry_ctx).unwrap_or_else(|| {
                    evaluate_assert_with_expected_fail(body, unit.expected_fail(), &mut |expr| {
                        eval_root(&entry_ctx.executable(expr)?, values, &entry_ctx)
                    })
                });
            Ok((
                ScopedName::local(entry.name().clone()),
                assert_result,
                entry.span,
            ))
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
            for ResolvedProjection {
                target: owner,
                projection,
            } in semantic_instance(tir, record, src)?.assertion_projections()
            {
                let unit = declaration_body(tir, &owner, src)?;
                let entry = unit.assertion().ok_or_else(|| {
                    GraphcalError::internal_error(
                        format!("projected assertion `{owner}` is absent from semantic instance"),
                        src,
                        DiagnosticAnchor::WholeFile,
                    )
                })?;
                let assertion_ctx = ctx.with_src(src).for_decl(&owner);
                let expected = projection
                    .expected_fail
                    .as_ref()
                    .or_else(|| unit.expected_fail());
                let result = evaluate_assert_with_expected_fail(
                    entry.map(|entry| &*entry.body),
                    expected,
                    &mut |expr| eval_root(&assertion_ctx.executable(expr)?, values, &assertion_ctx),
                );
                assertions.push((
                    root_instance_name(
                        tir.root_dag_id(),
                        parent_dag.dag_id(),
                        &record.instance.exposed_name(projection),
                    ),
                    result,
                    entry.get().span,
                ));
            }
        }
    }
    Ok(assertions)
}

/// Source-level names of the runtime declarations the root DAG exposes, in
/// deterministic order: root declarations in source order, then the output
/// and assertion projections of each root semantic instance in record order.
///
/// Declarations private to a semantic instance have no root source name and
/// are absent.
pub(super) fn root_source_names(
    tir: &graphcal_compiler::tir::typed::CheckedTir,
    src: &NamedSource<Arc<String>>,
) -> Result<Vec<(ResolvedDeclName, ScopedName)>, GraphcalError> {
    let mut names = tir
        .root()
        .decls()
        .iter()
        .map(|entry| (entry.identity(), ScopedName::local(entry.name().clone())))
        .collect::<Vec<_>>();
    for record in tir.root().semantic_instances() {
        let instance = semantic_instance(tir, record, src)?;
        names.extend(
            instance
                .output_projections()
                .map(|resolved| {
                    let name = record.instance.exposed_name(resolved.projection);
                    (resolved.target, name)
                })
                .chain(instance.assertion_projections().map(|resolved| {
                    let name = record.instance.exposed_name(resolved.projection);
                    (resolved.target, name)
                })),
        );
    }
    Ok(names)
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
    body: Scoped<'_, graphcal_compiler::hir::AssertBody>,
    errors: &HashMap<ResolvedDeclName, NodeUnavailable>,
    ctx: &EvalSession<'_>,
) -> Option<AssertResult> {
    let body_exprs = match body.operands() {
        AssertionOperands::Condition(expr) => vec![expr],
        AssertionOperands::Tolerance {
            actual,
            expected,
            tolerance,
        } => vec![actual, expected, tolerance],
    };
    match ctx.unavailable_dependencies(body_exprs.iter().copied()) {
        Ok(Some(reason)) if reason.is_incomplete() => Some(AssertResult::Blocked { reason }),
        Err(error) => Some(AssertResult::Error {
            message: error.to_string(),
        }),
        _ => dependency_failure_message(body_exprs, errors)
            .map(|message| AssertResult::Error { message }),
    }
}

/// If any declaration referenced by the given expressions failed to
/// evaluate, render a `dependency failed: ...` message naming each failed
/// dependency (direct failures carry their root cause inline).
///
/// Shared by assertions (#814) and plots (#842): a reference to a failed
/// declaration is not "undefined", it is unevaluable, and the report must
/// point at the root cause.
fn dependency_failure_message<'a>(
    exprs: impl IntoIterator<Item = Scoped<'a, graphcal_compiler::hir::Expr>>,
    errors: &HashMap<ResolvedDeclName, NodeUnavailable>,
) -> Option<String> {
    if errors.is_empty() {
        return None;
    }
    let deps: std::collections::BTreeSet<_> =
        exprs.into_iter().flat_map(Scoped::graph_refs).collect();
    let failed: Vec<String> =
        deps.iter()
            .filter_map(|dep| {
                errors.get(dep).map(|err| {
                    let leaf = dep.atom();
                    match err {
                        NodeUnavailable::EvalFailed { message } => format!("{leaf} ({message})"),
                        NodeUnavailable::DependencyFailed { .. } => leaf.to_string(),
                        reason @ (NodeUnavailable::Todo { .. }
                        | NodeUnavailable::Blocked { .. }) => format!("{leaf} ({reason})"),
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
    expr: Scoped<'_, graphcal_compiler::hir::Expr>,
    values: &RuntimeValueMap,
    ctx: &EvalSession<'_>,
) -> Result<PlotFieldValue, PlotEvaluationError> {
    ctx.cancellation.checkpoint().map_err(GraphcalError::from)?;
    if let graphcal_compiler::hir::ExprKind::StringLiteral(_) = expr.get().kind() {
        let text = ctx
            .checked_string(expr)
            .map_err(PlotEvaluationError::from)?;
        return Ok(PlotFieldValue::String(text.to_owned()));
    }
    ctx.executable(expr)
        .and_then(|tree| eval_root(&tree, values, ctx))
        .map_err(PlotEvaluationError::from)
        .and_then(|rv| runtime_to_plot_field_value(&rv).map_err(PlotEvaluationError::from))
}

#[derive(Debug, thiserror::Error)]
enum PlotEvaluationError {
    #[error("{0}")]
    Unavailable(NodeUnavailable),
    #[error(transparent)]
    Fatal(GraphcalError),
}

impl From<GraphcalError> for PlotEvaluationError {
    fn from(error: GraphcalError) -> Self {
        match error {
            error @ (GraphcalError::InternalError { .. } | GraphcalError::Cancelled(_)) => {
                Self::Fatal(error)
            }
            error => Self::Unavailable(eval_failed_node_error(&error)),
        }
    }
}

impl PlotEvaluationError {
    fn with_property(self, property: &graphcal_compiler::ir::lower::LoweredPlotProperty) -> Self {
        match self {
            Self::Unavailable(NodeUnavailable::EvalFailed { message }) => {
                Self::from(match property {
                    graphcal_compiler::ir::lower::LoweredPlotProperty::Mark(_) => {
                        format!("mark property `{}`: {message}", property.name())
                    }
                    property => format!("property `{}`: {message}", property.name()),
                })
            }
            other => other,
        }
    }
}

impl From<String> for PlotEvaluationError {
    fn from(message: String) -> Self {
        Self::Unavailable(NodeUnavailable::EvalFailed { message })
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
    unit: DeclarationBody<'_>,
    entry: Scoped<'_, graphcal_compiler::tir::typed::TypedPlotEntry>,
    values: &RuntimeValueMap,
    presentation_values: &PresentationInstanceMap,
    errors: &HashMap<ResolvedDeclName, NodeUnavailable>,
    ctx: &EvalSession<'_>,
) -> Result<PlotSpec, PlotEvaluationError> {
    // A reference to a failed declaration must report the root cause, not a
    // generic lookup failure on the missing value.
    let lowered = entry.map(|entry| &entry.body);
    let encodings = lowered
        .map(|lowered| lowered.encodings.as_slice())
        .iter()
        .map(|encoding| (encoding.get().0, encoding.map(|(_, expr)| &**expr)))
        .collect::<Vec<_>>();
    let mark_fields = lowered.map(|lowered| lowered.mark_properties.as_slice());
    let plot_fields = lowered.map(|lowered| lowered.properties.as_slice());
    let body_exprs = encodings
        .iter()
        .map(|(_, expr)| *expr)
        .chain(
            mark_fields
                .iter()
                .map(|field| field.map(|field| &*field.value)),
        )
        .chain(
            plot_fields
                .iter()
                .map(|field| field.map(|field| &*field.value)),
        )
        .collect::<Vec<_>>();
    check_plot_expression_dependencies(&body_exprs, errors, ctx)?;

    let owner = unit.identity();
    let channel_facts = unit.plot_channel_presentations().ok_or_else(|| {
        PlotEvaluationError::Fatal(ctx.internal_error(
            format!("checked presentation facts are missing for plot `{owner}`"),
            DiagnosticAnchor::WholeFile,
        ))
    })?;
    let mut encoding_meta = Vec::new();
    let mut presentation_diagnostics = Vec::new();

    // Evaluate channels and apply their checked structured presentation before
    // row alignment. Numeric projection and axis labels consume the same fact.
    let mut channel_data = Vec::new();
    for (channel, expr) in encodings {
        let fact = channel_facts.get(&channel).ok_or_else(|| {
            PlotEvaluationError::Fatal(ctx.internal_error(
                format!("checked presentation is missing channel `{channel}`"),
                expr.get().span,
            ))
        })?;
        let (data, unit_label, diagnostics) =
            evaluate_plot_channel(channel, expr, fact, values, presentation_values, ctx)?;

        presentation_diagnostics.extend(diagnostics.into_iter().map(|detail| {
            PresentationDiagnostic {
                declaration: owner.clone(),
                channel: Some(channel),
                detail,
            }
        }));
        let dimension_label = match fact.leaf() {
            PlotLeafKind::Quantity(dimension) if !dimension.is_dimensionless() => {
                Some(ctx.registry.dimensions.format_dimension(dimension))
            }
            _ => None,
        };
        encoding_meta.push((
            channel,
            AxisMeta {
                dimension_label,
                unit_label,
            },
        ));
        channel_data.push((channel, data));
    }
    let encodings = super::plot_data::align_encoding_channels(&channel_data)?;

    // Evaluate mark properties (e.g., stroke_width, opacity). Unknown names
    // are rejected at check time (#845); one that still reaches evaluation
    // is an internal inconsistency.
    let mark_properties = evaluate_mark_properties(mark_fields, values, ctx)?;

    // Evaluate top-level properties (e.g., title, width, height)
    let mut properties = Vec::new();
    for scoped_field in plot_fields.iter() {
        let field = scoped_field.get();
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
        let field_value = eval_plot_property(scoped_field.map(|field| &*field.value), values, ctx)
            .map_err(|error| error.with_property(&field.property))?;
        check_positive_property(plot_prop.name(), plot_prop.value_type(), &field_value)?;
        properties.push((*plot_prop, field_value));
    }
    let entry = entry.get();

    Ok(PlotSpec {
        name: ScopedName::local(entry.name().clone()),
        mark_type: entry.mark_type,
        encodings,
        encoding_meta,
        presentation_diagnostics,
        mark_properties,
        properties,
        visibility: entry.visibility,
    })
}

fn evaluate_mark_properties(
    fields: Scoped<'_, [graphcal_compiler::ir::lower::LoweredPlotField]>,
    values: &RuntimeValueMap,
    ctx: &EvalSession<'_>,
) -> Result<Vec<(graphcal_compiler::plot_props::MarkProperty, PlotFieldValue)>, PlotEvaluationError>
{
    fields
        .iter()
        .map(|scoped_field| {
            let field = scoped_field.get();
            let graphcal_compiler::ir::lower::LoweredPlotProperty::Mark(mark_prop) =
                &field.property
            else {
                return Err(PlotEvaluationError::Fatal(ctx.internal_error(
                    format!(
                        "checked mark property has incompatible classification `{}`",
                        field.property.name()
                    ),
                    field.value.span,
                )));
            };
            let value = eval_plot_property(scoped_field.map(|field| &*field.value), values, ctx)
                .map_err(|error| error.with_property(&field.property))?;
            Ok((*mark_prop, value))
        })
        .collect()
}

fn evaluate_plot_channel(
    channel: graphcal_compiler::syntax::ast::EncodingChannel,
    scoped_expr: Scoped<'_, graphcal_compiler::hir::Expr>,
    fact: &graphcal_compiler::plot_shape::PlotChannelShape,
    values: &RuntimeValueMap,
    presentation_values: &PresentationInstanceMap,
    ctx: &EvalSession<'_>,
) -> Result<
    (
        super::plot_data::ChannelData,
        Option<String>,
        Vec<LeafPresentationDiagnostic>,
    ),
    PlotEvaluationError,
> {
    let expr = scoped_expr.get();
    if let graphcal_compiler::hir::ExprKind::StringLiteral(value) = expr.kind() {
        return Ok((
            super::plot_data::ChannelData::unindexed_label(value.clone()),
            None,
            Vec::new(),
        ));
    }
    let evaluated = ctx
        .executable(scoped_expr)
        .and_then(|tree| eval_root_with_presentation(&tree, values, presentation_values, ctx))
        .map_err(|error| classify_plot_channel_error(channel, error))?;
    let (runtime, presentation_instance) = evaluated.into_parts();
    let presentation_instance =
        crate::eval_expr::presentation::resolve(presentation_instance, values, ctx)
            .map_err(|error| classify_plot_channel_error(channel, error))?;
    let declared_type =
        plot_declared_type(fact, ctx, expr.span).map_err(PlotEvaluationError::Fatal)?;
    let mut presented = EvaluatedValue::new(&runtime, &declared_type)
        .project(ctx.tir, ctx.src)
        .map_err(PlotEvaluationError::Fatal)?;
    let mut diagnostics = attach_presentation(&mut presented, Some(&presentation_instance))
        .map_err(|error| {
            PlotEvaluationError::Fatal(ctx.internal_error(error.to_string(), expr.span))
        })?;
    let label = super::plot_data::uniform_quantity_unit_label(&presented);
    let unit_label = match label {
        Ok(label) if diagnostics.is_empty() => label,
        label => {
            if let Err(error) = label {
                diagnostics.push(LeafPresentationDiagnostic {
                    path: Vec::new(),
                    failure: PresentationFailure::Projection { message: error },
                });
            }
            // A numeric channel must use one scale. Fall back atomically to SI,
            // rather than mixing successfully converted leaves with SI leaves.
            presented = EvaluatedValue::new(&runtime, &declared_type)
                .project(ctx.tir, ctx.src)
                .map_err(PlotEvaluationError::Fatal)?;
            None
        }
    };
    let data = super::plot_data::channel_data_from_presented_value(&runtime, &presented)
        .map_err(|error| format!("encoding channel `{channel}`: {error}"))?;
    Ok((data, unit_label, diagnostics))
}

fn classify_plot_channel_error(
    channel: graphcal_compiler::syntax::ast::EncodingChannel,
    error: GraphcalError,
) -> PlotEvaluationError {
    match PlotEvaluationError::from(error) {
        PlotEvaluationError::Unavailable(NodeUnavailable::EvalFailed { message }) => {
            PlotEvaluationError::from(format!("encoding channel `{channel}`: {message}"))
        }
        other => other,
    }
}

/// Convert the retained checked plot shape into the public projection type.
fn plot_declared_type(
    shape: &graphcal_compiler::plot_shape::PlotChannelShape,
    ctx: &EvalSession<'_>,
    span: Span,
) -> Result<CheckedType, GraphcalError> {
    // A contextual string channel has no runtime value type, and a symbolic
    // `Fin(N)` axis belongs to a template body, never to an executed plot.
    let leaf: Option<CheckedType<Symbolic>> = match shape.leaf() {
        PlotLeafKind::Quantity(dimension) => Some(CheckedType::Quantity(dimension.clone())),
        PlotLeafKind::Int => Some(CheckedType::Int),
        PlotLeafKind::Bool => Some(CheckedType::Bool),
        PlotLeafKind::Datetime(scale) => Some(CheckedType::Datetime(*scale)),
        PlotLeafKind::Key(index) => Some(CheckedType::Key(index.clone())),
        PlotLeafKind::ContextualString => None,
    };
    leaf.and_then(|leaf| {
        shape
            .axes()
            .iter()
            .rev()
            .fold(leaf, |element, index| CheckedType::Indexed {
                element: Box::new(element),
                index: index.clone(),
            })
            .to_concrete()
    })
    .ok_or_else(|| {
        ctx.internal_error(
            "plot channel shape has no concrete runtime value type",
            span,
        )
    })
}

#[cfg(test)]
mod tests;

fn check_plot_expression_dependencies(
    expressions: &[Scoped<'_, graphcal_compiler::hir::Expr>],
    errors: &HashMap<ResolvedDeclName, NodeUnavailable>,
    ctx: &EvalSession<'_>,
) -> Result<(), PlotEvaluationError> {
    if let Some(reason) = ctx.unavailable_dependencies(expressions.iter().copied())?
        && reason.is_incomplete()
    {
        return Err(PlotEvaluationError::Unavailable(reason));
    }
    dependency_failure_message(expressions.iter().copied(), errors)
        .map_or(Ok(()), |message| Err(PlotEvaluationError::from(message)))
}

/// Propagate known plot unavailability without hiding unknown checked
/// references, which `owner` (the DAG declaring the composition) binds.
fn check_plot_dependencies(
    references: &[graphcal_compiler::syntax::span::Spanned<ScopedName>],
    errors: &[super::types::PlotError],
    owner: &graphcal_compiler::tir::typed::CheckedDag,
    ctx: &EvalSession<'_>,
) -> Result<(), PlotEvaluationError> {
    let dependencies = references
        .iter()
        .filter_map(|reference| {
            errors
                .iter()
                .find(|error| error.name == reference.value)
                .map(|error| (reference, error))
        })
        .map(|(reference, error)| {
            owner
                .require_bound_decl_identity(
                    &reference.value,
                    ctx.src,
                    DiagnosticAnchor::Source(reference.span),
                )
                .map(|identity| (identity, &error.reason))
        })
        .collect::<Result<Vec<_>, _>>()?;
    NodeUnavailable::blocked_by(
        dependencies
            .iter()
            .map(|(identity, reason)| (identity, *reason)),
    )
    .map_or(Ok(()), |reason| {
        Err(PlotEvaluationError::Unavailable(reason))
    })
}

/// Evaluated fields of a figure/layer declaration.
#[derive(Debug)]
struct CompositionFields {
    properties: Vec<(super::types::CompositionProperty, PlotFieldValue)>,
    plot_names: Vec<ScopedName>,
}

/// Evaluate composition fields (properties and plot names) shared by figures and layers.
fn eval_composition_fields(
    fields: Scoped<'_, [graphcal_compiler::tir::typed::LoweredPlotField]>,
    plot_name_spans: &[graphcal_compiler::syntax::span::Spanned<ScopedName>],
    values: &RuntimeValueMap,
    ctx: &EvalSession<'_>,
) -> Result<CompositionFields, PlotEvaluationError> {
    if let Some(reason) =
        ctx.unavailable_dependencies(fields.iter().map(|field| field.map(|field| &*field.value)))?
    {
        return Err(PlotEvaluationError::Unavailable(reason));
    }
    let mut properties = Vec::new();
    for scoped_field in fields.iter() {
        let field = scoped_field.get();
        let graphcal_compiler::ir::lower::LoweredPlotProperty::Composition(comp_prop) =
            &field.property
        else {
            return Err(PlotEvaluationError::Fatal(ctx.internal_error(
                format!(
                    "checked composition property has incompatible classification `{}`",
                    field.property.name()
                ),
                field.value.span,
            )));
        };
        let field_value = eval_plot_property(scoped_field.map(|field| &*field.value), values, ctx)
            .map_err(|error| error.with_property(&field.property))?;
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
        RuntimeValue::Indexed(_) => super::plot_data::flatten_to_field_value(rv),
        RuntimeValue::Struct(_) => Err(format!("{} cannot be plotted", rv.describe())),
        RuntimeValue::Datetime(epoch) => super::types::epoch_to_rfc3339(epoch)
            .map(PlotFieldValue::Datetime)
            .map_err(|error| error.to_string()),
        RuntimeValue::CoordinateLabel { value, .. } => Ok(PlotFieldValue::Number(value.get())),
    }
}
