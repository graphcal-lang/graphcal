//! Pure dense kernels for built-in linear algebra.
//!
//! Indexed runtime values are flattened into [`DenseArray`]s, whose
//! construction establishes the rectangular shape, then narrowed to explicit
//! vector/matrix carriers. Kernels operate on row-major `f64` buffers, and
//! results are rebuilt over the exact typed axes supplied by the arguments.
//!
//! A call arrives with exactly its checked operands ([`LinearAlgebraCall`]),
//! each read as the vector or matrix its checked type is by a
//! [`LinearOperands`] reader: an operand whose shape contradicts its checked
//! type — not indexed, of another rank, ragged, or with non-quantity entries
//! — is reported where the reader reports every operand shape. Operands over
//! axes the checker proved to agree but that differ are a violated
//! [`Invariant`].

use graphcal_compiler::builtin::LinearAlgebraFn;
use graphcal_compiler::finite_value::FiniteQuantity;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::texpr::operators::LinearAlgebraCall;
use graphcal_compiler::tir::typed::scoped_node::ScopedNode;

use crate::runtime_value::dense_array::{DenseArray, DenseShapeError};
use crate::runtime_value::{IndexAxis, RuntimeValue};

use graphcal_compiler::outcome::Outcome;

use super::EvalSession;
use super::linear_algebra_error::LinearAlgebraError;
use super::linear_algebra_lu::{LinearSystem, LuFailure, SquareMatrix};
use super::numeric::{self, QuantityValidationError};
use super::work_budget::{KernelCheckpoint, WorkAmount, WorkAmountError};
use crate::invariant::{Failure, Invariant};

/// A rank-two operand whose rows and columns enumerate the same axis.
#[derive(Debug)]
struct Square {
    rows: IndexAxis,
    columns: IndexAxis,
    matrix: SquareMatrix,
}

/// Reads the operands of a linear-algebra call as the vectors and matrices
/// their checked types are.
pub(super) trait LinearOperands<N> {
    /// The value of `node`, checked as `expected`, read by `read`, which
    /// returns the value back when its shape contradicts the checked type.
    fn read_operand<T>(
        &self,
        node: N,
        expected: &str,
        read: impl FnOnce(RuntimeValue) -> Result<T, RuntimeValue>,
    ) -> Result<T, Outcome<SemanticError>>;
}

/// A rank-one operand: one value per key of `axis`.
#[derive(Debug)]
pub(super) struct Vector {
    axis: IndexAxis,
    values: Vec<f64>,
}

/// A rank-one operand of exactly three values.
#[derive(Debug)]
struct Vector3 {
    axis: IndexAxis,
    values: [f64; 3],
}

/// A rank-two operand: `rows.len() * columns.len()` values, row-major.
#[derive(Debug)]
pub(super) struct Matrix {
    rows: IndexAxis,
    columns: IndexAxis,
    values: Vec<f64>,
}

impl Vector {
    /// `value` as a vector.
    ///
    /// # Errors
    ///
    /// Returns `value` back unless it is a rank-one array of quantities.
    fn try_from_value(value: RuntimeValue) -> Result<Self, RuntimeValue> {
        Self::read(value, None)
    }

    /// `value` as a vector over `axis`, the axis of another operand the
    /// checker proved it shares.
    fn try_on_axis(value: RuntimeValue, axis: &IndexAxis) -> Result<Self, RuntimeValue> {
        Self::read(value, Some(axis))
    }

    fn read(value: RuntimeValue, shared: Option<&IndexAxis>) -> Result<Self, RuntimeValue> {
        let Some((axes, values)) = dense_quantities(&value) else {
            return Err(value);
        };
        match axes.as_slice() {
            [only] if shared.is_none_or(|shared| shared.matches(only)) => Ok(Self {
                axis: only.clone(),
                values,
            }),
            _ => Err(value),
        }
    }
}

impl Vector3 {
    /// `value` as a three-dimensional vector, over `axis` when given.
    fn try_from_value(
        value: RuntimeValue,
        shared: Option<&IndexAxis>,
    ) -> Result<Self, RuntimeValue> {
        let Some((axes, values)) = dense_quantities(&value) else {
            return Err(value);
        };
        match (axes.as_slice(), <[f64; 3]>::try_from(values.as_slice())) {
            ([only], Ok(values)) if shared.is_none_or(|shared| shared.matches(only)) => Ok(Self {
                axis: only.clone(),
                values,
            }),
            _ => Err(value),
        }
    }
}

