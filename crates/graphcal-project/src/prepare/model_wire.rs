//! How the value schema of a model port travels on the batch-protocol wire.
//!
//! Quantities travel in their canonical SI unit at scale one, datetimes as
//! seconds since J2000 in their time scale, indexed values as one entry per
//! axis entry, and algebraic values as their constructors' fields. A type
//! with an unsupported part is listed as unsupported, with the reason.

use graphcal_compiler::dimension::Dimension as GraphcalDimension;
use graphcal_compiler::semantic::checked_type::CheckedType;
use graphcal_compiler::semantic::time_scale::TimeScale as GraphcalTimeScale;
use graphcal_model_protocol::{
    AlgebraicType, ConstructorName, ConstructorType, DatetimeType, FieldName, FieldType,
    IndexedType, KeyType, PortWire, QuantityType, TimeScale, TypeName, UnitSpelling, WireType,
};

use super::model_axis::{axis_wire, protocol_dimension};
use super::model_session::ModelInterfaceError;
use super::{ModelValueSchema, PreparedProject};

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
            ModelValueSchema::Key(index) => {
                axis_wire(index)?.map(|axis| WireType::Key(KeyType::new(axis)))
            }
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
