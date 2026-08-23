//! Pure validation and typed decoding for values crossing the host-function ABI.
//!
//! The wire representation deliberately uses `f64` slots for quantities,
//! `Int`, and `Bool`. This module is the single boundary at which those raw
//! slots acquire Graphcal semantics. Embedders such as the CLI plugin tester
//! and the evaluator must use these functions rather than interpreting slots
//! independently.

use std::fmt;

use graphcal_compiler::function_signature::{
    DimMonomial, ScalarValueKind, StructFieldKind, ValueKind,
};
use graphcal_compiler::syntax::index_name::IndexVarName;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::syntax::type_name::FieldName;
use thiserror::Error;

use crate::host_fns::HostFnValue;

/// Why one raw scalar cannot represent its declared ABI kind.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum HostScalarError {
    /// A Graphcal `Int` argument cannot cross binary64 without loss.
    #[error("integer {value} cannot be represented exactly as binary64")]
    InexactIntArgument {
        /// Integer that could not be encoded.
        value: i64,
    },
    /// A quantity slot must never carry NaN or infinity.
    #[error("quantity must be finite, got {value}")]
    NonFiniteQuantity {
        /// Invalid raw quantity.
        value: f64,
    },
    /// An `Int` result slot must be finite, integral, and in the `i64` range.
    #[error("Int slot {value} {reason}")]
    InvalidInt {
        /// Invalid raw slot.
        value: f64,
        /// Specific failed integer invariant.
        reason: InvalidIntReason,
    },
    /// A `Bool` result slot has exactly two numeric encodings.
    #[error("Bool slot must be 0.0 or 1.0, got {value}")]
    InvalidBool {
        /// Invalid raw slot.
        value: f64,
    },
}

/// Which invariant prevented a raw binary64 slot from becoming an `Int`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidIntReason {
    /// NaN and infinities are not integers.
    NonFinite,
    /// The value has a fractional component.
    NonInteger,
    /// The integral value falls outside the `i64` domain.
    OutOfRange,
}

impl fmt::Display for InvalidIntReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NonFinite => "is not finite",
            Self::NonInteger => "is not integer-valued",
            Self::OutOfRange => "is outside the representable i64 range",
        })
    }
}

/// A quantity already proven finite at the host ABI boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FiniteHostQuantity(f64);

impl FiniteHostQuantity {
    /// Recover the finite binary64 payload.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

/// Encode a Graphcal `Bool` for the host ABI.
#[must_use]
pub const fn encode_bool(value: bool) -> f64 {
    if value { 1.0 } else { 0.0 }
}

/// Encode a Graphcal `Int` only when the evaluator's binary64 ABI policy
/// permits a lossless conversion.
///
/// # Errors
///
/// Returns [`HostScalarError::InexactIntArgument`] for unsupported magnitudes.
pub fn encode_int(value: i64) -> Result<f64, HostScalarError> {
    crate::eval_expr::numeric::exact_i64_to_f64(value)
        .map_err(|_| HostScalarError::InexactIntArgument { value })
}

/// Validate a quantity entering or leaving the host ABI.
///
/// # Errors
///
/// Returns [`HostScalarError::NonFiniteQuantity`] for NaN and infinities.
pub fn validate_quantity(value: f64) -> Result<FiniteHostQuantity, HostScalarError> {
    value
        .is_finite()
        .then_some(FiniteHostQuantity(value))
        .ok_or(HostScalarError::NonFiniteQuantity { value })
}

/// Decode an ABI `Bool` slot. Numeric equality intentionally treats `-0.0`
/// as the valid false encoding, matching Graphcal's numeric conversion policy.
///
/// # Errors
///
/// Returns [`HostScalarError::InvalidBool`] for every value other than numeric
/// zero and one.
pub fn decode_bool(value: f64) -> Result<bool, HostScalarError> {
    #[expect(
        clippy::float_cmp,
        reason = "the Bool ABI has exact numeric encodings and deliberately accepts signed zero"
    )]
    if value == 0.0 {
        Ok(false)
    } else if value == 1.0 {
        Ok(true)
    } else {
        Err(HostScalarError::InvalidBool { value })
    }
}

