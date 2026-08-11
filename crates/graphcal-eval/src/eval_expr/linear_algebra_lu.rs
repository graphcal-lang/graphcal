//! Dense square-matrix algorithms used by the public linear-algebra builtins.
//!
//! Scaled partial-pivoting LU is shared by `solve`, `inverse`, and `det`.
//! Singular matrices are represented explicitly rather than by a sentinel or
//! `Option`; solve-like operations additionally reject a numerically unsafe
//! scaled-pivot profile and verify the residual of every returned solution.

use thiserror::Error;

use super::work_budget::KernelCheckpoint;

#[derive(Debug, Error)]
pub(super) enum LuError {
    #[error("linear-algebra runtime shape invariant failed: {0}")]
    ShapeInvariant(String),
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
    #[error(
        "{operation} failed its numerical residual check (residual {residual:e}, tolerance {tolerance:e})"
    )]
    ResidualTooLarge {
        operation: &'static str,
        residual: f64,
        tolerance: f64,
    },
    #[error(transparent)]
    Cancelled(#[from] graphcal_compiler::cancellation::Cancelled),
}

impl LuError {
    pub(super) const fn is_internal_invariant(&self) -> bool {
        matches!(self, Self::ShapeInvariant(_))
    }

    pub(super) const fn cancellation(&self) -> Option<graphcal_compiler::cancellation::Cancelled> {
        match self {
            Self::Cancelled(cancelled) => Some(*cancelled),
            Self::ShapeInvariant(_)
            | Self::Singular { .. }
            | Self::IllConditioned { .. }
            | Self::NonFinite { .. }
            | Self::ResidualTooLarge { .. } => None,
        }
    }
}

#[derive(Debug)]
enum Factorization {
    Singular,
    Nonsingular(LuDecomposition),
}

#[derive(Debug)]
struct LuDecomposition {
    order: usize,
    factors: Vec<f64>,
    permutation: Vec<usize>,
    parity: f64,
    minimum_scaled_pivot: f64,
}

fn matrix_len(order: usize) -> Result<usize, LuError> {
    order.checked_mul(order).ok_or_else(|| {
        LuError::ShapeInvariant("square matrix cardinality overflowed usize".to_string())
    })
}

fn dense_position(order: usize, row: usize, column: usize) -> Result<usize, LuError> {
    if row >= order || column >= order {
        return Err(LuError::ShapeInvariant(format!(
            "matrix position ({row}, {column}) is outside order {order}"
        )));
    }
    row.checked_mul(order)
        .and_then(|offset| offset.checked_add(column))
        .ok_or_else(|| LuError::ShapeInvariant("matrix position overflowed usize".to_string()))
}

fn dense_value(values: &[f64], order: usize, row: usize, column: usize) -> Result<f64, LuError> {
    let position = dense_position(order, row, column)?;
    values.get(position).copied().ok_or_else(|| {
        LuError::ShapeInvariant("matrix position is outside its dense buffer".to_string())
    })
}

