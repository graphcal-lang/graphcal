//! Pure type rules for built-in linear algebra.
//!
//! HIR walking and diagnostic rendering stay in `hir`; this module receives
//! already-inferred argument types and returns either a result type or a typed
//! shape error. Axis matching uses [`InferredIndex`] identity rather than
//! display names.

use crate::builtin::LinearAlgebraFn;
use crate::dimension::Dimension;

use super::super::{InferredIndex, InferredType};

/// A linear-algebra call cannot be typed from the supplied argument shapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LinearAlgebraTypeError {
    /// The caller supplied the wrong number of arguments.
    WrongArity { expected: usize, found: usize },
    /// An argument is not an indexed quantity of the required rank.
    ExpectedIndexedQuantity { argument: usize, rank: usize },
    /// Two axis positions that participate in one contraction do not have the
    /// same typed identity.
    AxisMismatch {
        argument: usize,
        expected: InferredIndex,
        found: InferredIndex,
    },
    /// An operation requires a fixed axis cardinality.
    CardinalityMismatch {
        argument: usize,
        expected: usize,
        found: Option<usize>,
    },
    /// An operation needs the concrete cardinality of an axis to determine its
    /// result dimension.
    ConcreteCardinalityRequired { argument: usize },
    /// Dimension exponent arithmetic overflowed.
    DimensionOverflow,
}

#[derive(Debug)]
struct IndexedQuantity<'a> {
    dimension: &'a Dimension,
    axes: Vec<&'a InferredIndex>,
}

impl IndexedQuantity<'_> {
    fn axis(&self, position: usize) -> &InferredIndex {
        self.axes[position]
    }
}

fn indexed_quantity(
    argument: usize,
    ty: &InferredType,
    rank: usize,
) -> Result<IndexedQuantity<'_>, LinearAlgebraTypeError> {
    let mut current = ty;
    let mut axes = Vec::new();
    while let InferredType::Indexed { element, index } = current {
        axes.push(index);
        current = element;
    }
    let InferredType::Quantity(dimension) = current else {
        return Err(LinearAlgebraTypeError::ExpectedIndexedQuantity { argument, rank });
    };
    if axes.len() != rank {
        return Err(LinearAlgebraTypeError::ExpectedIndexedQuantity { argument, rank });
    }
    Ok(IndexedQuantity { dimension, axes })
}

fn require_same_axis(
    expected: &InferredIndex,
    argument: usize,
    found: &InferredIndex,
) -> Result<(), LinearAlgebraTypeError> {
    if expected == found {
        Ok(())
    } else {
        Err(LinearAlgebraTypeError::AxisMismatch {
            argument,
            expected: expected.clone(),
            found: found.clone(),
        })
    }
}

fn quantity_over(dimension: Dimension, axes: &[&InferredIndex]) -> InferredType {
    axes.iter()
        .rev()
        .fold(InferredType::Quantity(dimension), |element, index| {
            InferredType::Indexed {
                element: Box::new(element),
                index: (*index).clone(),
            }
        })
}

fn product_dimension(
    lhs: &Dimension,
    rhs: &Dimension,
) -> Result<Dimension, LinearAlgebraTypeError> {
    lhs.clone()
        .checked_mul(rhs)
        .map_err(|_| LinearAlgebraTypeError::DimensionOverflow)
}

fn quotient_dimension(
    numerator: &Dimension,
    denominator: &Dimension,
) -> Result<Dimension, LinearAlgebraTypeError> {
    numerator
        .clone()
        .checked_div(denominator)
        .map_err(|_| LinearAlgebraTypeError::DimensionOverflow)
}

fn reciprocal_dimension(dimension: &Dimension) -> Result<Dimension, LinearAlgebraTypeError> {
    quotient_dimension(&Dimension::dimensionless(), dimension)
}

