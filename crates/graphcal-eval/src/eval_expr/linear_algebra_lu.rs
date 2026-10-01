//! Dense square-matrix algorithms used by the public linear-algebra builtins.
//!
//! Scaled partial-pivoting LU is shared by `solve`, `inverse`, and `det`.
//! Singular matrices are represented explicitly rather than by a sentinel or
//! `Option`; solve-like operations additionally reject a numerically unsafe
//! scaled-pivot profile and verify the residual of every returned solution.

use num_rational::BigRational;
use num_traits::ToPrimitive;
use thiserror::Error;

use graphcal_compiler::outcome::Outcome;

use graphcal_compiler::cancellation::Cancelled;

use super::work_budget::KernelCheckpoint;
use crate::invariant::{Failure, Invariant};

#[derive(Debug, Error)]
pub(super) enum LuError {
    #[error("matrix is singular and cannot be {operation}")]
    Singular { operation: &'static str },
    #[error(
        "matrix is numerically ill-conditioned for {operation} (smallest scaled pivot {scaled_pivot:e})"
    )]
    IllConditioned {
        operation: &'static str,
        scaled_pivot: f64,
    },
    #[error("{operation} produced a non-finite intermediate result")]
    NonFinite { operation: &'static str },
    #[error("det() underflowed to zero")]
    DeterminantUnderflow,
    #[error(
        "{operation} failed its numerical residual check (residual {residual:e}, tolerance {tolerance:e})"
    )]
    ResidualTooLarge {
        operation: &'static str,
        residual: f64,
        tolerance: f64,
    },
}

/// Why an LU operation did not produce a value.
pub(super) type LuFailure = Outcome<Failure<LuError>>;

impl From<LuError> for LuFailure {
    fn from(error: LuError) -> Self {
        Self::Failed(Failure::Error(error))
    }
}

/// A dense buffer that does not have the shape its caller established.
fn shape_invariant(description: impl std::fmt::Display) -> Invariant {
    Invariant::violated(format_args!(
        "linear-algebra runtime shape invariant failed: {description}"
    ))
}

/// A nonempty square matrix of `order` rows and columns, row-major.
///
/// Construction checks that the buffer holds exactly `order * order` values,
/// so every position `(row, column)` with `row, column < order` is in the
/// buffer.
#[derive(Debug, Clone)]
pub(super) struct SquareMatrix {
    order: usize,
    values: Vec<f64>,
}

impl SquareMatrix {
    /// The order-`order` matrix with row-major `values`.
    ///
    /// # Errors
    ///
    /// Returns an [`Invariant`] when `order` is zero or `values` does not
    /// hold exactly `order * order` values.
    pub(super) fn try_new(order: usize, values: Vec<f64>) -> Result<Self, Invariant> {
        if order == 0 || order.checked_mul(order) != Some(values.len()) {
            return Err(shape_invariant(format_args!(
                "{} values do not form a nonempty order-{order} square matrix",
                values.len()
            )));
        }
        Ok(Self { order, values })
    }

    /// The number of rows (and columns).
    pub(super) const fn order(&self) -> usize {
        self.order
    }

    /// The buffer position of `(row, column)`; both are below the order.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "row, column < order, and order * order fits usize by construction"
    )]
    const fn position(&self, row: usize, column: usize) -> usize {
        row * self.order + column
    }

    /// The value at `(row, column)`; both are below the order.
    fn at(&self, row: usize, column: usize) -> f64 {
        self.values[self.position(row, column)]
    }

    fn set(&mut self, row: usize, column: usize, value: f64) {
        let position = self.position(row, column);
        self.values[position] = value;
    }

    fn swap_rows(
        &mut self,
        lhs: usize,
        rhs: usize,
        control: &mut KernelCheckpoint<'_>,
    ) -> Result<(), Cancelled> {
        for column in 0..self.order {
            control.step()?;
            let (lhs, rhs) = (self.position(lhs, column), self.position(rhs, column));
            self.values.swap(lhs, rhs);
        }
        Ok(())
    }

    /// The row-major values.
    pub(super) fn into_values(self) -> Vec<f64> {
        self.values
    }
}

