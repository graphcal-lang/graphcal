//! Compile-once project evaluation and transport-independent model projection.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use graphcal_compiler::declaration_category::DeclCategory;
use graphcal_compiler::desugar::desugared_ast::{Expr, ExprKind as AstExprKind};
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::hir::ExprLoweringContext;
use graphcal_compiler::hir::expr::ExprKind as HirExprKind;
use graphcal_compiler::hir::lower::{GenericScope, ModuleScope};
use graphcal_compiler::outcome::{CancellationMode, Outcome, without_cancellation};
use graphcal_compiler::resolve::ModuleResolver;
use graphcal_compiler::semantic::checked_type::CheckedType;
use graphcal_compiler::semantic::index_def::ConcreteIndexKind;
use graphcal_compiler::semantic::time_scale::TimeScale;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::index::IndexError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::source_registry::SourceRegistry;
use graphcal_compiler::static_interface::StaticInputKind;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::index_name::{IndexEntryKey, IndexVariantName};
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::span::Span;
use graphcal_eval::runtime_value::{IndexAxis, KeyValue, RuntimeValue};
use miette::{NamedSource, SourceSpan};
use thiserror::Error;

use crate::binding_error::{BindingError, BindingValueKind};
use crate::compile_error::CompileError;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_eval::domain_constraint::{ResolvedDomainConstraint, ResolvedDomainConstraintRef};
use graphcal_eval::eval::bindings::{RuntimeParameterBinding, RuntimeParameterBindings};
use graphcal_eval::eval::types::{EvalResult, Value};
use graphcal_eval::eval_expr::{EvalSession, RuntimeValueMap};

use crate::project_compiler::{
    CheckedEntryInterface, CheckedProject, CheckedProjectRuntimeParts, CompiledFile,
    ProjectCompiler,
};
use graphcal_eval::host_fns::HostFunctionRegistry;

use super::model_schema::{
    ModelIndexKind, ModelIndexSchema, ModelSchemaGraph, ModelSchemaGraphBuilder, ModelTypeId,
    ModelValueSchema,
};
use super::output::apply_include_debug_names;

#[path = "binding_compile.rs"]
mod binding_compile;
#[path = "tenax_model.rs"]
mod tenax_model;

use binding_compile::build_parameter_ports;
pub use binding_compile::{
    ExternalValue, ParameterBindingRow, StructuredBindingError, StructuredBindingErrorKind,
    StructuredBindingPathSegment, StructuredValueExpr,
};
pub use tenax_model::{
    InclusiveBounds, InclusiveBoundsError, ModelDefinitionError, ModelExecutionError,
    ModelOutputPort, ModelRowFailure, ModelRowOutcome, ParameterDomain, ParameterPort,
    PreparedModel, TenaxV2Input, TenaxV2InputKind, TenaxV2Model, TenaxV2Output, TenaxV2RowOutcome,
};

static NEXT_PLAN_ID: AtomicU64 = AtomicU64::new(1);

/// A prepared-project-specific parameter position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParameterPosition {
    plan_id: u64,
    index: usize,
}

impl ParameterPosition {
    /// Return the declaration-order boundary index.
    #[must_use]
    pub const fn index(self) -> usize {
        self.index
    }
}

/// One closed, recursively validated value tied to a parameter and plan.
///
/// Values are intentionally opaque: callers obtain them from a
/// [`PreparedProject`] so unresolved nominal owners, incomplete indexes, and
/// cross-plan values cannot be fabricated.
#[derive(Debug, Clone)]
pub struct ParameterValue {
    plan_id: u64,
    position: ParameterPosition,
    binding: RuntimeParameterBinding,
}

impl ParameterValue {
    /// Parameter position this value was validated for.
    #[must_use]
    pub const fn position(&self) -> ParameterPosition {
        self.position
    }
}

/// Builder for one prepared-project binding row.
pub struct ParameterBindingBuilder<'project> {
    project: &'project PreparedProject,
    slots: Vec<Option<RuntimeParameterBinding>>,
}