/// Decode a finite, integral, in-range ABI slot as a Graphcal `Int`.
/// Numeric equality intentionally accepts `-0.0` as integer zero.
///
/// # Errors
///
/// Returns [`HostScalarError::InvalidInt`] with the failed invariant.
pub fn decode_int(value: f64) -> Result<i64, HostScalarError> {
    if !value.is_finite() {
        return Err(HostScalarError::InvalidInt {
            value,
            reason: InvalidIntReason::NonFinite,
        });
    }

    let lower_inclusive = -2.0_f64.powi(63);
    let upper_exclusive = 2.0_f64.powi(63);
    if value < lower_inclusive || value >= upper_exclusive {
        return Err(HostScalarError::InvalidInt {
            value,
            reason: InvalidIntReason::OutOfRange,
        });
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "the following binary64 round trip rejects truncated values"
    )]
    let converted = value as i64;
    #[expect(
        clippy::cast_precision_loss,
        reason = "the binary64 round trip is the exactness check"
    )]
    let round_trip = converted as f64;
    #[expect(
        clippy::float_cmp,
        reason = "Int decoding requires exact equality and deliberately accepts signed zero"
    )]
    if round_trip != value {
        return Err(HostScalarError::InvalidInt {
            value,
            reason: InvalidIntReason::NonInteger,
        });
    }

    Ok(converted)
}

/// A typed location for an invalid slot in a composite ABI result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostSlotLocation {
    /// The sole slot of a scalar result.
    Scalar,
    /// One row-major element of an array result.
    ArrayElement {
        /// Zero-based flat element index.
        index: usize,
    },
    /// One named record field.
    StructField {
        /// Declared field identity.
        name: FieldName,
    },
}

impl fmt::Display for HostSlotLocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scalar => formatter.write_str("scalar result"),
            Self::ArrayElement { index } => write!(formatter, "array element #{index}"),
            Self::StructField { name } => write!(formatter, "field `{name}`"),
        }
    }
}

/// Why a host result cannot inhabit the result kind declared by its signature.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum HostResultDecodeError {
    /// A scalar declaration received an array or record.
    #[error("declared a single-value result but returned a composite value")]
    ExpectedScalar,
    /// An array declaration received a scalar or record.
    #[error("declared an array result but returned a non-array value")]
    ExpectedArray,
    /// A struct declaration received a scalar or array.
    #[error("declared a struct result but returned a non-record value")]
    ExpectedRecord,
    /// The returned array rank differs from the signature rank.
    #[error("returned array rank {actual}, expected {expected}")]
    ArrayRankMismatch {
        /// Declared rank.
        expected: usize,
        /// Returned rank.
        actual: usize,
    },
    /// The returned record slot count differs from its declared field count.
    #[error("returned {actual} record slot(s), expected {expected}")]
    RecordArityMismatch {
        /// Declared field count.
        expected: usize,
        /// Returned slot count.
        actual: usize,
    },
    /// One slot violates the scalar policy for its declared kind.
    #[error("invalid {location}: {source}")]
    InvalidSlot {
        /// Semantic slot location.
        location: HostSlotLocation,
        /// Failed scalar invariant.
        #[source]
        source: HostScalarError,
    },
}

/// Typed, validated row-major values of a host array.
///
/// The quantity variant carries its dimension monomial, so the semantic kind
/// and payload cannot disagree.
#[derive(Debug, Clone, PartialEq)]
pub enum ValidatedHostArrayValues {
    /// Finite quantity elements.
    Quantity {
        /// Declared element dimension.
        element: DimMonomial,
        /// Finite SI values.
        values: Vec<FiniteHostQuantity>,
    },
    /// Boolean elements.
    Bool(Vec<bool>),
    /// Integer elements.
    Int(Vec<i64>),
}

