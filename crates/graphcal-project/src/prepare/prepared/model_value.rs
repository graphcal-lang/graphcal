//! Batch-protocol values of one port, converted to and from Graphcal runtime
//! values.
//!
//! A [`ValueCodec`] is built once per session port from the port's value
//! schema and its chosen unit, so a converted value always has the port's
//! type: the codec resolves each axis and each constructor application up
//! front, and per row only walks the value.

use std::sync::Arc;

use graphcal_compiler::cancellation::CancellationToken;
use graphcal_compiler::complex_value::ComplexValue as GraphcalComplex;
use graphcal_compiler::outcome::Outcome;
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
    WireType,
};
use thiserror::Error;

use super::model_port::OutputProjectionError;
use super::{ModelValueSchema, PreparedProject};

/// One constructor of an algebraic port with its field codecs.
#[derive(Debug, Clone)]
pub(super) struct ConstructorCodec {
    applied: Arc<AppliedConstructor>,
    /// Its position in the port's protocol type.
    position: ConstructorPosition,
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
    #[error("type `{identity}` has no model definition")]
    MissingDefinition { identity: StructTypeRef },
    #[error("type `{identity}` has another constructor list than its model definition")]
    ConstructorMismatch { identity: StructTypeRef },
    #[error("the session wire type of a model port does not have the port's shape")]
    WireShape,
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
    /// The codec of a port of `schema` carried as `wire`, the port's session
    /// wire type, whose quantities travel in the unit of `scale` (SI when
    /// `None`).
    ///
    /// Keys and constructors are taken from `wire`, so converted values
    /// carry the positions of the session's own key and algebraic types.
    ///
    /// # Errors
    ///
    /// Returns [`ValueCodecError`] for a schema the protocol does not carry
    /// (which a configured session never selects), a `wire` of another
    /// shape, or an algebraic type the compiler cannot apply.
    pub(super) fn value_codec(
        &self,
        schema: &ModelValueSchema,
        wire: &WireType,
        scale: Option<SiScale>,
    ) -> Result<ValueCodec, ValueCodecError> {
        let axis = |index: &IndexTypeRef| {
            IndexAxis::resolve(self.tir(), index).map_err(|_| ValueCodecError::MissingAxis {
                index: index.clone(),
            })
        };
        Ok(match (schema, wire) {
            (ModelValueSchema::Quantity(_), WireType::Quantity(_)) => ValueCodec::Quantity(scale),
            (ModelValueSchema::Complex(_), WireType::Complex(_)) => ValueCodec::Complex(scale),
            (ModelValueSchema::Int, WireType::Int) => ValueCodec::Int,
            (ModelValueSchema::Bool, WireType::Bool) => ValueCodec::Bool,
            (ModelValueSchema::Datetime(time_scale), WireType::Datetime(_)) => {
                ValueCodec::Datetime(*time_scale)
            }
            (ModelValueSchema::Key(index), WireType::Key(key)) => ValueCodec::Key {
                axis: axis(index.identity())?,
                key: key.clone(),
            },
            (
                ModelValueSchema::Indexed {
                    element,
                    axis: index,
                },
                WireType::Indexed(indexed),
            ) => ValueCodec::Indexed {
                axis: axis(index.identity())?,
                element: Box::new(self.value_codec(element, indexed.element(), scale)?),
            },
            (ModelValueSchema::Algebraic(id), WireType::Algebraic(wire)) => {
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
                if applied.len() != definition.constructors().len()
                    || wire.constructors().len() != applied.len()
                {
                    return Err(ValueCodecError::ConstructorMismatch {
                        identity: id.identity().clone(),
                    });
                }
                let constructors = applied
                    .into_iter()
                    .zip(definition.constructors())
                    .zip(wire.entries())
                    .map(|((applied, constructor), (position, wire_constructor))| {
                        if constructor.fields().len() != wire_constructor.fields().len() {
                            return Err(ValueCodecError::ConstructorMismatch {
                                identity: id.identity().clone(),
                            });
                        }
                        let fields = constructor
                            .fields()
                            .iter()
                            .zip(wire_constructor.fields())
                            .map(|(field, wire_field)| {
                                // Units apply to quantity ports only, never to
                                // the fields of an algebraic value.
                                Ok((
                                    field.name().clone(),
                                    self.value_codec(field.value(), wire_field.wire(), None)?,
                                ))
                            })
                            .collect::<Result<_, ValueCodecError>>()?;
                        Ok(ConstructorCodec {
                            applied,
                            position,
                            fields,
                        })
                    })
                    .collect::<Result<_, ValueCodecError>>()?;
                ValueCodec::Algebraic(constructors)
            }
            _ => return Err(ValueCodecError::WireShape),
        })
    }
}