impl Matrix {
    /// `value` as a matrix.
    ///
    /// # Errors
    ///
    /// Returns `value` back unless it is a rank-two rectangular array of
    /// quantities.
    fn try_from_value(value: RuntimeValue) -> Result<Self, RuntimeValue> {
        Self::read(value, None)
    }

    /// `value` as a matrix whose rows enumerate `rows`, the axis of another
    /// operand the checker proved it shares.
    fn try_with_rows(value: RuntimeValue, rows: &IndexAxis) -> Result<Self, RuntimeValue> {
        Self::read(value, Some(rows))
    }

    fn read(value: RuntimeValue, row_axis: Option<&IndexAxis>) -> Result<Self, RuntimeValue> {
        let Some((axes, values)) = dense_quantities(&value) else {
            return Err(value);
        };
        match axes.as_slice() {
            [rows, columns] if row_axis.is_none_or(|axis| axis.matches(rows)) => Ok(Self {
                rows: rows.clone(),
                columns: columns.clone(),
                values,
            }),
            _ => Err(value),
        }
    }

    /// The rows as slices of `columns.len()` values each.
    fn row_slices(&self) -> Vec<&[f64]> {
        self.values.chunks_exact(self.columns.len()).collect()
    }
}

impl Square {
    /// `value` as a square matrix: a rank-two array of quantities whose rows
    /// and columns enumerate the same axis.
    fn try_from_value(value: RuntimeValue) -> Result<Self, RuntimeValue> {
        let Some((axes, values)) = dense_quantities(&value) else {
            return Err(value);
        };
        let [rows, columns] = axes.as_slice() else {
            return Err(value);
        };
        if !rows.matches(columns) {
            return Err(value);
        }
        let (rows, columns) = (rows.clone(), columns.clone());
        SquareMatrix::try_new(rows.len(), values).map_or(Err(value), |matrix| {
            Ok(Self {
                rows,
                columns,
                matrix,
            })
        })
    }

    /// This matrix with the right-hand side `value`, a vector over its rows.
    fn system(self, value: RuntimeValue) -> Result<(IndexAxis, LinearSystem), RuntimeValue> {
        let Some((axes, values)) = dense_quantities(&value) else {
            return Err(value);
        };
        match axes.as_slice() {
            [only] if only.matches(&self.rows) => {
                match LinearSystem::try_new(self.matrix, values) {
                    Ok(system) => Ok((self.rows, system)),
                    Err(_) => Err(value),
                }
            }
            _ => Err(value),
        }
    }
}

/// The axes and row-major quantities of a rectangular indexed array of
/// quantities; `None` for any other value.
fn dense_quantities(value: &RuntimeValue) -> Option<(NonEmpty<IndexAxis>, Vec<f64>)> {
    let RuntimeValue::Indexed(indexed) = value else {
        return None;
    };
    DenseArray::try_from_indexed(indexed, |leaf| match leaf {
        RuntimeValue::Quantity(quantity) => Ok(quantity.get()),
        _ => Err(()),
    })
    .ok()
    .map(DenseArray::into_parts)
}

/// Why a linear-algebra operation did not produce a value.
pub(super) type LinearAlgebraFailure = Outcome<Failure<LinearAlgebraError>>;

impl From<LinearAlgebraError> for LinearAlgebraFailure {
    fn from(error: LinearAlgebraError) -> Self {
        Self::Failed(Failure::Error(error))
    }
}

impl From<QuantityValidationError> for LinearAlgebraFailure {
    fn from(error: QuantityValidationError) -> Self {
        LinearAlgebraError::from(error).into()
    }
}

impl From<WorkAmountError> for LinearAlgebraFailure {
    fn from(error: WorkAmountError) -> Self {
        LinearAlgebraError::from(error).into()
    }
}

impl From<DenseShapeError> for LinearAlgebraFailure {
    fn from(error: DenseShapeError) -> Self {
        Invariant::violated(format_args!(
            "linear-algebra result invariant failed: {error}"
        ))
        .into()
    }
}

