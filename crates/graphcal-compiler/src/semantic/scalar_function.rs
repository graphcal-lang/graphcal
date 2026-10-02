use std::sync::LazyLock;

use thiserror::Error;

use crate::builtin::ScalarFn;
use crate::dimension::{BaseDimId, Dimension, PreludeBaseDimension, Rational};
use crate::function_signature::{FunctionSignature, QuantitySignature};
use crate::syntax::function_name::FnParamName;

const fn dimensionless() -> Dimension {
    Dimension::dimensionless()
}

fn angle() -> Dimension {
    Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Angle))
}

fn fn_param(name: &str) -> FnParamName {
    FnParamName::expect_valid(name)
}

#[derive(Clone, Copy)]
enum BuiltinKernel {
    Unary(fn(f64) -> f64),
    Binary(fn(f64, f64) -> f64),
    Ternary(fn(f64, f64, f64) -> f64),
}

impl BuiltinKernel {
    const fn arity(self) -> usize {
        match self {
            Self::Unary(_) => 1,
            Self::Binary(_) => 2,
            Self::Ternary(_) => 3,
        }
    }
}

/// A scalar built-in: a private, arity-encoded evaluation kernel paired with
/// its typed, all-quantity [`QuantitySignature`].
pub struct ScalarFunction {
    kernel: BuiltinKernel,
    signature: QuantitySignature,
}

impl ScalarFunction {
    #[expect(
        clippy::expect_used,
        reason = "built-in signature shapes are all-quantity by construction; a failure is a compiler bug caught by tests"
    )]
    fn new(function: ScalarFn, kernel: BuiltinKernel, signature: FunctionSignature) -> Self {
        assert!(
            kernel.arity() == function.arity() && signature.arity() == function.arity(),
            "built-in `{function}` kernel, signature, and static arities must agree"
        );
        let signature = QuantitySignature::try_new(signature)
            .expect("scalar built-in signatures must be all-quantity");
        Self { kernel, signature }
    }

    /// Returns the arity (number of parameters) of this function.
    #[must_use]
    pub const fn arity(&self) -> usize {
        self.kernel.arity()
    }

    /// Returns the function's typed dimension signature.
    #[must_use]
    pub const fn signature(&self) -> &FunctionSignature {
        self.signature.signature()
    }

    /// Returns the function's signature as all-quantity.
    #[must_use]
    pub const fn quantity_signature(&self) -> &QuantitySignature {
        &self.signature
    }

    /// Evaluate the function after checking the argument count.
    ///
    /// # Errors
    ///
    /// Returns [`BuiltinEvalError::WrongArity`] when `args` does not contain
    /// exactly [`Self::arity`] values. The private kernels therefore cannot be
    /// called with a slice shape that would panic on indexing.
    pub fn eval(&self, args: &[f64]) -> Result<f64, BuiltinEvalError> {
        match (self.kernel, args) {
            (BuiltinKernel::Unary(kernel), [arg]) => Ok(kernel(*arg)),
            (BuiltinKernel::Binary(kernel), [lhs, rhs]) => Ok(kernel(*lhs, *rhs)),
            (BuiltinKernel::Ternary(kernel), [first, second, third]) => {
                Ok(kernel(*first, *second, *third))
            }
            (kernel, _) => Err(BuiltinEvalError::WrongArity {
                expected: kernel.arity(),
                actual: args.len(),
            }),
        }
    }
}

/// Failure to invoke a built-in quantity kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum BuiltinEvalError {
    /// The call supplied a different number of values than the kernel accepts.
    #[error("expected {expected} argument(s), got {actual}")]
    WrongArity {
        /// Required number of arguments.
        expected: usize,
        /// Supplied number of arguments.
        actual: usize,
    },
}

/// Evaluation kernel of each scalar built-in.
const fn kernel(function: ScalarFn) -> BuiltinKernel {
    match function {
        ScalarFn::Sqrt => BuiltinKernel::Unary(f64::sqrt),
        ScalarFn::Cbrt => BuiltinKernel::Unary(f64::cbrt),
        ScalarFn::Expm1 => BuiltinKernel::Unary(f64::exp_m1),
        ScalarFn::Ln => BuiltinKernel::Unary(f64::ln),
        ScalarFn::Log10 => BuiltinKernel::Unary(f64::log10),
        ScalarFn::Log2 => BuiltinKernel::Unary(f64::log2),
        ScalarFn::Log => BuiltinKernel::Binary(f64::log),
        ScalarFn::Log1p => BuiltinKernel::Unary(f64::ln_1p),
        ScalarFn::Sin => BuiltinKernel::Unary(f64::sin),
        ScalarFn::Cos => BuiltinKernel::Unary(f64::cos),
        ScalarFn::Tan => BuiltinKernel::Unary(f64::tan),
        ScalarFn::Asin => BuiltinKernel::Unary(f64::asin),
        ScalarFn::Acos => BuiltinKernel::Unary(f64::acos),
        ScalarFn::Atan => BuiltinKernel::Unary(f64::atan),
        ScalarFn::Atan2 => BuiltinKernel::Binary(f64::atan2),
        ScalarFn::Sinh => BuiltinKernel::Unary(f64::sinh),
        ScalarFn::Cosh => BuiltinKernel::Unary(f64::cosh),
        ScalarFn::Tanh => BuiltinKernel::Unary(f64::tanh),
        ScalarFn::Asinh => BuiltinKernel::Unary(f64::asinh),
        ScalarFn::Acosh => BuiltinKernel::Unary(f64::acosh),
        ScalarFn::Atanh => BuiltinKernel::Unary(f64::atanh),
        ScalarFn::Floor => BuiltinKernel::Unary(f64::floor),
        ScalarFn::Ceil => BuiltinKernel::Unary(f64::ceil),
        ScalarFn::Round => BuiltinKernel::Unary(f64::round),
        ScalarFn::Trunc => BuiltinKernel::Unary(f64::trunc),
        ScalarFn::Sign => BuiltinKernel::Unary(sign),
        ScalarFn::Least => BuiltinKernel::Binary(f64::min),
        ScalarFn::Greatest => BuiltinKernel::Binary(f64::max),
        ScalarFn::Hypot => BuiltinKernel::Binary(f64::hypot),
        ScalarFn::Clamp => BuiltinKernel::Ternary(clamp),
    }
}

