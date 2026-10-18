//! The protocol axes and dimensions of model index and quantity schemas.
//!
//! An axis or dimension the protocol cannot carry yields the reason instead.

use graphcal_compiler::dimension::{BaseDimId, Dimension as GraphcalDimension};
use graphcal_model_protocol::{
    Axis, AxisKind, BaseDimensionName, Dimension, Exponent, FiniteF64, IndexName, LabelError,
    VariantName,
};

use super::{ModelIndexKind, ModelIndexSchema};

/// The protocol name of a base dimension.
fn base_dimension_name(base: &BaseDimId) -> String {
    match base {
        BaseDimId::Prelude(prelude) => prelude.as_str().to_owned(),
        BaseDimId::UserDefined(resolved) => resolved.to_string(),
    }
}

/// The protocol dimension of `dimension`, or why it has none.
pub(super) fn protocol_dimension(
    dimension: &GraphcalDimension,
) -> Result<Result<Dimension, String>, LabelError> {
    let mut exponents = Vec::new();
    for (base, exponent) in dimension.iter() {
        let name = base_dimension_name(base);
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
pub(super) fn axis_wire(index: &ModelIndexSchema) -> Result<Result<Axis, String>, LabelError> {
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
