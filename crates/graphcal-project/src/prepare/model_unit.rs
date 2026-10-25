//! The units a batch-evaluation session chooses for its quantity ports.
//!
//! A chosen unit is a Graphcal unit expression resolved in the entry file's
//! scope, as if written there after `->`. Only units with a fixed scale
//! (base, `const`, and prelude units) qualify, so one factor converts every
//! row.

use std::collections::HashMap;

use graphcal_compiler::resolve::prelude::prelude_type_scope;
use graphcal_compiler::semantic::checked_type::CheckedType;
use graphcal_compiler::semantic::unit_scale::{
    UnitResolveError, UnitScale, resolve_unit_expr_with,
};
use graphcal_compiler::syntax::dimension::UnitRef;
use graphcal_compiler::syntax::parser::Parser;
use graphcal_model_protocol::{PortName, ResolvedUnit, SiScale, UnitSpelling};
use thiserror::Error;

use super::PreparedProject;
use super::model_axis::protocol_dimension;

/// Why a unit cannot carry a quantity port's values.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModelUnitError {
    #[error("the model has no quantity port `{port}`")]
    NotQuantityPort { port: PortName },
    #[error("not a unit expression: {message}")]
    Syntax { message: String },
    #[error("unit `{unit}` is not in scope in the model's entry file")]
    UnknownUnit { unit: UnitRef },
    #[error(
        "unit `{unit}` is a runtime unit; only base, `const`, and prelude units have a fixed scale"
    )]
    RuntimeUnit { unit: UnitRef },
    #[error("the unit's scale is not a positive finite number")]
    InvalidScale,
    #[error("the unit's dimension exponents overflow")]
    Overflow,
    #[error("the unit measures `{actual}`, but the port is `{expected}`")]
    DimensionMismatch { expected: String, actual: String },
    #[error("the unit's dimension has no protocol representation: {reason}")]
    UnrepresentableDimension { reason: String },
}

impl PreparedProject {
    /// The scale of `unit` for the quantity-like port `port`: a quantity, a
    /// complex quantity, or an indexed value of one of those.
    ///
    /// # Errors
    ///
    /// Returns [`ModelUnitError`] when `port` is not a quantity port, or
    /// `unit` does not parse, is not in scope, has no fixed scale, or
    /// measures another dimension.
    pub fn model_unit_scale(
        &self,
        port: &PortName,
        unit: &UnitSpelling,
    ) -> Result<SiScale, ModelUnitError> {
        let Some(expected) = self.port_type(port).and_then(quantity_dimension) else {
            return Err(ModelUnitError::NotQuantityPort { port: port.clone() });
        };
        let (dimension, scale) = self.fixed_unit(unit)?;
        if dimension != *expected {
            let dimensions = &self.tir().registry().dimensions;
            return Err(ModelUnitError::DimensionMismatch {
                expected: dimensions.format_dimension(expected),
                actual: dimensions.format_dimension(&dimension),
            });
        }
        Ok(scale)
    }

    /// The dimension and scale of `unit`, a unit expression with a fixed
    /// scale resolved in the entry file's scope, as if written there after
    /// `->`.
    ///
    /// A client uses it to convert quantities it read in their canonical SI
    /// unit, such as fields of a structured output, into a unit of its
    /// choice.
    ///
    /// # Errors
    ///
    /// Returns [`ModelUnitError`] when `unit` does not parse, is not in
    /// scope, has no fixed scale, or measures a dimension the protocol cannot
    /// represent.
    pub fn resolve_model_unit(&self, unit: &UnitSpelling) -> Result<ResolvedUnit, ModelUnitError> {
        let (dimension, scale) = self.fixed_unit(unit)?;
        let dimension = match protocol_dimension(&dimension) {
            Ok(Ok(dimension)) => dimension,
            Ok(Err(reason)) => return Err(ModelUnitError::UnrepresentableDimension { reason }),
            Err(error) => {
                return Err(ModelUnitError::UnrepresentableDimension {
                    reason: error.to_string(),
                });
            }
        };
        Ok(ResolvedUnit::new(unit.clone(), dimension, scale))
    }

    /// The dimension and scale of the fixed-scale unit expression `unit`.
    fn fixed_unit(
        &self,
        unit: &UnitSpelling,
    ) -> Result<(graphcal_compiler::dimension::Dimension, SiScale), ModelUnitError> {
        let expr = Parser::new(unit.as_str())
            .parse_standalone_unit_expr()
            .map_err(|error| ModelUnitError::Syntax {
                message: error.to_string(),
            })?;
        let tir = self.tir();
        let mut infos = HashMap::with_capacity(expr.terms.len());
        for term in &expr.terms {
            let reference = &term.name.value;
            let unknown = || ModelUnitError::UnknownUnit {
                unit: reference.clone(),
            };
            let resolved = self
                .module_resolver
                .resolve_unit_path(tir.root_dag_id(), &reference.to_name_path())
                .map(graphcal_compiler::resolve::symbols::SymbolRef::into_resolved)
                .ok()
                .or_else(|| {
                    reference
                        .as_bare()
                        .and_then(|leaf| prelude_type_scope().unit(leaf))
                })
                .ok_or_else(unknown)?;
            let info = tir.unit_info(&resolved).ok_or_else(unknown)?;
            match info.scale {
                UnitScale::Const(_) => {}
                UnitScale::Runtime(_) | UnitScale::Dynamic { .. } => {
                    return Err(ModelUnitError::RuntimeUnit {
                        unit: reference.clone(),
                    });
                }
            }
            infos.insert(reference.clone(), info);
        }
        let (dimension, scale) =
            resolve_unit_expr_with(&expr, |reference| infos.get(reference).copied()).map_err(
                |error| match error {
                    UnitResolveError::UnknownUnit(unit) => ModelUnitError::UnknownUnit { unit },
                    UnitResolveError::DynamicScale(unit) => ModelUnitError::RuntimeUnit { unit },
                    UnitResolveError::InvalidScale(_) => ModelUnitError::InvalidScale,
                    UnitResolveError::Overflow(_) => ModelUnitError::Overflow,
                },
            )?;
        let scale = SiScale::new(scale.get()).map_err(|_| ModelUnitError::InvalidScale)?;
        Ok((dimension, scale))
    }

    /// The declared type of the parameter or public output named `port`.
    fn port_type(&self, port: &PortName) -> Option<&CheckedType> {
        self.parameter_ports
            .iter()
            .find(|parameter| parameter.name.as_str() == port.as_str())
            .map(|parameter| &parameter.declared_type)
            .or_else(|| {
                self.output_ports
                    .iter()
                    .find(|output| output.is_public && output.name.as_str() == port.as_str())
                    .map(|output| &output.declared_type)
            })
    }
}

/// The dimension of a quantity-like type's quantities.
fn quantity_dimension(
    declared_type: &CheckedType,
) -> Option<&graphcal_compiler::dimension::Dimension> {
    match declared_type {
        CheckedType::Quantity(dimension) | CheckedType::Complex(dimension) => Some(dimension),
        CheckedType::Indexed { element, .. } => quantity_dimension(element),
        CheckedType::Int
        | CheckedType::Bool
        | CheckedType::Key(_)
        | CheckedType::Datetime(_)
        | CheckedType::Struct(..) => None,
    }
}
