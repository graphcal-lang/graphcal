//! How the value schema of a model port travels on the batch-protocol wire.
//!
//! Quantities travel in their canonical SI unit at scale one, datetimes as
//! seconds since J2000 in their time scale, indexed values as one entry per
//! axis entry, and algebraic values as their constructors' fields. A type
//! with no finite wire schema (a recursive algebraic type) or with an
//! unsupported part is listed as unsupported, with the reason.

use graphcal_compiler::dimension::{BaseDimId, Dimension as GraphcalDimension};
use graphcal_compiler::semantic::checked_type::CheckedType;
use graphcal_compiler::semantic::time_scale::TimeScale as GraphcalTimeScale;
use graphcal_model_protocol::{
    AlgebraicType, Axis, AxisKind, BaseDimensionName, ConstructorName, ConstructorType,
    DatetimeType, Dimension, Exponent, FieldName, FieldType, FiniteF64, IndexName, IndexedType,
    KeyType, PortWire, QuantityType, TimeScale, TypeName, UnitSpelling, VariantName, WireType,
};

use super::model_session::ModelInterfaceError;
use super::{ModelIndexKind, ModelIndexSchema, ModelValueSchema, PreparedProject};

/// A wire type, or why the protocol cannot carry the value.
type Wire = Result<WireType, String>;

impl PreparedProject {
    /// How a port of `schema` travels on the wire.
    ///
    /// # Errors
    ///
    /// Returns [`ModelInterfaceError`] only for a violated internal
    /// invariant.
    pub(super) fn port_wire(
        &self,
        schema: &ModelValueSchema,
    ) -> Result<PortWire, ModelInterfaceError> {
        if self.schema_graph.contains_recursive_type(schema) {
            return Ok(PortWire::Unsupported {
                reason: format!(
                    "type `{}` is recursive, so it has no finite wire schema",
                    self.type_label(&schema.declared_type())
                ),
            });
        }
        Ok(match self.wire(schema)? {
            Ok(wire) => PortWire::Supported(wire),
            Err(reason) => PortWire::Unsupported { reason },
        })
    }

    /// The human-readable spelling of `declared_type`.
    pub(super) fn type_label(&self, declared_type: &CheckedType) -> String {
        declared_type.format(&self.tir().registry().dimensions)
    }

    fn wire(&self, schema: &ModelValueSchema) -> Result<Wire, ModelInterfaceError> {
        Ok(match schema {
            ModelValueSchema::Quantity(quantity) => self
                .quantity_type(quantity.dimension())?
                .map(WireType::Quantity),
            ModelValueSchema::Complex(quantity) => self
                .quantity_type(quantity.dimension())?
                .map(WireType::Complex),
            ModelValueSchema::Int => Ok(WireType::Int),
            ModelValueSchema::Bool => Ok(WireType::Bool),
            ModelValueSchema::Datetime(scale) => Ok(WireType::Datetime(DatetimeType::new(
                protocol_time_scale(*scale),
            ))),
            ModelValueSchema::Key(index) => match index.kind() {
                ModelIndexKind::Named { variants } => {
                    let variants = variants
                        .as_slice()
                        .iter()
                        .map(|variant| VariantName::new(variant.as_str()))
                        .collect::<Result<Vec<_>, _>>()?;
                    KeyType::new(IndexName::new(index.identity().to_string())?, variants)
                        .map(WireType::Key)
                        .map_err(|error| error.to_string())
                }
                ModelIndexKind::Finite { .. } | ModelIndexKind::Coordinate { .. } => Err(format!(
                    "key index `{}` is not a named index",
                    index.identity()
                )),
            },
            ModelValueSchema::Indexed { element, axis } => match axis_wire(axis)? {
                Err(reason) => Err(reason),
                Ok(axis) => self
                    .wire(element)?
                    .map(|element| WireType::Indexed(IndexedType::new(axis, element))),
            },
            ModelValueSchema::Algebraic(id) => {
                let Some(definition) = self.schema_graph.definition(id) else {
                    return Err(ModelInterfaceError::MissingDefinition {
                        type_name: self.type_label(&schema.declared_type()),
                    });
                };
                let mut constructors = Vec::with_capacity(definition.constructors().len());
                for constructor in definition.constructors() {
                    let mut fields = Vec::with_capacity(constructor.fields().len());
                    for field in constructor.fields() {
                        match self.wire(field.value())? {
                            Ok(wire) => fields
                                .push(FieldType::new(FieldName::new(field.name().as_str())?, wire)),
                            Err(reason) => {
                                return Ok(Err(format!(
                                    "field `{}` of `{}`: {reason}",
                                    field.name(),
                                    constructor.name()
                                )));
                            }
                        }
                    }
                    constructors.push(ConstructorType::new(
                        ConstructorName::new(constructor.name().as_str())?,
                        fields,
                    ));
                }
                AlgebraicType::new(
                    TypeName::new(self.type_label(&schema.declared_type()))?,
                    constructors,
                )
                .map(WireType::Algebraic)
                .map_err(|error| error.to_string())
            }
        })
    }