fn sign(value: f64) -> f64 {
    match value.partial_cmp(&0.0) {
        Some(std::cmp::Ordering::Greater) => 1.0,
        Some(std::cmp::Ordering::Less) => -1.0,
        Some(std::cmp::Ordering::Equal) | None => 0.0,
    }
}

fn clamp(value: f64, min: f64, max: f64) -> f64 {
    // `f64::clamp` panics for invalid bounds. NaN instead routes the failure
    // through the evaluator's normal finite-result diagnostic.
    if min.is_nan() || max.is_nan() || min > max {
        f64::NAN
    } else {
        value.max(min).min(max)
    }
}

/// Dimension signature of each scalar built-in.
fn signature(function: ScalarFn) -> FunctionSignature {
    match function {
        // Root functions.
        ScalarFn::Sqrt => FunctionSignature::free_to_pow(fn_param("x"), Rational::HALF),
        ScalarFn::Cbrt => FunctionSignature::free_to_pow(fn_param("x"), Rational::THIRD),
        // Exponential, logarithmic, and hyperbolic functions are all
        // dimensionless. Rounding functions are dimensionless-only too:
        // rounding does not commute with unit rescaling.
        ScalarFn::Expm1
        | ScalarFn::Ln
        | ScalarFn::Log10
        | ScalarFn::Log2
        | ScalarFn::Log1p
        | ScalarFn::Sinh
        | ScalarFn::Cosh
        | ScalarFn::Tanh
        | ScalarFn::Asinh
        | ScalarFn::Acosh
        | ScalarFn::Atanh
        | ScalarFn::Floor
        | ScalarFn::Ceil
        | ScalarFn::Round
        | ScalarFn::Trunc => FunctionSignature::all_dimensionless(&["x"]),
        ScalarFn::Log => FunctionSignature::all_dimensionless(&["x", "base"]),
        // Trigonometric functions (Angle -> Dimensionless).
        ScalarFn::Sin | ScalarFn::Cos | ScalarFn::Tan => {
            FunctionSignature::fixed_to_fixed(fn_param("x"), angle(), dimensionless())
        }
        // Inverse trigonometric functions (Dimensionless -> Angle).
        ScalarFn::Asin | ScalarFn::Acos | ScalarFn::Atan => {
            FunctionSignature::fixed_to_fixed(fn_param("x"), dimensionless(), angle())
        }
        ScalarFn::Atan2 => FunctionSignature::same_dim_to_fixed(&["y", "x"], angle()),
        // Sign accepts any dimension: it commutes with unit rescaling, so its
        // result does not depend on the source unit.
        ScalarFn::Sign => FunctionSignature::free_to_fixed("x", dimensionless()),
        // Multi-argument same-dimension functions.
        ScalarFn::Least | ScalarFn::Greatest | ScalarFn::Hypot => {
            FunctionSignature::same_dim(&["a", "b"])
        }
        ScalarFn::Clamp => FunctionSignature::same_dim(&["x", "min", "max"]),
    }
}

/// The scalar catalog, in [`ScalarFn::ALL`] (declaration) order.
static SCALAR_FUNCTIONS: LazyLock<Vec<ScalarFunction>> = LazyLock::new(|| {
    ScalarFn::ALL
        .iter()
        .map(|&function| ScalarFunction::new(function, kernel(function), signature(function)))
        .collect()
});

