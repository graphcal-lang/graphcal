//! Batch-protocol values of one port, converted to and from Graphcal runtime
//! values.
//!
//! A [`ValueCodec`] is built once per session port from the port's value
//! schema and its chosen unit, so a converted value always has the port's
//! type: the codec resolves each axis and each constructor application up
//! front, and per row only walks the value.

use std::sync::Arc;

use graphcal_compiler::complex_value::ComplexValue as GraphcalComplex;
use graphcal_compiler::semantic::applied_constructor::AppliedConstructor;
use graphcal_compiler::semantic::checked_type::{IndexTypeRef, StructTypeRef};
use graphcal_compiler::semantic::time_scale::TimeScale;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::syntax::type_name::FieldName;
use graphcal_eval::eval::public_projection::{
    project_bool, project_complex, project_constructor, project_datetime, project_indexed_entries,
    project_int, project_key_position, project_quantity,
};
use graphcal_eval::eval_expr::j2000_seconds::{epoch_from_j2000_seconds, seconds_since_j2000};
use graphcal_eval::runtime_value::{IndexAxis, IndexedValue, KeyValue, RuntimeValue, StructValue};
use graphcal_model_protocol::{
    ComplexValue, ConstructorPosition, FiniteF64, KeyType, LabelError, SiScale, Value, ValueRef,
};
use thiserror::Error;

use super::model_axis::axis_wire;
use super::model_port::OutputProjectionError;
use super::{ModelValueSchema, PreparedProject};

/// One constructor of an algebraic port with its field codecs.
#[derive(Debug, Clone)]
pub(super) struct ConstructorCodec {
    applied: Arc<AppliedConstructor>,
    fields: Vec<(FieldName, ValueCodec)>,
}

/// The conversion of one port's values; see the module documentation.
#[derive(Debug, Clone)]
pub(super) enum ValueCodec {
    /// A real quantity in the unit of the scale, or in SI.
    Quantity(Option<SiScale>),
    /// A complex quantity in the unit of the scale, or in SI.
    Complex(Option<SiScale>),
    Int,
    Bool,
    /// A key of an axis, with the protocol's view of its entries.
    Key {
        axis: IndexAxis,
        key: KeyType,
    },
    /// Seconds since J2000 in the time scale.
    Datetime(TimeScale),
    Indexed {
        axis: IndexAxis,
        element: Box<Self>,
    },
    /// The constructors in declaration order.
    Algebraic(Vec<ConstructorCodec>),
}

