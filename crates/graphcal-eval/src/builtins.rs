use std::collections::HashMap;

/// Describes how a built-in function interacts with dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DimSignature {
    /// All arguments must be Dimensionless, returns Dimensionless.
    /// e.g., exp, ln
    AllDimensionless,
    /// Argument must be Angle, returns Dimensionless.
    /// e.g., sin, cos, tan
    AngleToDimensionless,
    /// Returns Angle from Dimensionless arguments.
    /// e.g., asin, acos
    DimensionlessToAngle,
    /// Argument is D, returns D^(1/2). Dimension must have even exponents.
    /// e.g., sqrt
    Sqrt,
    /// Argument is D, returns D (preserves dimension).
    /// e.g., abs, floor, ceil
    Passthrough,
    /// Both arguments must have same dimension D, returns D.
    /// e.g., min, max
    SameDimension,
    /// Both arguments must have same dimension D, returns Angle.
    /// e.g., atan2
    SameDimensionToAngle,
    /// Argument is D, returns Dimensionless.
    /// e.g., sign
    PassthroughToDimensionless,
    /// All three arguments must have same dimension D, returns D.
    /// e.g., clamp(x, lo, hi)
    SameDimension3,
    /// Argument is D, returns D^(1/3). Dimension exponents must be divisible by 3.
    /// e.g., cbrt
    Cbrt,
}

pub struct BuiltinFunction {
    pub arity: usize,
    pub eval: fn(&[f64]) -> f64,
    pub dim_sig: DimSignature,
}

#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "declarative list of built-in functions"
)]
pub fn builtin_functions() -> HashMap<&'static str, BuiltinFunction> {
    let mut m = HashMap::new();
    m.insert(
        "sqrt",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].sqrt(),
            dim_sig: DimSignature::Sqrt,
        },
    );
    m.insert(
        "exp",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].exp(),
            dim_sig: DimSignature::AllDimensionless,
        },
    );
    m.insert(
        "ln",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].ln(),
            dim_sig: DimSignature::AllDimensionless,
        },
    );
    m.insert(
        "abs",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].abs(),
            dim_sig: DimSignature::Passthrough,
        },
    );
    m.insert(
        "sin",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].sin(),
            dim_sig: DimSignature::AngleToDimensionless,
        },
    );
    m.insert(
        "cos",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].cos(),
            dim_sig: DimSignature::AngleToDimensionless,
        },
    );
    m.insert(
        "tan",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].tan(),
            dim_sig: DimSignature::AngleToDimensionless,
        },
    );
    m.insert(
        "asin",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].asin(),
            dim_sig: DimSignature::DimensionlessToAngle,
        },
    );
    m.insert(
        "acos",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].acos(),
            dim_sig: DimSignature::DimensionlessToAngle,
        },
    );
    m.insert(
        "floor",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].floor(),
            dim_sig: DimSignature::Passthrough,
        },
    );
    m.insert(
        "ceil",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].ceil(),
            dim_sig: DimSignature::Passthrough,
        },
    );
    m.insert(
        "atan2",
        BuiltinFunction {
            arity: 2,
            eval: |a| a[0].atan2(a[1]),
            dim_sig: DimSignature::SameDimensionToAngle,
        },
    );
    m.insert(
        "min",
        BuiltinFunction {
            arity: 2,
            eval: |a| a[0].min(a[1]),
            dim_sig: DimSignature::SameDimension,
        },
    );
    m.insert(
        "max",
        BuiltinFunction {
            arity: 2,
            eval: |a| a[0].max(a[1]),
            dim_sig: DimSignature::SameDimension,
        },
    );
    // --- New Phase A functions ---
    m.insert(
        "atan",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].atan(),
            dim_sig: DimSignature::DimensionlessToAngle,
        },
    );
    m.insert(
        "round",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].round(),
            dim_sig: DimSignature::Passthrough,
        },
    );
    m.insert(
        "trunc",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].trunc(),
            dim_sig: DimSignature::Passthrough,
        },
    );
    m.insert(
        "sign",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].signum(),
            dim_sig: DimSignature::PassthroughToDimensionless,
        },
    );
    m.insert(
        "clamp",
        BuiltinFunction {
            arity: 3,
            eval: |a| a[0].clamp(a[1], a[2]),
            dim_sig: DimSignature::SameDimension3,
        },
    );
    m.insert(
        "hypot",
        BuiltinFunction {
            arity: 2,
            eval: |a| a[0].hypot(a[1]),
            dim_sig: DimSignature::SameDimension,
        },
    );
    m.insert(
        "log10",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].log10(),
            dim_sig: DimSignature::AllDimensionless,
        },
    );
    m.insert(
        "log2",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].log2(),
            dim_sig: DimSignature::AllDimensionless,
        },
    );
    m.insert(
        "log",
        BuiltinFunction {
            arity: 2,
            eval: |a| a[0].log(a[1]),
            dim_sig: DimSignature::AllDimensionless,
        },
    );
    m.insert(
        "cbrt",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].cbrt(),
            dim_sig: DimSignature::Cbrt,
        },
    );
    m.insert(
        "sinh",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].sinh(),
            dim_sig: DimSignature::AllDimensionless,
        },
    );
    m.insert(
        "cosh",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].cosh(),
            dim_sig: DimSignature::AllDimensionless,
        },
    );
    m.insert(
        "tanh",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].tanh(),
            dim_sig: DimSignature::AllDimensionless,
        },
    );
    m.insert(
        "asinh",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].asinh(),
            dim_sig: DimSignature::AllDimensionless,
        },
    );
    m.insert(
        "acosh",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].acosh(),
            dim_sig: DimSignature::AllDimensionless,
        },
    );
    m.insert(
        "atanh",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].atanh(),
            dim_sig: DimSignature::AllDimensionless,
        },
    );
    m.insert(
        "expm1",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].exp_m1(),
            dim_sig: DimSignature::AllDimensionless,
        },
    );
    m.insert(
        "log1p",
        BuiltinFunction {
            arity: 1,
            eval: |a| a[0].ln_1p(),
            dim_sig: DimSignature::AllDimensionless,
        },
    );
    m
}

