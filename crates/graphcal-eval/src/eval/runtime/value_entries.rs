//! The value stage of runtime output assembly: one public entry per runtime
//! value the root DAG reports, projected with its presentation.
//!
//! Entries come from three sources, in output order: the root's own value
//! declarations, the values each include site exposes, and every value of a
//! root semantic instance for the debug view. One runtime identity is
//! reported once, and one output name never denotes two identities.

use std::collections::{HashMap, HashSet};

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::declaration_category::ValueDeclCategory;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::ir::instance::ExposedValueBody;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::tir::typed::{CheckedDag, ResolvedProjection};

use crate::eval::public_projection;
use crate::eval::types::{NodeUnavailable, Value};
use crate::eval_expr::{EvalSession, RuntimeValue};
use crate::execution_plan::{ExecPlan, PlannedInstance};
use crate::presentation_evidence::PresentationDiagnostic;
use crate::runtime_presentation::PresentedRef;

use super::evaluated_root::EvaluatedRoot;
use super::root_names::instance_member_name;

/// Whether an entry belongs to the root's consumer-facing output surface or
/// only to the debug view.
#[derive(Clone, Copy)]
enum OutputExposure {
    Surface,
    Debug,
}

/// One reported runtime value, before names are deduplicated.
struct ValueEntry {
    key: ResolvedDeclName,
    name: ScopedName,
    result: Result<Value, NodeUnavailable>,
    category: ValueDeclCategory,
    exposure: OutputExposure,
    diagnostics: Vec<PresentationDiagnostic>,
}

/// The value entries of one evaluation, in output order.
pub(super) struct ValueEntries {
    pub entries: Vec<(
        ScopedName,
        Result<Value, NodeUnavailable>,
        ValueDeclCategory,
    )>,
    pub output_surface: HashSet<ScopedName>,
    pub presentation_diagnostics: Vec<PresentationDiagnostic>,
}

impl ValueEntries {
    /// Report each runtime identity once, under its first name.
    ///
    /// # Errors
    ///
    /// Returns an internal error when one output name denotes two runtime
    /// identities.
    fn collect(
        entries: impl IntoIterator<Item = ValueEntry>,
        ctx: &EvalSession<'_>,
    ) -> Result<Self, SemanticError> {
        let mut identities = HashMap::<ScopedName, ResolvedDeclName>::new();
        let mut collected = Self {
            entries: Vec::new(),
            output_surface: HashSet::new(),
            presentation_diagnostics: Vec::new(),
        };
        for entry in entries {
            if matches!(entry.exposure, OutputExposure::Surface) {
                collected.output_surface.insert(entry.name.clone());
            }
            match identities.get(&entry.name) {
                Some(existing) if existing == &entry.key => continue,
                Some(existing) => {
                    return Err(ctx.internal_error(
                        format!(
                            "runtime output name `{}` identifies both `{existing}` and `{}`",
                            entry.name, entry.key
                        ),
                        DiagnosticAnchor::WholeFile,
                    ));
                }
                None => {}
            }
            identities.insert(entry.name.clone(), entry.key);
            collected.presentation_diagnostics.extend(entry.diagnostics);
            collected
                .entries
                .push((entry.name, entry.result, entry.category));
        }
        Ok(collected)
    }
}

/// Assemble the value entries of the root DAG.
pub(super) fn assemble_value_entries(
    plan: &ExecPlan<'_>,
    evaluated: EvaluatedRoot<'_>,
    ctx: &EvalSession<'_>,
) -> Result<ValueEntries, SemanticError> {
    let root = root_entries(plan, evaluated, ctx)?;
    let root_id = plan.tir().root_dag_id();
    let mut projected = Vec::new();
    for planned in plan.root().semantic_instances() {
        projected.extend(projection_entries(*planned, evaluated, ctx)?);
        projected.extend(debug_entries(*planned, root_id, evaluated, ctx)?);
    }
    ValueEntries::collect(root.into_iter().chain(projected), ctx)
}

/// The root's own value declarations, in source order.
fn root_entries(
    plan: &ExecPlan<'_>,
    evaluated: EvaluatedRoot<'_>,
    ctx: &EvalSession<'_>,
) -> Result<Vec<ValueEntry>, SemanticError> {
    let scope = plan.root().scope();
    scope
        .dag()
        .declarations()
        .filter_map(|entry| entry.value().map(|value| (entry, value.category)))
        .map(|(entry, category)| {
            let key = entry.identity().clone();
            let (result, diagnostics) = match category {
                ValueDeclCategory::Const => {
                    let runtime = scope.const_values().get(&key);
                    let (value, diagnostics) = project_value(&key, runtime, evaluated, ctx)?;
                    (Ok(value), diagnostics)
                }
                ValueDeclCategory::Param | ValueDeclCategory::Node => {
                    evaluated_value(&key, evaluated, ctx)?
                }
            };
            Ok(ValueEntry {
                name: ScopedName::local(entry.name().clone()),
                key,
                result,
                category,
                exposure: OutputExposure::Surface,
                diagnostics,
            })
        })
        .collect()
}