/// Return the kernel and dimension signature of a scalar built-in.
#[must_use]
pub fn scalar_function(function: ScalarFn) -> &'static ScalarFunction {
    // `ScalarFn::ALL` lists the variants in declaration order, so the
    // discriminant is the catalog position.
    &SCALAR_FUNCTIONS[function as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(function: ScalarFn, args: &[f64]) -> f64 {
        scalar_function(function).eval(args).unwrap()
    }

    #[test]
    fn representative_kernels_evaluate() {
        assert!((apply(ScalarFn::Sqrt, &[4.0]) - 2.0).abs() < f64::EPSILON);
        assert!((apply(ScalarFn::Log, &[27.0, 3.0]) - 3.0).abs() < 1e-10);
        assert!((apply(ScalarFn::Hypot, &[3.0, 4.0]) - 5.0).abs() < f64::EPSILON);
        assert!((apply(ScalarFn::Clamp, &[15.0, 0.0, 10.0]) - 10.0).abs() < f64::EPSILON);
        assert!((apply(ScalarFn::Clamp, &[-5.0, 0.0, 10.0])).abs() < f64::EPSILON);
        assert!((apply(ScalarFn::Clamp, &[5.0, 0.0, 10.0]) - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn sign_kernel_maps_to_unit_values() {
        assert!((apply(ScalarFn::Sign, &[3.5]) - 1.0).abs() < f64::EPSILON);
        assert!((apply(ScalarFn::Sign, &[-0.1]) + 1.0).abs() < f64::EPSILON);
        assert!(apply(ScalarFn::Sign, &[0.0]).abs() < f64::EPSILON);
        assert!(apply(ScalarFn::Sign, &[f64::NAN]).abs() < f64::EPSILON);
    }

    #[test]
    fn invalid_clamp_bounds_return_nan() {
        assert!(apply(ScalarFn::Clamp, &[5.0, 10.0, 0.0]).is_nan());
        assert!(apply(ScalarFn::Clamp, &[5.0, f64::NAN, 1.0]).is_nan());
        assert!(apply(ScalarFn::Clamp, &[5.0, 1.0, f64::NAN]).is_nan());
    }

    /// Each kernel is the `f64` function its spelling names.
    #[test]
    fn every_kernel_matches_its_spelling() {
        let x = 0.375;
        let y = 1.75;
        for function in ScalarFn::all() {
            let (args, expected): (Vec<f64>, f64) = match function {
                ScalarFn::Sqrt => (vec![y], y.sqrt()),
                ScalarFn::Cbrt => (vec![y], y.cbrt()),
                ScalarFn::Expm1 => (vec![x], x.exp_m1()),
                ScalarFn::Ln => (vec![y], y.ln()),
                ScalarFn::Log10 => (vec![y], y.log10()),
                ScalarFn::Log2 => (vec![y], y.log2()),
                ScalarFn::Log => (vec![y, 3.0], y.log(3.0)),
                ScalarFn::Log1p => (vec![x], x.ln_1p()),
                ScalarFn::Sin => (vec![x], x.sin()),
                ScalarFn::Cos => (vec![x], x.cos()),
                ScalarFn::Tan => (vec![x], x.tan()),
                ScalarFn::Asin => (vec![x], x.asin()),
                ScalarFn::Acos => (vec![x], x.acos()),
                ScalarFn::Atan => (vec![x], x.atan()),
                ScalarFn::Atan2 => (vec![x, y], x.atan2(y)),
                ScalarFn::Sinh => (vec![x], x.sinh()),
                ScalarFn::Cosh => (vec![x], x.cosh()),
                ScalarFn::Tanh => (vec![x], x.tanh()),
                ScalarFn::Asinh => (vec![x], x.asinh()),
                ScalarFn::Acosh => (vec![y], y.acosh()),
                ScalarFn::Atanh => (vec![x], x.atanh()),
                ScalarFn::Floor => (vec![y], y.floor()),
                ScalarFn::Ceil => (vec![y], y.ceil()),
                ScalarFn::Round => (vec![y], y.round()),
                ScalarFn::Trunc => (vec![-y], (-y).trunc()),
                ScalarFn::Sign => (vec![-y], -1.0),
                ScalarFn::Least => (vec![x, y], x),
                ScalarFn::Greatest => (vec![x, y], y),
                ScalarFn::Hypot => (vec![x, y], x.hypot(y)),
                ScalarFn::Clamp => (vec![y, 0.0, 1.0], 1.0),
            };
            assert_eq!(args.len(), function.arity(), "arity of `{function}`");
            let actual = apply(function, &args);
            assert!(
                (actual - expected).abs() < 1e-12,
                "`{function}` evaluated to {actual}, expected {expected}"
            );
        }
    }

    #[test]
    fn catalog_is_indexed_by_declaration_order() {
        for (position, function) in ScalarFn::ALL.iter().enumerate() {
            assert_eq!(*function as usize, position);
            let entry = scalar_function(*function);
            assert_eq!(entry.arity(), function.arity());
            assert_eq!(entry.signature().arity(), function.arity());
        }
    }

    #[test]
    fn every_kernel_rejects_wrong_arity_without_panicking() {
        for function in ScalarFn::all() {
            let entry = scalar_function(function);
            for wrong_len in [entry.arity() - 1, entry.arity() + 1] {
                let args = vec![0.0; wrong_len];
                assert_eq!(
                    entry.eval(&args),
                    Err(BuiltinEvalError::WrongArity {
                        expected: entry.arity(),
                        actual: wrong_len,
                    }),
                    "wrong-arity behavior for `{function}`"
                );
            }
        }
    }
}
