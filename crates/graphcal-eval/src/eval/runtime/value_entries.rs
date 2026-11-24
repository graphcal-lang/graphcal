//! The value stage of runtime output assembly: one public entry per runtime
//! value the root DAG reports, projected with its presentation.
//!
//! Entries come from three sources, in output order: the root's own value
//! declarations, the values each include site exposes, and every value of a
//! root semantic instance for the debug view. One runtime identity is
//! reported once, and one output name never denotes two identities.

use std::collections::{HashMap, HashSet};

use graphcal_compiler::declaration_category::ValueDeclCategory;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::syntax::module_name::ScopedName;

use crate::cells::Cell;
use crate::eval::output_decl_name::OutputUnavailable;
use crate::eval::public_projection;
use crate::eval::types::{EvalEntry, Value, ValueCells};
use crate::eval_expr::{EvalSession, RuntimeValue};
use crate::execution_plan::{Entry, PlannedInstance};
use crate::presentation_evidence::PresentationDiagnostic;
use crate::runtime_presentation::PresentedRef;
use crate::runtime_presentation::ResolvedValue;

use super::evaluated_root::EvaluatedRoot;

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
    result: Result<Value, OutputUnavailable>,
    cells: Option<ValueCells>,
    category: ValueDeclCategory,
    exposure: OutputExposure,
    diagnostics: Vec<PresentationDiagnostic>,
}

/// The value entries of one evaluation, in output order.
pub(super) struct ValueEntries {
    /// Each with the runtime identity its output name reports.
    pub entries: Vec<EvalEntry>,
    pub output_surface: HashSet<ScopedName>,
    pub presentation_diagnostics: Vec<PresentationDiagnostic>,
    /// The values of the instances the root's instances include.
    pub nested_instance_entries: Vec<EvalEntry>,
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
        let mut collected = Self {
            entries: Vec::new(),
            output_surface: HashSet::new(),
            presentation_diagnostics: Vec::new(),
            nested_instance_entries: Vec::new(),
        };
        let mut identities = HashMap::<ScopedName, ResolvedDeclName>::new();
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
            identities.insert(entry.name.clone(), entry.key.clone());
            collected.presentation_diagnostics.extend(entry.diagnostics);
            collected.entries.push(EvalEntry {
                name: entry.name,
                outcome: entry.result,
                cells: entry.cells,
                category: entry.category,
                identity: entry.key,
            });
        }
        Ok(collected)
    }
}

/// Assemble the value entries of the root DAG.
pub(super) fn assemble_value_entries(
    entry: Entry<'_, '_>,
    evaluated: EvaluatedRoot<'_>,
    ctx: &EvalSession<'_>,
) -> Result<ValueEntries, SemanticError> {
    let root = root_entries(entry, evaluated, ctx)?;
    let mut projected = Vec::new();
    for planned in entry.callable().semantic_instances() {
        projected.extend(projection_entries(planned, evaluated, ctx)?);
        projected.extend(debug_entries(planned, evaluated, ctx)?);
    }
    let mut entries = ValueEntries::collect(root.into_iter().chain(projected), ctx)?;
    entries.nested_instance_entries = nested_instance_entries(entry, evaluated, ctx)?;
    Ok(entries)
}

/// Every value of the instances the root's instances include, at any depth,
/// by identity: for tools that show one instance's values. They are no
/// output of the root, so they have no output name of their own.
fn nested_instance_entries(
    entry: Entry<'_, '_>,
    evaluated: EvaluatedRoot<'_>,
    ctx: &EvalSession<'_>,
) -> Result<Vec<EvalEntry>, SemanticError> {
    let root_dag = entry.dag_id();
    let mut nested = Vec::new();
    for (parent, instances) in entry.callable().closure_instances() {
        if parent.scope().dag().dag_id() == root_dag {
            continue;
        }
        for planned in instances {
            nested.extend(
                debug_entries(planned, evaluated, ctx)?
                    .into_iter()
                    .map(|entry| EvalEntry {
                        name: entry.name,
                        outcome: entry.result,
                        cells: entry.cells,
                        category: entry.category,
                        identity: entry.key,
                    }),
            );
        }
    }
    Ok(nested)
}

