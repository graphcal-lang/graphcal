//! Pure type rules for dimension-aware complex built-ins.

use thiserror::Error;

use crate::builtin::ComplexFn;
use crate::dimension::{BaseDimId, Dimension, PreludeBaseDimension};

use crate::semantic::checked_type::{CheckedType, Symbolic};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(super) enum ComplexTypeError {
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

/// Infer one complex built-in call from already-inferred arguments.
///
/// The caller has already checked `arguments` against the function's static
/// entry (`check_builtin_arity`), so it holds exactly `function.arity()`
/// types; this rule does not re-check the count.
pub(super) fn infer(
    function: ComplexFn,
    arguments: &[CheckedType<Symbolic>],
) -> Result<CheckedType<Symbolic>, ComplexTypeError> {
    match function {
        ComplexFn::Rectangular => {
            let re = quantity_dimension(arguments, 0)?;
            let im = quantity_dimension(arguments, 1)?;
            if re != im {
                return Err(ComplexTypeError::DimensionMismatch { left: 0, right: 1 });
            }
            Ok(CheckedType::Complex(re.clone()))
        }
        ComplexFn::Polar => {
            let magnitude = quantity_dimension(arguments, 0)?;
            let phase = quantity_dimension(arguments, 1)?;
            let angle = Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Angle));
            if *phase != angle {
                return Err(ComplexTypeError::ExpectedAngle { argument: 1 });
            }
            Ok(CheckedType::Complex(magnitude.clone()))
        }
        ComplexFn::ToComplex => quantity_dimension(arguments, 0)
            .cloned()
            .map(CheckedType::Complex),
        ComplexFn::Real | ComplexFn::Imaginary => complex_dimension(arguments, 0)
            .cloned()
            .map(CheckedType::Quantity),
        ComplexFn::Absolute => match &arguments[0] {
            CheckedType::Complex(dimension) | CheckedType::Quantity(dimension) => {
                Ok(CheckedType::Quantity(dimension.clone()))
            }
            _ => Err(ComplexTypeError::ExpectedQuantityOrComplex { argument: 0 }),
        },
        ComplexFn::Phase => complex_dimension(arguments, 0).map(|_| {
            CheckedType::Quantity(Dimension::base(BaseDimId::Prelude(
                PreludeBaseDimension::Angle,
            )))
        }),
        ComplexFn::Conjugate => complex_dimension(arguments, 0)
            .cloned()
            .map(CheckedType::Complex),
        ComplexFn::Exponential => match &arguments[0] {
            CheckedType::Complex(dimension) => {
                if !dimension.is_dimensionless() {
                    return Err(ComplexTypeError::ExpectedDimensionless { argument: 0 });
                }
                Ok(CheckedType::Complex(Dimension::dimensionless()))
            }
            CheckedType::Quantity(dimension) => {
                if !dimension.is_dimensionless() {
                    return Err(ComplexTypeError::ExpectedDimensionless { argument: 0 });
                }
                Ok(CheckedType::Quantity(Dimension::dimensionless()))
            }
            _ => Err(ComplexTypeError::ExpectedQuantityOrComplex { argument: 0 }),
        },
    }
}

fn quantity_dimension(
    arguments: &[CheckedType<Symbolic>],
    argument: usize,
) -> Result<&Dimension, ComplexTypeError> {
    arguments[argument]
        .quantity_dimension()
        .ok_or(ComplexTypeError::ExpectedQuantity { argument })
}

fn complex_dimension(
    arguments: &[CheckedType<Symbolic>],
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
                    CheckedType::Quantity(length()),
                    CheckedType::Quantity(length()),
                ],
            ),
            Ok(CheckedType::Complex(length()))
        );
        assert!(matches!(
            infer(
                ComplexFn::Rectangular,
                &[
                    CheckedType::Quantity(length()),
                    CheckedType::Quantity(Dimension::dimensionless()),
                ],
            ),
            Err(ComplexTypeError::DimensionMismatch { .. })
        ));
    }

    #[test]
    fn exponential_requires_dimensionless_input() {
        assert!(matches!(
            infer(ComplexFn::Exponential, &[CheckedType::Complex(length())]),
            Err(ComplexTypeError::ExpectedDimensionless { .. })
        ));
        assert_eq!(
            infer(
                ComplexFn::Exponential,
                &[CheckedType::Complex(Dimension::dimensionless())],
            ),
            Ok(CheckedType::Complex(Dimension::dimensionless()))
        );
    }
}
