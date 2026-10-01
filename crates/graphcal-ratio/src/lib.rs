//! Reduced rational numbers for Graphcal dimension exponents.
//!
//! [`Ratio<T>`] is the single rational implementation shared by the
//! compiler's dimension exponents (`Ratio<i32>`, `dimension::Rational`), its
//! exact source-level power exponents (`Ratio<i64>`,
//! `exact_rational::ExactRational`), and the dimension exponents the plugin
//! SDK's macros compute at expansion time. It is a crate of its own so the
//! plugin SDK can share it without depending on the compiler.

mod ratio;
mod real_power;

pub use ratio::{ExponentStyle, Ratio, RatioError, RatioInt};
pub use real_power::ExactPowerError;
