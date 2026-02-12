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
}

pub struct BuiltinFunction {
    pub name: &'static str,
    pub arity: usize,
    pub eval: fn(&[f64]) -> f64,
    pub dim_sig: DimSignature,
}

#[must_use]
#[expect(clippy::too_many_lines)] // Declarative list of built-in functions
pub fn builtin_functions() -> HashMap<&'static str, BuiltinFunction> {
    let mut m = HashMap::new();
    m.insert(
        "sqrt",
        BuiltinFunction {
            name: "sqrt",
            arity: 1,
            eval: |a| a[0].sqrt(),
            dim_sig: DimSignature::Sqrt,
        },
    );
    m.insert(
        "exp",
        BuiltinFunction {
            name: "exp",
            arity: 1,
            eval: |a| a[0].exp(),
            dim_sig: DimSignature::AllDimensionless,
        },
    );
    m.insert(
        "ln",
        BuiltinFunction {
            name: "ln",
            arity: 1,
            eval: |a| a[0].ln(),
            dim_sig: DimSignature::AllDimensionless,
        },
    );
    m.insert(
        "abs",
        BuiltinFunction {
            name: "abs",
            arity: 1,
            eval: |a| a[0].abs(),
            dim_sig: DimSignature::Passthrough,
        },
    );
    m.insert(
        "sin",
        BuiltinFunction {
            name: "sin",
            arity: 1,
            eval: |a| a[0].sin(),
            dim_sig: DimSignature::AngleToDimensionless,
        },
    );
    m.insert(
        "cos",
        BuiltinFunction {
            name: "cos",
            arity: 1,
            eval: |a| a[0].cos(),
            dim_sig: DimSignature::AngleToDimensionless,
        },
    );
    m.insert(
        "tan",
        BuiltinFunction {
            name: "tan",
            arity: 1,
            eval: |a| a[0].tan(),
            dim_sig: DimSignature::AngleToDimensionless,
        },
    );
    m.insert(
        "asin",
        BuiltinFunction {
            name: "asin",
            arity: 1,
            eval: |a| a[0].asin(),
            dim_sig: DimSignature::DimensionlessToAngle,
        },
    );
    m.insert(
        "acos",
        BuiltinFunction {
            name: "acos",
            arity: 1,
            eval: |a| a[0].acos(),
            dim_sig: DimSignature::DimensionlessToAngle,
        },
    );
    m.insert(
        "floor",
        BuiltinFunction {
            name: "floor",
            arity: 1,
            eval: |a| a[0].floor(),
            dim_sig: DimSignature::Passthrough,
        },
    );
    m.insert(
        "ceil",
        BuiltinFunction {
            name: "ceil",
            arity: 1,
            eval: |a| a[0].ceil(),
            dim_sig: DimSignature::Passthrough,
        },
    );
    m.insert(
        "atan2",
        BuiltinFunction {
            name: "atan2",
            arity: 2,
            eval: |a| a[0].atan2(a[1]),
            dim_sig: DimSignature::SameDimensionToAngle,
        },
    );
    m.insert(
        "min",
        BuiltinFunction {
            name: "min",
            arity: 2,
            eval: |a| a[0].min(a[1]),
            dim_sig: DimSignature::SameDimension,
        },
    );
    m.insert(
        "max",
        BuiltinFunction {
            name: "max",
            arity: 2,
            eval: |a| a[0].max(a[1]),
            dim_sig: DimSignature::SameDimension,
        },
    );
    m
}

#[must_use]
pub fn builtin_constants() -> HashMap<&'static str, f64> {
    let mut m = HashMap::new();
    m.insert("PI", std::f64::consts::PI);
    m.insert("E", std::f64::consts::E);
    m
}

#[cfg(test)]
mod tests {
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
                        "ceil"
                    ]
                    .contains(name),
                    "unexpected 1-arity fn: {name}"
                ),
                2 => assert!(
                    ["atan2", "min", "max"].contains(name),
                    "unexpected 2-arity fn: {name}"
                ),
                _ => panic!("unexpected arity for {name}: {}", f.arity),
            }
        }
    }
}
