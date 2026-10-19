//! Transport-independent model ports and per-row model evaluation.

use graphcal_eval::eval::runtime::{RootFailure, RootOutcome};

use super::{
    Arc, CheckedType, CompileError, DeclName, Error, HashSet, ModelSchemaGraph, ModelValueSchema,
    ParameterBindingRow, ParameterPosition, PreparedProject, ResolvedDeclName, ScopedName, Span,
    TimeScale, Value,
};

/// Inclusive lower and upper bounds for one external input family.
///
/// Every declared bound is ordered (comparable with itself, so never NaN),
/// and a lower bound never exceeds an upper bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InclusiveBounds<T> {
    lower: Option<T>,
    upper: Option<T>,
}

/// Why bounds do not form an inclusive interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum InclusiveBoundsError {
    #[error("a domain bound is unordered")]
    Unordered,
    #[error("the lower domain bound exceeds the upper bound")]
    Inverted,
}

impl<T: PartialOrd> InclusiveBounds<T> {
    /// Bounds from `lower` to `upper`, each inclusive when declared.
    ///
    /// # Errors
    ///
    /// Returns [`InclusiveBoundsError`] when a bound is unordered or the
    /// lower bound exceeds the upper bound.
    pub fn try_new(lower: Option<T>, upper: Option<T>) -> Result<Self, InclusiveBoundsError> {
        let ordered = |bound: &Option<T>| {
            bound
                .as_ref()
                .is_none_or(|bound| bound.partial_cmp(bound).is_some())
        };
        if !(ordered(&lower) && ordered(&upper)) {
            return Err(InclusiveBoundsError::Unordered);
        }
        if let (Some(lower), Some(upper)) = (&lower, &upper)
            && lower > upper
        {
            return Err(InclusiveBoundsError::Inverted);
        }
        Ok(Self { lower, upper })
    }
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
    pub(super) declared_type: CheckedType,
    pub(super) value_schema: ModelValueSchema,
    pub(super) domain: Option<ParameterDomain>,
    pub(super) has_default: bool,
    pub(super) runtime_key: ResolvedDeclName,
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
    pub const fn declared_type(&self) -> &CheckedType {
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

/// One root node available to model-output selection: a node the entry DAG
/// declares, or a public node member of one of its whole-instance includes.
#[derive(Debug, Clone)]
pub struct ModelOutputPort {
    pub(super) name: ScopedName,
    pub(super) declared_type: CheckedType,
    pub(super) value_schema: ModelValueSchema,
    pub(super) is_public: bool,
    pub(super) runtime_key: ResolvedDeclName,
}

impl ModelOutputPort {
    /// Source-level node name: local for a declared node, qualified by the
    /// include alias for an instance member (`l::x`).
    #[must_use]
    pub const fn name(&self) -> &ScopedName {
        &self.name
    }

    /// Fully concrete Graphcal output type.
    #[must_use]
    pub const fn declared_type(&self) -> &CheckedType {
        &self.declared_type
    }

    /// Recursive concrete value schema.
    #[must_use]
    pub const fn value_schema(&self) -> &ModelValueSchema {
        &self.value_schema
    }

    /// Whether the source node is explicitly public; instance members are
    /// public in their template.
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
        requested_outputs: &[ScopedName],
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
                let actual_kind = name.as_bare().and_then(|name| {
                    self.tir()
                        .root()
                        .declarations()
                        .find_map(|entry| (entry.name() == name).then_some(entry.category()))
                });
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

    /// Evaluate one generic model row without display/plot assembly.
    pub fn evaluate_model_row(
        &self,
        row: &ParameterBindingRow,
        model: &PreparedModel,
    ) -> Result<ModelRowOutcome, ModelExecutionError> {
        if model.plan_id != self.plan_id {
            return Err(ModelExecutionError::PlanMismatch);
        }
        self.evaluate_row(row, &model.outputs, |runtime, position| {
            let output = &model.outputs[position];
            graphcal_eval::eval::public_projection::project(
                graphcal_eval::runtime_presentation::PresentedRef::plain(runtime),
                &output.declared_type,
            )
            .map(|(value, _)| value)
            .map_err(OutputProjectionError::Invariant)
        })
    }

    /// Evaluate one row, projecting the value of each of `outputs` with
    /// `project`, which receives the output's position in `outputs`.
    pub(super) fn evaluate_row<T>(
        &self,
        row: &ParameterBindingRow,
        outputs: &[ModelOutputPort],
        project: impl Fn(
            &graphcal_eval::eval_expr::RuntimeValue,
            usize,
        ) -> Result<T, OutputProjectionError>,
    ) -> Result<ModelRowOutcome<T>, ModelExecutionError> {
        self.validate_row_identity(row)
            .map_err(ModelExecutionError::from)?;

        let outcome = graphcal_compiler::outcome::without_cancellation(|cancellation| {
            RootOutcome::evaluate(
                self.plan(),
                &row.bindings,
                self.source,
                &self.sources,
                &self.host_fns,
                &self.output_assembly.display_names,
                cancellation,
            )
        })
        .map_err(|error| self.render(error))?;
        if let Some(failure) =
            outcome.first_failure(self.plan(), &self.output_assembly.display_names)
        {
            return Ok(ModelRowOutcome::Failure(self.row_failure(failure)));
        }
        let values = outcome.values();
        outputs
            .iter()
            .enumerate()
            .map(|(position, output)| {
                let runtime = values.get(&output.runtime_key).ok_or_else(|| {
                    ModelExecutionError::MissingOutputValue {
                        name: output.name.clone(),
                    }
                })?;
                project(runtime, position).map_err(|error| match error {
                    OutputProjectionError::Invariant(invariant) => ModelExecutionError::Compile(
                        self.render(invariant.into_internal_error(self.source)),
                    ),
                    OutputProjectionError::Unrepresentable => {
                        ModelExecutionError::UnrepresentableOutput {
                            name: output.name.clone(),
                        }
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(ModelRowOutcome::Success)
    }

    /// The diagnostic a model row reports for the first failure of its
    /// evaluation, naming private include scopes as the project displays them.
    fn row_failure(&self, failure: RootFailure<'_>) -> ModelRowFailure {
        let display = |name: &graphcal_compiler::syntax::module_name::ScopedName| {
            graphcal_compiler::display::include_scope_names::name_include_scopes(
                name,
                self.output_assembly.display_names.include_scopes(),
            )
        };
        match failure {
            RootFailure::Declaration { name, reason } => ModelRowFailure::Declaration {
                name: display(&name),
                reason,
            },
            RootFailure::Assertion { name, message } => ModelRowFailure::Assertion {
                name: display(name),
                message,
            },
            RootFailure::UnfinishedCalls(calls) => ModelRowFailure::UnfinishedCalls(calls),
        }
    }
}

/// Why an output value cannot be projected for a model row.
pub(super) enum OutputProjectionError {
    /// The value contradicts its checked type.
    Invariant(graphcal_eval::invariant::Invariant),
    /// The value has no representation in the requested wire type.
    Unrepresentable,
}

impl From<graphcal_eval::invariant::Invariant> for OutputProjectionError {
    fn from(invariant: graphcal_eval::invariant::Invariant) -> Self {
        Self::Invariant(invariant)
    }
}

/// Ordinary recursive typed per-row model outcome: the selected outputs,
/// each projected as a `T`.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelRowOutcome<T = Value> {
    Success(Vec<T>),
    Failure(ModelRowFailure),
}

/// The first ordinary Graphcal failure of a model row, naming declarations
/// as the project displays them; its rendering is the row's stable human
/// diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModelRowFailure {
    /// A declaration failed, or is unavailable.
    #[error("{name}: {reason}")]
    Declaration {
        name: graphcal_compiler::syntax::module_name::ScopedName,
        reason: graphcal_eval::eval::OutputUnavailable,
    },
    /// An assertion did not pass.
    #[error("assertion `{name}`: {message}")]
    Assertion {
        name: graphcal_compiler::syntax::module_name::ScopedName,
        message: String,
    },
    /// A call reached unfinished formulas of the DAG it invoked.
    #[error("unfinished formulas in invoked DAGs: {}", unfinished_calls(.0))]
    UnfinishedCalls(Vec<graphcal_eval::eval::OutputDeclName>),
}

fn unfinished_calls(calls: &[graphcal_eval::eval::OutputDeclName]) -> String {
    calls
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// Process-scoped failure while evaluating a prepared model.
#[derive(Debug, Error)]
pub enum ModelExecutionError {
    #[error(transparent)]
    Compile(#[from] CompileError),
    #[error("model projection belongs to another prepared project")]
    PlanMismatch,
    /// A selected output was planned but evaluation left no value for it.
    #[error(
        "internal model projection invariant failed: selected output `{name}` has no runtime value"
    )]
    MissingOutputValue { name: ScopedName },
    /// A selected output's value has no representation in its wire type.
    #[error(
        "internal model projection invariant failed: selected output `{name}` has no wire representation"
    )]
    UnrepresentableOutput { name: ScopedName },
    #[error("the request batch belongs to another model session")]
    SessionMismatch,
    /// An evaluated row does not fit the session's result batch.
    #[error("internal model session invariant failed: {0}")]
    ResultBatch(#[from] graphcal_model_protocol::ResultBatchError),
}

/// Why outputs cannot be selected for a prepared model.
#[derive(Debug, Clone, Error)]
pub enum ModelDefinitionError {
    #[error("at least one output is required")]
    NoOutputs,
    #[error("selected output `{name}` occurs more than once")]
    DuplicateOutput { name: ScopedName },
    #[error("entry DAG has no declaration named `{name}`")]
    UnknownOutput { name: ScopedName },
    #[error("selected output `{name}` is a {actual_kind}, not a node")]
    OutputNotNode {
        name: ScopedName,
        actual_kind: graphcal_compiler::declaration_category::DeclCategory,
    },
    #[error("selected output `{name}` is private")]
    PrivateOutput { name: ScopedName },
}
