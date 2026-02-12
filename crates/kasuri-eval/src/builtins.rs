use std::collections::HashMap;

pub struct BuiltinFunction {
    pub name: &'static str,
    pub arity: usize,
    pub eval: fn(&[f64]) -> f64,
}

pub fn builtin_functions() -> HashMap<&'static str, BuiltinFunction> {
    let mut m = HashMap::new();
    m.insert(
        "sqrt",
        BuiltinFunction {
            name: "sqrt",
            arity: 1,
            eval: |a| a[0].sqrt(),
        },
    );
    m.insert(
        "exp",
        BuiltinFunction {
            name: "exp",
            arity: 1,
            eval: |a| a[0].exp(),
        },
    );
    m.insert(
        "ln",
        BuiltinFunction {
            name: "ln",
            arity: 1,
            eval: |a| a[0].ln(),
        },
    );
    m.insert(
        "abs",
        BuiltinFunction {
            name: "abs",
            arity: 1,
            eval: |a| a[0].abs(),
        },
    );
    m.insert(
        "sin",
        BuiltinFunction {
            name: "sin",
            arity: 1,
            eval: |a| a[0].sin(),
        },
    );
    m.insert(
        "cos",
        BuiltinFunction {
            name: "cos",
            arity: 1,
            eval: |a| a[0].cos(),
        },
    );
    m.insert(
        "tan",
        BuiltinFunction {
            name: "tan",
            arity: 1,
            eval: |a| a[0].tan(),
        },
    );
    m.insert(
        "asin",
        BuiltinFunction {
            name: "asin",
            arity: 1,
            eval: |a| a[0].asin(),
        },
    );
    m.insert(
        "acos",
        BuiltinFunction {
            name: "acos",
            arity: 1,
            eval: |a| a[0].acos(),
        },
    );
    m.insert(
        "floor",
        BuiltinFunction {
            name: "floor",
            arity: 1,
            eval: |a| a[0].floor(),
        },
    );
    m.insert(
        "ceil",
        BuiltinFunction {
            name: "ceil",
            arity: 1,
            eval: |a| a[0].ceil(),
        },
    );
    m.insert(
        "atan2",
        BuiltinFunction {
            name: "atan2",
            arity: 2,
            eval: |a| a[0].atan2(a[1]),
        },
    );
    m.insert(
        "min",
        BuiltinFunction {
            name: "min",
            arity: 2,
            eval: |a| a[0].min(a[1]),
        },
    );
    m.insert(
        "max",
        BuiltinFunction {
            name: "max",
            arity: 2,
            eval: |a| a[0].max(a[1]),
        },
    );
    m
}

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