impl ParameterBindingBuilder<'_> {
    /// Bind a closed Graphcal value parsed from its own text by entry name.
    /// Diagnostics about the value are drawn against that text.
    pub fn bind_expression(
        &mut self,
        name: &DeclName,
        value: &ExternalValue,
    ) -> Result<(), CompileError> {
        let value = self.project.compile_named_parameter_value(name, value)?;
        self.bind_value(value)
    }

    /// Bind an expression supplied by an external format and report semantic
    /// failures against that format's source location.
    pub fn bind_external_expression(
        &mut self,
        name: &DeclName,
        expr: &Expr,
        src: &NamedSource<Arc<String>>,
        span: SourceSpan,
    ) -> Result<(), CompileError> {
        let value = self
            .project
            .parameter_position(name)
            .and_then(|position| self.project.compile_synthesized_value(position, expr))
            .map_err(|error| CompileError::ExternalBinding {
                name: name.clone(),
                reason: error.to_string(),
                src: src.clone(),
                span,
            })?;
        self.bind_value(value)
    }

    /// Bind an already validated opaque parameter value.
    pub fn bind_value(&mut self, value: ParameterValue) -> Result<(), CompileError> {
        if value.plan_id != self.project.plan_id {
            return Err(CompileError::semantic(
                SemanticError::internal_error(
                    "parameter value belongs to another prepared project",
                    self.project.source,
                    DiagnosticAnchor::Builtin,
                ),
                &self.project.sources,
            ));
        }
        self.insert(value.position, value.binding)
    }

    /// Bind one SI real quantity. The parameter's concrete dimension supplies
    /// the semantics; no dimensionless conversion is performed.
    pub fn bind_quantity(
        &mut self,
        position: ParameterPosition,
        si_value: f64,
    ) -> Result<(), CompileError> {
        let port = self.project.port_at(position)?;
        if !matches!(port.declared_type, CheckedType::Quantity(_)) {
            return Err(self
                .project
                .binding_kind_error(port, BindingValueKind::Quantity));
        }
        let value = RuntimeValue::quantity(si_value).map_err(|_| {
            self.project
                .port_error(port, |name, src, span| BindingError::NonFiniteQuantity {
                    name,
                    src,
                    span,
                })
        })?;
        self.insert(position, RuntimeParameterBinding::plain(value))
    }

    /// Bind one exact signed integer.
    pub fn bind_integer(
        &mut self,
        position: ParameterPosition,
        value: i64,
    ) -> Result<(), CompileError> {
        let port = self.project.port_at(position)?;
        if port.declared_type != CheckedType::Int {
            return Err(self.project.binding_kind_error(port, BindingValueKind::Int));
        }
        self.insert(
            position,
            RuntimeParameterBinding::plain(RuntimeValue::Int(value)),
        )
    }

    /// Bind one Boolean value.
    pub fn bind_boolean(
        &mut self,
        position: ParameterPosition,
        value: bool,
    ) -> Result<(), CompileError> {
        let port = self.project.port_at(position)?;
        if port.declared_type != CheckedType::Bool {
            return Err(self
                .project
                .binding_kind_error(port, BindingValueKind::Bool));
        }
        self.insert(
            position,
            RuntimeParameterBinding::plain(RuntimeValue::Bool(value)),
        )
    }

    /// Bind a named-index category by its typed variant identity.
    pub fn bind_named_key(
        &mut self,
        position: ParameterPosition,
        variant: &IndexVariantName,
    ) -> Result<(), CompileError> {
        let port = self.project.port_at(position)?;
        let CheckedType::Key(index) = &port.declared_type else {
            return Err(self.project.binding_kind_error(port, BindingValueKind::Key));
        };
        // A checked `Key<I>` port type names an index the checked program defines.
        let Some(axis) = IndexAxis::resolve(self.project.tir(), index) else {
            return Err(CompileError::semantic(
                SemanticError::internal_error(
                    format!(
                        "index `{index}` of parameter `{}` is not defined",
                        port.name
                    ),
                    self.project.source,
                    DiagnosticAnchor::Source(port.span),
                ),
                &self.project.sources,
            ));
        };
        if !matches!(axis.kind(), ConcreteIndexKind::Named { .. }) {
            return Err(self.project.port_error(port, |name, src, span| {
                BindingError::KeyIndexNotNamed { name, src, span }
            }));
        }
        let Some(key) = KeyValue::for_entry(axis, &IndexEntryKey::named(variant.clone())) else {
            return Err(self.project.port_error(port, |name, src, span| {
                BindingError::UnknownCategory {
                    name,
                    variant: variant.clone(),
                    index: index.clone(),
                    src,
                    span,
                }
            }));
        };
        self.insert(
            position,
            RuntimeParameterBinding::plain(RuntimeValue::Key(key)),
        )
    }
}
struct ProjectOutputAssembly {
    output_surface: HashSet<ScopedName>,
    include_debug_names: graphcal_compiler::display::include_scope_names::IncludeScopeNames,
    imported_source_order: Vec<(ScopedName, DeclCategory)>,
    imported_values: HashMap<ScopedName, graphcal_eval::checked_program::ImportedConstant>,
}