/// A validated dense scalar array plus the typed result metadata needed by
/// consumers to rebuild or render it.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedHostArray {
    indexes: NonEmpty<IndexVarName>,
    shape: Vec<usize>,
    values: ValidatedHostArrayValues,
}

impl ValidatedHostArray {
    /// Declared index variables in row-major axis order.
    #[must_use]
    pub const fn indexes(&self) -> &NonEmpty<IndexVarName> {
        &self.indexes
    }

    /// Returned axis extents.
    #[must_use]
    pub fn shape(&self) -> &[usize] {
        &self.shape
    }

    /// Typed row-major element values.
    #[must_use]
    pub const fn values(&self) -> &ValidatedHostArrayValues {
        &self.values
    }
}

/// One validated struct field value.
#[derive(Debug, Clone, PartialEq)]
pub enum ValidatedHostFieldValue {
    /// Boolean field.
    Bool(bool),
    /// Integer field.
    Int(i64),
    /// Finite quantity field.
    Quantity(FiniteHostQuantity),
}

/// One named field in a validated host record.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedHostField {
    name: FieldName,
    value: ValidatedHostFieldValue,
}

impl ValidatedHostField {
    /// Declared field identity.
    #[must_use]
    pub const fn name(&self) -> &FieldName {
        &self.name
    }

    /// Decoded field value.
    #[must_use]
    pub const fn value(&self) -> &ValidatedHostFieldValue {
        &self.value
    }
}

/// A host-function result proven to match its declared ABI kind.
#[derive(Debug, Clone, PartialEq)]
pub enum ValidatedHostResult {
    /// Boolean result.
    Bool(bool),
    /// Integer result.
    Int(i64),
    /// Finite quantity result with its declared dimension expression.
    Quantity {
        /// Declared result dimension.
        dimension: DimMonomial,
        /// Finite SI value.
        value: FiniteHostQuantity,
    },
    /// Dense typed scalar array.
    Array(ValidatedHostArray),
    /// Fixed-layout record whose fields are decoded by declared kind.
    Struct(Vec<ValidatedHostField>),
}