fn out_of_range(value: f64) -> ValueBindError {
    ValueBindError::OutOfRange(format!(
        "{value} in the chosen unit overflows when converted to SI"
    ))
}

/// `value`, given in the unit of `scale` (SI when `None`), in SI.
///
/// # Errors
///
/// Returns [`ValueBindError::OutOfRange`] when the converted value overflows.
pub(super) fn to_si(scale: Option<SiScale>, value: FiniteF64) -> Result<f64, ValueBindError> {
    scale
        .map_or(Ok(value), |scale| scale.to_si(value))
        .map(FiniteF64::get)
        .map_err(|_| out_of_range(value.get()))
}

fn from_si(scale: Option<SiScale>, si: f64) -> Result<FiniteF64, OutputProjectionError> {
    let si = FiniteF64::new(si).map_err(|_| OutputProjectionError::Unrepresentable)?;
    scale
        .map_or(Ok(si), |scale| scale.from_si(si))
        .map_err(|_| OutputProjectionError::Overflow)
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
                    .iter()
                    .find(|codec| codec.position == constructor)
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
    ///
    /// Projection observes `cancellation` once per value it projects, so a
    /// large indexed output cannot outlive a request's deadline.
    pub(super) fn protocol_value(
        &self,
        runtime: &RuntimeValue,
        cancellation: &CancellationToken,
    ) -> Result<Value, Outcome<OutputProjectionError>> {
        cancellation.checkpoint()?;
        Ok(match self {
            Self::Quantity(scale) => Value::Quantity(from_si(
                *scale,
                project_quantity(runtime)
                    .map_err(OutputProjectionError::from)?
                    .get(),
            )?),
            Self::Complex(scale) => {
                let value = project_complex(runtime).map_err(OutputProjectionError::from)?;
                Value::Complex(ComplexValue {
                    re: from_si(*scale, value.re())?,
                    im: from_si(*scale, value.im())?,
                })
            }
            Self::Int => Value::Int(project_int(runtime).map_err(OutputProjectionError::from)?),
            Self::Bool => Value::Bool(project_bool(runtime).map_err(OutputProjectionError::from)?),
            Self::Key { key, .. } => Value::Key(
                key.axis()
                    .key(project_key_position(runtime).map_err(OutputProjectionError::from)?)
                    .ok_or(OutputProjectionError::Unrepresentable)?
                    .to_key_value(),
            ),
            Self::Datetime(time_scale) => {
                let seconds = seconds_since_j2000(
                    project_datetime(runtime).map_err(OutputProjectionError::from)?,
                    *time_scale,
                )
                .map_err(|_| OutputProjectionError::Unrepresentable)?;
                Value::Datetime(
                    FiniteF64::new(seconds).map_err(|_| OutputProjectionError::Unrepresentable)?,
                )
            }
            Self::Indexed { element, .. } => Value::Indexed(
                project_indexed_entries(runtime)
                    .map_err(OutputProjectionError::from)?
                    .iter()
                    .map(|entry| element.protocol_value(entry, cancellation))
                    .collect::<Result<_, _>>()?,
            ),
            Self::Algebraic(constructors) => {
                let (position, value) = project_constructor(
                    runtime,
                    constructors.iter().map(|codec| codec.applied.constructor()),
                )
                .map_err(OutputProjectionError::from)?;
                let fields = constructors[position]
                    .fields
                    .iter()
                    .zip(value.fields())
                    .map(|((_, codec), (_, field))| codec.protocol_value(field, cancellation))
                    .collect::<Result<_, _>>()?;
                Value::Algebraic {
                    constructor: constructors[position].position,
                    fields,
                }
            }
        })
    }
}
