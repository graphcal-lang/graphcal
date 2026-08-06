//! Pure type rules for dimension-aware complex built-ins.

use thiserror::Error;

use crate::builtin::ComplexFn;
use crate::dimension::{BaseDimId, Dimension, PreludeBaseDimension};

use super::super::InferredType;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(super) enum ComplexTypeError {
    #[error("expected {expected} argument(s), got {got}")]
    WrongArity { expected: usize, got: usize },
    #[error("argument {argument} must be a quantity")]
    ExpectedQuantity { argument: usize },
    #[error("argument {argument} must be a complex quantity")]
    ExpectedComplex { argument: usize },
    #[error("argument {argument} must be a real or complex quantity")]
    ExpectedQuantityOrComplex { argument: usize },
    #[error("arguments {left} and {right} must have the same dimension")]
    DimensionMismatch { left: usize, right: usize },
    #[error("argument {argument} must have dimension Angle")]
    ExpectedAngle { argument: usize },
    #[error("argument {argument} must be dimensionless")]
    ExpectedDimensionless { argument: usize },
}

pub(super) fn infer(
    function: ComplexFn,
    arguments: &[InferredType],
) -> Result<InferredType, ComplexTypeError> {
    if arguments.len() != function.arity() {
        return Err(ComplexTypeError::WrongArity {
            expected: function.arity(),
            got: arguments.len(),
        });
    }

    match function {
        ComplexFn::Rectangular => {
            let re = quantity_dimension(arguments, 0)?;
            let im = quantity_dimension(arguments, 1)?;
            if re != im {
                return Err(ComplexTypeError::DimensionMismatch { left: 0, right: 1 });
            }
            Ok(InferredType::Complex(re.clone()))
        }
        ComplexFn::Polar => {
            let magnitude = quantity_dimension(arguments, 0)?;
            let phase = quantity_dimension(arguments, 1)?;
            let angle = Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Angle));
            if *phase != angle {
                return Err(ComplexTypeError::ExpectedAngle { argument: 1 });
            }
            Ok(InferredType::Complex(magnitude.clone()))
        }
        ComplexFn::ToComplex => quantity_dimension(arguments, 0)
            .cloned()
            .map(InferredType::Complex),
        ComplexFn::Real | ComplexFn::Imaginary => complex_dimension(arguments, 0)
            .cloned()
            .map(InferredType::Quantity),
        ComplexFn::Absolute => match &arguments[0] {
            InferredType::Complex(dimension) | InferredType::Quantity(dimension) => {
                Ok(InferredType::Quantity(dimension.clone()))
            }
            _ => Err(ComplexTypeError::ExpectedQuantityOrComplex { argument: 0 }),
        },
        ComplexFn::Phase => complex_dimension(arguments, 0).map(|_| {
            InferredType::Quantity(Dimension::base(BaseDimId::Prelude(
                PreludeBaseDimension::Angle,
            )))
        }),
        ComplexFn::Conjugate => complex_dimension(arguments, 0)
            .cloned()
            .map(InferredType::Complex),
        ComplexFn::Exponential => match &arguments[0] {
            InferredType::Complex(dimension) => {
                if !dimension.is_dimensionless() {
                    return Err(ComplexTypeError::ExpectedDimensionless { argument: 0 });
                }
                Ok(InferredType::Complex(Dimension::dimensionless()))
            }
            InferredType::Quantity(dimension) => {
                if !dimension.is_dimensionless() {
                    return Err(ComplexTypeError::ExpectedDimensionless { argument: 0 });
                }
                Ok(InferredType::Quantity(Dimension::dimensionless()))
            }
            _ => Err(ComplexTypeError::ExpectedQuantityOrComplex { argument: 0 }),
        },
    }
}

fn quantity_dimension(
    arguments: &[InferredType],
    argument: usize,
) -> Result<&Dimension, ComplexTypeError> {
    arguments[argument]
        .quantity_dimension()
        .ok_or(ComplexTypeError::ExpectedQuantity { argument })
}

fn complex_dimension(
    arguments: &[InferredType],
    argument: usize,
) -> Result<&Dimension, ComplexTypeError> {
    arguments[argument]
        .complex_dimension()
        .ok_or(ComplexTypeError::ExpectedComplex { argument })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn length() -> Dimension {
        Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Length))
    }

    #[test]
    fn rectangular_requires_matching_dimensions() {
        assert_eq!(
            infer(
                ComplexFn::Rectangular,
                &[
                    InferredType::Quantity(length()),
                    InferredType::Quantity(length()),
                ],
            ),
            Ok(InferredType::Complex(length()))
        );
        assert!(matches!(
            infer(
                ComplexFn::Rectangular,
                &[
                    InferredType::Quantity(length()),
                    InferredType::Quantity(Dimension::dimensionless()),
                ],
            ),
            Err(ComplexTypeError::DimensionMismatch { .. })
        ));
    }

    #[test]
    fn exponential_requires_dimensionless_input() {
        assert!(matches!(
            infer(ComplexFn::Exponential, &[InferredType::Complex(length())]),
            Err(ComplexTypeError::ExpectedDimensionless { .. })
        ));
        assert_eq!(
            infer(
                ComplexFn::Exponential,
                &[InferredType::Complex(Dimension::dimensionless())],
            ),
            Ok(InferredType::Complex(Dimension::dimensionless()))
        );
    }
}