#[derive(Debug)]
enum Factorization {
    Singular,
    Nonsingular(LuDecomposition),
}

#[derive(Debug)]
struct LuDecomposition {
    factors: SquareMatrix,
    permutation: Vec<usize>,
    parity: f64,
    minimum_scaled_pivot: f64,
}

const fn finite(value: f64, operation: &'static str) -> Result<f64, LuError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(LuError::NonFinite { operation })
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "validated eager index cardinalities are at most 1,000,000 and exactly representable as f64"
)]
fn numerical_threshold(order: usize) -> f64 {
    64.0 * f64::EPSILON * order as f64
}

impl LuDecomposition {
    fn factor(
        matrix: &SquareMatrix,
        control: &mut KernelCheckpoint<'_>,
    ) -> Result<Factorization, LuFailure> {
        control.boundary()?;
        let order = matrix.order();
        if matrix.values.iter().any(|value| !value.is_finite()) {
            return Err(LuError::NonFinite {
                operation: "LU factorization",
            }
            .into());
        }

        let mut factors = matrix.clone();
        let mut scales = Vec::with_capacity(order);
        for row in 0..order {
            let mut scale = 0.0_f64;
            for column in 0..order {
                control.step()?;
                scale = scale.max(factors.at(row, column).abs());
            }
            scales.push(scale);
        }
        if scales.contains(&0.0) {
            return Ok(Factorization::Singular);
        }

        let mut permutation = (0..order).collect::<Vec<_>>();
        let mut parity = 1.0;
        let mut minimum_scaled_pivot = f64::INFINITY;

        for pivot_column in 0..order {
            // The pivot column's own row is the first candidate; a later row
            // replaces the incumbent unless its scaled ratio is no larger.
            control.step()?;
            let mut pivot_row = pivot_column;
            let mut best_ratio =
                factors.at(pivot_column, pivot_column).abs() / scales[pivot_column];
            for (row, scale) in scales
                .iter()
                .copied()
                .enumerate()
                .skip(pivot_column.saturating_add(1))
            {
                control.step()?;
                let ratio = factors.at(row, pivot_column).abs() / scale;
                if !matches!(
                    ratio.partial_cmp(&best_ratio),
                    Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
                ) {
                    pivot_row = row;
                    best_ratio = ratio;
                }
            }
            if factors.at(pivot_row, pivot_column) == 0.0 {
                return Ok(Factorization::Singular);
            }

            if pivot_row != pivot_column {
                factors.swap_rows(pivot_row, pivot_column, control)?;
                scales.swap(pivot_row, pivot_column);
                permutation.swap(pivot_row, pivot_column);
                parity = -parity;
            }

            let pivot = factors.at(pivot_column, pivot_column);
            minimum_scaled_pivot = minimum_scaled_pivot.min(pivot.abs() / scales[pivot_column]);
            for row in pivot_column.saturating_add(1)..order {
                control.step()?;
                let multiplier = finite(factors.at(row, pivot_column) / pivot, "LU factorization")?;
                factors.set(row, pivot_column, multiplier);
                for column in pivot_column.saturating_add(1)..order {
                    control.step()?;
                    let current = factors.at(row, column);
                    let upper = factors.at(pivot_column, column);
                    let updated =
                        finite((-multiplier).mul_add(upper, current), "LU factorization")?;
                    factors.set(row, column, updated);
                }
            }
        }

        Ok(Factorization::Nonsingular(Self {
            factors,
            permutation,
            parity,
            minimum_scaled_pivot,
        }))
    }

    const fn order(&self) -> usize {
        self.factors.order()
    }

    fn ensure_conditioned(&self, operation: &'static str) -> Result<(), LuError> {
        if self.minimum_scaled_pivot <= numerical_threshold(self.order()) {
            Err(LuError::IllConditioned {
                operation,
                scaled_pivot: self.minimum_scaled_pivot,
            })
        } else {
            Ok(())
        }
    }