/// Why a port's codec cannot be built; each is a violated invariant of a
/// configured session, whose ports the protocol carries.
#[derive(Debug, Error)]
pub(super) enum ValueCodecError {
    #[error("index `{index}` of a model port has no axis")]
    MissingAxis { index: IndexTypeRef },
    #[error("key index `{index}` of a model port has no wire axis: {reason}")]
    KeyAxis { index: IndexTypeRef, reason: String },
    #[error("type `{identity}` has no model definition")]
    MissingDefinition { identity: StructTypeRef },
    #[error("type `{identity}` has another constructor list than its model definition")]
    ConstructorMismatch { identity: StructTypeRef },
    #[error(transparent)]
    Label(#[from] LabelError),
    #[error(transparent)]
    Semantic(#[from] SemanticError),
}

/// Why a protocol value cannot be converted to a runtime value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ValueBindError {
    /// A number leaves the representable range when converted to SI: the
    /// row's failure, described by the message.
    OutOfRange(String),
    /// The value does not have the port's shape, which the validated
    /// request rules out.
    Shape,
}

impl PreparedProject {
    /// The codec of a port of `schema` whose quantities travel in the unit of
    /// `scale` (SI when `None`).
    ///
    /// # Errors
    ///
    /// Returns [`ValueCodecError`] for a schema the protocol does not carry
    /// (which a configured session never selects) or an algebraic type the
    /// compiler cannot apply.
    pub(super) fn value_codec(
        &self,
        schema: &ModelValueSchema,
        scale: Option<SiScale>,
    ) -> Result<ValueCodec, ValueCodecError> {
        let axis = |index: &IndexTypeRef| {
            IndexAxis::resolve(self.tir(), index).ok_or_else(|| ValueCodecError::MissingAxis {
                index: index.clone(),
            })
        };
        Ok(match schema {
            ModelValueSchema::Quantity(_) => ValueCodec::Quantity(scale),
            ModelValueSchema::Complex(_) => ValueCodec::Complex(scale),
            ModelValueSchema::Int => ValueCodec::Int,
            ModelValueSchema::Bool => ValueCodec::Bool,
            ModelValueSchema::Datetime(time_scale) => ValueCodec::Datetime(*time_scale),
            ModelValueSchema::Key(index) => ValueCodec::Key {
                axis: axis(index.identity())?,
                key: KeyType::new(axis_wire(index)?.map_err(|reason| {
                    ValueCodecError::KeyAxis {
                        index: index.identity().clone(),
                        reason,
                    }
                })?),
            },
            ModelValueSchema::Indexed {
                element,
                axis: index,
            } => ValueCodec::Indexed {
                axis: axis(index.identity())?,
                element: Box::new(self.value_codec(element, scale)?),
            },
            ModelValueSchema::Algebraic(id) => {
                let definition = self.schema_graph.definition(id).ok_or_else(|| {
                    ValueCodecError::MissingDefinition {
                        identity: id.identity().clone(),
                    }
                })?;
                let applied = graphcal_compiler::tir::dim_check::ConcreteModelType::try_new(
                    self.tir(),
                    id.identity(),
                    id.generic_args(),
                    self.source,
                )
                .map_err(|error| error.into_semantic_error(self.source))?
                .applied_constructors(self.source)?;
                if applied.len() != definition.constructors().len() {
                    return Err(ValueCodecError::ConstructorMismatch {
                        identity: id.identity().clone(),
                    });
                }
                let constructors = applied
                    .into_iter()
                    .zip(definition.constructors())
                    .map(|(applied, constructor)| {
                        let fields = constructor
                            .fields()
                            .iter()
                            .map(|field| {
                                // Units apply to quantity ports only, never to
                                // the fields of an algebraic value.
                                Ok((field.name().clone(), self.value_codec(field.value(), None)?))
                            })
                            .collect::<Result<_, ValueCodecError>>()?;
                        Ok(ConstructorCodec { applied, fields })
                    })
                    .collect::<Result<_, ValueCodecError>>()?;
                ValueCodec::Algebraic(constructors)
            }
        })
    }
}

fn out_of_range(value: f64) -> ValueBindError {
    ValueBindError::OutOfRange(format!(
        "{value} in the chosen unit overflows when converted to SI"
    ))
}

fn to_si(scale: Option<SiScale>, value: FiniteF64) -> Result<f64, ValueBindError> {
    scale
        .map_or(Ok(value), |scale| scale.to_si(value))
        .map(FiniteF64::get)
        .map_err(|_| out_of_range(value.get()))
}

fn from_si(scale: Option<SiScale>, si: f64) -> Result<FiniteF64, OutputProjectionError> {
    FiniteF64::new(si)
        .and_then(|si| scale.map_or(Ok(si), |scale| scale.from_si(si)))
        .map_err(|_| OutputProjectionError::Unrepresentable)
}

impl ValueCodec {
    /// The runtime value of `value`.
    ///
    /// # Errors
    ///
    /// Returns [`ValueBindError`] when a number overflows on conversion or
    /// `value` does not have this port's shape.
    pub(super) fn runtime_value(
        &self,
        value: ValueRef<'_>,
    ) -> Result<RuntimeValue, ValueBindError> {
        Ok(match (self, value) {
            (Self::Quantity(scale), ValueRef::Quantity(value)) => {
                RuntimeValue::quantity(to_si(*scale, value)?)
                    .map_err(|_| out_of_range(value.get()))?
            }
            (Self::Complex(scale), ValueRef::Complex(value)) => RuntimeValue::Complex(
                GraphcalComplex::try_new(to_si(*scale, value.re)?, to_si(*scale, value.im)?)
                    .map_err(|_| out_of_range(value.re.get()))?,
            ),
            (Self::Int, ValueRef::Int(value)) => RuntimeValue::Int(value),
            (Self::Bool, ValueRef::Bool(value)) => RuntimeValue::Bool(value),
            (Self::Key { axis, key }, ValueRef::Key(value)) => RuntimeValue::Key(
                key.axis()
                    .position(value)
                    .and_then(|position| KeyValue::at(axis.clone(), position))
                    .ok_or(ValueBindError::Shape)?,
            ),
            (Self::Datetime(time_scale), ValueRef::Datetime(seconds)) => RuntimeValue::Datetime(
                epoch_from_j2000_seconds(seconds.get(), *time_scale).map_err(|error| {
                    ValueBindError::OutOfRange(format!("{} s since J2000: {error}", seconds.get()))
                })?,
            ),
            (Self::Indexed { axis, element }, ValueRef::Indexed(entries)) => {
                let entries = entries
                    .iter()
                    .map(|entry| element.runtime_value(entry.as_value_ref()))
                    .collect::<Result<Vec<_>, _>>()?;
                RuntimeValue::Indexed(
                    IndexedValue::try_from_entries(axis.clone(), entries)
                        .map_err(|_| ValueBindError::Shape)?,
                )
            }
            (
                Self::Algebraic(constructors),
                ValueRef::Algebraic {
                    constructor,
                    fields,
                },
            ) => {
                let codec = constructors
                    .get(constructor.get())
                    .ok_or(ValueBindError::Shape)?;
                if codec.fields.len() != fields.len() {
                    return Err(ValueBindError::Shape);
                }
                let fields = codec
                    .fields
                    .iter()
                    .zip(fields)
                    .map(|((name, field), value)| {
                        Ok((name.clone(), field.runtime_value(value.as_value_ref())?))
                    })
                    .collect::<Result<Vec<_>, ValueBindError>>()?;
                RuntimeValue::Struct(
                    StructValue::try_from_model_constructor(&codec.applied, fields)
                        .map_err(|_| ValueBindError::Shape)?,
                )
            }
            _ => return Err(ValueBindError::Shape),
        })
    }