/// Validate and decode a raw host-function result according to one declared
/// result kind.
///
/// # Errors
///
/// Returns [`HostResultDecodeError`] for a wrong wire shape, rank/arity
/// mismatch, non-finite quantity, invalid `Int`, or invalid `Bool` encoding.
pub fn decode_result(
    declared: &ValueKind,
    raw: &HostFnValue,
) -> Result<ValidatedHostResult, HostResultDecodeError> {
    match declared {
        ValueKind::Scalar(ScalarValueKind::Bool) => scalar_slot(raw).and_then(|value| {
            decode_bool(value)
                .map(ValidatedHostResult::Bool)
                .map_err(|source| invalid_slot(HostSlotLocation::Scalar, source))
        }),
        ValueKind::Scalar(ScalarValueKind::Int) => scalar_slot(raw).and_then(|value| {
            decode_int(value)
                .map(ValidatedHostResult::Int)
                .map_err(|source| invalid_slot(HostSlotLocation::Scalar, source))
        }),
        ValueKind::Scalar(ScalarValueKind::Quantity(dimension)) => {
            scalar_slot(raw).and_then(|value| {
                validate_quantity(value)
                    .map(|value| ValidatedHostResult::Quantity {
                        dimension: dimension.clone(),
                        value,
                    })
                    .map_err(|source| invalid_slot(HostSlotLocation::Scalar, source))
            })
        }
        ValueKind::Indexed { element, indexes } => {
            let HostFnValue::Array(array) = raw else {
                return Err(HostResultDecodeError::ExpectedArray);
            };
            if array.shape().len() != indexes.len() {
                return Err(HostResultDecodeError::ArrayRankMismatch {
                    expected: indexes.len(),
                    actual: array.shape().len(),
                });
            }
            let values = match element {
                ScalarValueKind::Quantity(monomial) => ValidatedHostArrayValues::Quantity {
                    element: monomial.clone(),
                    values: decode_array_values(array.values(), validate_quantity)?,
                },
                ScalarValueKind::Bool => ValidatedHostArrayValues::Bool(decode_array_values(
                    array.values(),
                    decode_bool,
                )?),
                ScalarValueKind::Int => {
                    ValidatedHostArrayValues::Int(decode_array_values(array.values(), decode_int)?)
                }
            };
            Ok(ValidatedHostResult::Array(ValidatedHostArray {
                indexes: indexes.clone(),
                shape: array.shape().to_vec(),
                values,
            }))
        }
        ValueKind::Struct(shape) => {
            let HostFnValue::Record(slots) = raw else {
                return Err(HostResultDecodeError::ExpectedRecord);
            };
            if slots.len() != shape.fields().len() {
                return Err(HostResultDecodeError::RecordArityMismatch {
                    expected: shape.fields().len(),
                    actual: slots.len(),
                });
            }
            let fields = shape
                .fields()
                .iter()
                .zip(slots)
                .map(|(field, slot)| {
                    let location = HostSlotLocation::StructField {
                        name: field.name.clone(),
                    };
                    let value = match &field.kind {
                        StructFieldKind::Bool => decode_bool(*slot)
                            .map(ValidatedHostFieldValue::Bool)
                            .map_err(|source| invalid_slot(location, source))?,
                        StructFieldKind::Int => decode_int(*slot)
                            .map(ValidatedHostFieldValue::Int)
                            .map_err(|source| invalid_slot(location, source))?,
                        StructFieldKind::Quantity(_) => validate_quantity(*slot)
                            .map(ValidatedHostFieldValue::Quantity)
                            .map_err(|source| invalid_slot(location, source))?,
                    };
                    Ok(ValidatedHostField {
                        name: field.name.clone(),
                        value,
                    })
                })
                .collect::<Result<Vec<_>, HostResultDecodeError>>()?;
            Ok(ValidatedHostResult::Struct(fields))
        }
    }
}

fn decode_array_values<T>(
    raw: &[f64],
    decode: impl Fn(f64) -> Result<T, HostScalarError>,
) -> Result<Vec<T>, HostResultDecodeError> {
    raw.iter()
        .copied()
        .enumerate()
        .map(|(index, value)| {
            decode(value)
                .map_err(|source| invalid_slot(HostSlotLocation::ArrayElement { index }, source))
        })
        .collect()
}

const fn scalar_slot(raw: &HostFnValue) -> Result<f64, HostResultDecodeError> {
    match raw {
        HostFnValue::F64(value) => Ok(*value),
        HostFnValue::Array(_) | HostFnValue::Record(_) => {
            Err(HostResultDecodeError::ExpectedScalar)
        }
    }
}