/// Infer one built-in linear-algebra call from already-inferred arguments.
///
/// `cardinality` supplies concrete axis sizes from the caller's semantic
/// registry. It is consulted only by fixed-size operations such as `cross`.
pub(super) fn infer_linear_algebra_type(
    function: LinearAlgebraFn,
    arguments: &[InferredType],
    mut cardinality: impl FnMut(&InferredIndex) -> Option<usize>,
) -> Result<InferredType, LinearAlgebraTypeError> {
    if arguments.len() != function.arity() {
        return Err(LinearAlgebraTypeError::WrongArity {
            expected: function.arity(),
            found: arguments.len(),
        });
    }
    match function {
        LinearAlgebraFn::Dot => {
            let lhs = indexed_quantity(0, &arguments[0], 1)?;
            let rhs = indexed_quantity(1, &arguments[1], 1)?;
            require_same_axis(lhs.axis(0), 1, rhs.axis(0))?;
            Ok(InferredType::Quantity(product_dimension(
                lhs.dimension,
                rhs.dimension,
            )?))
        }
        LinearAlgebraFn::Matmul => {
            let lhs = indexed_quantity(0, &arguments[0], 2)?;
            let rhs = indexed_quantity(1, &arguments[1], 2)?;
            require_same_axis(lhs.axis(1), 1, rhs.axis(0))?;
            Ok(quantity_over(
                product_dimension(lhs.dimension, rhs.dimension)?,
                &[lhs.axis(0), rhs.axis(1)],
            ))
        }
        LinearAlgebraFn::Transpose => {
            let matrix = indexed_quantity(0, &arguments[0], 2)?;
            Ok(quantity_over(
                matrix.dimension.clone(),
                &[matrix.axis(1), matrix.axis(0)],
            ))
        }
        LinearAlgebraFn::Trace => {
            let matrix = indexed_quantity(0, &arguments[0], 2)?;
            require_same_axis(matrix.axis(0), 0, matrix.axis(1))?;
            Ok(InferredType::Quantity(matrix.dimension.clone()))
        }
        LinearAlgebraFn::Norm => {
            let vector = indexed_quantity(0, &arguments[0], 1)?;
            Ok(InferredType::Quantity(vector.dimension.clone()))
        }
        LinearAlgebraFn::Cross => {
            let lhs = indexed_quantity(0, &arguments[0], 1)?;
            let rhs = indexed_quantity(1, &arguments[1], 1)?;
            require_same_axis(lhs.axis(0), 1, rhs.axis(0))?;
            let found = cardinality(lhs.axis(0));
            if found != Some(3) {
                return Err(LinearAlgebraTypeError::CardinalityMismatch {
                    argument: 0,
                    expected: 3,
                    found,
                });
            }
            Ok(quantity_over(
                product_dimension(lhs.dimension, rhs.dimension)?,
                &[lhs.axis(0)],
            ))
        }
        LinearAlgebraFn::Outer => {
            let lhs = indexed_quantity(0, &arguments[0], 1)?;
            let rhs = indexed_quantity(1, &arguments[1], 1)?;
            Ok(quantity_over(
                product_dimension(lhs.dimension, rhs.dimension)?,
                &[lhs.axis(0), rhs.axis(0)],
            ))
        }
        LinearAlgebraFn::Solve => {
            let matrix = indexed_quantity(0, &arguments[0], 2)?;
            let rhs = indexed_quantity(1, &arguments[1], 1)?;
            require_same_axis(matrix.axis(0), 0, matrix.axis(1))?;
            require_same_axis(matrix.axis(0), 1, rhs.axis(0))?;
            Ok(quantity_over(
                quotient_dimension(rhs.dimension, matrix.dimension)?,
                &[matrix.axis(0)],
            ))
        }
        LinearAlgebraFn::Inverse => {
            let matrix = indexed_quantity(0, &arguments[0], 2)?;
            require_same_axis(matrix.axis(0), 0, matrix.axis(1))?;
            Ok(quantity_over(
                reciprocal_dimension(matrix.dimension)?,
                &[matrix.axis(0), matrix.axis(1)],
            ))
        }
        LinearAlgebraFn::Determinant => {
            let matrix = indexed_quantity(0, &arguments[0], 2)?;
            require_same_axis(matrix.axis(0), 0, matrix.axis(1))?;
            let cardinality = cardinality(matrix.axis(0))
                .ok_or(LinearAlgebraTypeError::ConcreteCardinalityRequired { argument: 0 })?;
            let exponent = i32::try_from(cardinality)
                .map_err(|_| LinearAlgebraTypeError::DimensionOverflow)?;
            let dimension = matrix
                .dimension
                .pow(exponent)
                .map_err(|_| LinearAlgebraTypeError::DimensionOverflow)?;
            Ok(InferredType::Quantity(dimension))
        }
    }
}
