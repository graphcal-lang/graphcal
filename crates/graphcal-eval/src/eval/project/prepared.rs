//! Compile-once project evaluation and transport-independent model projection.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use graphcal_compiler::builtin::BuiltinFnName;
use graphcal_compiler::desugar::desugared_ast::{Expr, ExprKind as AstExprKind};
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::hir::{
    ConstRef, ExprKind as HirExprKind, ExprLoweringContext, FunctionRef, GenericScope,
    PreludeTypeScope,
};
use graphcal_compiler::ir::static_interface::StaticInputKind;
use graphcal_compiler::registry::builtins::builtin_functions;
use graphcal_compiler::registry::declared_type::DeclaredType;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::registry::time_scale::TimeScale;
use graphcal_compiler::registry::types::IndexKind;
use graphcal_compiler::syntax::ast::UnaryOp;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::index_name::IndexVariantName;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::module_resolve::ModuleResolver;
use graphcal_compiler::syntax::span::Span;
use miette::{NamedSource, SourceSpan};
use thiserror::Error;

use crate::decl_key::RuntimeDeclKey;
use crate::domain_check::{ResolvedDomainConstraint, ResolvedDomainConstraintRef};
use crate::eval::bindings::{RuntimeParameterBinding, RuntimeParameterBindings};
use crate::eval::runtime::{EvalLoopResult, run_eval_loop_with_bindings};
use crate::eval::types::{AssertResult, CompileError, EvalResult, NodeError, Value};
use crate::eval_expr::{EvalContext, HirLocalValueMap, RuntimeValueMap, eval_hir_expr};

use crate::host_fns::HostFunctionRegistry;
use crate::project_compiler::{
    CheckedEntryInterface, CheckedProject, CheckedProjectRuntimeParts, CompiledFile,
    IncludeDebugNameMap, ProjectCompiler,
};

use super::model_schema::{
    ModelSchemaGraph, ModelSchemaGraphBuilder, ModelValueSchema, index_def_for_ref,
};
use super::output::{
    apply_include_debug_names, output_decl_type, push_output_value, remap_include_debug_name,
};

#[path = "binding_compile.rs"]
mod binding_compile;
#[path = "tenax_model.rs"]
mod tenax_model;

pub use binding_compile::ParameterBindingRow;
use binding_compile::build_parameter_ports;
pub use tenax_model::{
    InclusiveBounds, ModelDefinitionError, ModelExecutionError, ModelOutputPort, ModelRowFailure,
    ModelRowOutcome, ParameterDomain, ParameterPort, PreparedModel, TenaxV2Input, TenaxV2InputKind,
    TenaxV2Model, TenaxV2Output, TenaxV2RowOutcome,
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
    /// Bind a parsed, desugared closed Graphcal value expression by entry name.
    pub fn bind_expression(&mut self, name: &DeclName, expr: &Expr) -> Result<(), CompileError> {
        let value = self.project.compile_named_parameter_value(name, expr)?;
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
            .compile_named_parameter_value(name, expr)
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
            return Err(CompileError::Eval(GraphcalError::internal_error(
                "parameter value belongs to another prepared project",
                &self.project.source,
                DiagnosticAnchor::Builtin,
            )));
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
        if !matches!(port.declared_type, DeclaredType::Quantity(_)) {
            return Err(self.project.binding_kind_error(port, "Quantity"));
        }
        if !si_value.is_finite() {
            return Err(self
                .project
                .binding_value_error(port, "quantity must be finite"));
        }
        self.insert(
            position,
            RuntimeParameterBinding {
                value: RuntimeValue::Quantity(si_value),
                presentation: graphcal_compiler::tir::presentation::PresentationProvenance::None,
            },
        )
    }

    /// Bind one exact signed integer.
    pub fn bind_integer(
        &mut self,
        position: ParameterPosition,
        value: i64,
    ) -> Result<(), CompileError> {
        let port = self.project.port_at(position)?;
        if port.declared_type != DeclaredType::Int {
            return Err(self.project.binding_kind_error(port, "Int"));
        }
        self.insert(
            position,
            RuntimeParameterBinding {
                value: RuntimeValue::Int(value),
                presentation: graphcal_compiler::tir::presentation::PresentationProvenance::None,
            },
        )
    }

    /// Bind one Boolean value.
    pub fn bind_boolean(
        &mut self,
        position: ParameterPosition,
        value: bool,
    ) -> Result<(), CompileError> {
        let port = self.project.port_at(position)?;
        if port.declared_type != DeclaredType::Bool {
            return Err(self.project.binding_kind_error(port, "Bool"));
        }
        self.insert(
            position,
            RuntimeParameterBinding {
                value: RuntimeValue::Bool(value),
                presentation: graphcal_compiler::tir::presentation::PresentationProvenance::None,
            },
        )
    }

    /// Bind a named-index category by its typed variant identity.
    pub fn bind_named_key(
        &mut self,
        position: ParameterPosition,
        variant: &IndexVariantName,
    ) -> Result<(), CompileError> {
        let port = self.project.port_at(position)?;
        let DeclaredType::Key(index) = &port.declared_type else {
            return Err(self.project.binding_kind_error(port, "Key"));
        };
        let Some(definition) = index_def_for_ref(index, &self.project.tir) else {
            return Err(self
                .project
                .binding_value_error(port, "index definition is unavailable"));
        };
        let IndexKind::Named { variants } = &definition.kind else {
            return Err(self
                .project
                .binding_value_error(port, "Tenax v2 requires a concrete named index"));
        };
        if !variants.contains(variant) {
            return Err(self.project.binding_value_error(
                port,
                &format!(
                    "unknown category `{variant}` for index `{}`",
                    index.display_name()
                ),
            ));
        }
        self.insert(
            position,
            RuntimeParameterBinding {
                value: RuntimeValue::Label {
                    index_name: index.clone(),
                    variant: variant.clone(),
                },
                presentation: graphcal_compiler::tir::presentation::PresentationProvenance::None,
            },
        )
    }
}
struct ProjectOutputAssembly {
    output_surface: HashSet<ScopedName>,
    include_debug_names: IncludeDebugNameMap,
    imported_source_order: Vec<(ScopedName, graphcal_compiler::ir::resolve::DeclCategory)>,
    imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
}

