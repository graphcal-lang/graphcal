//! Transport-independent model and strict Tenax-v2 projections.

use super::*;

/// Inclusive lower and upper bounds for one external input family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InclusiveBounds<T> {
    pub(super) lower: Option<T>,
    pub(super) upper: Option<T>,
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
    pub(super) name: DeclName,
    pub(super) position: ParameterPosition,
    pub(super) declared_type: DeclaredType,
    pub(super) value_schema: ModelValueSchema,
    pub(super) domain: Option<ParameterDomain>,
    pub(super) has_default: bool,
    pub(super) runtime_key: RuntimeDeclKey,
    pub(super) span: Span,
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
    pub(super) name: DeclName,
    pub(super) declared_type: DeclaredType,
    pub(super) value_schema: ModelValueSchema,
    pub(super) is_public: bool,
    pub(super) runtime_key: RuntimeDeclKey,
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
    schema_graph: Arc<ModelSchemaGraph>,
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

    /// Algebraic definitions referenced by every input and output schema.
    #[must_use]
    pub fn schema_graph(&self) -> &ModelSchemaGraph {
        &self.schema_graph
    }
}

impl PreparedProject {
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
                    .source_order()
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
            schema_graph: Arc::clone(&self.schema_graph),
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
                    actual: port.declared_type.format(&self.tir.registry().dimensions),
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
        let EvalLoopResult { values, errors, .. } = run_eval_loop_with_bindings(
            &self.plan,
            &row.bindings,
            &self.tir,
            &self.source,
            builtin_fns,
            &self.host_fns,
            &cancellation,
        )?;

        if !errors.is_empty() {
            let (name, error) = self.first_runtime_error(&errors)?.ok_or_else(|| {
                ModelExecutionError::Internal(
                    "runtime error map contains no declaration from root source order".to_string(),
                )
            })?;
            return Ok(ModelRowOutcome::Failure(ModelRowFailure {
                message: format!("{name}: {error}"),
            }));
        }

        let empty_locals = HirLocalValueMap::root();
        let ctx = EvalContext {
            cancellation,
            work_budget: crate::eval_expr::fresh_work_budget(),
            builtin_fns,
            registry: self.tir.registry(),
            src: &self.source,
            tir: &self.tir,
            current_dag: self.tir.root(),
            current_decl: None,
            root_values: Some(&values),
            root_presentation_instances: None,
            checked_execution_facts: Some(&self.plan.checked_execution_facts),
            presentation_calls: None,
            struct_field_constraints: Some(&self.plan.struct_field_constraints),
            generic_nat_bindings: None,
            host_fns: Some(&self.host_fns),
        };
        for assertion in self.tir.root().asserts() {
            let owner = self
                .tir
                .root()
                .lookup_decl_identity(&assertion.name)
                .into_bound()
                .map_err(|probe| ModelExecutionError::Internal(probe.to_string()))?;
            let result = crate::eval::runtime::evaluate_assert_with_expected_fail(
                &assertion.body,
                self.plan
                    .expected_fail
                    .get(&RuntimeDeclKey::resolved(owner.clone())),
                &values,
                &empty_locals,
                &ctx.for_decl(&owner),
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
                let runtime = values.get(&output.runtime_key).ok_or_else(|| {
                    ModelExecutionError::Internal(format!(
                        "selected output `{}` has no runtime value",
                        output.name
                    ))
                })?;
                crate::eval::public_projection::EvaluatedValue::new(runtime, &output.declared_type)
                    .project(&self.tir, &self.source)
                    .map_err(ModelExecutionError::from)
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
        if self
            .schema_graph
            .contains_recursive_type(&port.value_schema)
        {
            return Err(ModelDefinitionError::RecursiveInputTypeUnsupported {
                name: port.name.clone(),
                actual: port.declared_type.format(&self.tir.registry().dimensions),
            });
        }
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
                            self.tir.registry().dimensions.base_dim_symbols(),
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
                        actual: port.declared_type.format(&self.tir.registry().dimensions),
                    });
                };
                let IndexKind::Named { variants } = &definition.kind else {
                    return Err(ModelDefinitionError::UnsupportedInputType {
                        name: port.name.clone(),
                        actual: port.declared_type.format(&self.tir.registry().dimensions),
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
                    actual: port.declared_type.format(&self.tir.registry().dimensions),
                });
            }
        };
        Ok(TenaxV2Input {
            name: port.name.clone(),
            position: port.position,
            kind,
        })
    }
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
    #[error("input `{name}` has recursive type `{actual}`, which Tenax schema v2 cannot represent")]
    RecursiveInputTypeUnsupported { name: DeclName, actual: String },
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
