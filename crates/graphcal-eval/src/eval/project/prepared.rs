//! Compile-once project evaluation and transport-independent model projection.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use graphcal_compiler::builtin::BuiltinFnName;
use graphcal_compiler::desugar::desugared_ast::{DeclKind, Expr, ExprKind as AstExprKind};
use graphcal_compiler::hir::{
    ConstRef, ExprKind as HirExprKind, ExprLoweringContext, FunctionRef, GenericScope,
    PreludeTypeScope,
};
use graphcal_compiler::registry::builtins::builtin_functions;
use graphcal_compiler::registry::declared_type::{DeclaredType, IndexTypeRef};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::registry::time_scale::TimeScale;
use graphcal_compiler::registry::types::{IndexDef, IndexKind};
use graphcal_compiler::syntax::ast::UnaryOp;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::index_name::IndexVariantName;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::module_resolve::ModuleResolver;
use graphcal_compiler::syntax::span::Span;
use miette::NamedSource;
use thiserror::Error;

use crate::decl_key::RuntimeDeclKey;
use crate::domain_check::{ResolvedDomainConstraint, ResolvedDomainConstraintRef};
use crate::eval::bindings::{RuntimeParameterBinding, RuntimeParameterBindings};
use crate::eval::runtime::{EvalLoopResult, run_eval_loop_with_bindings};
use crate::eval::types::{AssertResult, CompileError, EvalResult, NodeError, Value};
use crate::eval_expr::{EvalContext, HirLocalValueMap, RuntimeValueMap, eval_hir_expr};

use super::pipeline::{
    apply_include_debug_names, output_decl_type, push_output_value, remap_include_debug_name,
};
use super::{CompiledFile, EvaluatedOutputValue, IncludeDebugNameMap};

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

/// Inclusive lower and upper bounds for one external input family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InclusiveBounds<T> {
    lower: Option<T>,
    upper: Option<T>,
}

impl<T> InclusiveBounds<T> {
    /// Inclusive lower bound, when declared.
    #[must_use]
    pub const fn lower(&self) -> Option<&T> {
        self.lower.as_ref()
    }

    /// Inclusive upper bound, when declared.
    #[must_use]
    pub const fn upper(&self) -> Option<&T> {
        self.upper.as_ref()
    }
}

/// Concrete fixed-axis schema retained by the generic model interface.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelIndexSchema {
    identity: IndexTypeRef,
    kind: ModelIndexKind,
}

impl ModelIndexSchema {
    /// Owner-qualified or structural index identity.
    #[must_use]
    pub const fn identity(&self) -> &IndexTypeRef {
        &self.identity
    }

    /// Closed concrete axis representation.
    #[must_use]
    pub const fn kind(&self) -> &ModelIndexKind {
        &self.kind
    }
}

/// Concrete index families supported by recursive Graphcal values.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelIndexKind {
    Named {
        variants: Vec<IndexVariantName>,
    },
    Coordinate {
        coordinates_si: Vec<f64>,
        dimension: graphcal_compiler::dimension::Dimension,
        display_label: Option<String>,
        display_scale: f64,
    },
    Finite {
        cardinality: usize,
    },
}

/// One recursively typed algebraic constructor field.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelFieldSchema {
    name: graphcal_compiler::syntax::type_name::FieldName,
    value: ModelValueSchema,
}

impl ModelFieldSchema {
    #[must_use]
    pub const fn name(&self) -> &graphcal_compiler::syntax::type_name::FieldName {
        &self.name
    }

    #[must_use]
    pub const fn value(&self) -> &ModelValueSchema {
        &self.value
    }
}

/// One constructor in a concrete recursive algebraic schema.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelConstructorSchema {
    name: graphcal_compiler::syntax::type_name::ConstructorName,
    fields: Vec<ModelFieldSchema>,
}

impl ModelConstructorSchema {
    #[must_use]
    pub const fn name(&self) -> &graphcal_compiler::syntax::type_name::ConstructorName {
        &self.name
    }

    #[must_use]
    pub fn fields(&self) -> &[ModelFieldSchema] {
        &self.fields
    }
}

/// Canonical boundary unit attached to a recursive quantity schema.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelUnitSchema {
    label: String,
    scale_to_si: f64,
}

impl ModelUnitSchema {
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    #[must_use]
    pub const fn scale_to_si(&self) -> f64 {
        self.scale_to_si
    }
}

/// One real or complex physical quantity family.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelQuantitySchema {
    dimension: graphcal_compiler::dimension::Dimension,
    canonical_unit: Option<ModelUnitSchema>,
}

impl ModelQuantitySchema {
    #[must_use]
    pub const fn dimension(&self) -> &graphcal_compiler::dimension::Dimension {
        &self.dimension
    }

    #[must_use]
    pub const fn canonical_unit(&self) -> Option<&ModelUnitSchema> {
        self.canonical_unit.as_ref()
    }
}

/// Transport-independent recursive schema for one concrete Graphcal value.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelValueSchema {
    Quantity(ModelQuantitySchema),
    Complex(ModelQuantitySchema),
    Bool,
    Int,
    Datetime(TimeScale),
    Key(ModelIndexSchema),
    Algebraic {
        identity: graphcal_compiler::registry::declared_type::StructTypeRef,
        generic_args: Vec<graphcal_compiler::registry::declared_type::DeclaredGenericArg>,
        constructors: Vec<ModelConstructorSchema>,
    },
    Indexed {
        element: Box<Self>,
        axis: ModelIndexSchema,
    },
}

/// Resolved sampling-domain information on a Graphcal parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum ParameterDomain {
    /// Real quantity bounds in Graphcal's SI runtime representation.
    Quantity(InclusiveBounds<f64>),
    /// Exact signed-integer bounds.
    Integer(InclusiveBounds<i64>),
    /// Exact physical-instant bounds for one declared time scale.
    Datetime {
        /// Declared Graphcal time scale.
        scale: TimeScale,
        /// Bounds canonicalized to TAI durations for ordering.
        bounds: InclusiveBounds<hifitime::Duration>,
    },
}

/// One entry-DAG parameter port in declaration order.
#[derive(Debug, Clone)]
pub struct ParameterPort {
    name: DeclName,
    position: ParameterPosition,
    declared_type: DeclaredType,
    value_schema: ModelValueSchema,
    domain: Option<ParameterDomain>,
    has_default: bool,
    runtime_key: RuntimeDeclKey,
    span: Span,
}

impl ParameterPort {
    /// Source-level entry parameter name.
    #[must_use]
    pub const fn name(&self) -> &DeclName {
        &self.name
    }

    /// Plan-scoped position.
    #[must_use]
    pub const fn position(&self) -> ParameterPosition {
        self.position
    }

    /// Fully concrete Graphcal declared type.
    #[must_use]
    pub const fn declared_type(&self) -> &DeclaredType {
        &self.declared_type
    }

    /// Recursive concrete value schema.
    #[must_use]
    pub const fn value_schema(&self) -> &ModelValueSchema {
        &self.value_schema
    }

    /// Resolved domain constraint, when one is declared.
    #[must_use]
    pub const fn domain(&self) -> Option<&ParameterDomain> {
        self.domain.as_ref()
    }

