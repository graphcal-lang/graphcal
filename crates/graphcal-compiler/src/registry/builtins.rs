use std::collections::HashMap;
use std::sync::LazyLock;

use crate::syntax::dimension::{BaseDimId, Dimension, Rational};
use crate::syntax::names::DimVarName;

/// Describes how a single parameter's dimension is constrained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamDim {
    /// Must be a specific fixed dimension (e.g., dimensionless, Angle).
    Fixed(Dimension),
    /// Introduces a new dimension variable. The variable is bound to
    /// the argument's actual dimension.
    Bind(DimVarName),
    /// Must match the dimension already bound to this variable name.
    Ref(DimVarName),
}

/// Describes how the result dimension is computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultDim {
    /// A specific fixed dimension.
    Fixed(Dimension),
    /// The dimension bound to the named variable.
    Var(DimVarName),
    /// The dimension bound to the named variable, raised to a rational power.
    VarPow(DimVarName, Rational),
}

/// A parameter with its display name and dimension constraint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamSig {
    /// Display name (e.g., "x", "a", "min").
    pub name: String,
    /// Dimension constraint.
    pub dim: ParamDim,
}

/// Describes how a built-in function interacts with dimensions.
///
/// Each parameter has an independent constraint, and the result dimension
/// is computed from fixed values or dimension variables bound by the parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimSignature {
    /// Per-parameter constraints, in order. Length determines arity.
    pub params: Vec<ParamSig>,
    /// How the result dimension is computed.
    pub result: ResultDim,
}

const fn dimensionless() -> Dimension {
    Dimension::dimensionless()
}

fn dim_var_d() -> DimVarName {
    DimVarName::expect_valid("D")
}

fn angle() -> Dimension {
    Dimension::base(BaseDimId::Prelude("Angle".to_string()))
}

impl DimSignature {
    /// All params must be dimensionless, result is dimensionless.
    #[must_use]
    pub fn all_dimensionless(names: &[&str]) -> Self {
        Self {
            params: names
                .iter()
                .map(|&n| ParamSig {
                    name: n.to_string(),
                    dim: ParamDim::Fixed(dimensionless()),
                })
                .collect(),
            result: ResultDim::Fixed(dimensionless()),
        }
    }

    /// Single fixed-dimension param, fixed-dimension result.
    #[must_use]
    pub fn fixed_to_fixed(name: &str, input: Dimension, output: Dimension) -> Self {
        Self {
            params: vec![ParamSig {
                name: name.to_string(),
                dim: ParamDim::Fixed(input),
            }],
            result: ResultDim::Fixed(output),
        }
    }

    /// Single free param D, result is D.
    #[must_use]
    pub fn passthrough(name: &str) -> Self {
        Self {
            params: vec![ParamSig {
                name: name.to_string(),
                dim: ParamDim::Bind(dim_var_d()),
            }],
            result: ResultDim::Var(dim_var_d()),
        }
    }

    /// Single free param D, result is a fixed dimension.
    #[must_use]
    pub fn free_to_fixed(name: &str, output: Dimension) -> Self {
        Self {
            params: vec![ParamSig {
                name: name.to_string(),
                dim: ParamDim::Bind(dim_var_d()),
            }],
            result: ResultDim::Fixed(output),
        }
    }

    /// Single free param D, result is D^power.
    #[must_use]
    pub fn free_to_pow(name: &str, power: Rational) -> Self {
        Self {
            params: vec![ParamSig {
                name: name.to_string(),
                dim: ParamDim::Bind(dim_var_d()),
            }],
            result: ResultDim::VarPow(dim_var_d(), power),
        }
    }

    /// N params all same dimension D, result is D.
    #[must_use]
    pub fn same_dim(names: &[&str]) -> Self {
        Self {
            params: names
                .iter()
                .enumerate()
                .map(|(i, &n)| ParamSig {
                    name: n.to_string(),
                    dim: if i == 0 {
                        ParamDim::Bind(dim_var_d())
                    } else {
                        ParamDim::Ref(dim_var_d())
                    },
                })
                .collect(),
            result: ResultDim::Var(dim_var_d()),
        }
    }