    /// Solve for a right-hand side of exactly `order` values.
    fn solve_raw(
        &self,
        rhs: &[f64],
        operation: &'static str,
        control: &mut KernelCheckpoint<'_>,
    ) -> Result<Vec<f64>, LuFailure> {
        self.ensure_conditioned(operation)?;
        if rhs.iter().any(|value| !value.is_finite()) {
            return Err(LuError::NonFinite { operation }.into());
        }

        // The permutation permutes `0..order`.
        let mut solution = self
            .permutation
            .iter()
            .map(|position| rhs[*position])
            .collect::<Vec<_>>();

        for row in 0..self.order() {
            let mut value = solution[row];
            for (column, solved) in solution.iter().copied().enumerate().take(row) {
                control.step()?;
                value = finite(
                    (-self.factors.at(row, column)).mul_add(solved, value),
                    operation,
                )?;
            }
            solution[row] = value;
        }
        for row in (0..self.order()).rev() {
            let mut value = solution[row];
            for (column, solved) in solution
                .iter()
                .copied()
                .enumerate()
                .skip(row.saturating_add(1))
            {
                control.step()?;
                value = finite(
                    (-self.factors.at(row, column)).mul_add(solved, value),
                    operation,
                )?;
            }
            let diagonal = self.factors.at(row, row);
            solution[row] = finite(value / diagonal, operation)?;
        }
        Ok(solution)
    }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "multiplication of arbitrary-precision rationals cannot overflow"
    )]
    fn determinant(&self, control: &mut KernelCheckpoint<'_>) -> Result<f64, LuFailure> {
        // LU itself is binary64. Accumulate its finite pivots exactly so an
        // intermediate product cannot erase or overflow a representable result.
        let initial = BigRational::from_float(self.parity)
            .ok_or(LuError::NonFinite { operation: "det()" })?;
        let product = (0..self.order()).try_fold(initial, |product, diagonal| {
            control.step()?;
            let pivot = self.factors.at(diagonal, diagonal);
            let pivot =
                BigRational::from_float(pivot).ok_or(LuError::NonFinite { operation: "det()" })?;
            Ok::<_, LuFailure>(product * pivot)
        })?;
        let result = product
            .to_f64()
            .ok_or(LuError::NonFinite { operation: "det()" })?;
        let result = finite(result, "det()")?;
        if result == 0.0 {
            // Singular matrices take the distinct Factorization::Singular path.
            Err(LuError::DeterminantUnderflow.into())
        } else {
            Ok(result)
        }
    }
}

/// Check the residual of `solution` for `rhs`, both of exactly `order`
/// values.
fn checked_residual(
    matrix: &SquareMatrix,
    rhs: &[f64],
    solution: &[f64],
    operation: &'static str,
    control: &mut KernelCheckpoint<'_>,
) -> Result<(), LuFailure> {
    let order = matrix.order();
    let mut matrix_norm = 0.0_f64;
    let mut residual_norm = 0.0_f64;
    for (row, expected) in rhs.iter().enumerate() {
        let mut row_norm = 0.0_f64;
        let mut product = 0.0_f64;
        for (column, value) in solution.iter().enumerate() {
            control.step()?;
            let coefficient = matrix.at(row, column);
            row_norm = finite(row_norm + coefficient.abs(), operation)?;
            product = finite(coefficient.mul_add(*value, product), operation)?;
        }
        matrix_norm = matrix_norm.max(row_norm);
        residual_norm = residual_norm.max((product - expected).abs());
    }
    let solution_norm = solution
        .iter()
        .fold(0.0_f64, |norm, value| norm.max(value.abs()));
    let rhs_norm = rhs
        .iter()
        .fold(0.0_f64, |norm, value| norm.max(value.abs()));
    let scale = finite(matrix_norm.mul_add(solution_norm, rhs_norm), operation)?;
    let tolerance = finite(numerical_threshold(order) * scale, operation)?;
    if residual_norm <= tolerance {
        Ok(())
    } else {
        Err(LuError::ResidualTooLarge {
            operation,
            residual: residual_norm,
            tolerance,
        }
        .into())
    }
}

