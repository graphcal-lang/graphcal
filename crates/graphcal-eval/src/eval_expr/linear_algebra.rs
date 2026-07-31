//! Pure dense kernels for built-in linear algebra.
//!
//! Indexed runtime values are converted to explicit vector/matrix carriers,
//! kernels operate on row-major `f64` buffers, and results are rebuilt with the
//! exact typed axis identities and key order supplied by the arguments.

use graphcal_compiler::builtin::LinearAlgebraFn;
use graphcal_compiler::registry::declared_type::IndexTypeRef;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use indexmap::IndexMap;
use thiserror::Error;

use super::numeric::{self, QuantityValidationError};

#[derive(Debug, Clone)]
struct Axis {
    index_name: IndexTypeRef,
    keys: Vec<IndexEntryKey>,
}

impl Axis {
    const fn len(&self) -> usize {
        self.keys.len()
    }

    fn matches(&self, other: &Self) -> bool {
        self.index_name.matches_ref(&other.index_name) && self.keys == other.keys
    }
}

#[derive(Debug)]
struct Vector {
    axis: Axis,
    values: Vec<f64>,
}

#[derive(Debug)]
struct Matrix {
    rows: Axis,
    columns: Axis,
    values: Vec<f64>,
}

#[derive(Debug, Error)]
pub(super) enum LinearAlgebraError {
    #[error("linear-algebra runtime shape invariant failed: {0}")]
    ShapeInvariant(String),
    #[error(transparent)]
    Numeric(#[from] QuantityValidationError),
}

impl LinearAlgebraError {
    pub(super) const fn is_internal_invariant(&self) -> bool {
        matches!(self, Self::ShapeInvariant(_))
    }
}

fn vector_from_value(
    value: RuntimeValue,
    context: &'static str,
) -> Result<Vector, LinearAlgebraError> {
    let RuntimeValue::Indexed {
        index_name,
        entries,
    } = value
    else {
        return Err(LinearAlgebraError::ShapeInvariant(format!(
            "{context} expected a vector"
        )));
    };
    let (keys, values) = entries
        .into_iter()
        .map(|(key, value)| {
            value
                .expect_quantity(context)
                .map(|quantity| (key, quantity))
                .map_err(|error| LinearAlgebraError::ShapeInvariant(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .unzip();
    Ok(Vector {
        axis: Axis { index_name, keys },
        values,
    })
}

fn matrix_from_value(
    value: RuntimeValue,
    context: &'static str,
) -> Result<Matrix, LinearAlgebraError> {
    let RuntimeValue::Indexed {
        index_name,
        entries,
    } = value
    else {
        return Err(LinearAlgebraError::ShapeInvariant(format!(
            "{context} expected a matrix"
        )));
    };
    let row_keys = entries.keys().cloned().collect::<Vec<_>>();
    let mut rows = entries.into_values();
    let first = rows.next().ok_or_else(|| {
        LinearAlgebraError::ShapeInvariant(format!("{context} received an empty matrix"))
    })?;
    let first_row = vector_from_value(first, context)?;
    if first_row.axis.len() == 0 {
        return Err(LinearAlgebraError::ShapeInvariant(format!(
            "{context} received a matrix with an empty column axis"
        )));
    }
    let columns = first_row.axis;
    let mut values = first_row.values;
    for row in rows {
        let row = vector_from_value(row, context)?;
        if !columns.matches(&row.axis) {
            return Err(LinearAlgebraError::ShapeInvariant(format!(
                "{context} received a ragged matrix"
            )));
        }
        values.extend(row.values);
    }
    Ok(Matrix {
        rows: Axis {
            index_name,
            keys: row_keys,
        },
        columns,
        values,
    })
}

fn vector_value(axis: Axis, values: Vec<f64>) -> Result<RuntimeValue, LinearAlgebraError> {
    if axis.len() != values.len() {
        return Err(LinearAlgebraError::ShapeInvariant(format!(
            "vector result has {} values for an axis with {} keys",
            values.len(),
            axis.len()
        )));
    }
    Ok(RuntimeValue::Indexed {
        index_name: axis.index_name,
        entries: axis
            .keys
            .into_iter()
            .zip(values.into_iter().map(RuntimeValue::Quantity))
            .collect(),
    })
}

fn matrix_value(
    rows: Axis,
    columns: Axis,
    values: Vec<f64>,
) -> Result<RuntimeValue, LinearAlgebraError> {
    if rows.len() == 0 || columns.len() == 0 {
        return Err(LinearAlgebraError::ShapeInvariant(
            "matrix result axes must be nonempty".to_string(),
        ));
    }
    let expected = rows.len().checked_mul(columns.len()).ok_or_else(|| {
        LinearAlgebraError::ShapeInvariant("matrix result cardinality overflowed".to_string())
    })?;
    if expected != values.len() {
        return Err(LinearAlgebraError::ShapeInvariant(format!(
            "matrix result has {} values for shape {} x {}",
            values.len(),
            rows.len(),
            columns.len()
        )));
    }
    let Axis {
        index_name: column_index_name,
        keys: column_keys,
    } = columns;
    let mut values = values.into_iter();
    let row_entries = rows
        .keys
        .into_iter()
        .map(|row_key| {
            let entries = column_keys
                .iter()
                .cloned()
                .zip(
                    values
                        .by_ref()
                        .take(column_keys.len())
                        .map(RuntimeValue::Quantity),
                )
                .collect::<IndexMap<_, _>>();
            (
                row_key,
                RuntimeValue::Indexed {
                    index_name: column_index_name.clone(),
                    entries,
                },
            )
        })
        .collect();
    Ok(RuntimeValue::Indexed {
        index_name: rows.index_name,
        entries: row_entries,
    })
}

fn finite_product(lhs: f64, rhs: f64, context: &'static str) -> Result<f64, LinearAlgebraError> {
    numeric::computed_finite_quantity(lhs * rhs, context).map_err(LinearAlgebraError::from)
}

fn sum_products(
    lhs: impl IntoIterator<Item = f64>,
    rhs: impl IntoIterator<Item = f64>,
    context: &'static str,
) -> Result<f64, LinearAlgebraError> {
    lhs.into_iter().zip(rhs).try_fold(0.0, |sum, (lhs, rhs)| {
        let product = finite_product(lhs, rhs, context)?;
        numeric::computed_finite_quantity(sum + product, context).map_err(LinearAlgebraError::from)
    })
}

fn norm(values: &[f64]) -> Result<f64, LinearAlgebraError> {
    // LAPACK-style scaled sum of squares avoids intermediate overflow and
    // underflow while preserving the ordinary Euclidean norm.
    let (scale, sum_squares) = values.iter().copied().filter(|value| *value != 0.0).fold(
        (0.0_f64, 1.0_f64),
        |(scale, sum_squares), value| {
            let magnitude = value.abs();
            if scale < magnitude {
                let ratio = scale / magnitude;
                (magnitude, (sum_squares * ratio).mul_add(ratio, 1.0))
            } else {
                let ratio = magnitude / scale;
                (scale, ratio.mul_add(ratio, sum_squares))
            }
        },
    );
    let result = if scale == 0.0 {
        0.0
    } else {
        scale * sum_squares.sqrt()
    };
    numeric::computed_finite_quantity(result, "norm()").map_err(LinearAlgebraError::from)
}

fn one_argument(
    function: LinearAlgebraFn,
    arguments: Vec<RuntimeValue>,
) -> Result<RuntimeValue, LinearAlgebraError> {
    <[RuntimeValue; 1]>::try_from(arguments)
        .map(|[argument]| argument)
        .map_err(|arguments| {
            LinearAlgebraError::ShapeInvariant(format!(
                "{}() received {} arguments after type checking",
                function.builtin_name(),
                arguments.len()
            ))
        })
}

fn two_arguments(
    function: LinearAlgebraFn,
    arguments: Vec<RuntimeValue>,
) -> Result<[RuntimeValue; 2], LinearAlgebraError> {
    <[RuntimeValue; 2]>::try_from(arguments).map_err(|arguments| {
        LinearAlgebraError::ShapeInvariant(format!(
            "{}() received {} arguments after type checking",
            function.builtin_name(),
            arguments.len()
        ))
    })
}

fn matrix_element(
    matrix: &Matrix,
    row: usize,
    column: usize,
    context: &'static str,
) -> Result<f64, LinearAlgebraError> {
    let position = row
        .checked_mul(matrix.columns.len())
        .and_then(|offset| offset.checked_add(column))
        .ok_or_else(|| {
            LinearAlgebraError::ShapeInvariant(format!(
                "{context} matrix element position overflowed"
            ))
        })?;
    matrix.values.get(position).copied().ok_or_else(|| {
        LinearAlgebraError::ShapeInvariant(format!(
            "{context} matrix element is outside the dense buffer"
        ))
    })
}

fn vector_element(
    vector: &Vector,
    position: usize,
    context: &'static str,
) -> Result<f64, LinearAlgebraError> {
    vector.values.get(position).copied().ok_or_else(|| {
        LinearAlgebraError::ShapeInvariant(format!(
            "{context} vector element is outside the dense buffer"
        ))
    })
}

fn evaluate_dot(arguments: Vec<RuntimeValue>) -> Result<RuntimeValue, LinearAlgebraError> {
    let function = LinearAlgebraFn::Dot;
    let [lhs, rhs] = two_arguments(function, arguments)?;
    let lhs = vector_from_value(lhs, "dot")?;
    let rhs = vector_from_value(rhs, "dot")?;
    if !lhs.axis.matches(&rhs.axis) {
        return Err(LinearAlgebraError::ShapeInvariant(
            "dot() received vectors over different axes".to_string(),
        ));
    }
    sum_products(lhs.values, rhs.values, "dot()").map(RuntimeValue::Quantity)
}

fn evaluate_matmul(arguments: Vec<RuntimeValue>) -> Result<RuntimeValue, LinearAlgebraError> {
    let function = LinearAlgebraFn::Matmul;
    let [lhs, rhs] = two_arguments(function, arguments)?;
    let lhs = matrix_from_value(lhs, "matmul")?;
    let rhs = matrix_from_value(rhs, "matmul")?;
    if !lhs.columns.matches(&rhs.rows) {
        return Err(LinearAlgebraError::ShapeInvariant(
            "matmul() received matrices with different contraction axes".to_string(),
        ));
    }
    let (rows, inner, columns) = (lhs.rows.len(), lhs.columns.len(), rhs.columns.len());
    let values = (0..rows)
        .flat_map(|row| (0..columns).map(move |column| (row, column)))
        .map(|(row, column)| {
            (0..inner).try_fold(0.0, |sum, position| {
                let lhs = matrix_element(&lhs, row, position, "matmul()")?;
                let rhs = matrix_element(&rhs, position, column, "matmul()")?;
                let product = finite_product(lhs, rhs, "matmul()")?;
                numeric::computed_finite_quantity(sum + product, "matmul()")
                    .map_err(LinearAlgebraError::from)
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    matrix_value(lhs.rows, rhs.columns, values)
}

fn evaluate_transpose(arguments: Vec<RuntimeValue>) -> Result<RuntimeValue, LinearAlgebraError> {
    let function = LinearAlgebraFn::Transpose;
    let matrix = matrix_from_value(one_argument(function, arguments)?, "transpose")?;
    let values = (0..matrix.columns.len())
        .flat_map(|column| (0..matrix.rows.len()).map(move |row| (row, column)))
        .map(|(row, column)| matrix_element(&matrix, row, column, "transpose()"))
        .collect::<Result<Vec<_>, _>>()?;
    matrix_value(matrix.columns, matrix.rows, values)
}

fn evaluate_trace(arguments: Vec<RuntimeValue>) -> Result<RuntimeValue, LinearAlgebraError> {
    let function = LinearAlgebraFn::Trace;
    let matrix = matrix_from_value(one_argument(function, arguments)?, "trace")?;
    if !matrix.rows.matches(&matrix.columns) {
        return Err(LinearAlgebraError::ShapeInvariant(
            "trace() received a matrix with distinct row and column axes".to_string(),
        ));
    }
    (0..matrix.rows.len())
        .try_fold(0.0, |sum, diagonal| {
            let value = matrix_element(&matrix, diagonal, diagonal, "trace()")?;
            numeric::computed_finite_quantity(sum + value, "trace()")
                .map_err(LinearAlgebraError::from)
        })
        .map(RuntimeValue::Quantity)
}

fn evaluate_norm(arguments: Vec<RuntimeValue>) -> Result<RuntimeValue, LinearAlgebraError> {
    let function = LinearAlgebraFn::Norm;
    let vector = vector_from_value(one_argument(function, arguments)?, "norm")?;
    norm(&vector.values).map(RuntimeValue::Quantity)
}

fn evaluate_cross(arguments: Vec<RuntimeValue>) -> Result<RuntimeValue, LinearAlgebraError> {
    let function = LinearAlgebraFn::Cross;
    let [lhs, rhs] = two_arguments(function, arguments)?;
    let lhs = vector_from_value(lhs, "cross")?;
    let rhs = vector_from_value(rhs, "cross")?;
    if !lhs.axis.matches(&rhs.axis) || lhs.axis.len() != 3 {
        return Err(LinearAlgebraError::ShapeInvariant(
            "cross() requires two vectors over the same three-entry axis".to_string(),
        ));
    }
    let component = |a: usize, b: usize, c: usize, d: usize| {
        let positive = finite_product(
            vector_element(&lhs, a, "cross()")?,
            vector_element(&rhs, b, "cross()")?,
            "cross()",
        )?;
        let negative = finite_product(
            vector_element(&lhs, c, "cross()")?,
            vector_element(&rhs, d, "cross()")?,
            "cross()",
        )?;
        numeric::computed_finite_quantity(positive - negative, "cross()")
            .map_err(LinearAlgebraError::from)
    };
    let values = vec![
        component(1, 2, 2, 1)?,
        component(2, 0, 0, 2)?,
        component(0, 1, 1, 0)?,
    ];
    vector_value(lhs.axis, values)
}

fn evaluate_outer(arguments: Vec<RuntimeValue>) -> Result<RuntimeValue, LinearAlgebraError> {
    let function = LinearAlgebraFn::Outer;
    let [lhs, rhs] = two_arguments(function, arguments)?;
    let lhs = vector_from_value(lhs, "outer")?;
    let rhs = vector_from_value(rhs, "outer")?;
    let values = lhs
        .values
        .iter()
        .flat_map(|lhs| rhs.values.iter().map(move |rhs| (*lhs, *rhs)))
        .map(|(lhs, rhs)| finite_product(lhs, rhs, "outer()"))
        .collect::<Result<Vec<_>, _>>()?;
    matrix_value(lhs.axis, rhs.axis, values)
}

/// Evaluate a shape-checked built-in linear-algebra operation.
pub(super) fn evaluate(
    function: LinearAlgebraFn,
    arguments: Vec<RuntimeValue>,
) -> Result<RuntimeValue, LinearAlgebraError> {
    match function {
        LinearAlgebraFn::Dot => evaluate_dot(arguments),
        LinearAlgebraFn::Matmul => evaluate_matmul(arguments),
        LinearAlgebraFn::Transpose => evaluate_transpose(arguments),
        LinearAlgebraFn::Trace => evaluate_trace(arguments),
        LinearAlgebraFn::Norm => evaluate_norm(arguments),
        LinearAlgebraFn::Cross => evaluate_cross(arguments),
        LinearAlgebraFn::Outer => evaluate_outer(arguments),
    }
}