    /// N params all same dimension D, result is a fixed dimension.
    #[must_use]
    pub fn same_dim_to_fixed(names: &[&str], output: Dimension) -> Self {
        Self {
            params: names
                .iter()
                .enumerate()
                .map(|(i, &n)| ParamSig {
                    name: n.to_string(),
                    dim: if i == 0 {
                        ParamDim::Bind(dim_var_d())
                    } else {
                        ParamDim::Ref(dim_var_d())
                    },
                })
                .collect(),
            result: ResultDim::Fixed(output),
        }
    }
}

pub struct BuiltinFunction {
    pub eval: fn(&[f64]) -> f64,
    pub dim_sig: DimSignature,
}

impl BuiltinFunction {
    /// Returns the arity (number of parameters) of this function.
    #[must_use]
    pub const fn arity(&self) -> usize {
        self.dim_sig.params.len()
    }
}

static BUILTIN_FUNCTIONS: LazyLock<HashMap<&'static str, BuiltinFunction>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    // Root functions
    m.insert(
        "sqrt",
        BuiltinFunction {
            eval: |a| a[0].sqrt(),
            dim_sig: DimSignature::free_to_pow("x", Rational::HALF),
        },
    );
    m.insert(
        "cbrt",
        BuiltinFunction {
            eval: |a| a[0].cbrt(),
            dim_sig: DimSignature::free_to_pow("x", Rational::THIRD),
        },
    );
    // Exponential and logarithmic functions (all dimensionless)
    m.insert(
        "exp",
        BuiltinFunction {
            eval: |a| a[0].exp(),
            dim_sig: DimSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "expm1",
        BuiltinFunction {
            eval: |a| a[0].exp_m1(),
            dim_sig: DimSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "ln",
        BuiltinFunction {
            eval: |a| a[0].ln(),
            dim_sig: DimSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "log10",
        BuiltinFunction {
            eval: |a| a[0].log10(),
            dim_sig: DimSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "log2",
        BuiltinFunction {
            eval: |a| a[0].log2(),
            dim_sig: DimSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "log",
        BuiltinFunction {
            eval: |a| a[0].log(a[1]),
            dim_sig: DimSignature::all_dimensionless(&["x", "base"]),
        },
    );
    m.insert(
        "log1p",
        BuiltinFunction {
            eval: |a| a[0].ln_1p(),
            dim_sig: DimSignature::all_dimensionless(&["x"]),
        },
    );
    // Trigonometric functions (Angle -> Dimensionless)
    m.insert(
        "sin",
        BuiltinFunction {
            eval: |a| a[0].sin(),
            dim_sig: DimSignature::fixed_to_fixed("x", angle(), dimensionless()),
        },
    );
    m.insert(
        "cos",
        BuiltinFunction {
            eval: |a| a[0].cos(),
            dim_sig: DimSignature::fixed_to_fixed("x", angle(), dimensionless()),
        },
    );
    m.insert(
        "tan",
        BuiltinFunction {
            eval: |a| a[0].tan(),
            dim_sig: DimSignature::fixed_to_fixed("x", angle(), dimensionless()),
        },
    );
    // Inverse trigonometric functions (Dimensionless -> Angle)
    m.insert(
        "asin",
        BuiltinFunction {
            eval: |a| a[0].asin(),
            dim_sig: DimSignature::fixed_to_fixed("x", dimensionless(), angle()),
        },
    );
    m.insert(
        "acos",
        BuiltinFunction {
            eval: |a| a[0].acos(),
            dim_sig: DimSignature::fixed_to_fixed("x", dimensionless(), angle()),
        },
    );
    m.insert(
        "atan",
        BuiltinFunction {
            eval: |a| a[0].atan(),
            dim_sig: DimSignature::fixed_to_fixed("x", dimensionless(), angle()),
        },
    );
    m.insert(
        "atan2",
        BuiltinFunction {
            eval: |a| a[0].atan2(a[1]),
            dim_sig: DimSignature::same_dim_to_fixed(&["y", "x"], angle()),
        },
    );
    // Hyperbolic functions (all dimensionless)
    m.insert(
        "sinh",
        BuiltinFunction {
            eval: |a| a[0].sinh(),
            dim_sig: DimSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "cosh",
        BuiltinFunction {
            eval: |a| a[0].cosh(),
            dim_sig: DimSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "tanh",
        BuiltinFunction {
            eval: |a| a[0].tanh(),
            dim_sig: DimSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "asinh",
        BuiltinFunction {
            eval: |a| a[0].asinh(),
            dim_sig: DimSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "acosh",
        BuiltinFunction {
            eval: |a| a[0].acosh(),
            dim_sig: DimSignature::all_dimensionless(&["x"]),
        },
    );
    m.insert(
        "atanh",
        BuiltinFunction {
            eval: |a| a[0].atanh(),
            dim_sig: DimSignature::all_dimensionless(&["x"]),
        },
    );
    // Rounding and sign functions (passthrough dimension)
    m.insert(
        "abs",
        BuiltinFunction {
            eval: |a| a[0].abs(),
            dim_sig: DimSignature::passthrough("x"),
        },
    );
    m.insert(
        "floor",
        BuiltinFunction {
            eval: |a| a[0].floor(),
            dim_sig: DimSignature::passthrough("x"),
        },
    );
    m.insert(
        "ceil",
        BuiltinFunction {
            eval: |a| a[0].ceil(),
            dim_sig: DimSignature::passthrough("x"),
        },
    );
    m.insert(
        "round",
        BuiltinFunction {
            eval: |a| a[0].round(),
            dim_sig: DimSignature::passthrough("x"),
        },
    );
    m.insert(
        "trunc",
        BuiltinFunction {
            eval: |a| a[0].trunc(),
            dim_sig: DimSignature::passthrough("x"),
        },
    );
    m.insert(
        "sign",
        BuiltinFunction {
            eval: |a| a[0].signum(),
            dim_sig: DimSignature::free_to_fixed("x", dimensionless()),
        },
    );
    // Multi-argument same-dimension functions
    m.insert(
        "min",
        BuiltinFunction {
            eval: |a| a[0].min(a[1]),
            dim_sig: DimSignature::same_dim(&["a", "b"]),
        },
    );
    m.insert(
        "max",
        BuiltinFunction {
            eval: |a| a[0].max(a[1]),
            dim_sig: DimSignature::same_dim(&["a", "b"]),
        },
    );
    m.insert(
        "hypot",
        BuiltinFunction {
            eval: |a| a[0].hypot(a[1]),
            dim_sig: DimSignature::same_dim(&["a", "b"]),
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
            dim_sig: DimSignature::same_dim(&["x", "min", "max"]),
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
        // f64::signum(0.0) returns 1.0, signum(-0.0) returns -1.0
        assert!(((f.eval)(&[0.0]) - 1.0).abs() < f64::EPSILON);
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
        use crate::hir::BuiltinConst;
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
        // Every name in the eval function map must be a typed BuiltinFnName,
        // and every BuiltinFnName must be evaluable: either via the function
        // map or via the special-function classification. A name added to
        // only one table would resolve in one phase and fail in another.
        use crate::hir::BuiltinFnName;
        use crate::registry::resolve_types::classify_special_fn;
        let map = builtin_functions();
        for name in map.keys() {
            assert!(
                BuiltinFnName::parse(name).is_some(),
                "builtin_functions() entry `{name}` missing from BuiltinFnName"
            );
        }
        for f in BuiltinFnName::ALL {
            let name = f.as_str();
            assert!(
                map.contains_key(name) || classify_special_fn(name).is_some(),
                "BuiltinFnName::{f:?} (`{name}`) is neither in builtin_functions() \
                 nor classified by classify_special_fn"
            );
        }
    }
}