    /// Whether evaluation can use a compiled default when this slot is absent.
    #[must_use]
    pub const fn has_default(&self) -> bool {
        self.has_default
    }
}

/// One directly declared root node available to model-output selection.
#[derive(Debug, Clone)]
pub struct ModelOutputPort {
    name: DeclName,
    declared_type: DeclaredType,
    value_schema: ModelValueSchema,
    is_public: bool,
    runtime_key: RuntimeDeclKey,
}

impl ModelOutputPort {
    /// Source-level node name.
    #[must_use]
    pub const fn name(&self) -> &DeclName {
        &self.name
    }

    /// Fully concrete Graphcal output type.
    #[must_use]
    pub const fn declared_type(&self) -> &DeclaredType {
        &self.declared_type
    }

    /// Recursive concrete value schema.
    #[must_use]
    pub const fn value_schema(&self) -> &ModelValueSchema {
        &self.value_schema
    }

    /// Whether the source node is explicitly public.
    #[must_use]
    pub const fn is_public(&self) -> bool {
        self.is_public
    }
}

/// Transport-independent typed model selected from one prepared project.
#[derive(Debug, Clone)]
pub struct PreparedModel {
    plan_id: u64,
    inputs: Vec<ParameterPort>,
    outputs: Vec<ModelOutputPort>,
}

impl PreparedModel {
    /// Every concrete entry-DAG parameter in source order.
    #[must_use]
    pub fn inputs(&self) -> &[ParameterPort] {
        &self.inputs
    }

    /// Selected concrete public root nodes in source order.
    #[must_use]
    pub fn outputs(&self) -> &[ModelOutputPort] {
        &self.outputs
    }
}