fn set_dense_value(
    values: &mut [f64],
    order: usize,
    row: usize,
    column: usize,
    value: f64,
) -> Result<(), LuError> {
    let position = dense_position(order, row, column)?;
    let target = values.get_mut(position).ok_or_else(|| {
        LuError::ShapeInvariant("matrix position is outside its dense buffer".to_string())
    })?;
    *target = value;
    Ok(())
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
        matrix: &[f64],
        order: usize,
        control: &mut KernelCheckpoint<'_>,
    ) -> Result<Factorization, LuError> {
        control.boundary()?;
        if order == 0 {
            return Err(LuError::ShapeInvariant(
                "LU requires a nonempty square matrix".to_string(),
            ));
        }
        let expected = matrix_len(order)?;
        if matrix.len() != expected {
            return Err(LuError::ShapeInvariant(format!(
                "LU received {} values for an order-{order} matrix",
                matrix.len()
            )));
        }
        if matrix.iter().any(|value| !value.is_finite()) {
            return Err(LuError::NonFinite {
                operation: "LU factorization",
            });
        }

        let mut factors = matrix.to_vec();
        let mut scales = Vec::with_capacity(order);
        for row in 0..order {
            let mut scale = 0.0_f64;
            for column in 0..order {
                control.step()?;
                scale = scale.max(dense_value(&factors, order, row, column)?.abs());
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
            let mut best: Option<(usize, f64)> = None;
            for (row, scale) in scales.iter().copied().enumerate().skip(pivot_column) {
                control.step()?;
                let ratio = dense_value(&factors, order, row, pivot_column)?.abs() / scale;
                match best {
                    Some((_, best_ratio)) if ratio <= best_ratio => {}
                    Some(_) | None => best = Some((row, ratio)),
                }
            }
            let (pivot_row, _) = best.ok_or_else(|| {
                LuError::ShapeInvariant("LU could not select a pivot row".to_string())
            })?;
            if dense_value(&factors, order, pivot_row, pivot_column)? == 0.0 {
                return Ok(Factorization::Singular);
            }

            if pivot_row != pivot_column {
                for column in 0..order {
                    control.step()?;
                    let lhs = dense_position(order, pivot_row, column)?;
                    let rhs = dense_position(order, pivot_column, column)?;
                    factors.swap(lhs, rhs);
                }
                scales.swap(pivot_row, pivot_column);
                permutation.swap(pivot_row, pivot_column);
                parity = -parity;
            }

            let pivot = dense_value(&factors, order, pivot_column, pivot_column)?;
            minimum_scaled_pivot = minimum_scaled_pivot.min(pivot.abs() / scales[pivot_column]);
            for row in pivot_column.saturating_add(1)..order {
                control.step()?;
                let multiplier = finite(
                    dense_value(&factors, order, row, pivot_column)? / pivot,
                    "LU factorization",
                )?;
                set_dense_value(&mut factors, order, row, pivot_column, multiplier)?;
                for column in pivot_column.saturating_add(1)..order {
                    control.step()?;
                    let current = dense_value(&factors, order, row, column)?;
                    let upper = dense_value(&factors, order, pivot_column, column)?;
                    let updated =
                        finite((-multiplier).mul_add(upper, current), "LU factorization")?;
                    set_dense_value(&mut factors, order, row, column, updated)?;
                }
            }
        }

        Ok(Factorization::Nonsingular(Self {
            order,
            factors,
            permutation,
            parity,
            minimum_scaled_pivot,
        }))
    }

    fn ensure_conditioned(&self, operation: &'static str) -> Result<(), LuError> {
        if self.minimum_scaled_pivot <= numerical_threshold(self.order) {
            Err(LuError::IllConditioned {
                operation,
                scaled_pivot: self.minimum_scaled_pivot,
            })
        } else {
            Ok(())
        }
    }

    fn solve_raw(
        &self,
        rhs: &[f64],
        operation: &'static str,
        control: &mut KernelCheckpoint<'_>,
    ) -> Result<Vec<f64>, LuError> {
        self.ensure_conditioned(operation)?;
        if rhs.len() != self.order {
            return Err(LuError::ShapeInvariant(format!(
                "{operation} received a right-hand side of length {} for order {}",
                rhs.len(),
                self.order
            )));
        }
        if rhs.iter().any(|value| !value.is_finite()) {
            return Err(LuError::NonFinite { operation });
        }

        let mut solution = self
            .permutation
            .iter()
            .map(|position| {
                rhs.get(*position).copied().ok_or_else(|| {
                    LuError::ShapeInvariant(
                        "LU permutation points outside the right-hand side".to_string(),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        for row in 0..self.order {
            let mut value = solution[row];
            for (column, solved) in solution.iter().copied().enumerate().take(row) {
                control.step()?;
                value = finite(
                    (-dense_value(&self.factors, self.order, row, column)?).mul_add(solved, value),
                    operation,
                )?;
            }
            solution[row] = value;
        }
        for row in (0..self.order).rev() {
            let mut value = solution[row];
            for (column, solved) in solution
                .iter()
                .copied()
                .enumerate()
                .skip(row.saturating_add(1))
            {
                control.step()?;
                value = finite(
                    (-dense_value(&self.factors, self.order, row, column)?).mul_add(solved, value),
                    operation,
                )?;
            }
            let diagonal = dense_value(&self.factors, self.order, row, row)?;
            solution[row] = finite(value / diagonal, operation)?;
        }
        Ok(solution)
    }

    fn determinant(&self, control: &mut KernelCheckpoint<'_>) -> Result<f64, LuError> {
        (0..self.order).try_fold(self.parity, |determinant, diagonal| {
            control.step()?;
            finite(
                determinant * dense_value(&self.factors, self.order, diagonal, diagonal)?,
                "det()",
            )
        })
    }
}

fn checked_residual(
    matrix: &[f64],
    order: usize,
    rhs: &[f64],
    solution: &[f64],
    operation: &'static str,
    control: &mut KernelCheckpoint<'_>,
) -> Result<(), LuError> {
    let mut matrix_norm = 0.0_f64;
    let mut residual_norm = 0.0_f64;
    for row in 0..order {
        let mut row_norm = 0.0_f64;
        let mut product = 0.0_f64;
        for column in 0..order {
            control.step()?;
            let coefficient = dense_value(matrix, order, row, column)?;
            row_norm = finite(row_norm + coefficient.abs(), operation)?;
            let value = solution.get(column).copied().ok_or_else(|| {
                LuError::ShapeInvariant("solution is shorter than the matrix order".to_string())
            })?;
            product = finite(coefficient.mul_add(value, product), operation)?;
        }
        matrix_norm = matrix_norm.max(row_norm);
        let expected = rhs.get(row).copied().ok_or_else(|| {
            LuError::ShapeInvariant("right-hand side is shorter than the matrix order".to_string())
        })?;
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
        })
    }
}

fn require_nonsingular(
    matrix: &[f64],
    order: usize,
    operation: &'static str,
    control: &mut KernelCheckpoint<'_>,
) -> Result<LuDecomposition, LuError> {
    match LuDecomposition::factor(matrix, order, control)? {
        Factorization::Singular => Err(LuError::Singular { operation }),
        Factorization::Nonsingular(decomposition) => Ok(decomposition),
    }
}

pub(super) fn solve_with_control(
    matrix: &[f64],
    order: usize,
    rhs: &[f64],
    control: &mut KernelCheckpoint<'_>,
) -> Result<Vec<f64>, LuError> {
    let operation = "solved";
    let decomposition = require_nonsingular(matrix, order, operation, control)?;
    let solution = decomposition.solve_raw(rhs, operation, control)?;
    checked_residual(matrix, order, rhs, &solution, operation, control)?;
    Ok(solution)
}

pub(super) fn inverse_with_control(
    matrix: &[f64],
    order: usize,
    control: &mut KernelCheckpoint<'_>,
) -> Result<Vec<f64>, LuError> {
    let operation = "inverted";
    let decomposition = require_nonsingular(matrix, order, operation, control)?;
    decomposition.ensure_conditioned(operation)?;
    let length = matrix_len(order)?;
    let mut inverse = vec![0.0; length];
    for column in 0..order {
        control.boundary()?;
        let rhs = (0..order)
            .map(|row| if row == column { 1.0 } else { 0.0 })
            .collect::<Vec<_>>();
        let solution = decomposition.solve_raw(&rhs, operation, control)?;
        checked_residual(matrix, order, &rhs, &solution, operation, control)?;
        for (row, value) in solution.into_iter().enumerate() {
            control.step()?;
            set_dense_value(&mut inverse, order, row, column, value)?;
        }
    }
    Ok(inverse)
}

pub(super) fn determinant_with_control(
    matrix: &[f64],
    order: usize,
    control: &mut KernelCheckpoint<'_>,
) -> Result<f64, LuError> {
    match LuDecomposition::factor(matrix, order, control)? {
        Factorization::Singular => Ok(0.0),
        Factorization::Nonsingular(decomposition) => decomposition.determinant(control),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solve(matrix: &[f64], order: usize, rhs: &[f64]) -> Result<Vec<f64>, LuError> {
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        solve_with_control(
            matrix,
            order,
            rhs,
            &mut KernelCheckpoint::new(&cancellation),
        )
    }

    fn inverse(matrix: &[f64], order: usize) -> Result<Vec<f64>, LuError> {
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        inverse_with_control(matrix, order, &mut KernelCheckpoint::new(&cancellation))
    }

    fn determinant(matrix: &[f64], order: usize) -> Result<f64, LuError> {
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        determinant_with_control(matrix, order, &mut KernelCheckpoint::new(&cancellation))
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
            LuError::IllConditioned { .. }
        ));
    }

    #[test]
    fn singularity_is_typed_per_operation() {
        let singular = [1.0, 2.0, 2.0, 4.0];
        assert!(matches!(
            solve(&singular, 2, &[1.0, 2.0]).unwrap_err(),
            LuError::Singular { .. }
        ));
        assert!(matches!(
            inverse(&singular, 2).unwrap_err(),
            LuError::Singular { .. }
        ));
        assert!(determinant(&singular, 2).unwrap().abs() < f64::EPSILON);
    }

    #[test]
    fn malformed_dense_shapes_are_errors() {
        assert!(matches!(
            solve(&[1.0], 2, &[1.0, 2.0]).unwrap_err(),
            LuError::ShapeInvariant(_)
        ));
        assert!(matches!(
            solve(MATRIX, 2, &[1.0]).unwrap_err(),
            LuError::ShapeInvariant(_)
        ));
    }
}