/// Re-type an LU failure as the failure of the builtin that ran it.
fn algorithm_failure(failure: LuFailure) -> LinearAlgebraFailure {
    failure.map_failed(|failure| failure.map_error(LinearAlgebraError::Algorithm))
}

fn kernel_control<'a>(
    function: LinearAlgebraFn,
    factors: &[usize],
    multiplier: u64,
    ctx: &'a EvalSession<'_>,
) -> Result<KernelCheckpoint<'a>, LinearAlgebraFailure> {
    let amount = WorkAmount::checked_product(factors, multiplier)?;
    ctx.work_budget
        .consume(amount)
        .map_err(|source| LinearAlgebraError::WorkBudget { function, source })?;
    Ok(KernelCheckpoint::new(&ctx.cancellation))
}

/// Rebuild a kernel result over `axes` from row-major `values`.
fn indexed_result(
    axes: NonEmpty<IndexAxis>,
    values: Vec<f64>,
    context: &'static str,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    DenseArray::try_new(axes, values)?
        .try_to_indexed(|value| finite_runtime_quantity(*value, context))
        .map(RuntimeValue::Indexed)
}

fn vector_value(axis: IndexAxis, values: Vec<f64>) -> Result<RuntimeValue, LinearAlgebraFailure> {
    indexed_result(
        NonEmpty::singleton(axis),
        values,
        "linear-algebra indexed result",
    )
}

fn matrix_value(
    rows: IndexAxis,
    columns: IndexAxis,
    values: Vec<f64>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    indexed_result(
        NonEmpty::new(rows, vec![columns]),
        values,
        "linear-algebra matrix result",
    )
}

fn finite_product(lhs: f64, rhs: f64, context: &'static str) -> Result<f64, LinearAlgebraFailure> {
    let result = lhs * rhs;
    if lhs != 0.0 && rhs != 0.0 {
        numeric::computed_nonzero_quantity(result, context)
    } else {
        numeric::computed_finite_quantity(result, context)
    }
    .map(FiniteQuantity::get)
    .map_err(LinearAlgebraFailure::from)
}

fn sum_products(
    lhs: impl IntoIterator<Item = f64>,
    rhs: impl IntoIterator<Item = f64>,
    context: &'static str,
    control: &mut KernelCheckpoint<'_>,
) -> Result<FiniteQuantity, LinearAlgebraFailure> {
    lhs.into_iter()
        .zip(rhs)
        .try_fold(FiniteQuantity::ZERO, |sum, (lhs, rhs)| {
            control.step()?;
            let product = finite_product(lhs, rhs, context)?;
            numeric::computed_finite_quantity(sum.get() + product, context)
                .map_err(LinearAlgebraFailure::from)
        })
}

fn norm(
    values: &[f64],
    control: &mut KernelCheckpoint<'_>,
) -> Result<FiniteQuantity, LinearAlgebraFailure> {
    let accumulator =
        values
            .iter()
            .try_fold(numeric::RootSumSquare::new(), |mut accumulator, value| {
                control.step()?;
                accumulator.add(*value, "norm()")?;
                Ok::<_, LinearAlgebraFailure>(accumulator)
            })?;
    accumulator.finish("norm()").map_err(Into::into)
}

fn finite_runtime_quantity(
    value: f64,
    context: &'static str,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    FiniteQuantity::try_new(value)
        .map(RuntimeValue::Quantity)
        .map_err(|error| {
            LinearAlgebraFailure::from(QuantityValidationError::NonFinite {
                context: context.to_string(),
                value: error.value,
            })
        })
}

fn evaluate_dot(
    lhs: Vector,
    rhs: Vector,
    ctx: &EvalSession<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Dot;
    let mut control = kernel_control(function, &[lhs.axis.len()], 1, ctx)?;
    sum_products(lhs.values, rhs.values, "dot()", &mut control).map(RuntimeValue::Quantity)
}

fn evaluate_matmul(
    lhs: Matrix,
    rhs: Matrix,
    ctx: &EvalSession<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Matmul;
    let (rows, inner, columns) = (lhs.rows.len(), lhs.columns.len(), rhs.columns.len());
    let mut control = kernel_control(function, &[rows, inner, columns], 1, ctx)?;
    let rhs_rows = rhs.row_slices();
    let mut values = Vec::new();
    for lhs_row in lhs.row_slices() {
        for column in 0..columns {
            let mut sum = 0.0;
            for (lhs_value, rhs_row) in lhs_row.iter().zip(&rhs_rows) {
                control.step()?;
                let product = finite_product(*lhs_value, rhs_row[column], "matmul()")?;
                sum = numeric::computed_finite_quantity(sum + product, "matmul()")?.get();
            }
            values.push(sum);
        }
    }
    matrix_value(lhs.rows, rhs.columns, values)
}

