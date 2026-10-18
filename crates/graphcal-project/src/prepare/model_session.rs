//! The batch model-evaluation protocol over a prepared project.
//!
//! A prepared project publishes its [`ModelInterface`]; a configured
//! [`SessionSchema`] then resolves to a [`ModelSession`] that evaluates
//! request batches into result batches. Quantities travel in their canonical
//! SI unit at scale one unless the session chooses another unit for them
//! (see [`PreparedProject::model_unit_scale`]).

use std::sync::Arc;

use graphcal_compiler::dimension::{BaseDimId, Dimension as GraphcalDimension};
use graphcal_compiler::semantic::index_def::ConcreteIndexKind;
use graphcal_eval::eval::public_projection;
use graphcal_eval::eval_expr::RuntimeValue;
use graphcal_eval::runtime_value::IndexAxis;
use graphcal_model_protocol::{
    BaseDimensionName, Bounds, Dimension, Domain, Exponent, FiniteF64, IndexName, InputColumn,
    InterfaceError, InterfaceFingerprint, KeyType, LabelError, ModelInterface, OutputDescription,
    ParameterDescription, ParameterError, PortName, PortWire, QuantityType, RequestBatch,
    ResultBatch, ResultBatchBuilder, RowFailure, RowOutcome, SessionError, SessionOutput,
    SessionSchema, SessionSpec, SiScale, UnitSpelling, Value, VariantName, WireType,
};

use super::{
    CheckedType, CompileError, DeclName, Error, IndexVariantName, ModelExecutionError,
    ModelOutputPort, ModelRowFailure, ModelRowOutcome, OutputProjectionError, ParameterBindingRow,
    ParameterDomain, ParameterPort, ParameterPosition, PreparedProject,
};
use crate::binding_error::BindingError;

/// How one varied parameter is bound from its request column.
#[derive(Debug, Clone)]
enum InputBinding {
    /// A quantity in the unit of the scale, or in SI without one.
    Quantity(Option<SiScale>),
    Int,
    Bool,
    /// The variants of the parameter's index, in the key type's order.
    Key(Vec<IndexVariantName>),
}

#[derive(Debug, Clone)]
struct SessionInputPort {
    name: PortName,
    position: ParameterPosition,
    binding: InputBinding,
}

/// A configured session resolved against the prepared project it belongs to.
#[derive(Debug, Clone)]
pub struct ModelSession {
    plan_id: u64,
    schema: Arc<SessionSchema>,
    inputs: Vec<SessionInputPort>,
    outputs: Vec<ModelOutputPort>,
}

impl ModelSession {
    /// The session schema both peers share.
    #[must_use]
    pub const fn schema(&self) -> &Arc<SessionSchema> {
        &self.schema
    }
}