struct ProjectOutputAssembly {
    output_surface: HashSet<ScopedName>,
    included_debug_values: Vec<EvaluatedOutputValue>,
    include_debug_names: IncludeDebugNameMap,
    imported_source_order: Vec<(ScopedName, graphcal_compiler::ir::resolve::DeclCategory)>,
    imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    included_plots: Vec<crate::eval::PlotSpec>,
    dependency_assertions: Vec<(ScopedName, AssertResult, Span)>,
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
        root_ast: &graphcal_compiler::desugar::desugared_ast::File,
        dependency_assertions: Vec<(ScopedName, AssertResult, Span)>,
    ) -> Result<Self, CompileError> {
        let plan_id = NEXT_PLAN_ID.fetch_add(1, Ordering::Relaxed);
        let CompiledFile {
            mut tir,
            declared_types,
            imported_values,
            imported_source_order,
            output_surface,
            included_debug_values,
            include_debug_names,
            included_plots,
        } = compiled;
        tir.prepare_external_value_constructors();

        let parameter_ports =
            build_parameter_ports(plan_id, root_ast, &tir, &plan, &declared_types, &source)?;
        let parameter_lookup = parameter_ports
            .iter()
            .enumerate()
            .map(|(index, port)| (port.name.clone(), index))
            .collect();
        let output_ports = build_output_ports(root_ast, &tir, &declared_types, &source)?;

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
            output_assembly: ProjectOutputAssembly {
                output_surface,
                included_debug_values,
                include_debug_names,
                imported_source_order,
                imported_values,
                included_plots,
                dependency_assertions,
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

    /// Begin constructing one plan-scoped parameter row.
    #[must_use]
    pub fn binding_builder(&self) -> ParameterBindingBuilder<'_> {
        ParameterBindingBuilder {
            project: self,
            slots: vec![None; self.parameter_ports.len()],
        }
    }

    /// Compile and validate one closed recursive value for a plan position.
    pub fn compile_parameter_value(
        &self,
        position: ParameterPosition,
        expr: &Expr,
    ) -> Result<ParameterValue, CompileError> {
        let port = self.port_at(position)?;
        let normalized =
            normalize_binding_literal(expr.clone(), &port.value_schema).map_err(|message| {
                CompileError::Eval(GraphcalError::EvalError {
                    message: format!("invalid binding for `{}`: {message}", port.name),
                    src: self.source.clone(),
                    span: expr.span.into(),
                })
            })?;
        let hir = self.lower_closed_binding(port, &normalized)?;
        let value = self.evaluate_closed_binding(&hir)?;
        Ok(ParameterValue {
            plan_id: self.plan_id,
            position,
            binding: RuntimeParameterBinding {
                value,
                display_expr: Some(hir),
            },
        })
    }

    /// Resolve an entry parameter name and compile one closed recursive value.
    pub fn compile_named_parameter_value(
        &self,
        name: &DeclName,
        expr: &Expr,
    ) -> Result<ParameterValue, CompileError> {
        let index = self.parameter_index(name)?;
        self.compile_parameter_value(self.parameter_ports[index].position, expr)
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

    /// Select a transport-independent recursive typed model interface.
    pub fn model(
        &self,
        requested_outputs: &[DeclName],
    ) -> Result<PreparedModel, ModelDefinitionError> {
        if requested_outputs.is_empty() {
            return Err(ModelDefinitionError::NoOutputs);
        }
        let mut requested = HashSet::with_capacity(requested_outputs.len());
        for name in requested_outputs {
            if !requested.insert(name.clone()) {
                return Err(ModelDefinitionError::DuplicateOutput { name: name.clone() });
            }
            let Some(port) = self.output_ports.iter().find(|port| &port.name == name) else {
                let actual_kind = self
                    .tir
                    .root()
                    .source_order
                    .iter()
                    .find_map(|(candidate, kind)| (candidate.member() == name).then_some(*kind));
                return Err(actual_kind.map_or_else(
                    || ModelDefinitionError::UnknownOutput { name: name.clone() },
                    |actual_kind| ModelDefinitionError::OutputNotNode {
                        name: name.clone(),
                        actual_kind,
                    },
                ));
            };
            if !port.is_public {
                return Err(ModelDefinitionError::PrivateOutput { name: name.clone() });
            }
        }
        Ok(PreparedModel {
            plan_id: self.plan_id,
            inputs: self.parameter_ports.clone(),
            outputs: self
                .output_ports
                .iter()
                .filter(|port| requested.contains(&port.name))
                .cloned()
                .collect(),
        })
    }

    /// Derive the strict transport-independent Tenax schema-v2 projection.
    pub fn tenax_v2_model(
        &self,
        requested_outputs: &[DeclName],
    ) -> Result<TenaxV2Model, ModelDefinitionError> {
        let prepared_model = self.model(requested_outputs)?;
        if prepared_model.inputs.is_empty() {
            return Err(ModelDefinitionError::NoInputs);
        }
        for port in &prepared_model.outputs {
            if port.declared_type != DeclaredType::Bool {
                return Err(ModelDefinitionError::UnsupportedOutputType {
                    name: port.name.clone(),
                    actual: port.declared_type.format(&self.tir.registry.dimensions),
                });
            }
        }
        let inputs = prepared_model
            .inputs
            .iter()
            .map(|port| self.tenax_v2_input(port))
            .collect::<Result<Vec<_>, _>>()?;
        let outputs = prepared_model
            .outputs
            .iter()
            .map(|port| TenaxV2Output {
                name: port.name.clone(),
            })
            .collect::<Vec<_>>();

        let mut names = HashSet::new();
        for name in inputs
            .iter()
            .map(TenaxV2Input::name)
            .chain(outputs.iter().map(TenaxV2Output::name))
        {
            if !names.insert(name.clone()) {
                return Err(ModelDefinitionError::DuplicateFieldName { name: name.clone() });
            }
        }

        Ok(TenaxV2Model {
            model: prepared_model,
            inputs,
            outputs,
        })
    }

    /// Evaluate one generic model row without display/plot assembly.
    pub fn evaluate_model_row(
        &self,
        row: &ParameterBindingRow,
        model: &PreparedModel,
    ) -> Result<ModelRowOutcome, ModelExecutionError> {
        self.validate_row_identity(row)
            .map_err(ModelExecutionError::from)?;
        if model.plan_id != self.plan_id {
            return Err(ModelExecutionError::PlanMismatch);
        }

        let builtin_fns = builtin_functions();
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        let EvalLoopResult { values, errors } = run_eval_loop_with_bindings(
            &self.plan,
            &row.bindings,
            &self.tir,
            &self.source,
            builtin_fns,
            &self.host_fns,
            &cancellation,
        )?;

        if !errors.is_empty() {
            let (name, error) = self.first_runtime_error(&errors).ok_or_else(|| {
                ModelExecutionError::Internal(
                    "runtime error map contains no declaration from root source order".to_string(),
                )
            })?;
            return Ok(ModelRowOutcome::Failure(ModelRowFailure {
                message: format!("{name}: {error}"),
            }));
        }

        if let Some((name, message)) =
            self.output_assembly
                .dependency_assertions
                .iter()
                .find_map(|(name, result, _)| match result {
                    AssertResult::Pass => None,
                    AssertResult::Fail { message } | AssertResult::Error { message } => {
                        Some((name, message))
                    }
                })
        {
            return Ok(ModelRowOutcome::Failure(ModelRowFailure {
                message: format!("assertion `{name}`: {message}"),
            }));
        }

        let empty_locals = HirLocalValueMap::root();
        let ctx = EvalContext {
            cancellation,
            builtin_fns,
            registry: &self.tir.registry,
            src: &self.source,
            tir: &self.tir,
            current_dag: Some(self.tir.root()),
            root_values: Some(&values),
            struct_field_constraints: Some(&self.plan.struct_field_constraints),
            host_fns: Some(&self.host_fns),
        };
        for assertion in &self.plan.assert_bodies {
            let result = crate::eval::runtime::evaluate_assert_with_expected_fail(
                &assertion.body,
                self.plan.expected_fail.get(&assertion.name),
                &values,
                &empty_locals,
                &ctx,
            );
            match result {
                AssertResult::Pass => {}
                AssertResult::Fail { message } | AssertResult::Error { message } => {
                    let name = remap_include_debug_name(
                        &assertion.name,
                        &self.output_assembly.include_debug_names,
                    );
                    return Ok(ModelRowOutcome::Failure(ModelRowFailure {
                        message: format!("assertion `{name}`: {message}"),
                    }));
                }
            }
        }

        let outputs = model
            .outputs
            .iter()
            .map(|output| {
                values
                    .get(&output.runtime_key)
                    .map(|value| {
                        crate::eval::runtime::runtime_to_value(
                            value,
                            Some(&output.declared_type),
                            &self.tir,
                        )
                    })
                    .ok_or_else(|| {
                        ModelExecutionError::Internal(format!(
                            "selected output `{}` has no runtime value",
                            output.name
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ModelRowOutcome::Success(outputs))
    }

    /// Evaluate one strict v2 row and enforce Boolean output representation.
    pub fn evaluate_tenax_v2_row(
        &self,
        row: &ParameterBindingRow,
        model: &TenaxV2Model,
    ) -> Result<TenaxV2RowOutcome, ModelExecutionError> {
        match self.evaluate_model_row(row, &model.model)? {
            ModelRowOutcome::Failure(failure) => Ok(TenaxV2RowOutcome::Failure(failure)),
            ModelRowOutcome::Success(values) => values
                .into_iter()
                .zip(&model.outputs)
                .map(|(value, output)| match value {
                    Value::Bool(value) => Ok(value),
                    _ => Err(ModelExecutionError::Internal(format!(
                        "selected Tenax v2 output `{}` did not produce Bool",
                        output.name
                    ))),
                })
                .collect::<Result<Vec<_>, _>>()
                .map(TenaxV2RowOutcome::Success),
        }
    }

    fn tenax_v2_input(&self, port: &ParameterPort) -> Result<TenaxV2Input, ModelDefinitionError> {
        let kind = match (&port.declared_type, &port.domain) {
            (DeclaredType::Quantity(dimension), Some(ParameterDomain::Quantity(bounds))) => {
                let (Some(lower), Some(upper)) = (bounds.lower, bounds.upper) else {
                    return Err(ModelDefinitionError::MissingClosedDomain {
                        name: port.name.clone(),
                    });
                };
                if !(lower.is_finite()
                    && upper.is_finite()
                    && (upper - lower).is_finite()
                    && lower <= upper)
                {
                    return Err(ModelDefinitionError::InvalidContinuousDomain {
                        name: port.name.clone(),
                    });
                }
                let unit = if dimension.is_dimensionless() {
                    None
                } else {
                    Some(
                        crate::eval::types::default_unit_label(
                            dimension,
                            self.tir.registry.dimensions.base_dim_symbols(),
                        )
                        .ok_or_else(|| {
                            ModelDefinitionError::MissingCanonicalUnit {
                                name: port.name.clone(),
                            }
                        })?,
                    )
                };
                TenaxV2InputKind::Continuous {
                    lower,
                    upper,
                    unit,
                    scale_to_si: 1.0,
                }
            }
            (DeclaredType::Int, Some(ParameterDomain::Integer(bounds))) => {
                let (Some(lower), Some(upper)) = (bounds.lower, bounds.upper) else {
                    return Err(ModelDefinitionError::MissingClosedDomain {
                        name: port.name.clone(),
                    });
                };
                TenaxV2InputKind::Integer { lower, upper }
            }
            (DeclaredType::Key(index), None) => {
                let Some(definition) = index_def_for_ref(index, &self.tir) else {
                    return Err(ModelDefinitionError::UnsupportedInputType {
                        name: port.name.clone(),
                        actual: port.declared_type.format(&self.tir.registry.dimensions),
                    });
                };
                let IndexKind::Named { variants } = &definition.kind else {
                    return Err(ModelDefinitionError::UnsupportedInputType {
                        name: port.name.clone(),
                        actual: port.declared_type.format(&self.tir.registry.dimensions),
                    });
                };
                if variants.is_empty() {
                    return Err(ModelDefinitionError::EmptyCategoricalDomain {
                        name: port.name.clone(),
                    });
                }
                if variants.len() > i32::MAX as usize {
                    return Err(ModelDefinitionError::CategoricalDomainTooLarge {
                        name: port.name.clone(),
                        count: variants.len(),
                    });
                }
                let mut categories = variants.clone();
                categories.sort_unstable_by(|left, right| left.as_str().cmp(right.as_str()));
                TenaxV2InputKind::Categorical { categories }
            }
            _ => {
                return Err(ModelDefinitionError::UnsupportedInputType {
                    name: port.name.clone(),
                    actual: port.declared_type.format(&self.tir.registry.dimensions),
                });
            }
        };
        Ok(TenaxV2Input {
            name: port.name.clone(),
            position: port.position,
            kind,
        })
    }

    fn validate_row_identity(&self, row: &ParameterBindingRow) -> Result<(), CompileError> {
        if row.plan_id == self.plan_id {
            Ok(())
        } else {
            Err(CompileError::Eval(GraphcalError::EvalError {
                message: "parameter binding row belongs to another prepared project".to_string(),
                src: self.source.clone(),
                span: Span::new(0, 0).into(),
            }))
        }
    }

    fn first_runtime_error<'errors>(
        &self,
        errors: &'errors HashMap<RuntimeDeclKey, NodeError>,
    ) -> Option<(ScopedName, &'errors NodeError)> {
        self.tir.root().source_order.iter().find_map(|(name, _)| {
            let key = RuntimeDeclKey::for_local_decl(self.tir.root(), name);
            errors.get(&key).map(|error| {
                (
                    remap_include_debug_name(name, &self.output_assembly.include_debug_names),
                    error,
                )
            })
        })
    }

    fn assemble_normal_result(
        &self,
        mut eval_result: EvalResult,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<EvalResult, CompileError> {
        apply_include_debug_names(&mut eval_result, &self.output_assembly.include_debug_names);
        let mut assertions = self.output_assembly.dependency_assertions.clone();
        assertions.extend(eval_result.assertions);

        let mut consts = Vec::new();
        let mut params = Vec::new();
        let mut nodes = Vec::new();
        let mut all = Vec::new();
        let mut seen = HashSet::new();

        for entry in &self.output_assembly.included_debug_values {
            cancellation.checkpoint()?;
            if seen.insert(entry.0.clone()) {
                push_output_value(
                    entry.clone(),
                    &mut consts,
                    &mut params,
                    &mut nodes,
                    &mut all,
                );
            }
        }
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
                    crate::eval::runtime::runtime_to_value(runtime, Some(declared_type), &self.tir);
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
        let mut plots = self.output_assembly.included_plots.clone();
        plots.extend(eval_result.plots);

        Ok(EvalResult {
            consts,
            params,
            nodes,
            all,
            output_surface: self.output_assembly.output_surface.clone(),
            assertions,
            plots,
            plot_errors: eval_result.plot_errors,
            figures: eval_result.figures,
            layers: eval_result.layers,
            assumes_map: eval_result.assumes_map,
            base_dim_symbols: eval_result.base_dim_symbols,
            domain_constraints: eval_result.domain_constraints,
        })
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

    /// Bind an already validated opaque parameter value.
    pub fn bind_value(&mut self, value: ParameterValue) -> Result<(), CompileError> {
        if value.plan_id != self.project.plan_id {
            return Err(CompileError::Eval(GraphcalError::EvalError {
                message: "parameter value belongs to another prepared project".to_string(),
                src: self.project.source.clone(),
                span: Span::new(0, 0).into(),
            }));
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
                display_expr: None,
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
                display_expr: None,
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
                display_expr: None,
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
                display_expr: None,
            },
        )
    }

    /// Validate required slots and finish this row.
    pub fn finish(self) -> Result<ParameterBindingRow, CompileError> {
        if let Some(port) = self
            .project
            .parameter_ports
            .iter()
            .find(|port| !port.has_default && self.slots[port.position.index].is_none())
        {
            return Err(CompileError::Eval(
                GraphcalError::RequiredParamNotProvided {
                    name: port.name.to_string(),
                    src: self.project.source.clone(),
                    span: port.span.into(),
                },
            ));
        }
        let bindings = self
            .project
            .parameter_ports
            .iter()
            .zip(self.slots)
            .filter_map(|(port, binding)| {
                binding.map(|binding| (port.runtime_key.clone(), binding))
            })
            .collect();
        Ok(ParameterBindingRow {
            plan_id: self.project.plan_id,
            bindings,
        })
    }

    fn insert(
        &mut self,
        position: ParameterPosition,
        binding: RuntimeParameterBinding,
    ) -> Result<(), CompileError> {
        let port = self.project.port_at(position)?;
        if let Some(constraint) = self.project.plan.domain_constraints.get(&port.runtime_key)
            && let Err(violation) =
                crate::domain_check::check_domain_constraint(&binding.value, constraint)
        {
            return Err(self.project.binding_value_error(port, &violation.message));
        }
        let slot = &mut self.slots[position.index];
        if slot.is_some() {
            return Err(self.project.binding_value_error(
                port,
                &format!("parameter `{}` is bound more than once", port.name),
            ));
        }
        *slot = Some(binding);
        Ok(())
    }
}

/// One validated row of optional/defaulted and supplied parameter values.
#[derive(Debug, Clone)]
pub struct ParameterBindingRow {
    plan_id: u64,
    bindings: RuntimeParameterBindings,
}

/// One strict Tenax schema-v2 input.
#[derive(Debug, Clone)]
pub struct TenaxV2Input {
    name: DeclName,
    position: ParameterPosition,
    kind: TenaxV2InputKind,
}

impl TenaxV2Input {
    #[must_use]
    pub const fn name(&self) -> &DeclName {
        &self.name
    }

    #[must_use]
    pub const fn position(&self) -> ParameterPosition {
        self.position
    }

    #[must_use]
    pub const fn kind(&self) -> &TenaxV2InputKind {
        &self.kind
    }
}

/// Tenax schema-v2 input kinds.
#[derive(Debug, Clone)]
pub enum TenaxV2InputKind {
    Continuous {
        lower: f64,
        upper: f64,
        unit: Option<String>,
        scale_to_si: f64,
    },
    Integer {
        lower: i64,
        upper: i64,
    },
    Categorical {
        categories: Vec<IndexVariantName>,
    },
}

impl TenaxV2InputKind {
    #[must_use]
    pub fn continuous(&self) -> Option<(f64, f64, Option<&str>, f64)> {
        match self {
            Self::Continuous {
                lower,
                upper,
                unit,
                scale_to_si,
            } => Some((*lower, *upper, unit.as_deref(), *scale_to_si)),
            Self::Integer { .. } | Self::Categorical { .. } => None,
        }
    }

    #[must_use]
    pub const fn integer(&self) -> Option<(i64, i64)> {
        match self {
            Self::Integer { lower, upper } => Some((*lower, *upper)),
            Self::Continuous { .. } | Self::Categorical { .. } => None,
        }
    }

    #[must_use]
    pub fn categories(&self) -> Option<&[IndexVariantName]> {
        match self {
            Self::Categorical { categories } => Some(categories),
            Self::Continuous { .. } | Self::Integer { .. } => None,
        }
    }
}

/// One selected Boolean v2 output.
#[derive(Debug, Clone)]
pub struct TenaxV2Output {
    name: DeclName,
}

impl TenaxV2Output {
    #[must_use]
    pub const fn name(&self) -> &DeclName {
        &self.name
    }
}

/// Strict, transport-independent Tenax schema-v2 projection.
#[derive(Debug, Clone)]
pub struct TenaxV2Model {
    model: PreparedModel,
    inputs: Vec<TenaxV2Input>,
    outputs: Vec<TenaxV2Output>,
}

impl TenaxV2Model {
    #[must_use]
    pub fn inputs(&self) -> &[TenaxV2Input] {
        &self.inputs
    }

    #[must_use]
    pub fn outputs(&self) -> &[TenaxV2Output] {
        &self.outputs
    }
}

/// Ordinary recursive typed per-row model outcome.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelRowOutcome {
    Success(Vec<Value>),
    Failure(ModelRowFailure),
}

/// Strict scalar-Boolean Tenax v2 row outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TenaxV2RowOutcome {
    Success(Vec<bool>),
    Failure(ModelRowFailure),
}

/// Stable human diagnostic for one ordinary Graphcal row failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct ModelRowFailure {
    message: String,
}

impl ModelRowFailure {
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Process-scoped failure while evaluating a prepared model.
#[derive(Debug, Error)]
pub enum ModelExecutionError {
    #[error(transparent)]
    Compile(#[from] CompileError),
    #[error(transparent)]
    Graphcal(#[from] GraphcalError),
    #[error("model projection belongs to another prepared project")]
    PlanMismatch,
    #[error("internal model projection invariant failed: {0}")]
    Internal(String),
}

/// Why a Graphcal model cannot be projected into Tenax schema v2.
#[derive(Debug, Clone, Error)]
pub enum ModelDefinitionError {
    #[error("a Tenax model must expose at least one entry parameter")]
    NoInputs,
    #[error("at least one --output is required")]
    NoOutputs,
    #[error("selected output `{name}` occurs more than once")]
    DuplicateOutput { name: DeclName },
    #[error("entry DAG has no declaration named `{name}`")]
    UnknownOutput { name: DeclName },
    #[error("selected output `{name}` is a {actual_kind}, not a node")]
    OutputNotNode {
        name: DeclName,
        actual_kind: graphcal_compiler::ir::resolve::DeclCategory,
    },
    #[error("selected output `{name}` is private")]
    PrivateOutput { name: DeclName },
    #[error("selected output `{name}` has unsupported Tenax v2 type `{actual}`; expected Bool")]
    UnsupportedOutputType { name: DeclName, actual: String },
    #[error("input `{name}` has no complete closed domain required by Tenax v2")]
    MissingClosedDomain { name: DeclName },
    #[error("input `{name}` has an invalid finite continuous domain")]
    InvalidContinuousDomain { name: DeclName },
    #[error("input `{name}` has unsupported Tenax v2 type `{actual}`")]
    UnsupportedInputType { name: DeclName, actual: String },
    #[error("categorical input `{name}` has an empty domain")]
    EmptyCategoricalDomain { name: DeclName },
    #[error(
        "categorical input `{name}` has {count} categories, exceeding Arrow Int32 dictionary capacity"
    )]
    CategoricalDomainTooLarge { name: DeclName, count: usize },
    #[error("input `{name}` has no canonical SI boundary unit")]
    MissingCanonicalUnit { name: DeclName },
    #[error("model field name `{name}` occurs in both the input and output interface")]
    DuplicateFieldName { name: DeclName },
}

impl PreparedProject {
    fn parameter_index(&self, name: &DeclName) -> Result<usize, CompileError> {
        self.parameter_lookup.get(name).copied().ok_or_else(|| {
            let actual_kind = self
                .tir
                .root()
                .source_order
                .iter()
                .find_map(|(candidate, kind)| (candidate.member() == name).then_some(*kind));
            actual_kind.map_or_else(
                || CompileError::Eval(GraphcalError::OverrideUnknownParam { name: name.clone() }),
                |actual_kind| {
                    CompileError::Eval(GraphcalError::OverrideNotAParam {
                        name: name.clone(),
                        actual_kind,
                    })
                },
            )
        })
    }

    fn port_at(&self, position: ParameterPosition) -> Result<&ParameterPort, CompileError> {
        if position.plan_id != self.plan_id {
            return Err(CompileError::Eval(GraphcalError::EvalError {
                message: "parameter position belongs to another prepared project".to_string(),
                src: self.source.clone(),
                span: Span::new(0, 0).into(),
            }));
        }
        self.parameter_ports.get(position.index).ok_or_else(|| {
            CompileError::Eval(GraphcalError::InternalError {
                message: format!("parameter position {} is out of bounds", position.index),
                src: self.source.clone(),
                span: Span::new(0, 0).into(),
            })
        })
    }

    fn lower_closed_binding(
        &self,
        port: &ParameterPort,
        expr: &Expr,
    ) -> Result<graphcal_compiler::hir::Expr, CompileError> {
        let hir =
            self.lower_closed_binding_expr(expr, &port.value_schema, &self.tir.root_dag_id)?;
        validate_closed_hir(&hir).map_err(|message| {
            CompileError::Eval(GraphcalError::EvalError {
                message: format!(
                    "binding for `{}` is not a closed value: {message}",
                    port.name
                ),
                src: self.source.clone(),
                span: hir.span.into(),
            })
        })?;
        graphcal_compiler::tir::dim_check::check_external_value_expr_type(
            &self.tir,
            &self.declared_types,
            &hir,
            &port.declared_type,
            &self.source,
        )?;
        Ok(hir)
    }

    /// Lower a closed boundary value against its canonical recursive schema.
    ///
    /// Constructor and field-value syntax is interpreted in the module that
    /// defines the expected algebraic type. This is deliberately different
    /// from lowering a source expression in the entry module: decoding a value
    /// for an imported type must not require the entry to import that type's
    /// constructors or the units used by its field contracts. Nested values
    /// switch owners again at each canonical constructor boundary.
    fn lower_closed_binding_expr(
        &self,
        expr: &Expr,
        expected: &ModelValueSchema,
        owner: &graphcal_compiler::dag_id::DagId,
    ) -> Result<graphcal_compiler::hir::Expr, CompileError> {
        match (&expr.kind, expected) {
            (AstExprKind::ConstructorCall { callee, .. }, ModelValueSchema::Algebraic { .. })
                if callee.as_bare().is_some() =>
            {
                self.lower_canonical_constructor_binding(expr, expected, owner)
            }
            (
                AstExprKind::UnresolvedRef(graphcal_compiler::syntax::ast::UnresolvedRef::Path(
                    path,
                )),
                ModelValueSchema::Algebraic { constructors, .. },
            ) if path.as_bare().is_some()
                && constructors.iter().any(|constructor| {
                    constructor.name.atom() == &path.leaf().name && constructor.fields.is_empty()
                }) =>
            {
                let constructor_expr = Expr::new(
                    AstExprKind::ConstructorCall {
                        callee: path.clone(),
                        generic_args: Vec::new(),
                        fields: Vec::new(),
                    },
                    expr.span,
                );
                self.lower_closed_binding_expr(&constructor_expr, expected, owner)
            }
            (AstExprKind::MapLiteral { .. }, ModelValueSchema::Indexed { .. }) => {
                self.lower_closed_map_binding(expr, expected, owner)
            }
            _ => self.lower_binding_expr_in_owner(expr, owner),
        }
    }

    fn lower_canonical_constructor_binding(
        &self,
        expr: &Expr,
        expected: &ModelValueSchema,
        owner: &graphcal_compiler::dag_id::DagId,
    ) -> Result<graphcal_compiler::hir::Expr, CompileError> {
        let AstExprKind::ConstructorCall {
            callee,
            generic_args,
            fields,
        } = &expr.kind
        else {
            return Err(self.binding_internal_error(
                "canonical external constructor has a non-constructor syntax node",
                expr.span,
            ));
        };
        let ModelValueSchema::Algebraic {
            identity,
            constructors,
            ..
        } = expected
        else {
            return Err(self.binding_internal_error(
                "canonical external constructor has a non-algebraic schema",
                expr.span,
            ));
        };
        let Some(constructor) = constructors
            .iter()
            .find(|constructor| constructor.name.atom() == &callee.leaf().name)
        else {
            return self.lower_binding_expr_in_owner(expr, owner);
        };
        let constructor_owner = identity.resolved().owner();
        let signature_expr = Expr::new(
            AstExprKind::ConstructorCall {
                callee: callee.clone(),
                generic_args: generic_args.clone(),
                fields: Vec::new(),
            },
            expr.span,
        );
        let signature = self.lower_binding_expr_in_owner(&signature_expr, constructor_owner)?;
        let HirExprKind::ConstructorCall {
            callee,
            generic_args,
            ..
        } = signature.kind
        else {
            return Err(self.binding_internal_error(
                "canonical external constructor did not lower to a constructor call",
                expr.span,
            ));
        };
        let fields = fields
            .iter()
            .map(|field| {
                let value = constructor
                    .fields
                    .iter()
                    .find(|schema| schema.name == field.name.value)
                    .map_or_else(
                        || self.lower_binding_expr_in_owner(&field.value, constructor_owner),
                        |schema| {
                            self.lower_closed_binding_expr(
                                &field.value,
                                &schema.value,
                                constructor_owner,
                            )
                        },
                    )?;
                Ok(graphcal_compiler::hir::expr::FieldInit {
                    name: field.name.clone(),
                    value,
                })
            })
            .collect::<Result<Vec<_>, CompileError>>()?;
        Ok(graphcal_compiler::hir::Expr::new(
            HirExprKind::ConstructorCall {
                callee,
                generic_args,
                fields,
            },
            expr.span,
        ))
    }

    fn lower_closed_map_binding(
        &self,
        expr: &Expr,
        expected: &ModelValueSchema,
        owner: &graphcal_compiler::dag_id::DagId,
    ) -> Result<graphcal_compiler::hir::Expr, CompileError> {
        let AstExprKind::MapLiteral { entries } = &expr.kind else {
            return Err(self.binding_internal_error(
                "external map binding has a non-map syntax node",
                expr.span,
            ));
        };
        let entries = entries
            .iter()
            .map(|entry| {
                // Resolve source-shaped keys normally, but lower each value
                // against its recursive schema so nested constructor owners survive.
                let mut key_entry = entry.clone();
                key_entry.value = Expr::new(AstExprKind::Bool(true), entry.value.span);
                let key_expr = Expr::new(
                    AstExprKind::MapLiteral {
                        entries: vec![key_entry],
                    },
                    expr.span,
                );
                let lowered_key = self.lower_binding_expr_in_owner(&key_expr, owner)?;
                let HirExprKind::MapLiteral {
                    entries: lowered_entries,
                } = lowered_key.kind
                else {
                    return Err(self.binding_internal_error(
                        "external map key did not lower to a map literal",
                        expr.span,
                    ));
                };
                let Some(lowered_entry) = lowered_entries.into_iter().next() else {
                    return Err(self.binding_internal_error(
                        "external map key lowering produced no entry",
                        expr.span,
                    ));
                };
                let entry_schema =
                    (0..entry.keys.len()).try_fold(expected, |schema, _| match schema {
                        ModelValueSchema::Indexed { element, .. } => Some(element.as_ref()),
                        _ => None,
                    });
                let value = entry_schema.map_or_else(
                    || self.lower_binding_expr_in_owner(&entry.value, owner),
                    |schema| self.lower_closed_binding_expr(&entry.value, schema, owner),
                )?;
                Ok(graphcal_compiler::hir::expr::MapEntry {
                    keys: lowered_entry.keys,
                    value,
                })
            })
            .collect::<Result<Vec<_>, CompileError>>()?;
        Ok(graphcal_compiler::hir::Expr::new(
            HirExprKind::MapLiteral { entries },
            expr.span,
        ))
    }

    fn lower_binding_expr_in_owner(
        &self,
        expr: &Expr,
        owner: &graphcal_compiler::dag_id::DagId,
    ) -> Result<graphcal_compiler::hir::Expr, CompileError> {
        let scope = GenericScope::new();
        let prelude = PreludeTypeScope::graphcal();
        let context = ExprLoweringContext::new(
            owner,
            &self.module_resolver,
            &scope,
            &self.tir.registry.time_zones,
        )
        .with_prelude(&prelude);
        let context = if owner == &self.tir.root_dag_id {
            context.with_unit_registry(&self.tir.registry.units)
        } else {
            context
        };
        let (hir, diagnostics) = graphcal_compiler::hir::lower_expr_tolerant(expr, context);
        if let Some(error) = diagnostics.first() {
            return Err(CompileError::Eval(
                graphcal_compiler::hir::expr_lower_error_to_graphcal(error, &self.source),
            ));
        }
        Ok(hir)
    }

    fn binding_internal_error(&self, message: &str, span: Span) -> CompileError {
        CompileError::Eval(GraphcalError::InternalError {
            message: message.to_string(),
            src: self.source.clone(),
            span: span.into(),
        })
    }

    fn evaluate_closed_binding(
        &self,
        expr: &graphcal_compiler::hir::Expr,
    ) -> Result<RuntimeValue, CompileError> {
        let values = RuntimeValueMap::new();
        let locals = HirLocalValueMap::root();
        let builtin_fns = builtin_functions();
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        let context = EvalContext {
            cancellation,
            builtin_fns,
            registry: &self.tir.registry,
            src: &self.source,
            tir: &self.tir,
            current_dag: Some(self.tir.root()),
            root_values: Some(&values),
            struct_field_constraints: Some(&self.plan.struct_field_constraints),
            host_fns: Some(&self.host_fns),
        };
        eval_hir_expr(expr, &values, &locals, &context).map_err(CompileError::from)
    }

    fn binding_kind_error(&self, port: &ParameterPort, actual: &str) -> CompileError {
        self.binding_value_error(
            port,
            &format!(
                "cannot bind {actual} to `{}` of type `{}`",
                port.name,
                port.declared_type.format(&self.tir.registry.dimensions)
            ),
        )
    }

    fn binding_value_error(&self, port: &ParameterPort, message: &str) -> CompileError {
        CompileError::Eval(GraphcalError::EvalError {
            message: message.to_string(),
            src: self.source.clone(),
            span: port.span.into(),
        })
    }
}

fn build_parameter_ports(
    plan_id: u64,
    root_ast: &graphcal_compiler::desugar::desugared_ast::File,
    tir: &graphcal_compiler::tir::typed::TIR,
    plan: &crate::exec_plan::ExecPlan,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    source: &NamedSource<Arc<String>>,
) -> Result<Vec<ParameterPort>, CompileError> {
    root_ast
        .declarations
        .iter()
        .filter_map(|declaration| match &declaration.kind {
            DeclKind::Param(param) => Some((declaration.span, param)),
            _ => None,
        })
        .enumerate()
        .map(|(index, (span, param))| {
            let scoped = ScopedName::from(&param.name.value);
            let declared_type = declared_types.get(&scoped).cloned().ok_or_else(|| {
                CompileError::Eval(GraphcalError::InternalError {
                    message: format!("declared type missing for entry parameter `{scoped}`"),
                    src: source.clone(),
                    span: span.into(),
                })
            })?;
            let value_schema = model_value_schema(&declared_type, tir, source)?;
            let runtime_key = RuntimeDeclKey::for_local_decl(tir.root(), &scoped);
            let has_default = tir
                .root()
                .params
                .iter()
                .find(|entry| entry.name == scoped)
                .is_some_and(|entry| entry.default_expr.is_some());
            let domain = plan
                .domain_constraints
                .get(&runtime_key)
                .map(parameter_domain);
            Ok(ParameterPort {
                name: param.name.value.clone(),
                position: ParameterPosition { plan_id, index },
                declared_type,
                value_schema,
                domain,
                has_default,
                runtime_key,
                span,
            })
        })
        .collect()
}

fn build_output_ports(
    root_ast: &graphcal_compiler::desugar::desugared_ast::File,
    tir: &graphcal_compiler::tir::typed::TIR,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    source: &NamedSource<Arc<String>>,
) -> Result<Vec<ModelOutputPort>, CompileError> {
    root_ast
        .declarations
        .iter()
        .filter_map(|declaration| match &declaration.kind {
            DeclKind::Node(node) => Some((declaration.span, node)),
            _ => None,
        })
        .map(|(span, node)| {
            let scoped = ScopedName::from(&node.name.value);
            let declared_type = declared_types.get(&scoped).cloned().ok_or_else(|| {
                CompileError::Eval(GraphcalError::InternalError {
                    message: format!("declared type missing for output node `{scoped}`"),
                    src: source.clone(),
                    span: span.into(),
                })
            })?;
            let value_schema = model_value_schema(&declared_type, tir, source)?;
            Ok(ModelOutputPort {
                name: node.name.value.clone(),
                declared_type,
                value_schema,
                is_public: node.visibility.is_public(),
                runtime_key: RuntimeDeclKey::for_local_decl(tir.root(), &scoped),
            })
        })
        .collect()
}

fn model_value_schema(
    declared_type: &DeclaredType,
    tir: &graphcal_compiler::tir::typed::TIR,
    source: &NamedSource<Arc<String>>,
) -> Result<ModelValueSchema, CompileError> {
    model_value_schema_inner(declared_type, tir, source, &mut HashSet::new())
        .map_err(CompileError::Eval)
}

fn model_value_schema_inner(
    declared_type: &DeclaredType,
    tir: &graphcal_compiler::tir::typed::TIR,
    source: &NamedSource<Arc<String>>,
    visiting: &mut HashSet<graphcal_compiler::registry::declared_type::StructTypeRef>,
) -> Result<ModelValueSchema, GraphcalError> {
    match declared_type {
        DeclaredType::Quantity(dimension) => Ok(ModelValueSchema::Quantity(model_quantity_schema(
            dimension, tir,
        ))),
        DeclaredType::Complex(dimension) => Ok(ModelValueSchema::Complex(model_quantity_schema(
            dimension, tir,
        ))),
        DeclaredType::Bool => Ok(ModelValueSchema::Bool),
        DeclaredType::Int => Ok(ModelValueSchema::Int),
        DeclaredType::Datetime(scale) => Ok(ModelValueSchema::Datetime(*scale)),
        DeclaredType::Key(index) => {
            model_index_schema(index, tir, source).map(ModelValueSchema::Key)
        }
        DeclaredType::Indexed { element, index } => Ok(ModelValueSchema::Indexed {
            element: Box::new(model_value_schema_inner(element, tir, source, visiting)?),
            axis: model_index_schema(index, tir, source)?,
        }),
        DeclaredType::Struct(identity, generic_args) => {
            if !visiting.insert(identity.clone()) {
                return Err(GraphcalError::InternalError {
                    message: format!(
                        "recursive model type `{identity}` cannot be represented as a finite schema"
                    ),
                    src: source.clone(),
                    span: Span::new(0, 0).into(),
                });
            }
            let result = (|| {
                graphcal_compiler::tir::dim_check::concrete_model_constructors(
                    tir,
                    identity,
                    generic_args,
                    source,
                )?
                .into_iter()
                .map(|constructor| {
                    let fields = constructor
                        .fields()
                        .iter()
                        .map(|field| {
                            Ok(ModelFieldSchema {
                                name: field.name().clone(),
                                value: model_value_schema_inner(
                                    field.declared_type(),
                                    tir,
                                    source,
                                    visiting,
                                )?,
                            })
                        })
                        .collect::<Result<Vec<_>, GraphcalError>>()?;
                    Ok(ModelConstructorSchema {
                        name: constructor.name().clone(),
                        fields,
                    })
                })
                .collect::<Result<Vec<_>, GraphcalError>>()
                .map(|constructors| ModelValueSchema::Algebraic {
                    identity: identity.clone(),
                    generic_args: generic_args.clone(),
                    constructors,
                })
            })();
            visiting.remove(identity);
            result
        }
        DeclaredType::IndexArg(index) => Err(GraphcalError::InternalError {
            message: format!(
                "unresolved index argument `{index}` cannot be a concrete model value port"
            ),
            src: source.clone(),
            span: Span::new(0, 0).into(),
        }),
    }
}

fn model_quantity_schema(
    dimension: &graphcal_compiler::dimension::Dimension,
    tir: &graphcal_compiler::tir::typed::TIR,
) -> ModelQuantitySchema {
    let canonical_unit = (!dimension.is_dimensionless())
        .then(|| {
            crate::eval::types::default_unit_label(
                dimension,
                tir.registry.dimensions.base_dim_symbols(),
            )
        })
        .flatten()
        .map(|label| ModelUnitSchema {
            label,
            scale_to_si: 1.0,
        });
    ModelQuantitySchema {
        dimension: dimension.clone(),
        canonical_unit,
    }
}

fn model_index_schema(
    index: &IndexTypeRef,
    tir: &graphcal_compiler::tir::typed::TIR,
    source: &NamedSource<Arc<String>>,
) -> Result<ModelIndexSchema, GraphcalError> {
    if let Some(finite) = index.finite_index() {
        return Ok(ModelIndexSchema {
            identity: index.clone(),
            kind: ModelIndexKind::Finite {
                cardinality: finite.cardinality().get(),
            },
        });
    }
    let definition = index_def_for_ref(index, tir).ok_or_else(|| GraphcalError::InternalError {
        message: format!("concrete model index definition is unavailable for `{index}`"),
        src: source.clone(),
        span: Span::new(0, 0).into(),
    })?;
    let kind = match &definition.kind {
        IndexKind::Named { variants } => ModelIndexKind::Named {
            variants: variants.clone(),
        },
        IndexKind::Coordinate(data) => ModelIndexKind::Coordinate {
            coordinates_si: (0..data.cardinality())
                .map(|position| data.coordinate_value(position))
                .collect(),
            dimension: data.dimension.clone(),
            display_label: data.display_label.clone(),
            display_scale: data.display_scale,
        },
        IndexKind::Finite { cardinality } => ModelIndexKind::Finite {
            cardinality: cardinality.get(),
        },
        IndexKind::RequiredNamed | IndexKind::RequiredCoordinate { .. } => {
            return Err(GraphcalError::InternalError {
                message: format!("required model index `{index}` was not concretely bound"),
                src: source.clone(),
                span: Span::new(0, 0).into(),
            });
        }
    };
    Ok(ModelIndexSchema {
        identity: index.clone(),
        kind,
    })
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

fn index_def_for_ref<'tir>(
    index: &IndexTypeRef,
    tir: &'tir graphcal_compiler::tir::typed::TIR,
) -> Option<&'tir IndexDef> {
    index.finite_index().map_or_else(
        || {
            tir.root()
                .semantic
                .collection_refs
                .index_defs
                .get(index.declared_resolved()?)
        },
        |finite| tir.registry.indexes.get_finite_index(finite),
    )
}

fn normalize_binding_literal(
    mut expr: Expr,
    expected: &ModelValueSchema,
) -> Result<Expr, &'static str> {
    normalize_binding_literal_in_place(&mut expr, expected)?;
    Ok(expr)
}

#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::float_cmp,
    reason = "the boundary checks exact f64/i64 range and round-trip identity before conversion"
)]
fn normalize_binding_literal_in_place(
    expr: &mut Expr,
    expected: &ModelValueSchema,
) -> Result<(), &'static str> {
    match (&mut expr.kind, expected) {
        (AstExprKind::Integer(value), ModelValueSchema::Quantity(quantity))
            if quantity.dimension.is_dimensionless() =>
        {
            const MAX_EXACT_INTEGER: u64 = 1_u64 << f64::MANTISSA_DIGITS;
            if value.unsigned_abs() > MAX_EXACT_INTEGER {
                return Err("integer is not exactly representable as a real quantity");
            }
            expr.kind = AstExprKind::Number(*value as f64);
        }
        (AstExprKind::Number(value), ModelValueSchema::Int) => {
            const I64_EXCLUSIVE_UPPER: f64 = 9_223_372_036_854_775_808.0;
            if !value.is_finite()
                || value.fract() != 0.0
                || *value < -I64_EXCLUSIVE_UPPER
                || *value >= I64_EXCLUSIVE_UPPER
            {
                return Err("number is not exactly representable as Int");
            }
            let integer = *value as i64;
            if integer as f64 != *value {
                return Err("number is not exactly representable as Int");
            }
            expr.kind = AstExprKind::Integer(integer);
        }
        (AstExprKind::UnaryOp { operand, .. }, _) => {
            normalize_binding_literal_in_place(operand, expected)?;
        }
        (AstExprKind::FnCall { args, .. }, ModelValueSchema::Complex(quantity)) => {
            let component = ModelValueSchema::Quantity(quantity.clone());
            for argument in args {
                normalize_binding_literal_in_place(argument, &component)?;
            }
        }
        (
            AstExprKind::ConstructorCall { callee, fields, .. },
            ModelValueSchema::Algebraic { constructors, .. },
        ) => {
            if let Some(constructor) = constructors
                .iter()
                .find(|constructor| constructor.name.atom() == &callee.leaf().name)
            {
                for field in fields {
                    if let Some(field_schema) = constructor
                        .fields
                        .iter()
                        .find(|schema| schema.name == field.name.value)
                    {
                        normalize_binding_literal_in_place(&mut field.value, &field_schema.value)?;
                    }
                }
            }
        }
        (AstExprKind::MapLiteral { entries }, ModelValueSchema::Indexed { .. }) => {
            for entry in entries {
                let mut entry_schema = expected;
                for _ in 0..entry.keys.len() {
                    let ModelValueSchema::Indexed { element, .. } = entry_schema else {
                        break;
                    };
                    entry_schema = element;
                }
                normalize_binding_literal_in_place(&mut entry.value, entry_schema)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_closed_hir(expr: &graphcal_compiler::hir::Expr) -> Result<(), &'static str> {
    match &expr.kind {
        HirExprKind::Number(value) if value.is_finite() => Ok(()),
        HirExprKind::Number(_) => Err("numeric values must be finite"),
        HirExprKind::Integer(_)
        | HirExprKind::Bool(_)
        | HirExprKind::OffsetDateTimeLiteral(_)
        | HirExprKind::CivilDateTimeLiteral(_)
        | HirExprKind::ZonedDateTimeLiteral(_)
        | HirExprKind::IanaTimeZoneLiteral(_)
        | HirExprKind::VariantLiteral(_) => Ok(()),
        HirExprKind::UnitLiteral { value, .. } if value.is_finite() => Ok(()),
        HirExprKind::UnitLiteral { .. } => Err("quantity values must be finite"),
        HirExprKind::ConstRef(reference) if matches!(reference.value, ConstRef::Constructor(_)) => {
            Ok(())
        }
        HirExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => validate_closed_hir(operand),
        HirExprKind::FnCall { callee, args }
            if matches!(
                callee.value,
                FunctionRef::Builtin(BuiltinFnName::Complex | BuiltinFnName::Datetime)
                    | FunctionRef::Epoch { .. }
            ) =>
        {
            args.iter().try_for_each(validate_closed_hir)
        }
        HirExprKind::DisplayTimezone { expr, .. } => validate_closed_hir(expr),
        HirExprKind::ConstructorCall { fields, .. } => fields
            .iter()
            .try_for_each(|field| validate_closed_hir(&field.value)),
        HirExprKind::MapLiteral { entries } => entries
            .iter()
            .try_for_each(|entry| validate_closed_hir(&entry.value)),
        HirExprKind::KeyForm { arg, .. } => validate_closed_hir(arg),
        HirExprKind::Error { .. } => Err("an unresolved expression is not allowed"),
        HirExprKind::StringLiteral(_)
        | HirExprKind::TypeSystemRef(_)
        | HirExprKind::GraphRef(_)
        | HirExprKind::ConstRef(_)
        | HirExprKind::LocalRef(_)
        | HirExprKind::BinOp { .. }
        | HirExprKind::UnaryOp { .. }
        | HirExprKind::FnCall { .. }
        | HirExprKind::If { .. }
        | HirExprKind::Convert { .. }
        | HirExprKind::FieldAccess { .. }
        | HirExprKind::ForComp { .. }
        | HirExprKind::IndexAccess { .. }
        | HirExprKind::Scan { .. }
        | HirExprKind::Unfold { .. }
        | HirExprKind::Match { .. }
        | HirExprKind::InlineDagRef { .. } => {
            Err("references, computations, and control-flow expressions are not allowed")
        }
    }
}