fn evaluate_transpose(
    matrix: Matrix,
    ctx: &EvalSession<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Transpose;
    let mut control = kernel_control(function, &[matrix.rows.len(), matrix.columns.len()], 1, ctx)?;
    let rows = matrix.row_slices();
    let values = (0..matrix.columns.len())
        .flat_map(|column| rows.iter().map(move |row| row[column]))
        .map(|value| {
            control.step()?;
            Ok::<_, LinearAlgebraFailure>(value)
        })
        .collect::<Result<Vec<_>, _>>()?;
    matrix_value(matrix.columns, matrix.rows, values)
}

fn evaluate_trace(
    square: &Square,
    ctx: &EvalSession<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Trace;
    let order = square.matrix.order();
    let mut control = kernel_control(function, &[order], 1, ctx)?;
    (0..order)
        .try_fold(FiniteQuantity::ZERO, |sum, diagonal| {
            control.step()?;
            numeric::computed_finite_quantity(
                sum.get() + square.matrix.at(diagonal, diagonal),
                "trace()",
            )
            .map_err(LinearAlgebraFailure::from)
        })
        .map(RuntimeValue::Quantity)
}

fn evaluate_norm(
    vector: &Vector,
    ctx: &EvalSession<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Norm;
    let mut control = kernel_control(function, &[vector.axis.len()], 1, ctx)?;
    norm(&vector.values, &mut control).map(RuntimeValue::Quantity)
}

fn evaluate_cross(
    lhs: Vector3,
    rhs: &Vector3,
    ctx: &EvalSession<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Cross;
    let (a, b) = (lhs.values, rhs.values);
    let control = kernel_control(function, &[6], 1, ctx)?;
    control.boundary()?;
    let component = |a_1: f64, b_1: f64, a_2: f64, b_2: f64| {
        let positive = finite_product(a_1, b_1, "cross()")?;
        let negative = finite_product(a_2, b_2, "cross()")?;
        numeric::computed_finite_quantity(positive - negative, "cross()")
            .map(FiniteQuantity::get)
            .map_err(LinearAlgebraFailure::from)
    };
    let values = vec![
        component(a[1], b[2], a[2], b[1])?,
        component(a[2], b[0], a[0], b[2])?,
        component(a[0], b[1], a[1], b[0])?,
    ];
    vector_value(lhs.axis, values)
}

fn evaluate_outer(
    lhs: Vector,
    rhs: Vector,
    ctx: &EvalSession<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Outer;
    let mut control = kernel_control(function, &[lhs.axis.len(), rhs.axis.len()], 1, ctx)?;
    let values = lhs
        .values
        .iter()
        .flat_map(|lhs| rhs.values.iter().map(move |rhs| (*lhs, *rhs)))
        .map(|(lhs, rhs)| {
            control.step()?;
            finite_product(lhs, rhs, "outer()")
        })
        .collect::<Result<Vec<_>, _>>()?;
    matrix_value(lhs.axis, rhs.axis, values)
}

fn evaluate_solve(
    rows: IndexAxis,
    system: &LinearSystem,
    ctx: &EvalSession<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Solve;
    let mut control = kernel_control(function, &[rows.len(); 3], 2, ctx)?;
    let solution = super::linear_algebra_lu::solve_with_control(system, &mut control)
        .map_err(algorithm_failure)?;
    vector_value(rows, solution)
}

fn evaluate_inverse(
    square: Square,
    ctx: &EvalSession<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Inverse;
    let mut control = kernel_control(function, &[square.rows.len(); 3], 3, ctx)?;
    let inverse = super::linear_algebra_lu::inverse_with_control(&square.matrix, &mut control)
        .map_err(algorithm_failure)?;
    matrix_value(square.rows, square.columns, inverse.into_values())
}

