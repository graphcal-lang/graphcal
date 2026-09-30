//! Pure dense kernels for built-in linear algebra.
//!
//! Indexed runtime values are flattened into [`DenseArray`]s, whose
//! construction establishes the rectangular shape, then narrowed to explicit
//! vector/matrix carriers. Kernels operate on row-major `f64` buffers, and
//! results are rebuilt over the exact typed axes supplied by the arguments.

use graphcal_compiler::builtin::LinearAlgebraFn;
use graphcal_compiler::finite_value::FiniteQuantity;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use thiserror::Error;

use crate::runtime_value::dense_array::{DenseArray, DenseArrayError, DenseShapeError};
use crate::runtime_value::{IndexAxis, RuntimeValue, RuntimeValueError};

use graphcal_compiler::outcome::Outcome;

use super::EvalContext;
use super::linear_algebra_lu::LuFailure;
use super::numeric::{self, QuantityValidationError};
use super::work_budget::{KernelCheckpoint, WorkAmount, WorkAmountError, WorkBudgetError};
use crate::invariant::{Failure, Invariant};

/// A rank-one operand: one value per key of `axis`.
#[derive(Debug)]
struct Vector {
    axis: IndexAxis,
    values: Vec<f64>,
}

/// A rank-two operand: `rows.len() * columns.len()` values, row-major.
#[derive(Debug)]
struct Matrix {
    rows: IndexAxis,
    columns: IndexAxis,
    values: Vec<f64>,
}

impl Vector {
    fn from_value(value: &RuntimeValue, context: &'static str) -> Result<Self, OperandInvariant> {
        let (axes, values) = dense_operand(value, context)?.into_parts();
        let (first, inner) = axes.split_first();
        if !inner.is_empty() {
            return Err(OperandInvariant::Rank {
                context,
                expected: 1,
                actual: axes.len(),
            });
        }
        Ok(Self {
            axis: first.clone(),
            values,
        })
    }
}

impl Matrix {
    fn from_value(value: &RuntimeValue, context: &'static str) -> Result<Self, OperandInvariant> {
        let (axes, values) = dense_operand(value, context)?.into_parts();
        let [rows, columns] = axes.as_slice() else {
            return Err(OperandInvariant::Rank {
                context,
                expected: 2,
                actual: axes.len(),
            });
        };
        Ok(Self {
            rows: rows.clone(),
            columns: columns.clone(),
            values,
        })
    }

    /// The rows as slices of `columns.len()` values each.
    fn row_slices(&self) -> Vec<&[f64]> {
        self.values.chunks_exact(self.columns.len()).collect()
    }
}

/// Flatten an indexed quantity operand into a rectangular array.
fn dense_operand(
    value: &RuntimeValue,
    context: &'static str,
) -> Result<DenseArray<f64>, OperandInvariant> {
    let RuntimeValue::Indexed(indexed) = value else {
        return Err(OperandInvariant::NotIndexed { context });
    };
    DenseArray::try_from_indexed(indexed, |leaf| {
        leaf.expect_quantity(context)
            .map(graphcal_compiler::finite_value::FiniteQuantity::get)
    })
    .map_err(|error| match error {
        DenseArrayError::Ragged => OperandInvariant::Ragged { context },
        DenseArrayError::Element(error) => OperandInvariant::Element(error),
    })
}