#[must_use]
pub fn builtin_constants() -> HashMap<&'static str, f64> {
    let mut m = HashMap::new();
    m.insert("PI", std::f64::consts::PI);
    m.insert("E", std::f64::consts::E);
    m.insert("TAU", std::f64::consts::TAU);
    m.insert("SQRT2", std::f64::consts::SQRT_2);
    m.insert("LN2", std::f64::consts::LN_2);
    m.insert("LN10", std::f64::consts::LN_10);
    m
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        reason = "test code"
    )]

    use super::*;

    #[test]
    fn builtin_sqrt() {
        let fns = builtin_functions();
        let sqrt = &fns["sqrt"];
        assert_eq!(sqrt.arity, 1);
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
        assert_eq!(atan2.arity, 2);
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
        for (name, f) in &fns {
            match f.arity {
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
                _ => panic!("unexpected arity for {name}: {}", f.arity),
            }
        }
    }

    #[test]
    fn builtin_atan() {
        let fns = builtin_functions();
        let f = &fns["atan"];
        assert_eq!(f.arity, 1);
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
        // f64::signum(0.0) returns 1.0, signum(-0.0) returns -1.0
        assert!(((f.eval)(&[0.0]) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn builtin_clamp() {
        let fns = builtin_functions();
        let f = &fns["clamp"];
        assert_eq!(f.arity, 3);
        assert!(((f.eval)(&[5.0, 0.0, 10.0]) - 5.0).abs() < f64::EPSILON);
        assert!(((f.eval)(&[-1.0, 0.0, 10.0]) - 0.0).abs() < f64::EPSILON);
        assert!(((f.eval)(&[15.0, 0.0, 10.0]) - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn builtin_hypot() {
        let fns = builtin_functions();
        let f = &fns["hypot"];
        assert_eq!(f.arity, 2);
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
        assert_eq!(f.arity, 2);
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
}