    /// A quantity of `dimension` in its canonical SI unit at scale one.
    fn quantity_type(
        &self,
        dimension: &GraphcalDimension,
    ) -> Result<Result<QuantityType, String>, ModelInterfaceError> {
        let dimensions = &self.tir().registry().dimensions;
        let exponents = match protocol_dimension(dimension)? {
            Ok(exponents) => exponents,
            Err(reason) => return Ok(Err(reason)),
        };
        let unit = if dimension.is_dimensionless() {
            None
        } else {
            let symbols = dimensions.base_unit_symbols();
            let Some(spelling) =
                graphcal_eval::eval::types::default_unit_label(dimension, &symbols)
            else {
                let unitless = dimension
                    .iter()
                    .map(|(base, _)| base)
                    .filter(|base| !symbols.contains_key(*base))
                    .map(|base| {
                        format!(
                            "`{}`",
                            dimensions.format_dimension(&GraphcalDimension::base(base.clone()))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                return Ok(Err(format!(
                    "dimension `{}` has no canonical unit; declare a `base unit` for base \
                     dimension {unitless}",
                    dimensions.format_dimension(dimension)
                )));
            };
            Some(UnitSpelling::new(spelling)?)
        };
        Ok(QuantityType::new(exponents, unit).map_err(|error| error.to_string()))
    }
}

/// The protocol name of a Graphcal time scale.
pub(super) const fn protocol_time_scale(scale: GraphcalTimeScale) -> TimeScale {
    match scale {
        GraphcalTimeScale::UTC => TimeScale::Utc,
        GraphcalTimeScale::TAI => TimeScale::Tai,
        GraphcalTimeScale::TT => TimeScale::Tt,
        GraphcalTimeScale::TDB => TimeScale::Tdb,
        GraphcalTimeScale::ET => TimeScale::Et,
        GraphcalTimeScale::GPST => TimeScale::Gpst,
        GraphcalTimeScale::GST => TimeScale::Gst,
        GraphcalTimeScale::BDT => TimeScale::Bdt,
        GraphcalTimeScale::QZSST => TimeScale::Qzsst,
    }
}

/// The protocol dimension of `dimension`, or why it has none.
fn protocol_dimension(
    dimension: &GraphcalDimension,
) -> Result<Result<Dimension, String>, ModelInterfaceError> {
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
            return Ok(Err(format!(
                "dimension exponent {exponent} is not representable"
            )));
        };
        exponents.push((BaseDimensionName::new(name)?, exponent));
    }
    Ok(Ok(exponents.into_iter().collect()))
}

/// The protocol axis of `index`, or why it has none.
fn axis_wire(index: &ModelIndexSchema) -> Result<Result<Axis, String>, ModelInterfaceError> {
    let kind = match index.kind() {
        ModelIndexKind::Named { variants } => AxisKind::Named {
            variants: variants
                .as_slice()
                .iter()
                .map(|variant| VariantName::new(variant.as_str()))
                .collect::<Result<_, _>>()?,
        },
        ModelIndexKind::Finite { index } => AxisKind::Finite {
            size: index.cardinality().get(),
        },
        ModelIndexKind::Coordinate {
            coordinates_si,
            dimension,
            ..
        } => {
            let dimension = match protocol_dimension(dimension)? {
                Ok(dimension) => dimension,
                Err(reason) => return Ok(Err(reason)),
            };
            let Ok(coordinates) = coordinates_si
                .iter()
                .map(|coordinate| FiniteF64::new(*coordinate))
                .collect::<Result<Vec<_>, _>>()
            else {
                return Ok(Err(format!(
                    "index `{}` has a non-finite coordinate",
                    index.identity()
                )));
            };
            AxisKind::Coordinate {
                dimension,
                coordinates,
            }
        }
    };
    Ok(
        Axis::new(IndexName::new(index.identity().to_string())?, kind)
            .map_err(|error| error.to_string()),
    )
}
