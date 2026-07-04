use std::collections::HashMap;
use std::sync::LazyLock;

use crate::dimension::{BaseDimId, Dimension, Rational};
use crate::function_signature::FunctionSignature;

const fn dimensionless() -> Dimension {
    Dimension::dimensionless()
}

fn angle() -> Dimension {
    Dimension::base(BaseDimId::Prelude("Angle".to_string()))
}

/// A built-in scalar function: an evaluation kernel paired with its
/// [`FunctionSignature`].
pub struct BuiltinFunction {
    pub eval: fn(&[f64]) -> f64,
    pub signature: FunctionSignature,
}

impl BuiltinFunction {
    /// Returns the arity (number of parameters) of this function.
    #[must_use]
    pub fn arity(&self) -> usize {
        self.signature.arity()
    }
}

static BUILTIN_FUNCTIONS: LazyLock<HashMap<&'static str, BuiltinFunction>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    // Root functions
    m.insert(
        "sqrt",
        BuiltinFunction {
            eval: |a| a[0].sqrt(),
            signature: FunctionSignature::free_to_pow("x", Rational::HALF),
        },
    );
    m.insert(
        "cbrt",
        BuiltinFunction {
            eval: |a| a[0].cbrt(),
            signature: FunctionSignature::free_to_pow("x", Rational::THIRD),
        },
    );
    // Exponential and logarithmic functions (all dimensionless)
    m.insert(
        "exp",
        BuiltinFunction {
            eval: |a| a[0].exp(),
            signature: FunctionSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "expm1",
        BuiltinFunction {
            eval: |a| a[0].exp_m1(),
            signature: FunctionSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "ln",
        BuiltinFunction {
            eval: |a| a[0].ln(),
            signature: FunctionSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "log10",
        BuiltinFunction {
            eval: |a| a[0].log10(),
            signature: FunctionSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "log2",
        BuiltinFunction {
            eval: |a| a[0].log2(),
            signature: FunctionSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "log",
        BuiltinFunction {
            eval: |a| a[0].log(a[1]),
            signature: FunctionSignature::all_dimensionless(&["x", "base"]),
        },
    );
    m.insert(
        "log1p",
        BuiltinFunction {
            eval: |a| a[0].ln_1p(),
            signature: FunctionSignature::all_dimensionless(&["x"]),
        },
    );
    // Trigonometric functions (Angle -> Dimensionless)
    m.insert(
        "sin",
        BuiltinFunction {
            eval: |a| a[0].sin(),
            signature: FunctionSignature::fixed_to_fixed("x", angle(), dimensionless()),
        },
    );
    m.insert(
        "cos",
        BuiltinFunction {
            eval: |a| a[0].cos(),
            signature: FunctionSignature::fixed_to_fixed("x", angle(), dimensionless()),
        },
    );
    m.insert(
        "tan",
        BuiltinFunction {
            eval: |a| a[0].tan(),
            signature: FunctionSignature::fixed_to_fixed("x", angle(), dimensionless()),
        },
    );
    // Inverse trigonometric functions (Dimensionless -> Angle)
    m.insert(
        "asin",
        BuiltinFunction {
            eval: |a| a[0].asin(),
            signature: FunctionSignature::fixed_to_fixed("x", dimensionless(), angle()),
        },
    );
    m.insert(
        "acos",
        BuiltinFunction {
            eval: |a| a[0].acos(),
            signature: FunctionSignature::fixed_to_fixed("x", dimensionless(), angle()),
        },
    );
    m.insert(
        "atan",
        BuiltinFunction {
            eval: |a| a[0].atan(),
            signature: FunctionSignature::fixed_to_fixed("x", dimensionless(), angle()),
        },
    );
    m.insert(
        "atan2",
        BuiltinFunction {
            eval: |a| a[0].atan2(a[1]),
            signature: FunctionSignature::same_dim_to_fixed(&["y", "x"], angle()),
        },
    );
    // Hyperbolic functions (all dimensionless)
    m.insert(
        "sinh",
        BuiltinFunction {
            eval: |a| a[0].sinh(),
            signature: FunctionSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "cosh",
        BuiltinFunction {
            eval: |a| a[0].cosh(),
            signature: FunctionSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "tanh",
        BuiltinFunction {
            eval: |a| a[0].tanh(),
            signature: FunctionSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "asinh",
        BuiltinFunction {
            eval: |a| a[0].asinh(),
            signature: FunctionSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "acosh",
        BuiltinFunction {
            eval: |a| a[0].acosh(),
            signature: FunctionSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "atanh",
        BuiltinFunction {
            eval: |a| a[0].atanh(),
            signature: FunctionSignature::all_dimensionless(&["x"]),
        },
    );
    // Rounding and sign functions (passthrough dimension)
    m.insert(
        "abs",
        BuiltinFunction {
            eval: |a| a[0].abs(),
            signature: FunctionSignature::passthrough("x"),
        },
    );
    m.insert(
        "floor",
        BuiltinFunction {
            eval: |a| a[0].floor(),
            signature: FunctionSignature::passthrough("x"),
        },
    );
    m.insert(
        "ceil",
        BuiltinFunction {
            eval: |a| a[0].ceil(),
            signature: FunctionSignature::passthrough("x"),
        },
    );
    m.insert(
        "round",
        BuiltinFunction {
            eval: |a| a[0].round(),
            signature: FunctionSignature::passthrough("x"),
        },
    );
    m.insert(
        "trunc",
        BuiltinFunction {
            eval: |a| a[0].trunc(),
            signature: FunctionSignature::passthrough("x"),
        },
    );
    m.insert(
        "sign",
        BuiltinFunction {
            eval: |a| match a[0].partial_cmp(&0.0) {
                Some(std::cmp::Ordering::Greater) => 1.0,
                Some(std::cmp::Ordering::Less) => -1.0,
                Some(std::cmp::Ordering::Equal) | None => 0.0,
            },
            signature: FunctionSignature::free_to_fixed("x", dimensionless()),
        },
    );
    // Multi-argument same-dimension functions
    m.insert(
        "min",
        BuiltinFunction {
            eval: |a| a[0].min(a[1]),
            signature: FunctionSignature::same_dim(&["a", "b"]),
        },
    );
    m.insert(
        "max",
        BuiltinFunction {
            eval: |a| a[0].max(a[1]),
            signature: FunctionSignature::same_dim(&["a", "b"]),
        },
    );
    m.insert(
        "hypot",
        BuiltinFunction {
            eval: |a| a[0].hypot(a[1]),
            signature: FunctionSignature::same_dim(&["a", "b"]),
        },
    );
    m.insert(
        "clamp",
        BuiltinFunction {
            // Avoid `f64::clamp`, which panics when min > max or either bound is NaN.
            // Returning NaN routes the failure through the same `check_finite` path
            // used by sqrt/asin/ln so the user sees a runtime diagnostic instead of
            // a Rust backtrace.
            eval: |a| {
                let (x, lo, hi) = (a[0], a[1], a[2]);
                if lo.is_nan() || hi.is_nan() || lo > hi {
                    f64::NAN
                } else {
                    x.max(lo).min(hi)
                }
            },
            signature: FunctionSignature::same_dim(&["x", "min", "max"]),
        },
    );
    m
});

#[must_use]
pub fn builtin_functions() -> &'static HashMap<&'static str, BuiltinFunction> {
    &BUILTIN_FUNCTIONS
}

static BUILTIN_CONSTANTS: LazyLock<HashMap<&'static str, f64>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    m.insert("PI", std::f64::consts::PI);
    m.insert("E", std::f64::consts::E);
    m.insert("TAU", std::f64::consts::TAU);
    m.insert("SQRT2", std::f64::consts::SQRT_2);
    m.insert("LN2", std::f64::consts::LN_2);
    m.insert("LN10", std::f64::consts::LN_10);
    m
});

#[must_use]
pub fn builtin_constants() -> &'static HashMap<&'static str, f64> {
    &BUILTIN_CONSTANTS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_sqrt() {
        let fns = builtin_functions();
        let sqrt = &fns["sqrt"];
        assert_eq!(sqrt.arity(), 1);
        assert!(((sqrt.eval)(&[4.0]) - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn builtin_ln() {
        let fns = builtin_functions();
        let ln = &fns["ln"];
        assert!(((ln.eval)(&[std::f64::consts::E]) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn builtin_atan2() {
        let fns = builtin_functions();
        let atan2 = &fns["atan2"];
        assert_eq!(atan2.arity(), 2);
        let result = (atan2.eval)(&[1.0, 1.0]);
        assert!((result - std::f64::consts::FRAC_PI_4).abs() < f64::EPSILON);
    }

    #[test]
    fn builtin_min_max() {
        let fns = builtin_functions();
        assert!(((fns["min"].eval)(&[3.0, 5.0]) - 3.0).abs() < f64::EPSILON);
        assert!(((fns["max"].eval)(&[3.0, 5.0]) - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn builtin_constants_values() {
        let consts = builtin_constants();
        assert!((consts["PI"] - std::f64::consts::PI).abs() < f64::EPSILON);
        assert!((consts["E"] - std::f64::consts::E).abs() < f64::EPSILON);
    }

    #[test]
    fn all_builtins_have_correct_arity() {
        let fns = builtin_functions();
        for (name, f) in fns {
            match f.arity() {
                1 => assert!(
                    [
                        "sqrt", "exp", "ln", "abs", "sin", "cos", "tan", "asin", "acos", "floor",
                        "ceil", "atan", "round", "trunc", "sign", "log10", "log2", "cbrt", "sinh",
                        "cosh", "tanh", "asinh", "acosh", "atanh", "expm1", "log1p",
                    ]
                    .contains(name),
                    "unexpected 1-arity fn: {name}"
                ),
                2 => assert!(
                    ["atan2", "min", "max", "hypot", "log"].contains(name),
                    "unexpected 2-arity fn: {name}"
                ),
                3 => assert!(["clamp"].contains(name), "unexpected 3-arity fn: {name}"),
                _ => panic!("unexpected arity for {name}: {}", f.arity()),
            }
        }
    }

    #[test]
    fn builtin_atan() {
        let fns = builtin_functions();
        let f = &fns["atan"];
        assert_eq!(f.arity(), 1);
        assert!(((f.eval)(&[1.0]) - std::f64::consts::FRAC_PI_4).abs() < f64::EPSILON);
    }

    #[test]
    fn builtin_round() {
        let fns = builtin_functions();
        let f = &fns["round"];
        assert!(((f.eval)(&[3.7]) - 4.0).abs() < f64::EPSILON);
        assert!(((f.eval)(&[3.2]) - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn builtin_trunc() {
        let fns = builtin_functions();
        let f = &fns["trunc"];
        assert!(((f.eval)(&[3.7]) - 3.0).abs() < f64::EPSILON);
        assert!(((f.eval)(&[-3.7]) - -3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn builtin_sign() {
        let fns = builtin_functions();
        let f = &fns["sign"];
        assert!(((f.eval)(&[5.0]) - 1.0).abs() < f64::EPSILON);
        assert!(((f.eval)(&[-5.0]) - -1.0).abs() < f64::EPSILON);
        assert!(((f.eval)(&[0.0]) - 0.0).abs() < f64::EPSILON);
        assert!(((f.eval)(&[-0.0]) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn builtin_clamp() {
        let fns = builtin_functions();
        let f = &fns["clamp"];
        assert_eq!(f.arity(), 3);
        assert!(((f.eval)(&[5.0, 0.0, 10.0]) - 5.0).abs() < f64::EPSILON);
        assert!(((f.eval)(&[-1.0, 0.0, 10.0]) - 0.0).abs() < f64::EPSILON);
        assert!(((f.eval)(&[15.0, 0.0, 10.0]) - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn builtin_clamp_min_exceeds_max_returns_nan() {
        let fns = builtin_functions();
        let f = &fns["clamp"];
        assert!((f.eval)(&[5.0, 10.0, 0.0]).is_nan());
    }

    #[test]
    fn builtin_clamp_nan_bound_returns_nan() {
        let fns = builtin_functions();
        let f = &fns["clamp"];
        assert!((f.eval)(&[5.0, f64::NAN, 1.0]).is_nan());
        assert!((f.eval)(&[5.0, 0.0, f64::NAN]).is_nan());
    }

    #[test]
    fn builtin_hypot() {
        let fns = builtin_functions();
        let f = &fns["hypot"];
        assert_eq!(f.arity(), 2);
        assert!(((f.eval)(&[3.0, 4.0]) - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn builtin_log10() {
        let fns = builtin_functions();
        let f = &fns["log10"];
        assert!(((f.eval)(&[100.0]) - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn builtin_log2() {
        let fns = builtin_functions();
        let f = &fns["log2"];
        assert!(((f.eval)(&[8.0]) - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn builtin_log() {
        let fns = builtin_functions();
        let f = &fns["log"];
        assert_eq!(f.arity(), 2);
        assert!(((f.eval)(&[27.0, 3.0]) - 3.0).abs() < 1e-10);
    }

    #[test]
    fn builtin_cbrt() {
        let fns = builtin_functions();
        let f = &fns["cbrt"];
        assert!(((f.eval)(&[27.0]) - 3.0).abs() < f64::EPSILON);
        assert!(((f.eval)(&[8.0]) - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn builtin_hyperbolic() {
        let fns = builtin_functions();
        // sinh(0) = 0, cosh(0) = 1, tanh(0) = 0
        assert!(((fns["sinh"].eval)(&[0.0]) - 0.0).abs() < f64::EPSILON);
        assert!(((fns["cosh"].eval)(&[0.0]) - 1.0).abs() < f64::EPSILON);
        assert!(((fns["tanh"].eval)(&[0.0]) - 0.0).abs() < f64::EPSILON);
        // asinh(0) = 0, acosh(1) = 0, atanh(0) = 0
        assert!(((fns["asinh"].eval)(&[0.0]) - 0.0).abs() < f64::EPSILON);
        assert!(((fns["acosh"].eval)(&[1.0]) - 0.0).abs() < f64::EPSILON);
        assert!(((fns["atanh"].eval)(&[0.0]) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn builtin_expm1_log1p() {
        let fns = builtin_functions();
        // expm1(0) = 0, log1p(0) = 0
        assert!(((fns["expm1"].eval)(&[0.0]) - 0.0).abs() < f64::EPSILON);
        assert!(((fns["log1p"].eval)(&[0.0]) - 0.0).abs() < f64::EPSILON);
        // expm1(1) ≈ e-1, log1p(e-1) ≈ 1
        assert!(((fns["expm1"].eval)(&[1.0]) - (std::f64::consts::E - 1.0)).abs() < 1e-10);
        assert!(((fns["log1p"].eval)(&[std::f64::consts::E - 1.0]) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn builtin_constants_new_values() {
        let consts = builtin_constants();
        assert!((consts["TAU"] - std::f64::consts::TAU).abs() < f64::EPSILON);
        assert!((consts["SQRT2"] - std::f64::consts::SQRT_2).abs() < f64::EPSILON);
        assert!((consts["LN2"] - std::f64::consts::LN_2).abs() < f64::EPSILON);
        assert!((consts["LN10"] - std::f64::consts::LN_10).abs() < f64::EPSILON);
    }

    #[test]
    fn builtin_constant_tables_agree() {
        // The typed BuiltinConst table and the registry constant map are
        // maintained separately; this pins them together so adding a
        // constant to one without the other fails loudly.
        use crate::builtin::BuiltinConst;
        let map = builtin_constants();
        for c in BuiltinConst::ALL {
            let registry_value = map.get(c.as_str()).unwrap_or_else(|| {
                panic!(
                    "BuiltinConst::{c:?} (`{}`) missing from builtin_constants()",
                    c.as_str()
                )
            });
            assert!(
                (registry_value - c.value()).abs() < f64::EPSILON,
                "value mismatch for `{}`",
                c.as_str()
            );
        }
        for name in map.keys() {
            assert!(
                BuiltinConst::parse(name).is_some(),
                "builtin_constants() entry `{name}` missing from BuiltinConst"
            );
        }
    }

    #[test]
    fn builtin_function_tables_agree() {
        // Every ordinary scalar eval function must be a typed BuiltinFnName.
        // Non-registry built-in call shapes are accounted for by the HIR type
        // inference routing test in `tir::dim_check::infer::builtin_call`.
        use crate::builtin::BuiltinFnName;
        let map = builtin_functions();
        for name in map.keys() {
            assert!(
                BuiltinFnName::parse(name).is_some(),
                "builtin_functions() entry `{name}` missing from BuiltinFnName"
            );
        }
    }
}
