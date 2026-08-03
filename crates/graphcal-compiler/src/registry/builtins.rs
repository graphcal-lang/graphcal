use std::collections::HashMap;
use std::sync::LazyLock;

use thiserror::Error;

use crate::builtin::BuiltinFnName;
use crate::dimension::{BaseDimId, Dimension, PreludeBaseDimension, Rational};
use crate::function_signature::FunctionSignature;
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

/// A built-in quantity function: a private, arity-encoded evaluation kernel
/// paired with its typed [`FunctionSignature`].
pub struct BuiltinFunction {
    kernel: BuiltinKernel,
    signature: FunctionSignature,
}

impl BuiltinFunction {
    fn new(kernel: BuiltinKernel, signature: FunctionSignature) -> Self {
        assert_eq!(
            kernel.arity(),
            signature.arity(),
            "built-in kernel and signature arities must agree"
        );
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

/// Typed catalog of ordinary scalar quantity functions.
pub type BuiltinFunctions = HashMap<BuiltinFnName, BuiltinFunction>;

fn register(
    functions: &mut BuiltinFunctions,
    name: BuiltinFnName,
    kernel: BuiltinKernel,
    signature: FunctionSignature,
) {
    let previous = functions.insert(name, BuiltinFunction::new(kernel, signature));
    assert!(previous.is_none(), "duplicate built-in function `{name}`");
}

static BUILTIN_FUNCTIONS: LazyLock<BuiltinFunctions> = LazyLock::new(|| {
    let mut functions = HashMap::new();

    // Root functions.
    register(
        &mut functions,
        BuiltinFnName::Sqrt,
        BuiltinKernel::Unary(f64::sqrt),
        FunctionSignature::free_to_pow(fn_param("x"), Rational::HALF),
    );
    register(
        &mut functions,
        BuiltinFnName::Cbrt,
        BuiltinKernel::Unary(f64::cbrt),
        FunctionSignature::free_to_pow(fn_param("x"), Rational::THIRD),
    );

    // Exponential and logarithmic functions (all dimensionless).
    register(
        &mut functions,
        BuiltinFnName::Exp,
        BuiltinKernel::Unary(f64::exp),
        FunctionSignature::all_dimensionless(&["x"]),
    );
    register(
        &mut functions,
        BuiltinFnName::Expm1,
        BuiltinKernel::Unary(f64::exp_m1),
        FunctionSignature::all_dimensionless(&["x"]),
    );
    register(
        &mut functions,
        BuiltinFnName::Ln,
        BuiltinKernel::Unary(f64::ln),
        FunctionSignature::all_dimensionless(&["x"]),
    );
    register(
        &mut functions,
        BuiltinFnName::Log10,
        BuiltinKernel::Unary(f64::log10),
        FunctionSignature::all_dimensionless(&["x"]),
    );
    register(
        &mut functions,
        BuiltinFnName::Log2,
        BuiltinKernel::Unary(f64::log2),
        FunctionSignature::all_dimensionless(&["x"]),
    );
    register(
        &mut functions,
        BuiltinFnName::Log,
        BuiltinKernel::Binary(f64::log),
        FunctionSignature::all_dimensionless(&["x", "base"]),
    );
    register(
        &mut functions,
        BuiltinFnName::Log1p,
        BuiltinKernel::Unary(f64::ln_1p),
        FunctionSignature::all_dimensionless(&["x"]),
    );

    // Trigonometric functions (Angle -> Dimensionless).
    for (name, kernel) in [
        (BuiltinFnName::Sin, f64::sin as fn(f64) -> f64),
        (BuiltinFnName::Cos, f64::cos),
        (BuiltinFnName::Tan, f64::tan),
    ] {
        register(
            &mut functions,
            name,
            BuiltinKernel::Unary(kernel),
            FunctionSignature::fixed_to_fixed(fn_param("x"), angle(), dimensionless()),
        );
    }

    // Inverse trigonometric functions (Dimensionless -> Angle).
    for (name, kernel) in [
        (BuiltinFnName::Asin, f64::asin as fn(f64) -> f64),
        (BuiltinFnName::Acos, f64::acos),
        (BuiltinFnName::Atan, f64::atan),
    ] {
        register(
            &mut functions,
            name,
            BuiltinKernel::Unary(kernel),
            FunctionSignature::fixed_to_fixed(fn_param("x"), dimensionless(), angle()),
        );
    }
    register(
        &mut functions,
        BuiltinFnName::Atan2,
        BuiltinKernel::Binary(f64::atan2),
        FunctionSignature::same_dim_to_fixed(&["y", "x"], angle()),
    );

    // Hyperbolic functions (all dimensionless).
    for (name, kernel) in [
        (BuiltinFnName::Sinh, f64::sinh as fn(f64) -> f64),
        (BuiltinFnName::Cosh, f64::cosh),
        (BuiltinFnName::Tanh, f64::tanh),
        (BuiltinFnName::Asinh, f64::asinh),
        (BuiltinFnName::Acosh, f64::acosh),
        (BuiltinFnName::Atanh, f64::atanh),
    ] {
        register(
            &mut functions,
            name,
            BuiltinKernel::Unary(kernel),
            FunctionSignature::all_dimensionless(&["x"]),
        );
    }

    // Absolute value and sign accept any dimension: both commute with unit
    // rescaling, so their results do not depend on the source unit.
    register(
        &mut functions,
        BuiltinFnName::Abs,
        BuiltinKernel::Unary(f64::abs),
        FunctionSignature::passthrough("x"),
    );
    register(
        &mut functions,
        BuiltinFnName::Sign,
        BuiltinKernel::Unary(|value| match value.partial_cmp(&0.0) {
            Some(std::cmp::Ordering::Greater) => 1.0,
            Some(std::cmp::Ordering::Less) => -1.0,
            Some(std::cmp::Ordering::Equal) | None => 0.0,
        }),
        FunctionSignature::free_to_fixed("x", dimensionless()),
    );

    // Rounding functions are dimensionless-only: rounding does not commute
    // with unit rescaling.
    for (name, kernel) in [
        (BuiltinFnName::Floor, f64::floor as fn(f64) -> f64),
        (BuiltinFnName::Ceil, f64::ceil),
        (BuiltinFnName::Round, f64::round),
        (BuiltinFnName::Trunc, f64::trunc),
    ] {
        register(
            &mut functions,
            name,
            BuiltinKernel::Unary(kernel),
            FunctionSignature::all_dimensionless(&["x"]),
        );
    }

    // Multi-argument same-dimension functions.
    register(
        &mut functions,
        BuiltinFnName::Least,
        BuiltinKernel::Binary(f64::min),
        FunctionSignature::same_dim(&["a", "b"]),
    );
    register(
        &mut functions,
        BuiltinFnName::Greatest,
        BuiltinKernel::Binary(f64::max),
        FunctionSignature::same_dim(&["a", "b"]),
    );
    register(
        &mut functions,
        BuiltinFnName::Hypot,
        BuiltinKernel::Binary(f64::hypot),
        FunctionSignature::same_dim(&["a", "b"]),
    );
    register(
        &mut functions,
        BuiltinFnName::Clamp,
        BuiltinKernel::Ternary(|value, min, max| {
            // `f64::clamp` panics for invalid bounds. NaN instead routes the
            // failure through the evaluator's normal finite-result diagnostic.
            if min.is_nan() || max.is_nan() || min > max {
                f64::NAN
            } else {
                value.max(min).min(max)
            }
        }),
        FunctionSignature::same_dim(&["x", "min", "max"]),
    );

    functions
});

/// Return the immutable typed catalog of ordinary scalar quantity functions.
#[must_use]
pub fn builtin_functions() -> &'static BuiltinFunctions {
    &BUILTIN_FUNCTIONS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::BuiltinConst;

    fn eval(name: BuiltinFnName, args: &[f64]) -> f64 {
        builtin_functions()[&name].eval(args).unwrap()
    }

    #[test]
    fn representative_kernels_evaluate() {
        assert!((eval(BuiltinFnName::Sqrt, &[4.0]) - 2.0).abs() < f64::EPSILON);
        assert!((eval(BuiltinFnName::Log, &[27.0, 3.0]) - 3.0).abs() < 1e-10);
        assert!((eval(BuiltinFnName::Hypot, &[3.0, 4.0]) - 5.0).abs() < f64::EPSILON);
        assert!((eval(BuiltinFnName::Clamp, &[15.0, 0.0, 10.0]) - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn invalid_clamp_bounds_return_nan() {
        assert!(eval(BuiltinFnName::Clamp, &[5.0, 10.0, 0.0]).is_nan());
        assert!(eval(BuiltinFnName::Clamp, &[5.0, f64::NAN, 1.0]).is_nan());
    }

    #[test]
    fn every_kernel_rejects_wrong_arity_without_panicking() {
        for (name, function) in builtin_functions() {
            let wrong_len = function.arity().saturating_sub(1);
            let args = vec![0.0; wrong_len];
            assert_eq!(
                function.eval(&args),
                Err(BuiltinEvalError::WrongArity {
                    expected: function.arity(),
                    actual: wrong_len,
                }),
                "wrong-arity behavior for `{name}`"
            );
        }
    }

    #[test]
    fn typed_constant_catalog_is_exhaustive_and_round_trips() {
        for constant in BuiltinConst::ALL {
            assert_eq!(BuiltinConst::parse(constant.as_str()), Some(*constant));
            assert!(constant.value().is_finite());
        }
    }
}
