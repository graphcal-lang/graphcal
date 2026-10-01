//! Exact rational values preserved from source syntax.
//!
//! Value-level power expressions need a wider exact representation than the
//! dimension algebra: an exact exponent can be meaningful for a dimensionless
//! base even when it does not fit the dimension model's `i32` exponents. This
//! type keeps the source value reduced and exact until dimensional analysis
//! decides whether narrowing is required.

use crate::ratio::Ratio;

/// A reduced exact rational with a positive denominator, in the symmetric
/// `i64` range.
pub type ExactRational = Ratio<i64>;
