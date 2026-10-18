//! The batch model-evaluation protocol over a prepared project.
//!
//! A prepared project publishes its [`ModelInterface`]; a configured
//! [`SessionSchema`] then resolves to a [`ModelSession`] that evaluates
//! request batches into result batches. Quantities travel in their canonical
//! SI unit at scale one unless the session chooses another unit for them
//! (see [`PreparedProject::model_unit_scale`]).

use std::sync::Arc;

use graphcal_eval::eval::bindings::RuntimeParameterBinding;
use graphcal_eval::eval_expr::j2000_seconds::seconds_since_j2000;
use graphcal_model_protocol::{
    Bounds, Domain, FiniteF64, InputColumn, InterfaceError, InterfaceFingerprint, LabelError,
    ModelInterface, OutputDescription, ParameterDescription, ParameterError, PortName,
    RequestBatch, ResultBatch, ResultBatchBuilder, RowFailure, RowOutcome, SessionError,
    SessionSchema, SessionSpec, SiScale, UnitSpelling, WireType,
};

use super::model_value::{ValueBindError, ValueCodec, ValueCodecError};
use super::{
    CompileError, DeclName, Error, ModelExecutionError, ModelOutputPort, ModelRowFailure,
    ModelRowOutcome, ParameterBindingRow, ParameterDomain, ParameterPort, ParameterPosition,
    PreparedProject,
};
use crate::binding_error::BindingError;

/// How one varied parameter is bound from its request column.
#[derive(Debug, Clone)]
enum InputBinding {
    /// A quantity in the unit of the scale, or in SI without one.
    Quantity(Option<SiScale>),
    Int,
    Bool,
    /// A key, complex, datetime, indexed, or algebraic value, converted by
    /// the parameter's codec.
    Value(ValueCodec),
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
    /// The codec of each output, in session order.
    output_codecs: Vec<ValueCodec>,
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
    #[error(
        "internal model interface invariant failed: type `{type_name}` has no model definition"
    )]
    MissingDefinition { type_name: String },
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
    /// A port's value conversion cannot be prepared.
    #[error("internal model session invariant failed for port `{name}`: {message}")]
    Codec { name: PortName, message: String },
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
                    self.port_wire(&port.value_schema)?,
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
                    WireType::Key(_)
                    | WireType::Complex(_)
                    | WireType::Datetime(_)
                    | WireType::Indexed(_)
                    | WireType::Algebraic(_) => InputBinding::Value(
                        self.value_codec(&port.value_schema, input.si_scale())
                            .map_err(|error| codec_error(input.name(), &error))?,
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
        let output_codecs = outputs
            .iter()
            .zip(schema.outputs())
            .map(|(port, output)| {
                self.value_codec(&port.value_schema, output.si_scale())
                    .map_err(|error| codec_error(output.name(), &error))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ModelSession {
            plan_id: self.plan_id,
            schema,
            inputs,
            outputs,
            output_codecs,
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
                        session.output_codecs[position].protocol_value(runtime)
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
                (InputBinding::Value(codec), _) => {
                    let value = request
                        .value(
                            session
                                .schema
                                .input_slot(input.name.as_str())
                                .ok_or(ModelExecutionError::SessionMismatch)?,
                            row,
                        )
                        .ok_or(ModelExecutionError::SessionMismatch)?;
                    match codec.runtime_value(value) {
                        Ok(runtime) => {
                            builder.insert(input.position, RuntimeParameterBinding::plain(runtime))
                        }
                        Err(ValueBindError::OutOfRange(message)) => {
                            return Ok(Err(RowFailure::InputOutOfDomain {
                                parameter: input.name.clone(),
                                message,
                            }));
                        }
                        Err(ValueBindError::Shape) => {
                            return Err(ModelExecutionError::SessionMismatch);
                        }
                    }
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

    fn parameter_description(
        &self,
        port: &ParameterPort,
    ) -> Result<ParameterDescription, ModelInterfaceError> {
        let wire = self.port_wire(&port.value_schema)?;
        // Checked domains have finite, ordered bounds; a domain that does not
        // convert is not published rather than misrepresented. The domain of
        // an indexed parameter bounds its innermost elements.
        let domain = match (wire.supported().map(WireType::leaf), &port.domain) {
            (Some(WireType::Quantity(_)), Some(ParameterDomain::Quantity(bounds))) => {
                let finite = |bound: Option<&f64>| {
                    bound.map(|bound| FiniteF64::new(*bound)).transpose().ok()
                };
                finite(bounds.lower())
                    .zip(finite(bounds.upper()))
                    .and_then(|(min, max)| Bounds::new(min, max).ok())
                    .map(Domain::Quantity)
            }
            (Some(WireType::Int), Some(ParameterDomain::Integer(bounds))) => {
                Bounds::new(bounds.lower().copied(), bounds.upper().copied())
                    .ok()
                    .map(Domain::Int)
            }
            (Some(WireType::Datetime(_)), Some(ParameterDomain::Datetime { scale, bounds })) => {
                let seconds = |bound: Option<&hifitime::Duration>| {
                    bound
                        .map(|tai| {
                            seconds_since_j2000(hifitime::Epoch::from_tai_duration(*tai), *scale)
                                .ok()
                                .and_then(|seconds| FiniteF64::new(seconds).ok())
                        })
                        .map_or(Some(None), |bound| bound.map(Some))
                };
                seconds(bounds.lower())
                    .zip(seconds(bounds.upper()))
                    .and_then(|(min, max)| Bounds::new(min, max).ok())
                    .map(Domain::Datetime)
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
}

fn codec_error(name: &PortName, error: &ValueCodecError) -> ModelSessionError {
    ModelSessionError::Codec {
        name: name.clone(),
        message: error.to_string(),
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