/// A checked, value-independent Graphcal project ready for repeated evaluation.
pub struct PreparedProject {
    plan_id: u64,
    plan: graphcal_eval::exec_plan::PreparedPlan,
    source: SourceId,
    /// The registry every source id of the prepared program resolves in.
    sources: Arc<SourceRegistry>,
    host_fns: graphcal_eval::host_fns::HostFunctionRegistry,
    module_resolver: ModuleResolver,
    parameter_ports: Vec<ParameterPort>,
    parameter_lookup: HashMap<DeclName, usize>,
    output_ports: Vec<ModelOutputPort>,
    schema_graph: Arc<ModelSchemaGraph>,
    output_assembly: ProjectOutputAssembly,
}

impl std::fmt::Debug for PreparedProject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedProject")
            .field("plan_id", &self.plan_id)
            .field("parameter_ports", &self.parameter_ports)
            .field("output_ports", &self.output_ports)
            .finish_non_exhaustive()
    }
}

impl PreparedProject {
    /// The checked TIR this project evaluates.
    fn tir(&self) -> &graphcal_compiler::tir::typed::CheckedTir {
        self.plan().tir()
    }

    /// The execution plan prepared for this project.
    fn plan(&self) -> &graphcal_eval::execution_plan::ExecPlan<'_> {
        self.plan.plan()
    }

    /// Source-visible index spelling for closed entry bindings, when available.
    #[must_use]
    pub fn source_index_path(
        &self,
        index: &graphcal_compiler::resolved_name::ResolvedIndexName,
    ) -> Option<graphcal_compiler::syntax::names::NamePath> {
        self.module_resolver
            .source_index_path(self.tir().root_dag_id(), index)
    }

    pub(super) fn from_compiled(
        compiled: CompiledFile,
        source: SourceId,
        sources: Arc<SourceRegistry>,
        host_fns: graphcal_eval::host_fns::HostFunctionRegistry,
        module_resolver: ModuleResolver,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<Self, Outcome<CompileError>> {
        let CompiledFile {
            program,
            entry_interface,
            imported_source_order,
            output_surface,
            include_debug_names,
        } = compiled;
        let prepared_plan = graphcal_eval::exec_plan::compile_checked_with_cancellation(
            program,
            source,
            cancellation,
        )
        .map_err(|outcome| outcome.map_failed(|error| CompileError::semantic(error, &sources)))?;
        let plan_id = NEXT_PLAN_ID.fetch_add(1, Ordering::Relaxed);

        let plan = prepared_plan.plan();
        let tir = plan.tir();
        let imported_values = plan
            .root()
            .scope()
            .imported_constants()
            .iter()
            .map(|constant| (constant.name().clone(), constant.clone()))
            .collect();
        let mut schema_builder = ModelSchemaGraphBuilder::new(tir, source);
        let parameter_ports = build_parameter_ports(
            plan_id,
            &entry_interface,
            plan,
            &sources,
            &mut schema_builder,
        )?;
        let parameter_lookup = parameter_ports
            .iter()
            .enumerate()
            .map(|(index, port)| (port.name.clone(), index))
            .collect();
        let output_ports = build_output_ports(&entry_interface, &sources, &mut schema_builder)?;
        let schema_graph = Arc::new(schema_builder.finish());

        Ok(Self {
            plan_id,
            plan: prepared_plan,
            source,
            sources,
            host_fns,
            module_resolver,
            parameter_ports,
            parameter_lookup,
            output_ports,
            schema_graph,
            output_assembly: ProjectOutputAssembly {
                output_surface,
                include_debug_names,
                imported_source_order,
                imported_values,
            },
        })
    }

    /// Entry-DAG parameters in source declaration order.
    #[must_use]
    pub fn parameter_ports(&self) -> &[ParameterPort] {
        &self.parameter_ports
    }

    /// Recursive value-schema arena shared by all model ports.
    #[must_use]
    pub fn model_schema_graph(&self) -> &ModelSchemaGraph {
        &self.schema_graph
    }

    /// Direct root nodes in source declaration order.
    #[must_use]
    pub fn output_ports(&self) -> &[ModelOutputPort] {
        &self.output_ports
    }

    /// Algebraic definitions referenced by every prepared boundary schema.
    #[must_use]
    pub fn schema_graph(&self) -> &ModelSchemaGraph {
        &self.schema_graph
    }

    /// Borrow the internal execution plan for unstable debugging output.
    ///
    /// The concrete plan type remains private so downstream code cannot depend
    /// on it as a runtime API. Its [`std::fmt::Debug`] representation has no
    /// compatibility guarantee.
    #[must_use]
    pub fn debug_plan(&self) -> impl std::fmt::Debug + '_ {
        self.plan()
    }

    /// Evaluate one validated row and assemble the normal Graphcal result view.
    ///
    /// # Errors
    ///
    /// Returns a compile/evaluator diagnostic for a row from another plan or
    /// an internal evaluator failure.
    pub fn evaluate(&self, row: &ParameterBindingRow) -> Result<EvalResult, CompileError> {
        without_cancellation(|cancellation| self.evaluate_with_cancellation(row, cancellation))
    }

    /// Evaluate one row with cooperative cancellation.
    ///
    /// # Errors
    ///
    /// Returns [`Outcome::Cancelled`] on cancellation, or the diagnostics of
    /// [`Self::evaluate`].
    pub fn evaluate_with_cancellation(
        &self,
        row: &ParameterBindingRow,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<EvalResult, Outcome<CompileError>> {
        self.validate_row_identity(row)?;
        let eval_result =
            graphcal_eval::eval::runtime::evaluate_plan_with_values_and_bindings_and_cancellation(
                self.plan(),
                &row.bindings,
                self.source,
                &self.sources,
                &self.host_fns,
                &self.output_assembly.include_debug_names,
                cancellation,
            )
            .map_err(|outcome| outcome.map_failed(|error| self.render(error)))?;
        self.assemble_normal_result(eval_result, cancellation)
    }

    /// Evaluate one validated row and retain the evaluator's internal values
    /// and contained errors for unstable debugging output.
    ///
    /// Unlike [`Self::evaluate`], this does not assemble dependency-instance
    /// values into the normal project-level public result.
    pub fn evaluate_runtime(
        &self,
        row: &ParameterBindingRow,
    ) -> Result<graphcal_eval::eval::RuntimeEvaluation, CompileError> {
        without_cancellation(|cancellation| {
            self.evaluate_runtime_with_cancellation(row, cancellation)
        })
    }

    /// Evaluate one row for debugging with cooperative cancellation.
    ///
    /// # Errors
    ///
    /// Returns [`Outcome::Cancelled`] on cancellation, or the diagnostics of
    /// [`Self::evaluate_runtime`].
    pub fn evaluate_runtime_with_cancellation(
        &self,
        row: &ParameterBindingRow,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<graphcal_eval::eval::RuntimeEvaluation, Outcome<CompileError>> {
        self.validate_row_identity(row)?;
        graphcal_eval::eval::runtime::evaluate_plan_with_values_and_bindings_and_cancellation(
            self.plan(),
            &row.bindings,
            self.source,
            &self.sources,
            &self.host_fns,
            &self.output_assembly.include_debug_names,
            cancellation,
        )
        .map_err(|outcome| outcome.map_failed(|error| self.render(error)))
    }

    /// Render a semantic error against this project's sources.
    pub(super) fn render(&self, error: SemanticError) -> CompileError {
        CompileError::semantic(error, &self.sources)
    }

    fn validate_row_identity(&self, row: &ParameterBindingRow) -> Result<(), CompileError> {
        if row.plan_id == self.plan_id {
            Ok(())
        } else {
            Err(CompileError::semantic(
                SemanticError::internal_error(
                    "parameter binding row belongs to another prepared project",
                    self.source,
                    DiagnosticAnchor::Builtin,
                ),
                &self.sources,
            ))
        }
    }

    fn assemble_normal_result(
        &self,
        evaluation: graphcal_eval::eval::runtime::RuntimeEvaluation,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<EvalResult, Outcome<CompileError>> {
        let (mut eval_result, presentations) = evaluation.into_result_and_presentations();
        apply_include_debug_names(&mut eval_result, &self.output_assembly.include_debug_names);

        let mut entries = Vec::new();
        let mut seen = HashSet::new();

        for (name, category) in &self.output_assembly.imported_source_order {
            cancellation.checkpoint()?;
            if !seen.insert(name.clone()) {
                continue;
            }
            let DeclCategory::Value(decl_type) = *category else {
                continue;
            };
            if let Some(imported) = self.output_assembly.imported_values.get(name) {
                let presented = presentations.get(imported.value().key()).map_or_else(
                    || {
                        graphcal_eval::runtime_presentation::PresentedRef::plain(
                            imported.value().value(),
                        )
                    },
                    |presented| presented.as_ref(),
                );
                let (value, diagnostics) = graphcal_eval::eval::public_projection::project(
                    presented,
                    imported.declared_type(),
                )
                .map_err(|invariant| self.render(invariant.into_internal_error(self.source)))?;
                eval_result
                    .presentation_diagnostics
                    .extend(diagnostics.into_iter().map(|detail| {
                        graphcal_eval::presentation_evidence::PresentationDiagnostic {
                            declaration: imported.value().key().clone(),
                            channel: None,
                            detail,
                        }
                    }));
                entries.push((name.clone(), Ok(value), decl_type));
            }
        }

        Ok(eval_result.with_imported_entries(entries, self.output_assembly.output_surface.clone()))
    }
}

fn build_output_ports(
    entry_interface: &CheckedEntryInterface,
    sources: &SourceRegistry,
    schemas: &mut ModelSchemaGraphBuilder<'_>,
) -> Result<Vec<ModelOutputPort>, CompileError> {
    entry_interface
        .outputs()
        .iter()
        .map(|output| {
            let declared_type = output.declared_type().clone();
            let value_schema = schemas
                .value_schema(&declared_type)
                .map_err(|error| CompileError::semantic(error, sources))?;
            Ok(ModelOutputPort {
                name: output.name().clone(),
                declared_type,
                value_schema,
                is_public: output.visibility().is_public(),
                runtime_key: output.runtime_key().clone(),
            })
        })
        .collect()
}

/// The sampling domain of a parameter with the resolved domain `constraint`.
///
/// # Errors
///
/// Returns [`InclusiveBoundsError`] when the bounds do not form an inclusive
/// interval, which checking rejects.
fn parameter_domain(
    constraint: &ResolvedDomainConstraint,
) -> Result<ParameterDomain, InclusiveBoundsError> {
    fn inclusive<T, U: PartialOrd>(
        bounds: &graphcal_eval::domain_constraint::ResolvedDomainBounds<T>,
        value: impl Fn(&T) -> U,
    ) -> Result<InclusiveBounds<U>, InclusiveBoundsError> {
        InclusiveBounds::try_new(
            bounds.min().map(|bound| value(bound.value())),
            bounds.max().map(|bound| value(bound.value())),
        )
    }
    Ok(match constraint.as_ref() {
        ResolvedDomainConstraintRef::Quantity(bounds) => {
            ParameterDomain::Quantity(inclusive(bounds, |value| value.get())?)
        }
        ResolvedDomainConstraintRef::Int(bounds) => {
            ParameterDomain::Integer(inclusive(bounds, |value| *value)?)
        }
        ResolvedDomainConstraintRef::Datetime { scale, bounds } => ParameterDomain::Datetime {
            scale,
            bounds: inclusive(bounds, |value| value.duration())?,
        },
    })
}

pub(super) fn prepare_checked_project(
    checked: CheckedProject,
    host_fns: &HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<PreparedProject, Outcome<CompileError>> {
    cancellation.checkpoint()?;
    let CheckedProjectRuntimeParts {
        compiled,
        source,
        sources,
        module_resolver,
    } = checked.into_runtime_parts();
    if let Some(index) = compiled.entry_interface.required_index() {
        let Some(span) = index.anchor().resolve(source.whole_span().len()) else {
            return Err(CompileError::semantic(
                SemanticError::internal_error(
                    format!(
                        "required index `{}` has no diagnostic source anchor",
                        index.name()
                    ),
                    source,
                    DiagnosticAnchor::Builtin,
                ),
                &sources,
            )
            .into());
        };
        return Err(CompileError::semantic(
            SemanticError::located(
                source,
                span,
                IndexError::RequiredStaticInputNotBound {
                    kind: StaticInputKind::Index,
                    name: index.name().atom().clone(),
                },
            ),
            &sources,
        )
        .into());
    }

    PreparedProject::from_compiled(
        compiled,
        source,
        sources,
        host_fns.clone(),
        module_resolver,
        cancellation,
    )
}

impl<Mode: CancellationMode> ProjectCompiler<'_, HostFunctionRegistry, Mode> {
    /// Check and prepare this configured session for repeated evaluation.
    ///
    /// # Errors
    ///
    /// Returns a compile, plan, interface, or plugin diagnostic, or (for a
    /// cancellable session) cancellation.
    pub fn prepare(self) -> Result<PreparedProject, Mode::Failure<CompileError>> {
        let mode = self.mode().clone();
        let host_fns = self.callable_host().clone();
        let checked = self.check()?;
        mode.run(|cancellation| prepare_checked_project(checked, &host_fns, cancellation))
    }

    /// Check, prepare, bind, and evaluate one row.
    ///
    /// # Errors
    ///
    /// Returns a compile, binding, or evaluation diagnostic, or (for a
    /// cancellable session) cancellation.
    pub fn eval(
        self,
        overrides: &std::collections::HashMap<
            graphcal_compiler::syntax::decl_name::DeclName,
            ExternalValue,
        >,
    ) -> Result<graphcal_eval::eval::types::EvalResult, Mode::Failure<CompileError>> {
        let mode = self.mode().clone();
        let prepared = self.prepare()?;
        mode.run(|cancellation| {
            let mut bindings = prepared.binding_builder();
            for (name, expression) in overrides {
                cancellation.checkpoint()?;
                bindings.bind_expression(name, expression)?;
            }
            let row = bindings.finish()?;
            prepared.evaluate_with_cancellation(&row, cancellation)
        })
    }
}

impl CheckedProject {
    /// Continue to runtime preparation with callable host functions.
    ///
    /// # Errors
    ///
    /// Returns [`Outcome::Cancelled`] on cancellation, or a plan,
    /// input-interface, or plugin diagnostic.
    pub fn prepare_with_host_fns_and_cancellation(
        self,
        host_fns: &HostFunctionRegistry,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<PreparedProject, Outcome<CompileError>> {
        prepare_checked_project(self, host_fns, cancellation)
    }

    /// Continue to runtime preparation without a cancellation deadline.
    ///
    /// # Errors
    ///
    /// Returns a plan, input-interface, or plugin diagnostic.
    pub fn prepare_with_host_fns(
        self,
        host_fns: &HostFunctionRegistry,
    ) -> Result<PreparedProject, CompileError> {
        without_cancellation(|cancellation| {
            self.prepare_with_host_fns_and_cancellation(host_fns, cancellation)
        })
    }
}