/// An operand combination the type checker rules out.
///
/// The value shapes themselves are established by [`DenseArray`]; these are
/// the remaining type-level facts (arity, rank, axis agreement) that the
/// untyped kernel entry still re-checks.
#[derive(Debug, Error)]
pub(super) enum OperandInvariant {
    #[error("{function}() received {received} arguments after type checking")]
    Arity {
        function: LinearAlgebraFn,
        received: usize,
    },
    #[error("{context} expected an indexed operand")]
    NotIndexed { context: &'static str },
    #[error("{context} expected a rank-{expected} operand, got rank {actual}")]
    Rank {
        context: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("{context} received a ragged indexed operand")]
    Ragged { context: &'static str },
    #[error(transparent)]
    Element(RuntimeValueError),
    #[error("{function}() received operands over incompatible axes")]
    AxisMismatch { function: LinearAlgebraFn },
}

#[derive(Debug, Error)]
pub(super) enum LinearAlgebraError {
    #[error(transparent)]
    Numeric(#[from] QuantityValidationError),
    #[error(transparent)]
    Algorithm(#[from] super::linear_algebra_lu::LuError),
    #[error("linear-algebra work estimate failed: {0}")]
    WorkAmount(#[from] WorkAmountError),
    #[error("`{function}()` {source}")]
    WorkBudget {
        function: LinearAlgebraFn,
        #[source]
        source: WorkBudgetError,
    },
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

impl From<OperandInvariant> for LinearAlgebraFailure {
    fn from(error: OperandInvariant) -> Self {
        Invariant::violated(format_args!(
            "linear-algebra operand invariant failed: {error}"
        ))
        .into()
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
    ctx: &'a EvalContext<'_>,
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

fn one_argument(
    function: LinearAlgebraFn,
    arguments: Vec<RuntimeValue>,
) -> Result<RuntimeValue, OperandInvariant> {
    <[RuntimeValue; 1]>::try_from(arguments)
        .map(|[argument]| argument)
        .map_err(|arguments| OperandInvariant::Arity {
            function,
            received: arguments.len(),
        })
}

fn two_arguments(
    function: LinearAlgebraFn,
    arguments: Vec<RuntimeValue>,
) -> Result<[RuntimeValue; 2], OperandInvariant> {
    <[RuntimeValue; 2]>::try_from(arguments).map_err(|arguments| OperandInvariant::Arity {
        function,
        received: arguments.len(),
    })
}

fn require_matching_axes(
    function: LinearAlgebraFn,
    lhs: &IndexAxis,
    rhs: &IndexAxis,
) -> Result<(), OperandInvariant> {
    if lhs.matches(rhs) {
        Ok(())
    } else {
        Err(OperandInvariant::AxisMismatch { function })
    }
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
    arguments: Vec<RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Dot;
    let [lhs, rhs] = two_arguments(function, arguments)?;
    let lhs = Vector::from_value(&lhs, "dot")?;
    let rhs = Vector::from_value(&rhs, "dot")?;
    require_matching_axes(function, &lhs.axis, &rhs.axis)?;
    let mut control = kernel_control(function, &[lhs.axis.len()], 1, ctx)?;
    sum_products(lhs.values, rhs.values, "dot()", &mut control).map(RuntimeValue::Quantity)
}

fn evaluate_matmul(
    arguments: Vec<RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Matmul;
    let [lhs, rhs] = two_arguments(function, arguments)?;
    let lhs = Matrix::from_value(&lhs, "matmul")?;
    let rhs = Matrix::from_value(&rhs, "matmul")?;
    require_matching_axes(function, &lhs.columns, &rhs.rows)?;
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
    arguments: Vec<RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Transpose;
    let matrix = Matrix::from_value(&one_argument(function, arguments)?, "transpose")?;
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
    arguments: Vec<RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Trace;
    let matrix = Matrix::from_value(&one_argument(function, arguments)?, "trace")?;
    require_matching_axes(function, &matrix.rows, &matrix.columns)?;
    let mut control = kernel_control(function, &[matrix.rows.len()], 1, ctx)?;
    matrix
        .row_slices()
        .into_iter()
        .enumerate()
        .try_fold(FiniteQuantity::ZERO, |sum, (diagonal, row)| {
            control.step()?;
            numeric::computed_finite_quantity(sum.get() + row[diagonal], "trace()")
                .map_err(LinearAlgebraFailure::from)
        })
        .map(RuntimeValue::Quantity)
}

fn evaluate_norm(
    arguments: Vec<RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Norm;
    let vector = Vector::from_value(&one_argument(function, arguments)?, "norm")?;
    let mut control = kernel_control(function, &[vector.axis.len()], 1, ctx)?;
    norm(&vector.values, &mut control).map(RuntimeValue::Quantity)
}

fn evaluate_cross(
    arguments: Vec<RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Cross;
    let [lhs, rhs] = two_arguments(function, arguments)?;
    let lhs = Vector::from_value(&lhs, "cross")?;
    let rhs = Vector::from_value(&rhs, "cross")?;
    require_matching_axes(function, &lhs.axis, &rhs.axis)?;
    let (Ok(a), Ok(b)) = (
        <[f64; 3]>::try_from(lhs.values.as_slice()),
        <[f64; 3]>::try_from(rhs.values.as_slice()),
    ) else {
        return Err(OperandInvariant::AxisMismatch { function }.into());
    };
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
    arguments: Vec<RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Outer;
    let [lhs, rhs] = two_arguments(function, arguments)?;
    let lhs = Vector::from_value(&lhs, "outer")?;
    let rhs = Vector::from_value(&rhs, "outer")?;
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
    arguments: Vec<RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Solve;
    let [matrix, rhs] = two_arguments(function, arguments)?;
    let matrix = Matrix::from_value(&matrix, "solve")?;
    let rhs = Vector::from_value(&rhs, "solve")?;
    require_matching_axes(function, &matrix.rows, &matrix.columns)?;
    require_matching_axes(function, &matrix.rows, &rhs.axis)?;
    let mut control = kernel_control(function, &[matrix.rows.len(); 3], 2, ctx)?;
    let solution = super::linear_algebra_lu::solve_with_control(
        &matrix.values,
        matrix.rows.len(),
        &rhs.values,
        &mut control,
    )
    .map_err(algorithm_failure)?;
    vector_value(matrix.rows, solution)
}

fn evaluate_inverse(
    arguments: Vec<RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Inverse;
    let matrix = Matrix::from_value(&one_argument(function, arguments)?, "inverse")?;
    require_matching_axes(function, &matrix.rows, &matrix.columns)?;
    let mut control = kernel_control(function, &[matrix.rows.len(); 3], 3, ctx)?;
    let inverse = super::linear_algebra_lu::inverse_with_control(
        &matrix.values,
        matrix.rows.len(),
        &mut control,
    )
    .map_err(algorithm_failure)?;
    matrix_value(matrix.rows, matrix.columns, inverse)
}

fn evaluate_determinant(
    arguments: Vec<RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    let function = LinearAlgebraFn::Determinant;
    let matrix = Matrix::from_value(&one_argument(function, arguments)?, "det")?;
    require_matching_axes(function, &matrix.rows, &matrix.columns)?;
    let mut control = kernel_control(function, &[matrix.rows.len(); 3], 1, ctx)?;
    super::linear_algebra_lu::determinant_with_control(
        &matrix.values,
        matrix.rows.len(),
        &mut control,
    )
    .map_err(algorithm_failure)
    .and_then(|value| finite_runtime_quantity(value, "det()"))
}

/// Evaluate a shape-checked built-in linear-algebra operation.
pub(super) fn evaluate(
    function: LinearAlgebraFn,
    arguments: Vec<RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, LinearAlgebraFailure> {
    match function {
        LinearAlgebraFn::Dot => evaluate_dot(arguments, ctx),
        LinearAlgebraFn::Matmul => evaluate_matmul(arguments, ctx),
        LinearAlgebraFn::Transpose => evaluate_transpose(arguments, ctx),
        LinearAlgebraFn::Trace => evaluate_trace(arguments, ctx),
        LinearAlgebraFn::Norm => evaluate_norm(arguments, ctx),
        LinearAlgebraFn::Cross => evaluate_cross(arguments, ctx),
        LinearAlgebraFn::Outer => evaluate_outer(arguments, ctx),
        LinearAlgebraFn::Solve => evaluate_solve(arguments, ctx),
        LinearAlgebraFn::Inverse => evaluate_inverse(arguments, ctx),
        LinearAlgebraFn::Determinant => evaluate_determinant(arguments, ctx),
    }
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::registry::index::FiniteIndex;
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

        let parsed = Vector::from_value(&row, "test").unwrap();
        assert_eq!(parsed.values, vec![1.0, 2.0]);
        assert!(parsed.axis.matches(&fin(2)));
        assert!(matches!(
            Vector::from_value(&matrix, "test"),
            Err(OperandInvariant::Rank {
                expected: 1,
                actual: 2,
                ..
            })
        ));

        let parsed = Matrix::from_value(&matrix, "test").unwrap();
        assert!(parsed.rows.matches(&fin(3)));
        assert!(parsed.columns.matches(&fin(2)));
        assert_eq!(
            parsed.row_slices(),
            vec![&[1.0, 2.0][..], &[3.0, 4.0], &[5.0, 6.0]]
        );
        assert!(matches!(
            Matrix::from_value(&row, "test"),
            Err(OperandInvariant::Rank {
                expected: 2,
                actual: 1,
                ..
            })
        ));
        assert!(matches!(
            Vector::from_value(&quantity(1.0), "test"),
            Err(OperandInvariant::NotIndexed { .. })
        ));
    }

    #[test]
    fn ragged_and_non_quantity_operands_are_invariant_failures() {
        let ragged = RuntimeValue::Indexed(IndexedValue::for_test(
            fin(2),
            vec![vector(&[1.0, 2.0]), vector(&[3.0])],
        ));
        assert!(matches!(
            Matrix::from_value(&ragged, "test"),
            Err(OperandInvariant::Ragged { .. })
        ));
        let booleans = RuntimeValue::Indexed(IndexedValue::for_test(
            fin(1),
            vec![RuntimeValue::Bool(true)],
        ));
        assert!(matches!(
            Vector::from_value(&booleans, "test"),
            Err(OperandInvariant::Element(_))
        ));
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
        assert!(matches!(
            require_matching_axes(LinearAlgebraFn::Dot, &fin(2), &fin(3)),
            Err(OperandInvariant::AxisMismatch { .. })
        ));
        assert!(matches!(
            two_arguments(LinearAlgebraFn::Dot, vec![quantity(1.0)]),
            Err(OperandInvariant::Arity { received: 1, .. })
        ));
    }
}
