//! The binary64 slot policy of the host-function ABI for one scalar, and the
//! validated scalar arguments built from it.
//!
//! Quantities, `Int`, and `Bool` each cross the ABI in one `f64` slot. This
//! module is the single boundary at which a raw slot acquires Graphcal
//! semantics, and the only constructor of the validated [`HostScalar`] a host
//! function receives.

use std::fmt;

use graphcal_compiler::finite_value::FiniteQuantity;
use graphcal_compiler::function_signature::ScalarValueKind;
use thiserror::Error;

use crate::eval_expr::numeric::exact_i64_to_f64;

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
    exact_i64_to_f64(value).map_err(|_| HostScalarError::InexactIntArgument { value })
}

/// Validate a quantity entering or leaving the host ABI.
///
/// # Errors
///
/// Returns [`HostScalarError::NonFiniteQuantity`] for NaN and infinities.
pub fn validate_quantity(value: f64) -> Result<FiniteQuantity, HostScalarError> {
    FiniteQuantity::try_new(value).map_err(|_| HostScalarError::NonFiniteQuantity { value })
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

/// A Graphcal `Int` whose value crosses the binary64 ABI slot exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HostInt {
    value: i64,
    slot: f64,
}

impl HostInt {
    /// Validate that `value` is representable exactly as binary64.
    ///
    /// # Errors
    ///
    /// Returns [`HostScalarError::InexactIntArgument`] for unsupported
    /// magnitudes.
    pub fn try_new(value: i64) -> Result<Self, HostScalarError> {
        encode_int(value).map(|slot| Self { value, slot })
    }

    /// The integer.
    #[cfg(test)]
    #[must_use]
    const fn get(self) -> i64 {
        self.value
    }

    /// Its exact binary64 ABI slot.
    #[must_use]
    pub(crate) const fn abi_slot(self) -> f64 {
        self.slot
    }
}

/// One validated scalar argument of a host function.
///
/// Every variant can only hold a value its ABI slot represents: a finite
/// quantity, a Boolean, or an exactly representable integer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HostScalar {
    /// A finite SI quantity.
    Quantity(FiniteQuantity),
    /// A Boolean.
    Bool(bool),
    /// An integer representable exactly as binary64.
    Int(HostInt),
}

impl HostScalar {
    /// The raw binary64 ABI slot encoding this scalar.
    #[must_use]
    pub const fn abi_slot(self) -> f64 {
        match self {
            Self::Quantity(value) => value.get(),
            Self::Bool(value) => encode_bool(value),
            Self::Int(value) => value.abi_slot(),
        }
    }

    /// Whether this scalar has the semantic kind `kind` declares.
    #[must_use]
    pub const fn has_kind<D>(self, kind: &ScalarValueKind<D>) -> bool {
        matches!(
            (self, kind),
            (Self::Quantity(_), ScalarValueKind::Quantity(_))
                | (Self::Bool(_), ScalarValueKind::Bool)
                | (Self::Int(_), ScalarValueKind::Int)
        )
    }
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::dimension::Dimension;
    use graphcal_compiler::function_signature::DimMonomial;
    use graphcal_compiler::syntax::dimension::DimVarName;

    use super::*;

    fn quantity_kind() -> ScalarValueKind<DimVarName> {
        ScalarValueKind::Quantity(DimMonomial::fixed(Dimension::dimensionless()))
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "ABI slots encode these values exactly")]
    fn validated_scalars_encode_their_exact_slots() {
        let large = HostInt::try_new(1_i64 << 54).unwrap();
        assert_eq!(
            (large.get(), large.abi_slot()),
            (1_i64 << 54, 2.0_f64.powi(54))
        );
        assert!(matches!(
            HostInt::try_new((1_i64 << 53) + 1),
            Err(HostScalarError::InexactIntArgument { .. })
        ));
        assert_eq!(
            HostScalar::Int(HostInt::try_new(-3).unwrap()).abi_slot(),
            -3.0
        );
        assert_eq!(HostScalar::Bool(true).abi_slot(), 1.0);
        assert_eq!(HostScalar::Bool(false).abi_slot(), 0.0);
        let quantity = FiniteQuantity::try_new(2.5).unwrap();
        assert_eq!(HostScalar::Quantity(quantity).abi_slot(), 2.5);
    }

    #[test]
    fn scalars_have_only_their_own_kind() {
        let quantity = HostScalar::Quantity(FiniteQuantity::try_new(1.0).unwrap());
        let int = HostScalar::Int(HostInt::try_new(1).unwrap());
        let flag = HostScalar::Bool(true);
        let kinds = [quantity_kind(), ScalarValueKind::Int, ScalarValueKind::Bool];
        for (scalar, own) in [(quantity, 0), (int, 1), (flag, 2)] {
            for (position, kind) in kinds.iter().enumerate() {
                assert_eq!(
                    scalar.has_kind(kind),
                    position == own,
                    "{scalar:?} {kind:?}"
                );
            }
        }
    }
}