/// The root's own value declarations, in source order.
fn root_entries(
    entry: Entry<'_, '_>,
    evaluated: EvaluatedRoot<'_>,
    ctx: &EvalSession<'_>,
) -> Result<Vec<ValueEntry>, SemanticError> {
    let scope = entry.callable().scope();
    scope
        .dag()
        .declarations()
        .filter_map(|declared| declared.value().map(|value| (declared, value.category)))
        .map(|(declared, category)| {
            let key = declared.identity().clone();
            let (result, cells, diagnostics) = match category {
                ValueDeclCategory::Const => match scope.const_values().get(&key) {
                    Some(runtime) => {
                        let (value, diagnostics) =
                            project_value(&key, Some(runtime), evaluated, ctx)?;
                        (Ok(value), None, diagnostics)
                    }
                    // An unfinished constant: the frame recorded why.
                    None => evaluated_value(&key, evaluated, ctx)?,
                },
                ValueDeclCategory::Param | ValueDeclCategory::Node => {
                    evaluated_value(&key, evaluated, ctx)?
                }
            };
            Ok(ValueEntry {
                name: ScopedName::local(declared.name().clone()),
                key,
                result,
                cells,
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
    planned: &PlannedInstance<'_>,
    evaluated: EvaluatedRoot<'_>,
    ctx: &EvalSession<'_>,
) -> Result<Vec<ValueEntry>, SemanticError> {
    let record = planned.instance().record();
    let instance_ctx = ctx.with_src(planned.scope().source());
    planned
        .outputs()
        .iter()
        .map(|output| {
            let (result, cells, diagnostics) =
                evaluated_value(&output.target, evaluated, &instance_ctx)?;
            Ok(ValueEntry {
                name: record.instance.exposed_name(output.projection),
                key: output.target.clone(),
                result,
                cells,
                category: output.category,
                exposure: OutputExposure::Surface,
                diagnostics,
            })
        })
        .collect()
}

/// Every value of one root semantic instance, for the debug view, under its
/// instance member name (the instance's scope in the root, then its leaf).
fn debug_entries(
    planned: &PlannedInstance<'_>,
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
            let name = planned.closure().member(entry.name());
            let (result, cells, diagnostics) = evaluated_value(&key, evaluated, &instance_ctx)?;
            Ok(ValueEntry {
                name,
                key,
                result,
                cells,
                category,
                exposure: OutputExposure::Debug,
                diagnostics,
            })
        })
        .collect()
}

/// The outcome of an evaluated declaration: its contained failure, with the
/// cells that have values when it has some, or its value projected with its
/// presentation.
fn evaluated_value(
    key: &ResolvedDeclName,
    evaluated: EvaluatedRoot<'_>,
    ctx: &EvalSession<'_>,
) -> Result<EvaluatedOutput, SemanticError> {
    if let Some(error) = evaluated.errors.get(key) {
        let (cells, diagnostics) = match evaluated.resolved_cells.get(key) {
            Some(cells) => {
                let (cells, diagnostics) = project_cells(key, cells, evaluated, ctx)?;
                (Some(cells), diagnostics)
            }
            None => (None, Vec::new()),
        };
        return Ok((Err(evaluated.names.present(error)), cells, diagnostics));
    }
    let (value, diagnostics) = project_value(key, evaluated.values.get(key), evaluated, ctx)?;
    Ok((Ok(value), None, diagnostics))
}

/// A declaration's output: its value or why it has none, the cells that
/// have values when it has none as a whole, and its presentation
/// diagnostics.
type EvaluatedOutput = (
    Result<Value, OutputUnavailable>,
    Option<ValueCells>,
    Vec<PresentationDiagnostic>,
);

/// Project `runtime`, the value of the successfully evaluated `declaration`,
/// to its public value, displayed as its resolved presented value (which
/// holds the same value) says, when it has one.
fn project_value(
    declaration: &ResolvedDeclName,
    runtime: Option<&RuntimeValue>,
    evaluated: EvaluatedRoot<'_>,
    ctx: &EvalSession<'_>,
) -> Result<(Value, Vec<PresentationDiagnostic>), SemanticError> {
    let (runtime, declared_type) =
        runtime.zip(checked_type(declaration, ctx)).ok_or_else(|| {
            ctx.internal_error(
                format!(
                    "successful declaration `{declaration}` has no runtime value or checked type"
                ),
                DiagnosticAnchor::WholeFile,
            )
        })?;
    let presented = evaluated.presentations.get(declaration).map_or_else(
        || PresentedRef::plain(runtime),
        |presented| presented.as_ref(),
    );
    let (value, notices) = public_projection::project(presented, declared_type)
        .map_err(|invariant| invariant.into_internal_error(ctx.src))?;
    Ok((value, diagnostics_of(declaration, notices)))
}

/// Project `cells`, the cells of `declaration` some of which have no value,
/// each value displayed as its resolved presentation says.
fn project_cells(
    declaration: &ResolvedDeclName,
    cells: &crate::runtime_value::IndexedValue<Cell<ResolvedValue>>,
    evaluated: EvaluatedRoot<'_>,
    ctx: &EvalSession<'_>,
) -> Result<(ValueCells, Vec<PresentationDiagnostic>), SemanticError> {
    let declared_type = checked_type(declaration, ctx).ok_or_else(|| {
        ctx.internal_error(
            format!("declaration `{declaration}` with cells has no checked type"),
            DiagnosticAnchor::WholeFile,
        )
    })?;
    let (cells, notices) = public_projection::project_cells(cells, declared_type, &mut |missing| {
        evaluated.names.present(&missing.reason())
    })
    .map_err(|invariant| invariant.into_internal_error(ctx.src))?;
    Ok((cells, diagnostics_of(declaration, notices)))
}

/// The checked type `declaration` is declared with.
fn checked_type<'t>(
    declaration: &ResolvedDeclName,
    ctx: &EvalSession<'t>,
) -> Option<&'t graphcal_compiler::semantic::checked_type::CheckedType> {
    ctx.tir
        .decl_type(declaration)
        .map(graphcal_compiler::tir::typed::CheckedDeclType::declared)
}

/// The presentation diagnostics of `declaration` from its leaves' notices.
fn diagnostics_of(
    declaration: &ResolvedDeclName,
    notices: Vec<crate::presentation_evidence::LeafPresentationDiagnostic>,
) -> Vec<PresentationDiagnostic> {
    notices
        .into_iter()
        .map(|detail| PresentationDiagnostic {
            declaration: declaration.clone(),
            channel: None,
            detail,
        })
        .collect()
}