fn require_nonsingular(
    matrix: &SquareMatrix,
    operation: &'static str,
    control: &mut KernelCheckpoint<'_>,
) -> Result<LuDecomposition, LuFailure> {
    match LuDecomposition::factor(matrix, control)? {
        Factorization::Singular => Err(LuError::Singular { operation }.into()),
        Factorization::Nonsingular(decomposition) => Ok(decomposition),
    }
}

pub(super) fn solve_with_control(
    matrix: &SquareMatrix,
    rhs: &[f64],
    control: &mut KernelCheckpoint<'_>,
) -> Result<Vec<f64>, LuFailure> {
    let operation = "solved";
    if rhs.len() != matrix.order() {
        return Err(shape_invariant(format_args!(
            "{operation} received a right-hand side of length {} for order {}",
            rhs.len(),
            matrix.order()
        ))
        .into());
    }
    let decomposition = require_nonsingular(matrix, operation, control)?;
    let solution = decomposition.solve_raw(rhs, operation, control)?;
    checked_residual(matrix, rhs, &solution, operation, control)?;
    Ok(solution)
}

pub(super) fn inverse_with_control(
    matrix: &SquareMatrix,
    control: &mut KernelCheckpoint<'_>,
) -> Result<SquareMatrix, LuFailure> {
    let operation = "inverted";
    let order = matrix.order();
    let decomposition = require_nonsingular(matrix, operation, control)?;
    decomposition.ensure_conditioned(operation)?;
    let mut inverse = SquareMatrix {
        order,
        values: vec![0.0; matrix.values.len()],
    };
    for column in 0..order {
        control.boundary()?;
        let rhs = (0..order)
            .map(|row| if row == column { 1.0 } else { 0.0 })
            .collect::<Vec<_>>();
        let solution = decomposition.solve_raw(&rhs, operation, control)?;
        checked_residual(matrix, &rhs, &solution, operation, control)?;
        for (row, value) in solution.into_iter().enumerate() {
            control.step()?;
            inverse.set(row, column, value);
        }
    }
    Ok(inverse)
}