const fn invalid_slot(
    location: HostSlotLocation,
    source: HostScalarError,
) -> HostResultDecodeError {
    HostResultDecodeError::InvalidSlot { location, source }
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::dimension::Dimension;
    use graphcal_compiler::function_signature::{DimMonomial, StructShape, StructShapeField};

    use super::*;
    use crate::host_fns::HostArray;

    #[test]
    fn scalar_policy_accepts_signed_zero_and_exact_sparse_integers() {
        assert_eq!(decode_bool(-0.0), Ok(false));
        assert_eq!(decode_int(-0.0), Ok(0));
        assert_eq!(
            validate_quantity(-0.0).map(FiniteHostQuantity::get),
            Ok(-0.0)
        );
        assert_eq!(encode_int(1_i64 << 54), Ok(2.0_f64.powi(54)));
        assert!(encode_int((1_i64 << 53) + 1).is_err());
        assert!(encode_int(i64::MAX).is_err());
    }

    #[test]
    fn scalar_policy_rejects_non_finite_fractional_and_out_of_range_values() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(matches!(
                validate_quantity(value),
                Err(HostScalarError::NonFiniteQuantity { .. })
            ));
            assert!(matches!(
                decode_int(value),
                Err(HostScalarError::InvalidInt { .. })
            ));
            assert!(decode_bool(value).is_err());
        }
        assert!(matches!(
            decode_int(0.5),
            Err(HostScalarError::InvalidInt {
                reason: InvalidIntReason::NonInteger,
                ..
            })
        ));
        assert!(matches!(
            decode_int(2.0_f64.powi(63)),
            Err(HostScalarError::InvalidInt {
                reason: InvalidIntReason::OutOfRange,
                ..
            })
        ));
    }

    fn indexed_kind(element: ScalarValueKind) -> ValueKind {
        ValueKind::Indexed {
            element,
            indexes: NonEmpty::new(IndexVarName::expect_valid("I"), Vec::new()),
        }
    }

    #[test]
    fn bool_and_int_arrays_decode_with_scalar_policy_and_element_locations() {
        let bool_kind = indexed_kind(ScalarValueKind::Bool);
        let bools = HostFnValue::Array(HostArray::try_new(vec![3], vec![0.0, -0.0, 1.0]).unwrap());
        assert!(matches!(
            decode_result(&bool_kind, &bools),
            Ok(ValidatedHostResult::Array(ValidatedHostArray {
                values: ValidatedHostArrayValues::Bool(values),
                ..
            })) if values == [false, false, true]
        ));
        for invalid in [0.5, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let raw = HostFnValue::Array(HostArray::vector(vec![0.0, invalid]).unwrap());
            assert!(matches!(
                decode_result(&bool_kind, &raw),
                Err(HostResultDecodeError::InvalidSlot {
                    location: HostSlotLocation::ArrayElement { index: 1 },
                    source: HostScalarError::InvalidBool { .. },
                })
            ));
        }

        let int_kind = indexed_kind(ScalarValueKind::Int);
        let ints =
            HostFnValue::Array(HostArray::vector(vec![-0.0, 2.0_f64.powi(54), -3.0]).unwrap());
        assert!(matches!(
            decode_result(&int_kind, &ints),
            Ok(ValidatedHostResult::Array(ValidatedHostArray {
                values: ValidatedHostArrayValues::Int(values),
                ..
            })) if values == [0, 1_i64 << 54, -3]
        ));
        for invalid in [0.5, f64::NAN, f64::INFINITY, 2.0_f64.powi(63)] {
            let raw = HostFnValue::Array(HostArray::vector(vec![1.0, invalid]).unwrap());
            assert!(matches!(
                decode_result(&int_kind, &raw),
                Err(HostResultDecodeError::InvalidSlot {
                    location: HostSlotLocation::ArrayElement { index: 1 },
                    source: HostScalarError::InvalidInt { .. },
                })
            ));
        }
    }

    #[test]
    fn composite_results_validate_every_quantity_slot() {
        let indexes = NonEmpty::new(IndexVarName::expect_valid("I"), Vec::new());
        let array_kind = ValueKind::Indexed {
            element: ScalarValueKind::Quantity(DimMonomial::fixed(Dimension::dimensionless())),
            indexes,
        };
        let array =
            HostFnValue::Array(HostArray::try_new(vec![2], vec![1.0, f64::INFINITY]).unwrap());
        assert!(matches!(
            decode_result(&array_kind, &array),
            Err(HostResultDecodeError::InvalidSlot {
                location: HostSlotLocation::ArrayElement { index: 1 },
                source: HostScalarError::NonFiniteQuantity { .. }
            })
        ));

        let shape = StructShape::try_new(vec![StructShapeField {
            name: FieldName::expect_valid("quantity"),
            kind: StructFieldKind::Quantity(Dimension::dimensionless()),
        }])
        .unwrap();
        assert!(matches!(
            decode_result(
                &ValueKind::Struct(shape),
                &HostFnValue::Record(vec![f64::NAN])
            ),
            Err(HostResultDecodeError::InvalidSlot {
                location: HostSlotLocation::StructField { .. },
                source: HostScalarError::NonFiniteQuantity { .. }
            })
        ));
    }
}