    /// The protocol value of `runtime`.
    ///
    /// # Errors
    ///
    /// Returns [`OutputProjectionError::Invariant`] when `runtime` does not
    /// have this port's type, and [`OutputProjectionError::Unrepresentable`]
    /// when a number has no finite value in the port's unit.
    pub(super) fn protocol_value(
        &self,
        runtime: &RuntimeValue,
    ) -> Result<Value, OutputProjectionError> {
        Ok(match self {
            Self::Quantity(scale) => {
                Value::Quantity(from_si(*scale, project_quantity(runtime)?.get())?)
            }
            Self::Complex(scale) => {
                let value = project_complex(runtime)?;
                Value::Complex(ComplexValue {
                    re: from_si(*scale, value.re())?,
                    im: from_si(*scale, value.im())?,
                })
            }
            Self::Int => Value::Int(project_int(runtime)?),
            Self::Bool => Value::Bool(project_bool(runtime)?),
            Self::Key { key, .. } => Value::Key(
                key.axis()
                    .key(project_key_position(runtime)?)
                    .ok_or(OutputProjectionError::Unrepresentable)?
                    .to_key_value(),
            ),
            Self::Datetime(time_scale) => {
                let seconds = seconds_since_j2000(project_datetime(runtime)?, *time_scale)
                    .map_err(|_| OutputProjectionError::Unrepresentable)?;
                Value::Datetime(
                    FiniteF64::new(seconds).map_err(|_| OutputProjectionError::Unrepresentable)?,
                )
            }
            Self::Indexed { element, .. } => Value::Indexed(
                project_indexed_entries(runtime)?
                    .iter()
                    .map(|entry| element.protocol_value(entry))
                    .collect::<Result<_, _>>()?,
            ),
            Self::Algebraic(constructors) => {
                let (position, value) = project_constructor(
                    runtime,
                    constructors.iter().map(|codec| codec.applied.constructor()),
                )?;
                let fields = constructors[position]
                    .fields
                    .iter()
                    .zip(value.fields())
                    .map(|((_, codec), (_, field))| codec.protocol_value(field))
                    .collect::<Result<_, _>>()?;
                Value::Algebraic {
                    // Both lists hold the constructors in declaration order.
                    constructor: ConstructorPosition::new(position),
                    fields,
                }
            }
        })
    }
}