/// A checked, value-independent Graphcal project ready for repeated evaluation.
pub struct PreparedProject {
    plan_id: u64,
    tir: graphcal_compiler::tir::typed::TIR,
    plan: crate::exec_plan::ExecPlan,
    declared_types: HashMap<ScopedName, DeclaredType>,
    source: NamedSource<Arc<String>>,
    host_fns: crate::host_fns::HostFunctionRegistry,
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
    pub(in crate::eval::project) fn from_compiled(
        compiled: CompiledFile,
        plan: crate::exec_plan::ExecPlan,
        source: NamedSource<Arc<String>>,
        host_fns: crate::host_fns::HostFunctionRegistry,
        module_resolver: ModuleResolver,
    ) -> Result<Self, CompileError> {
        let plan_id = NEXT_PLAN_ID.fetch_add(1, Ordering::Relaxed);
        let CompiledFile {
            tir,
            checked_execution_facts: _,
            entry_interface,
            declared_types,
            imported_values,
            imported_source_order,
            output_surface,
            include_debug_names,
        } = compiled;
        let tir = tir.with_external_value_constructors();

        let mut schema_builder = ModelSchemaGraphBuilder::new(&tir, &source);
        let parameter_ports =
            build_parameter_ports(plan_id, &entry_interface, &plan, &mut schema_builder)?;
        let parameter_lookup = parameter_ports
            .iter()
            .enumerate()
            .map(|(index, port)| (port.name.clone(), index))
            .collect();
        let output_ports = build_output_ports(&entry_interface, &mut schema_builder)?;
        let schema_graph = Arc::new(schema_builder.finish());

        Ok(Self {
            plan_id,
            tir,
            plan,
            declared_types,
            source,
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
        &self.plan
    }

    /// Evaluate one validated row and assemble the normal Graphcal result view.
    ///
    /// # Errors
    ///
    /// Returns a compile/evaluator diagnostic for a row from another plan or
    /// an internal evaluator failure.
    pub fn evaluate(&self, row: &ParameterBindingRow) -> Result<EvalResult, CompileError> {
        self.evaluate_with_cancellation(
            row,
            &graphcal_compiler::cancellation::CancellationToken::unbounded(),
        )
    }

    /// Evaluate one row with cooperative cancellation.
    pub fn evaluate_with_cancellation(
        &self,
        row: &ParameterBindingRow,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<EvalResult, CompileError> {
        self.validate_row_identity(row)?;
        let eval_result = super::super::runtime::evaluate_plan_with_bindings_and_cancellation(
            &self.tir,
            &self.plan,
            &row.bindings,
            &self.declared_types,
            &self.source,
            &self.host_fns,
            cancellation,
        )?;
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
    ) -> Result<crate::eval::RuntimeEvaluation, CompileError> {
        self.evaluate_runtime_with_cancellation(
            row,
            &graphcal_compiler::cancellation::CancellationToken::unbounded(),
        )
    }

    /// Evaluate one row for debugging with cooperative cancellation.
    pub fn evaluate_runtime_with_cancellation(
        &self,
        row: &ParameterBindingRow,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<crate::eval::RuntimeEvaluation, CompileError> {
        self.validate_row_identity(row)?;
        Ok(
            super::super::runtime::evaluate_plan_with_values_and_bindings_and_cancellation(
                &self.tir,
                &self.plan,
                &row.bindings,
                &self.declared_types,
                &self.source,
                &self.host_fns,
                cancellation,
            )?,
        )
    }

    fn validate_row_identity(&self, row: &ParameterBindingRow) -> Result<(), CompileError> {
        if row.plan_id == self.plan_id {
            Ok(())
        } else {
            Err(CompileError::Eval(GraphcalError::internal_error(
                "parameter binding row belongs to another prepared project",
                &self.source,
                DiagnosticAnchor::Builtin,
            )))
        }
    }

    fn first_runtime_error<'errors>(
        &self,
        errors: &'errors HashMap<RuntimeDeclKey, NodeError>,
    ) -> Result<Option<(ScopedName, &'errors NodeError)>, ModelExecutionError> {
        for (name, _) in self.tir.root().source_order() {
            let key = RuntimeDeclKey::for_local_decl(self.tir.root(), name)
                .map_err(|probe| ModelExecutionError::Internal(probe.to_string()))?;
            if let Some(error) = errors.get(&key) {
                return Ok(Some((
                    remap_include_debug_name(name, &self.output_assembly.include_debug_names),
                    error,
                )));
            }
        }
        Ok(None)
    }

    fn assemble_normal_result(
        &self,
        mut eval_result: EvalResult,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<EvalResult, CompileError> {
        apply_include_debug_names(&mut eval_result, &self.output_assembly.include_debug_names);
        let assertions = eval_result.assertions;

        let mut consts = Vec::new();
        let mut params = Vec::new();
        let mut nodes = Vec::new();
        let mut all = Vec::new();
        let mut seen = HashSet::new();

        for (name, category) in &self.output_assembly.imported_source_order {
            cancellation.checkpoint()?;
            if !seen.insert(name.clone()) {
                continue;
            }
            let Some(decl_type) = output_decl_type(*category) else {
                continue;
            };
            if let Some((runtime, declared_type)) = self.output_assembly.imported_values.get(name) {
                let value =
                    crate::eval::public_projection::EvaluatedValue::new(runtime, declared_type)
                        .project(&self.tir, &self.source)
                        .map_err(CompileError::from)?;
                push_output_value(
                    (name.clone(), Ok(value), decl_type),
                    &mut consts,
                    &mut params,
                    &mut nodes,
                    &mut all,
                );
            }
        }

        consts.extend(eval_result.consts);
        params.extend(eval_result.params);
        nodes.extend(eval_result.nodes);
        all.extend(eval_result.all);
        Ok(EvalResult {
            consts,
            params,
            nodes,
            all,
            output_surface: self.output_assembly.output_surface.clone(),
            assertions,
            plots: eval_result.plots,
            plot_errors: eval_result.plot_errors,
            figures: eval_result.figures,
            layers: eval_result.layers,
            assumes_map: eval_result.assumes_map,
            base_dim_symbols: eval_result.base_dim_symbols,
            domain_constraints: eval_result.domain_constraints,
        })
    }
}

fn build_output_ports(
    entry_interface: &CheckedEntryInterface,
    schemas: &mut ModelSchemaGraphBuilder<'_>,
) -> Result<Vec<ModelOutputPort>, CompileError> {
    entry_interface
        .outputs()
        .iter()
        .map(|output| {
            let declared_type = output.declared_type().clone();
            let value_schema = schemas
                .value_schema(&declared_type)
                .map_err(CompileError::Eval)?;
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

fn parameter_domain(constraint: &ResolvedDomainConstraint) -> ParameterDomain {
    match constraint.as_ref() {
        ResolvedDomainConstraintRef::Quantity(bounds) => {
            ParameterDomain::Quantity(InclusiveBounds {
                lower: bounds.min().map(|bound| *bound.value()),
                upper: bounds.max().map(|bound| *bound.value()),
            })
        }
        ResolvedDomainConstraintRef::Int(bounds) => ParameterDomain::Integer(InclusiveBounds {
            lower: bounds.min().map(|bound| *bound.value()),
            upper: bounds.max().map(|bound| *bound.value()),
        }),
        ResolvedDomainConstraintRef::Datetime { scale, bounds } => ParameterDomain::Datetime {
            scale,
            bounds: InclusiveBounds {
                lower: bounds.min().map(|bound| bound.value().duration()),
                upper: bounds.max().map(|bound| bound.value().duration()),
            },
        },
    }
}
pub(super) fn prepare_checked_project(
    checked: CheckedProject,
    host_fns: &HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<PreparedProject, CompileError> {
    cancellation.checkpoint()?;
    let CheckedProjectRuntimeParts {
        compiled,
        source,
        module_resolver,
    } = checked.into_runtime_parts();
    if let Some(index) = compiled.entry_interface.required_index() {
        let Some(span) = index.anchor().resolve(source.inner().len()) else {
            return Err(CompileError::Eval(GraphcalError::internal_error(
                format!(
                    "required index `{}` has no diagnostic source anchor",
                    index.name()
                ),
                &source,
                DiagnosticAnchor::Builtin,
            )));
        };
        return Err(CompileError::Eval(
            GraphcalError::RequiredStaticInputNotBound {
                kind: StaticInputKind::Index,
                name: index.name().to_string(),
                src: source,
                span: span.into(),
            },
        ));
    }

    let plan = crate::exec_plan::compile_checked_with_cancellation(
        &compiled.tir,
        &compiled.checked_execution_facts,
        &source,
        cancellation,
    )?;
    PreparedProject::from_compiled(compiled, plan, source, host_fns.clone(), module_resolver)
}

impl ProjectCompiler<'_, HostFunctionRegistry> {
    /// Check and prepare this configured session for repeated evaluation.
    ///
    /// # Errors
    ///
    /// Returns a compile, plan, interface, plugin, or cancellation diagnostic.
    pub fn prepare(self) -> Result<PreparedProject, CompileError> {
        let cancellation = self.cancellation_token().clone();
        let host_fns = self.callable_host().clone();
        self.check()?
            .prepare_with_host_fns_and_cancellation(&host_fns, &cancellation)
    }

    /// Check, prepare, bind, and evaluate one row.
    ///
    /// # Errors
    ///
    /// Returns a compile, binding, evaluation, or cancellation diagnostic.
    pub fn eval(
        self,
        overrides: &std::collections::HashMap<
            graphcal_compiler::syntax::decl_name::DeclName,
            graphcal_compiler::desugar::desugared_ast::Expr,
        >,
    ) -> Result<crate::eval::types::EvalResult, CompileError> {
        let cancellation = self.cancellation_token().clone();
        let prepared = self.prepare()?;
        let mut bindings = prepared.binding_builder();
        for (name, expression) in overrides {
            cancellation.checkpoint()?;
            bindings.bind_expression(name, expression)?;
        }
        let row = bindings.finish()?;
        prepared.evaluate_with_cancellation(&row, &cancellation)
    }
}

impl CheckedProject {
    /// Continue to runtime preparation with callable host functions.
    ///
    /// # Errors
    ///
    /// Returns a plan, input-interface, plugin, or cancellation diagnostic.
    pub fn prepare_with_host_fns_and_cancellation(
        self,
        host_fns: &HostFunctionRegistry,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<PreparedProject, CompileError> {
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
        self.prepare_with_host_fns_and_cancellation(
            host_fns,
            &graphcal_compiler::cancellation::CancellationToken::unbounded(),
        )
    }
}