/// A violated invariant while describing a prepared project as a protocol
/// interface. Graphcal names are valid labels and unique per DAG, so none of
/// these occurs for a checked project.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModelInterfaceError {
    #[error("internal model interface invariant failed: {0}")]
    Label(#[from] LabelError),
    #[error("internal model interface invariant failed: {0}")]
    Interface(#[from] InterfaceError),
    #[error("internal model interface invariant failed: {0}")]
    Parameter(#[from] ParameterError),
}

/// Why a session schema does not belong to a prepared project.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModelSessionError {
    #[error(transparent)]
    Interface(#[from] ModelInterfaceError),
    #[error("invalid session: {0}")]
    Session(#[from] SessionError),
    #[error(
        "the session was configured for interface {session}, but the model's interface is {model}"
    )]
    FingerprintMismatch {
        session: InterfaceFingerprint,
        model: InterfaceFingerprint,
    },
    /// The fingerprints agree, yet the model resolves the session's
    /// selection differently, e.g. to other unit scales.
    #[error("the session schema differs from the one this model configures for its selection")]
    SchemaMismatch,
    /// The fingerprints agree, yet the model lacks a session port.
    #[error("internal model session invariant failed: the model has no port `{name}`")]
    MissingPort { name: PortName },
}

impl PreparedProject {
    /// The protocol interface: every entry parameter and public root node in
    /// declaration order. Ports the protocol cannot carry are listed as
    /// unsupported, with the reason.
    ///
    /// # Errors
    ///
    /// Returns [`ModelInterfaceError`] only for a violated internal
    /// invariant.
    pub fn model_interface(&self) -> Result<ModelInterface, ModelInterfaceError> {
        let parameters = self
            .parameter_ports
            .iter()
            .map(|port| self.parameter_description(port))
            .collect::<Result<Vec<_>, _>>()?;
        let outputs = self
            .output_ports
            .iter()
            .filter(|port| port.is_public)
            .map(|port| {
                Ok(OutputDescription::new(
                    port_name(&port.name)?,
                    self.type_label(&port.declared_type),
                    self.port_wire(&port.declared_type)?,
                ))
            })
            .collect::<Result<Vec<_>, ModelInterfaceError>>()?;
        Ok(ModelInterface::new(parameters, outputs)?)
    }

    /// Configure the session `spec` selects over [`Self::model_interface`],
    /// resolving its unit choices with [`Self::model_unit_scale`].
    ///
    /// # Errors
    ///
    /// Returns [`ModelSessionError::Session`] for an invalid selection or
    /// unit choice.
    pub fn session_schema(&self, spec: &SessionSpec) -> Result<SessionSchema, ModelSessionError> {
        Ok(self
            .model_interface()?
            .configure(spec, |port: &PortName, unit: &UnitSpelling| {
                self.model_unit_scale(port, unit)
                    .map_err(|error| error.to_string())
            })?)
    }

    /// Resolve `schema`, configured over [`Self::model_interface`], for
    /// evaluation.
    ///
    /// # Errors
    ///
    /// Returns [`ModelSessionError::FingerprintMismatch`] when the schema was
    /// configured over another interface, and
    /// [`ModelSessionError::SchemaMismatch`] when it is not the schema
    /// [`Self::session_schema`] configures for its own selection.
    pub fn model_session(
        &self,
        schema: Arc<SessionSchema>,
    ) -> Result<ModelSession, ModelSessionError> {
        let model = InterfaceFingerprint::of(&self.model_interface()?);
        if schema.interface_fingerprint() != model {
            return Err(ModelSessionError::FingerprintMismatch {
                session: schema.interface_fingerprint(),
                model,
            });
        }
        if self.session_schema(&schema.spec())? != *schema {
            return Err(ModelSessionError::SchemaMismatch);
        }
        let missing = |name: &PortName| ModelSessionError::MissingPort { name: name.clone() };
        let inputs = schema
            .inputs()
            .iter()
            .map(|input| {
                let port = self
                    .parameter_ports
                    .iter()
                    .find(|port| port.name.as_str() == input.name().as_str())
                    .ok_or_else(|| missing(input.name()))?;
                let binding = match input.wire() {
                    WireType::Quantity(_) => InputBinding::Quantity(input.si_scale()),
                    WireType::Int => InputBinding::Int,
                    WireType::Bool => InputBinding::Bool,
                    WireType::Key(_) => InputBinding::Key(
                        self.named_variants(&port.declared_type)
                            .ok_or_else(|| missing(input.name()))?,
                    ),
                };
                Ok(SessionInputPort {
                    name: input.name().clone(),
                    position: port.position,
                    binding,
                })
            })
            .collect::<Result<Vec<_>, ModelSessionError>>()?;
        let outputs = schema
            .outputs()
            .iter()
            .map(|output| {
                self.output_ports
                    .iter()
                    .find(|port| port.is_public && port.name.as_str() == output.name().as_str())
                    .cloned()
                    .ok_or_else(|| missing(output.name()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ModelSession {
            plan_id: self.plan_id,
            schema,
            inputs,
            outputs,
        })
    }

    /// Evaluate every row of `request`, in order.
    ///
    /// A finite input outside its parameter's domain, a failed assertion, and
    /// an evaluation error are row failures. Any other binding failure is a
    /// process error.
    ///
    /// # Errors
    ///
    /// Returns [`ModelExecutionError`] when the session or request belongs to
    /// another project or session, or when binding or evaluation hits an
    /// internal failure.
    pub fn evaluate_request(
        &self,
        session: &ModelSession,
        request: &RequestBatch,
    ) -> Result<ResultBatch, ModelExecutionError> {
        if session.plan_id != self.plan_id {
            return Err(ModelExecutionError::PlanMismatch);
        }
        if request.schema().as_ref() != session.schema.as_ref() {
            return Err(ModelExecutionError::SessionMismatch);
        }
        let mut results = ResultBatchBuilder::new(Arc::clone(&session.schema));
        for row in 0..request.rows() {
            let outcome = match self.bind_request_row(session, request, row)? {
                Err(failure) => RowOutcome::Failure(failure),
                Ok(bindings) => {
                    match self.evaluate_row(&bindings, &session.outputs, |runtime, position| {
                        Self::wire_value(runtime, &session.schema.outputs()[position])
                    })? {
                        ModelRowOutcome::Success(values) => RowOutcome::Success(values),
                        ModelRowOutcome::Failure(failure) => {
                            RowOutcome::Failure(protocol_failure(&failure))
                        }
                    }
                }
            };
            results.push(outcome)?;
        }
        Ok(results.finish()?)
    }

    /// Bind one request row; a domain violation is the row's failure.
    fn bind_request_row(
        &self,
        session: &ModelSession,
        request: &RequestBatch,
        row: usize,
    ) -> Result<Result<ParameterBindingRow, RowFailure>, ModelExecutionError> {
        let mut builder = self.binding_builder();
        for (input, column) in session.inputs.iter().zip(request.columns()) {
            let bound = match (&input.binding, column) {
                (InputBinding::Quantity(scale), InputColumn::Quantity(values)) => {
                    let value = values[row];
                    match scale.map_or(Ok(value), |scale| scale.to_si(value)) {
                        Ok(si) => builder.bind_quantity(input.position, si.get()),
                        Err(_) => {
                            return Ok(Err(RowFailure::InputOutOfDomain {
                                parameter: input.name.clone(),
                                message: format!(
                                    "{} in the chosen unit overflows when converted to SI",
                                    value.get()
                                ),
                            }));
                        }
                    }
                }
                (InputBinding::Int, InputColumn::Int(values)) => {
                    builder.bind_integer(input.position, values[row])
                }
                (InputBinding::Bool, InputColumn::Bool(values)) => {
                    builder.bind_boolean(input.position, values[row])
                }
                (InputBinding::Key(variants), InputColumn::Key(positions)) => {
                    builder.bind_named_key(input.position, &variants[positions[row].get()])
                }
                _ => return Err(ModelExecutionError::SessionMismatch),
            };
            match bound {
                Ok(()) => {}
                Err(CompileError::Binding(BindingError::DomainViolation { violation, .. })) => {
                    return Ok(Err(RowFailure::InputOutOfDomain {
                        parameter: input.name.clone(),
                        message: violation.to_string(),
                    }));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(Ok(builder.finish()?))
    }

    /// The protocol value of an output's runtime value, in the output's unit.
    fn wire_value(
        runtime: &RuntimeValue,
        output: &SessionOutput,
    ) -> Result<Value, OutputProjectionError> {
        Ok(match output.wire() {
            WireType::Quantity(_) => {
                let si = FiniteF64::new(public_projection::project_quantity(runtime)?.get());
                Value::Quantity(
                    si.and_then(|si| output.si_scale().map_or(Ok(si), |scale| scale.from_si(si)))
                        .map_err(|_| OutputProjectionError::Unrepresentable)?,
                )
            }
            WireType::Int => Value::Int(public_projection::project_int(runtime)?),
            WireType::Bool => Value::Bool(public_projection::project_bool(runtime)?),
            WireType::Key(key) => Value::Key(
                key.variants()
                    .get(public_projection::project_key_position(runtime)?)
                    .cloned()
                    .ok_or(OutputProjectionError::Unrepresentable)?,
            ),
        })
    }

    fn parameter_description(
        &self,
        port: &ParameterPort,
    ) -> Result<ParameterDescription, ModelInterfaceError> {
        let wire = self.port_wire(&port.declared_type)?;
        // Checked domains have finite, ordered bounds; a domain that does not
        // convert is not published rather than misrepresented.
        let domain = match (&wire, &port.domain) {
            (
                PortWire::Supported(WireType::Quantity(_)),
                Some(ParameterDomain::Quantity(bounds)),
            ) => {
                let finite = |bound: Option<&f64>| {
                    bound.map(|bound| FiniteF64::new(*bound)).transpose().ok()
                };
                finite(bounds.lower())
                    .zip(finite(bounds.upper()))
                    .and_then(|(min, max)| Bounds::new(min, max).ok())
                    .map(Domain::Quantity)
            }
            (PortWire::Supported(WireType::Int), Some(ParameterDomain::Integer(bounds))) => {
                Bounds::new(bounds.lower().copied(), bounds.upper().copied())
                    .ok()
                    .map(Domain::Int)
            }
            _ => None,
        };
        Ok(ParameterDescription::new(
            port_name(&port.name)?,
            self.type_label(&port.declared_type),
            wire,
            domain,
            port.has_default,
        )?)
    }

    fn type_label(&self, declared_type: &CheckedType) -> String {
        declared_type.format(&self.tir().registry().dimensions)
    }

    /// How a value of `declared_type` travels on the wire.
    fn port_wire(&self, declared_type: &CheckedType) -> Result<PortWire, ModelInterfaceError> {
        let unsupported = |reason: String| PortWire::Unsupported { reason };
        Ok(match declared_type {
            CheckedType::Quantity(dimension) => self.quantity_wire(dimension)?,
            CheckedType::Int => PortWire::Supported(WireType::Int),
            CheckedType::Bool => PortWire::Supported(WireType::Bool),
            CheckedType::Key(index) => match self.named_variants(declared_type) {
                None => unsupported(format!("key index `{index}` is not a concrete named index")),
                Some(variants) => {
                    let variants = variants
                        .iter()
                        .map(|variant| VariantName::new(variant.as_str()))
                        .collect::<Result<Vec<_>, _>>()?;
                    match KeyType::new(IndexName::new(index.to_string())?, variants) {
                        Ok(key) => PortWire::Supported(WireType::Key(key)),
                        Err(error) => unsupported(error.to_string()),
                    }
                }
            },
            CheckedType::Complex(_)
            | CheckedType::Datetime(_)
            | CheckedType::Struct(..)
            | CheckedType::Indexed { .. } => unsupported(format!(
                "type `{}` is not supported by protocol version {}",
                self.type_label(declared_type),
                graphcal_model_protocol::PROTOCOL_VERSION
            )),
        })
    }

    /// A quantity of `dimension` in its canonical SI unit at scale one.
    fn quantity_wire(
        &self,
        dimension: &GraphcalDimension,
    ) -> Result<PortWire, ModelInterfaceError> {
        let dimensions = &self.tir().registry().dimensions;
        let unsupported = |reason: String| Ok(PortWire::Unsupported { reason });
        let mut exponents = Vec::new();
        for (base, exponent) in dimension.iter() {
            let name = match base {
                BaseDimId::Prelude(prelude) => prelude.as_str().to_owned(),
                BaseDimId::UserDefined(resolved) => resolved.to_string(),
            };
            let Some(exponent) = u32::try_from(exponent.den())
                .ok()
                .and_then(|denominator| Exponent::new(exponent.num(), denominator).ok())
            else {
                return unsupported(format!(
                    "dimension exponent {exponent} is not representable"
                ));
            };
            exponents.push((BaseDimensionName::new(name)?, exponent));
        }
        let unit = if dimension.is_dimensionless() {
            None
        } else {
            let Some(spelling) = graphcal_eval::eval::types::default_unit_label(
                dimension,
                &dimensions.base_unit_symbols(),
            ) else {
                return unsupported(format!(
                    "dimension `{}` has no canonical SI unit",
                    dimensions.format_dimension(dimension)
                ));
            };
            Some(UnitSpelling::new(spelling)?)
        };
        Ok(
            match QuantityType::new(exponents.into_iter().collect::<Dimension>(), unit) {
                Ok(quantity) => PortWire::Supported(WireType::Quantity(quantity)),
                Err(error) => PortWire::Unsupported {
                    reason: error.to_string(),
                },
            },
        )
    }

    /// The variants of a `Key<I>` type over a concrete named index, in
    /// declaration order.
    fn named_variants(&self, declared_type: &CheckedType) -> Option<Vec<IndexVariantName>> {
        let CheckedType::Key(index) = declared_type else {
            return None;
        };
        let axis = IndexAxis::resolve(self.tir(), index)?;
        match axis.kind() {
            ConcreteIndexKind::Named { variants } => Some(variants.as_slice().to_vec()),
            _ => None,
        }
    }
}

fn port_name(name: &DeclName) -> Result<PortName, LabelError> {
    PortName::new(name.as_str())
}

fn protocol_failure(failure: &ModelRowFailure) -> RowFailure {
    match failure {
        ModelRowFailure::Assertion { name, message } => RowFailure::AssertionFailed {
            assertion: name.to_string(),
            message: message.clone(),
        },
        ModelRowFailure::Declaration { .. } | ModelRowFailure::UnfinishedCalls(_) => {
            RowFailure::EvaluationError {
                message: failure.to_string(),
            }
        }
    }
}