pub(super) fn determinant_with_control(
    matrix: &SquareMatrix,
    control: &mut KernelCheckpoint<'_>,
) -> Result<f64, LuFailure> {
    match LuDecomposition::factor(matrix, control)? {
        Factorization::Singular => Ok(0.0),
        Factorization::Nonsingular(decomposition) => decomposition.determinant(control),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(matrix: &[f64], order: usize) -> SquareMatrix {
        SquareMatrix::try_new(order, matrix.to_vec()).unwrap()
    }

    fn solve(matrix: &[f64], order: usize, rhs: &[f64]) -> Result<Vec<f64>, LuFailure> {
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        solve_with_control(
            &square(matrix, order),
            rhs,
            &mut KernelCheckpoint::new(&cancellation),
        )
    }

    fn inverse(matrix: &[f64], order: usize) -> Result<Vec<f64>, LuFailure> {
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        inverse_with_control(
            &square(matrix, order),
            &mut KernelCheckpoint::new(&cancellation),
        )
        .map(SquareMatrix::into_values)
    }

    fn determinant(matrix: &[f64], order: usize) -> Result<f64, LuFailure> {
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        determinant_with_control(
            &square(matrix, order),
            &mut KernelCheckpoint::new(&cancellation),
        )
    }

    const MATRIX: &[f64] = &[3.0, 1.0, 1.0, 2.0];

    #[test]
    fn solve_inverse_and_determinant_share_pivoted_lu() {
        let solution = solve(MATRIX, 2, &[9.0, 8.0]).unwrap();
        assert_eq!(solution, vec![2.0, 3.0]);

        let inverse = inverse(MATRIX, 2).unwrap();
        let expected = [0.4, -0.2, -0.2, 0.6];
        assert!(
            inverse
                .iter()
                .zip(expected)
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-15)
        );
        assert!((determinant(MATRIX, 2).unwrap() - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn scaled_pivoting_swaps_rows_and_tolerates_unit_scaling() {
        let pivoted = [0.0, 2.0, 1.0, 3.0];
        let solution = solve(&pivoted, 2, &[4.0, 7.0]).unwrap();
        assert!(
            solution
                .iter()
                .zip([1.0, 2.0])
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-15)
        );

        let scaled_diagonal = [1.0e-200, 0.0, 0.0, 1.0e200];
        let solution = solve(&scaled_diagonal, 2, &[1.0e-200, 1.0e200]).unwrap();
        assert!(
            solution
                .iter()
                .zip([1.0, 1.0])
                .all(|(actual, expected)| (actual - expected).abs() < f64::EPSILON)
        );
    }

    #[test]
    fn ill_conditioned_system_is_rejected() {
        let ill_conditioned = [1.0, 1.0, 1.0, 1.0 + 1.0e-14];
        assert!(matches!(
            solve(&ill_conditioned, 2, &[2.0, 2.0 + 1.0e-14]).unwrap_err(),
            Outcome::Failed(Failure::Error(LuError::IllConditioned { .. }))
        ));
    }

    #[test]
    fn singularity_is_typed_per_operation() {
        let singular = [1.0, 2.0, 2.0, 4.0];
        assert!(matches!(
            solve(&singular, 2, &[1.0, 2.0]).unwrap_err(),
            Outcome::Failed(Failure::Error(LuError::Singular { .. }))
        ));
        assert!(matches!(
            inverse(&singular, 2).unwrap_err(),
            Outcome::Failed(Failure::Error(LuError::Singular { .. }))
        ));
        assert!(determinant(&singular, 2).unwrap().abs() < f64::EPSILON);
    }

    #[test]
    fn determinant_preserves_representable_products_across_exponent_extremes() {
        for pivots in [
            [1.0e-200, 1.0e-200, 1.0e200, 1.0e200],
            [1.0e200, 1.0e200, 1.0e-200, 1.0e-200],
            [1.0e-200, 1.0e200, 1.0e-200, 1.0e200],
        ] {
            let matrix = (0..16)
                .map(|i| if i / 4 == i % 4 { pivots[i / 4] } else { 0.0 })
                .collect::<Vec<_>>();
            assert!((determinant(&matrix, 4).unwrap() - 1.0).abs() <= 4.0 * f64::EPSILON);
        }
        let tiny = f64::from_bits(1);
        assert_eq!(
            determinant(&[tiny, 0.0, 0.0, 1.0], 2).unwrap().to_bits(),
            tiny.to_bits()
        );
        assert_eq!(
            determinant(&[0.0, tiny, 1.0, 0.0], 2).unwrap().to_bits(),
            (-tiny).to_bits()
        );
        assert!(matches!(
            determinant(&[tiny, 0.0, 0.0, 0.25], 2),
            Err(Outcome::Failed(Failure::Error(
                LuError::DeterminantUnderflow
            )))
        ));
        assert!(matches!(
            determinant(&[f64::MAX, 0.0, 0.0, 2.0], 2),
            Err(Outcome::Failed(Failure::Error(LuError::NonFinite { .. })))
        ));
    }

    #[test]
    fn malformed_dense_shapes_are_errors() {
        assert!(SquareMatrix::try_new(2, vec![1.0]).is_err());
        assert!(SquareMatrix::try_new(0, Vec::new()).is_err());
        assert!(SquareMatrix::try_new(usize::MAX, vec![1.0]).is_err());
        assert_eq!(SquareMatrix::try_new(1, vec![1.0]).unwrap().order(), 1);
        assert!(matches!(
            solve(MATRIX, 2, &[1.0]).unwrap_err(),
            Outcome::Failed(Failure::Invariant(_))
        ));
    }

    #[test]
    fn cancellation_is_an_outcome_not_an_error() {
        let cancellation = graphcal_compiler::cancellation::CancellationSource::new();
        cancellation.cancel();
        let token = cancellation.token();
        assert!(matches!(
            determinant_with_control(&square(MATRIX, 2), &mut KernelCheckpoint::new(&token)),
            Err(Outcome::Cancelled)
        ));
    }
}