/// The values an include site exposes through the instance's scope.
///
/// A value the including DAG declares itself (a projection alias) is
/// reported by the including DAG's own entry.
fn projection_entries(
    planned: PlannedInstance<'_>,
    evaluated: EvaluatedRoot<'_>,
    ctx: &EvalSession<'_>,
) -> Result<Vec<ValueEntry>, SemanticError> {
    let instance = planned.instance();
    let instance_ctx = ctx.with_src(planned.scope().source());
    instance
        .output_projections()
        .filter(|resolved| resolved.projection.body() == ExposedValueBody::Instance)
        .map(|ResolvedProjection { target, projection }| {
            let category = declared_value_category(instance.dag(), &target, ctx)?;
            let (result, diagnostics) = evaluated_value(&target, evaluated, &instance_ctx)?;
            Ok(ValueEntry {
                name: instance.record().instance.exposed_name(projection),
                key: target,
                result,
                category,
                exposure: OutputExposure::Surface,
                diagnostics,
            })
        })
        .collect()
}

/// The category of `declaration`, a runtime value of `dag`.
fn declared_value_category(
    dag: &CheckedDag,
    declaration: &ResolvedDeclName,
    ctx: &EvalSession<'_>,
) -> Result<ValueDeclCategory, SemanticError> {
    dag.declaration(declaration)
        .and_then(graphcal_compiler::tir::typed::declaration_view::DeclarationView::value)
        .map(|value| value.category)
        .ok_or_else(|| {
            ctx.internal_error(
                format!("projected declaration `{declaration}` is not a runtime value"),
                DiagnosticAnchor::WholeFile,
            )
        })
}

/// Every value of one root semantic instance, for the debug view, under its
/// instance member name (the instance's scope in the root, then its leaf).
fn debug_entries(
    planned: PlannedInstance<'_>,
    root: &DagId,
    evaluated: EvaluatedRoot<'_>,
    ctx: &EvalSession<'_>,
) -> Result<Vec<ValueEntry>, SemanticError> {
    let instance_ctx = ctx.with_src(planned.scope().source());
    planned
        .instance()
        .dag()
        .declarations()
        .filter_map(|entry| entry.value().map(|value| (entry, value.category)))
        .map(|(entry, category)| {
            let key = entry.identity().clone();
            let name = instance_member_name(root, &key, ctx.src)?;
            let (result, diagnostics) = evaluated_value(&key, evaluated, &instance_ctx)?;
            Ok(ValueEntry {
                name,
                key,
                result,
                category,
                exposure: OutputExposure::Debug,
                diagnostics,
            })
        })
        .collect()
}

/// The outcome of an evaluated declaration: its contained failure, or its
/// value projected with its presentation.
fn evaluated_value(
    key: &ResolvedDeclName,
    evaluated: EvaluatedRoot<'_>,
    ctx: &EvalSession<'_>,
) -> Result<(Result<Value, NodeUnavailable>, Vec<PresentationDiagnostic>), SemanticError> {
    if let Some(error) = evaluated.errors.get(key) {
        return Ok((Err(error.clone()), Vec::new()));
    }
    let (value, diagnostics) = project_value(key, evaluated.values.get(key), evaluated, ctx)?;
    Ok((Ok(value), diagnostics))
}

/// Project `runtime`, the value of the successfully evaluated `declaration`,
/// to its public value, displayed as its resolved presented value (which
/// holds the same value) says, when it has one.
fn project_value(
    declaration: &ResolvedDeclName,
    runtime: Option<&RuntimeValue>,
    evaluated: EvaluatedRoot<'_>,
    ctx: &EvalSession<'_>,
) -> Result<(Value, Vec<PresentationDiagnostic>), SemanticError> {
    let declared_type = ctx
        .tir
        .decl_type(declaration)
        .map(graphcal_compiler::tir::typed::CheckedDeclType::declared);
    let (runtime, declared_type) = runtime.zip(declared_type).ok_or_else(|| {
        ctx.internal_error(
            format!("successful declaration `{declaration}` has no runtime value or checked type"),
            DiagnosticAnchor::WholeFile,
        )
    })?;
    let presented = evaluated.presentations.get(declaration).map_or_else(
        || PresentedRef::plain(runtime),
        |presented| presented.as_ref(),
    );
    let (value, notices) = public_projection::project(presented, declared_type)
        .map_err(|invariant| invariant.into_internal_error(ctx.src))?;
    let diagnostics = notices
        .into_iter()
        .map(|detail| PresentationDiagnostic {
            declaration: declaration.clone(),
            channel: None,
            detail,
        })
        .collect();
    Ok((value, diagnostics))
}