fn evaluate_determinant(
    square: &Square,
    ctx: &EvalSession<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Determinant;
    let mut control = kernel_control(function, &[square.rows.len(); 3], 1, ctx)?;
    super::linear_algebra_lu::determinant_with_control(&square.matrix, &mut control)
        .map_err(algorithm_failure)
        .and_then(|value| finite_runtime_quantity(value, "det()"))
}

/// What each kind of linear-algebra operand is checked as.
const VECTOR: &str = "a rank-1 quantity array";
const MATRIX: &str = "a rank-2 quantity array";
const SQUARE: &str = "a rank-2 quantity array over one axis twice";
const VECTOR3: &str = "a rank-1 quantity array of three entries";
const SHARED_VECTOR: &str = "a rank-1 quantity array over the axis of the other operand";
const CHAINED_MATRIX: &str = "a rank-2 quantity array whose rows are the other operand's columns";

/// Evaluate a checked built-in linear-algebra call, reading each operand,
/// in argument order, as the vector or matrix its checked type is.
pub(super) fn evaluate<'t>(
    call: &LinearAlgebraCall<ScopedNode<'t>>,
    span: Span,
    operands: &impl LinearOperands<ScopedNode<'t>>,
    ctx: &EvalSession<'_>,
) -> Result<RuntimeValue, Outcome<SemanticError>> {
    // Operands the checker proved to share an axis are read against the
    // axis of the operand read before them.
    let vector = |node| operands.read_operand(node, VECTOR, Vector::try_from_value);
    let matrix = |node| operands.read_operand(node, MATRIX, Matrix::try_from_value);
    let square = |node| operands.read_operand(node, SQUARE, Square::try_from_value);
    match *call {
        LinearAlgebraCall::Dot { lhs, rhs } => {
            let lhs = vector(lhs)?;
            let rhs = operands.read_operand(rhs, SHARED_VECTOR, |value| {
                Vector::try_on_axis(value, &lhs.axis)
            })?;
            evaluate_dot(lhs, rhs, ctx)
        }
        LinearAlgebraCall::Matmul { lhs, rhs } => {
            let lhs = matrix(lhs)?;
            let rhs = operands.read_operand(rhs, CHAINED_MATRIX, |value| {
                Matrix::try_with_rows(value, &lhs.columns)
            })?;
            evaluate_matmul(lhs, rhs, ctx)
        }
        LinearAlgebraCall::Transpose(node) => evaluate_transpose(matrix(node)?, ctx),
        LinearAlgebraCall::Trace(node) => evaluate_trace(&square(node)?, ctx),
        LinearAlgebraCall::Norm(node) => evaluate_norm(&vector(node)?, ctx),
        LinearAlgebraCall::Cross { lhs, rhs } => {
            let lhs = operands
                .read_operand(lhs, VECTOR3, |value| Vector3::try_from_value(value, None))?;
            let rhs = operands.read_operand(rhs, VECTOR3, |value| {
                Vector3::try_from_value(value, Some(&lhs.axis))
            })?;
            evaluate_cross(lhs, &rhs, ctx)
        }
        LinearAlgebraCall::Outer { lhs, rhs } => evaluate_outer(vector(lhs)?, vector(rhs)?, ctx),
        LinearAlgebraCall::Solve { matrix, rhs } => {
            let square = square(matrix)?;
            let (rows, system) =
                operands.read_operand(rhs, SHARED_VECTOR, |value| square.system(value))?;
            evaluate_solve(rows, &system, ctx)
        }
        LinearAlgebraCall::Inverse(node) => evaluate_inverse(square(node)?, ctx),
        LinearAlgebraCall::Determinant(node) => evaluate_determinant(&square(node)?, ctx),
    }
    .map_err(|outcome| ctx.outcome_error(outcome, span))
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::semantic::index_def::FiniteIndex;
    use graphcal_compiler::syntax::non_empty::NonEmpty;

    use super::*;
    use crate::runtime_value::IndexedValue;

    fn fin(cardinality: u64) -> IndexAxis {
        IndexAxis::finite(FiniteIndex::try_from_u64(cardinality).unwrap()).unwrap()
    }

    fn quantity(value: f64) -> RuntimeValue {
        RuntimeValue::quantity(value).unwrap()
    }

    fn vector(values: &[f64]) -> RuntimeValue {
        RuntimeValue::Indexed(IndexedValue::for_test(
            fin(u64::try_from(values.len()).unwrap()),
            values.iter().copied().map(quantity).collect(),
        ))
    }

    #[test]
    fn operands_are_narrowed_by_rank() {
        let row = vector(&[1.0, 2.0]);
        let matrix = RuntimeValue::Indexed(IndexedValue::for_test(
            fin(3),
            vec![row.clone(), vector(&[3.0, 4.0]), vector(&[5.0, 6.0])],
        ));

        let parsed = Vector::try_from_value(row.clone()).unwrap();
        assert_eq!(parsed.values, vec![1.0, 2.0]);
        assert!(parsed.axis.matches(&fin(2)));
        assert_eq!(Vector::try_from_value(matrix.clone()).unwrap_err(), matrix);

        let parsed = Matrix::try_from_value(matrix).unwrap();
        assert!(parsed.rows.matches(&fin(3)));
        assert!(parsed.columns.matches(&fin(2)));
        assert_eq!(
            parsed.row_slices(),
            vec![&[1.0, 2.0][..], &[3.0, 4.0], &[5.0, 6.0]]
        );
        assert_eq!(Matrix::try_from_value(row.clone()).unwrap_err(), row);
        assert_eq!(
            Vector::try_from_value(quantity(1.0)).unwrap_err(),
            quantity(1.0)
        );
    }

    #[test]
    fn ragged_and_non_quantity_operands_are_not_matrices_or_vectors() {
        let ragged = RuntimeValue::Indexed(IndexedValue::for_test(
            fin(2),
            vec![vector(&[1.0, 2.0]), vector(&[3.0])],
        ));
        assert!(Matrix::try_from_value(ragged).is_err());
        let booleans = RuntimeValue::Indexed(IndexedValue::for_test(
            fin(1),
            vec![RuntimeValue::Bool(true)],
        ));
        assert!(Vector::try_from_value(booleans).is_err());
    }

    #[test]
    fn results_are_rebuilt_over_the_argument_axes() {
        let RuntimeValue::Indexed(result) = matrix_value(fin(2), fin(1), vec![1.0, 2.0]).unwrap()
        else {
            panic!("matrix results are indexed");
        };
        assert!(result.axis().matches(&fin(2)));
        assert!(matches!(
            vector_value(fin(2), vec![1.0]),
            Err(Outcome::Failed(Failure::Invariant(_)))
        ));
        assert!(matches!(
            indexed_result(NonEmpty::singleton(fin(1)), vec![f64::INFINITY], "test"),
            Err(Outcome::Failed(Failure::Error(
                LinearAlgebraError::Numeric(_)
            )))
        ));
    }

    #[test]
    fn operands_sharing_an_axis_are_read_against_it() {
        let pair = vector(&[1.0, 2.0]);
        assert!(Vector::try_on_axis(pair.clone(), &fin(2)).is_ok());
        assert_eq!(
            Vector::try_on_axis(pair.clone(), &fin(3)).unwrap_err(),
            pair
        );
        let triple = vector(&[1.0, 2.0, 3.0]);
        assert!(Vector3::try_from_value(triple.clone(), Some(&fin(3))).is_ok());
        assert!(Vector3::try_from_value(triple, Some(&fin(2))).is_err());
        assert!(Vector3::try_from_value(pair.clone(), None).is_err());
        let rectangle = RuntimeValue::Indexed(IndexedValue::for_test(
            fin(3),
            vec![pair.clone(), vector(&[3.0, 4.0]), vector(&[5.0, 6.0])],
        ));
        assert!(Matrix::try_with_rows(rectangle.clone(), &fin(3)).is_ok());
        assert!(Matrix::try_with_rows(rectangle.clone(), &fin(2)).is_err());
        assert!(Square::try_from_value(rectangle).is_err());
        let square = RuntimeValue::Indexed(IndexedValue::for_test(
            fin(2),
            vec![pair.clone(), vector(&[3.0, 4.0])],
        ));
        let read = || Square::try_from_value(square.clone()).unwrap();
        assert_eq!(read().matrix.at(1, 0).to_bits(), 3.0_f64.to_bits());
        assert!(read().system(pair).is_ok());
        let wrong = vector(&[1.0, 2.0, 3.0]);
        assert_eq!(read().system(wrong.clone()).unwrap_err(), wrong);
    }
}
