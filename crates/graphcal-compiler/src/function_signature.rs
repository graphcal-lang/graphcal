//! Function-signature IR: the typed dimensional calling convention shared by
//! built-in and externally-provided (plugin) functions.
//!
//! A [`FunctionSignature`] describes how a scalar-kernel function interacts
//! with the type system: which dimension variables it declares, what value
//! kind each named parameter requires, and how the result kind is computed
//! from the bound dimension variables. Param and result dimensions are
//! [`DimMonomial`]s — products of dimension-variable powers and a fixed
//! [`Dimension`] — generalizing single-variable forms like `D -> D^(1/2)`
//! to cross-variable algebra such as `(D1, D2) -> D1 * D2`.
//!
//! This module owns only the pure signature algebra. Interpreting a signature
//! against inferred argument types (producing diagnostics) lives in the
//! dimension checker; evaluating the kernel lives in the evaluator. The model
//! is plain data end-to-end so boundaries (Phase B plugin manifests) can
//! serialize it without the core ever carrying strings.

use std::collections::HashSet;

use thiserror::Error;

use crate::dimension::{Dimension, Rational, RationalError};
use crate::syntax::dimension::DimVarName;
use crate::syntax::function_name::FnParamName;

/// One dimension-variable factor in a [`DimMonomial`]: `var^power`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimVarPower {
    /// The dimension variable being raised.
    pub var: DimVarName,
    /// The rational exponent. Never zero in a validated signature.
    pub power: Rational,
}

/// A dimension monomial: a product of dimension-variable powers and a fixed
/// dimension, e.g. `D1 * D2^2 * Length^-1`.
///
/// The fixed factor reuses [`Dimension`], which is itself a product of base
/// dimensions with rational exponents. `DimMonomial::fixed(Dimension::dimensionless())`
/// with no variables is the dimensionless monomial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimMonomial {
    /// Dimension-variable factors, in declaration order. No duplicate variables.
    pub vars: Vec<DimVarPower>,
    /// The concrete dimension factor. [`Dimension::dimensionless`] when absent.
    pub fixed: Dimension,
}

impl DimMonomial {
    /// A monomial with no variable factors: just a concrete dimension.
    #[must_use]
    pub const fn fixed(dim: Dimension) -> Self {
        Self {
            vars: Vec::new(),
            fixed: dim,
        }
    }

    /// The dimensionless monomial (no variables, dimensionless fixed factor).
    #[must_use]
    pub const fn dimensionless() -> Self {
        Self::fixed(Dimension::dimensionless())
    }

    /// A bare dimension variable: `var^1`.
    #[must_use]
    pub fn var(var: DimVarName) -> Self {
        Self::var_pow(var, Rational::ONE)
    }

    /// A single dimension-variable power: `var^power`.
    #[must_use]
    pub fn var_pow(var: DimVarName, power: Rational) -> Self {
        Self {
            vars: vec![DimVarPower { var, power }],
            fixed: Dimension::dimensionless(),
        }
    }

    /// Returns the variable when this monomial is exactly one bare variable
    /// (`var^1` with a dimensionless fixed factor) — the only shape that can
    /// *bind* a dimension variable at a call site.
    #[must_use]
    pub fn as_bare_var(&self) -> Option<&DimVarName> {
        match self.vars.as_slice() {
            [DimVarPower { var, power }]
                if *power == Rational::ONE && self.fixed.is_dimensionless() =>
            {
                Some(var)
            }
            _ => None,
        }
    }

    /// Returns whether this monomial references no dimension variables.
    #[must_use]
    pub const fn is_concrete(&self) -> bool {
        self.vars.is_empty()
    }

    /// Iterate the dimension variables referenced by this monomial.
    pub fn referenced_vars(&self) -> impl Iterator<Item = &DimVarName> {
        self.vars.iter().map(|factor| &factor.var)
    }

    /// Compute the concrete dimension of this monomial under `lookup`, which
    /// maps each referenced variable to its bound dimension.
    ///
    /// # Errors
    ///
    /// Returns [`DimMonomialEvalError::UnboundVar`] when `lookup` has no
    /// binding for a referenced variable, and
    /// [`DimMonomialEvalError::Overflow`] when exponent arithmetic overflows.
    pub fn eval<'a>(
        &self,
        mut lookup: impl FnMut(&DimVarName) -> Option<&'a Dimension>,
    ) -> Result<Dimension, DimMonomialEvalError> {
        let mut result = self.fixed.clone();
        for factor in &self.vars {
            let bound = lookup(&factor.var).ok_or_else(|| DimMonomialEvalError::UnboundVar {
                var: factor.var.clone(),
            })?;
            let powered = bound.pow(factor.power)?;
            result = result.checked_mul(&powered)?;
        }
        Ok(result)
    }
}

/// Error from evaluating a [`DimMonomial`] against variable bindings.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DimMonomialEvalError {
    /// A referenced dimension variable had no binding.
    #[error("dimension variable `{var}` is unbound")]
    UnboundVar {
        /// The unbound variable.
        var: DimVarName,
    },
    /// Exponent arithmetic overflowed.
    #[error(transparent)]
    Overflow(#[from] RationalError),
}

/// The kind of a parameter or result value in a function signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueKind {
    /// A scalar with the dimension given by the monomial.
    Scalar(DimMonomial),
    /// A boolean value.
    Bool,
    /// An integer value.
    Int,
}

impl ValueKind {
    /// A dimensionless scalar.
    #[must_use]
    pub const fn dimensionless() -> Self {
        Self::Scalar(DimMonomial::dimensionless())
    }

    /// A scalar with a concrete fixed dimension.
    #[must_use]
    pub const fn scalar(dim: Dimension) -> Self {
        Self::Scalar(DimMonomial::fixed(dim))
    }
}

/// A named parameter with its value-kind constraint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionParam {
    /// Parameter name, for diagnostics, hover, and signature help.
    pub name: FnParamName,
    /// The value kind this parameter requires.
    pub kind: ValueKind,
}

/// A typed, serializable function signature: declared dimension variables,
/// named parameters, and the result kind.
///
/// Construction goes through [`FunctionSignature::try_new`], which enforces
/// the invariants that make call-site checking decidable:
///
/// - Declared dimension variables are distinct.
/// - Monomial factors carry no zero exponents and no duplicate variables.
/// - Every referenced variable is declared, and every declared variable has a
///   *binding occurrence*: a parameter that is exactly that bare variable
///   (`var^1`), appearing before (or as) the variable's first use in any
///   compound monomial. Binding occurrences are what unify a variable with a
///   concrete argument dimension at a call site; compound monomials are then
///   checked by direct evaluation, never by solving equations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSignature {
    dim_vars: Vec<DimVarName>,
    params: Vec<FunctionParam>,
    result: ValueKind,
}

impl FunctionSignature {
    /// Build a validated signature.
    ///
    /// # Errors
    ///
    /// Returns a [`SignatureError`] describing the first violated invariant.
    pub fn try_new(
        dim_vars: Vec<DimVarName>,
        params: Vec<FunctionParam>,
        result: ValueKind,
    ) -> Result<Self, SignatureError> {
        let mut declared: HashSet<&DimVarName> = HashSet::new();
        for var in &dim_vars {
            if !declared.insert(var) {
                return Err(SignatureError::DuplicateDimVar { var: var.clone() });
            }
        }

        let mut bound: HashSet<&DimVarName> = HashSet::new();
        for param in &params {
            let ValueKind::Scalar(monomial) = &param.kind else {
                continue;
            };
            validate_monomial_factors(monomial)?;
            if let Some(var) = monomial.as_bare_var() {
                if !declared.contains(var) {
                    return Err(SignatureError::UndeclaredDimVar { var: var.clone() });
                }
                bound.insert(var);
                continue;
            }
            for var in monomial.referenced_vars() {
                if !declared.contains(var) {
                    return Err(SignatureError::UndeclaredDimVar { var: var.clone() });
                }
                if !bound.contains(var) {
                    return Err(SignatureError::UseBeforeBinding {
                        var: var.clone(),
                        param: param.name.clone(),
                    });
                }
            }
        }

        if let ValueKind::Scalar(monomial) = &result {
            validate_monomial_factors(monomial)?;
            for var in monomial.referenced_vars() {
                if !declared.contains(var) {
                    return Err(SignatureError::UndeclaredDimVar { var: var.clone() });
                }
                if !bound.contains(var) {
                    return Err(SignatureError::UnboundResultVar { var: var.clone() });
                }
            }
        }

        if let Some(var) = dim_vars.iter().find(|var| !bound.contains(var)) {
            return Err(SignatureError::DimVarNeverBound { var: var.clone() });
        }

        Ok(Self {
            dim_vars,
            params,
            result,
        })
    }

    /// The declared dimension variables, in declaration order.
    #[must_use]
    pub fn dim_vars(&self) -> &[DimVarName] {
        &self.dim_vars
    }

    /// The named parameters, in declaration order.
    #[must_use]
    pub fn params(&self) -> &[FunctionParam] {
        &self.params
    }

    /// The result kind.
    #[must_use]
    pub const fn result(&self) -> &ValueKind {
        &self.result
    }

    /// The number of parameters.
    #[must_use]
    pub const fn arity(&self) -> usize {
        self.params.len()
    }

    /// Render this signature as `<D1, D2>(name: kind, ...) -> kind`, using
    /// `format_dim` to render concrete dimensions.
    ///
    /// This is a display boundary (hover, signature help, diagnostics); the
    /// checker and evaluator pattern-match the typed parts instead.
    #[must_use]
    pub fn format_with(&self, mut format_dim: impl FnMut(&Dimension) -> String) -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        if !self.dim_vars.is_empty() {
            let vars: Vec<&str> = self.dim_vars.iter().map(DimVarName::as_str).collect();
            let _ = write!(out, "<{}>", vars.join(", "));
        }
        out.push('(');
        for (i, param) in self.params.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            let _ = write!(
                out,
                "{}: {}",
                param.name,
                format_value_kind(&param.kind, &mut format_dim)
            );
        }
        out.push_str(") -> ");
        out.push_str(&format_value_kind(&self.result, &mut format_dim));
        out
    }
}

fn format_value_kind(
    kind: &ValueKind,
    format_dim: &mut impl FnMut(&Dimension) -> String,
) -> String {
    match kind {
        ValueKind::Bool => "Bool".to_string(),
        ValueKind::Int => "Int".to_string(),
        ValueKind::Scalar(monomial) => format_monomial(monomial, format_dim),
    }
}

fn format_monomial(
    monomial: &DimMonomial,
    format_dim: &mut impl FnMut(&Dimension) -> String,
) -> String {
    let mut parts: Vec<String> = monomial
        .vars
        .iter()
        .map(|factor| {
            if factor.power == Rational::ONE {
                factor.var.to_string()
            } else {
                format!(
                    "{}{}",
                    factor.var,
                    crate::registry::format::format_exponent(factor.power)
                )
            }
        })
        .collect();
    if !monomial.fixed.is_dimensionless() {
        parts.push(format_dim(&monomial.fixed));
    }
    if parts.is_empty() {
        let rendered = format_dim(&monomial.fixed);
        if rendered.is_empty() {
            "Dimensionless".to_string()
        } else {
            rendered
        }
    } else {
        parts.join(" * ")
    }
}

fn validate_monomial_factors(monomial: &DimMonomial) -> Result<(), SignatureError> {
    let mut seen: HashSet<&DimVarName> = HashSet::new();
    for factor in &monomial.vars {
        if factor.power.is_zero() {
            return Err(SignatureError::ZeroExponent {
                var: factor.var.clone(),
            });
        }
        if !seen.insert(&factor.var) {
            return Err(SignatureError::DuplicateMonomialVar {
                var: factor.var.clone(),
            });
        }
    }
    Ok(())
}

/// Error from [`FunctionSignature::try_new`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SignatureError {
    /// The same dimension variable was declared twice.
    #[error("dimension variable `{var}` is declared more than once")]
    DuplicateDimVar {
        /// The duplicated variable.
        var: DimVarName,
    },
    /// A monomial referenced a dimension variable that was not declared.
    #[error("dimension variable `{var}` is not declared by this signature")]
    UndeclaredDimVar {
        /// The undeclared variable.
        var: DimVarName,
    },
    /// A compound monomial used a variable before any bare binding occurrence.
    #[error(
        "dimension variable `{var}` is used in a compound form in parameter `{param}` before any parameter binds it as a bare variable"
    )]
    UseBeforeBinding {
        /// The variable used too early.
        var: DimVarName,
        /// The parameter carrying the compound use.
        param: FnParamName,
    },
    /// The result monomial referenced a variable no parameter binds.
    #[error("result dimension references `{var}`, which no parameter binds as a bare variable")]
    UnboundResultVar {
        /// The unbound variable.
        var: DimVarName,
    },
    /// A declared dimension variable has no bare binding occurrence.
    #[error("dimension variable `{var}` is declared but never bound by a bare parameter")]
    DimVarNeverBound {
        /// The never-bound variable.
        var: DimVarName,
    },
    /// A monomial factor carried a zero exponent.
    #[error("dimension variable `{var}` has a zero exponent")]
    ZeroExponent {
        /// The variable with the zero exponent.
        var: DimVarName,
    },
    /// The same variable appeared twice in one monomial.
    #[error("dimension variable `{var}` appears more than once in one monomial")]
    DuplicateMonomialVar {
        /// The repeated variable.
        var: DimVarName,
    },
}

// ---------------------------------------------------------------------------
// Built-in signature shapes
// ---------------------------------------------------------------------------

fn dim_var_d() -> DimVarName {
    DimVarName::expect_valid("D")
}

fn param(name: &str, kind: ValueKind) -> FunctionParam {
    FunctionParam {
        name: FnParamName::expect_valid(name),
        kind,
    }
}

#[expect(
    clippy::expect_used,
    reason = "built-in signature shapes are validated by construction; a failure is a compiler bug caught by tests"
)]
fn expect_signature(
    dim_vars: Vec<DimVarName>,
    params: Vec<FunctionParam>,
    result: ValueKind,
) -> FunctionSignature {
    FunctionSignature::try_new(dim_vars, params, result)
        .expect("built-in signature shape must be valid")
}

impl FunctionSignature {
    /// All params dimensionless scalars, dimensionless scalar result.
    #[must_use]
    pub fn all_dimensionless(names: &[&str]) -> Self {
        expect_signature(
            Vec::new(),
            names
                .iter()
                .map(|&n| param(n, ValueKind::dimensionless()))
                .collect(),
            ValueKind::dimensionless(),
        )
    }

    /// Single fixed-dimension param, fixed-dimension result.
    #[must_use]
    pub fn fixed_to_fixed(name: &str, input: Dimension, output: Dimension) -> Self {
        expect_signature(
            Vec::new(),
            vec![param(name, ValueKind::scalar(input))],
            ValueKind::scalar(output),
        )
    }

    /// Single free param `D`, result is `D`.
    #[must_use]
    pub fn passthrough(name: &str) -> Self {
        let d = dim_var_d();
        expect_signature(
            vec![d.clone()],
            vec![param(name, ValueKind::Scalar(DimMonomial::var(d.clone())))],
            ValueKind::Scalar(DimMonomial::var(d)),
        )
    }

    /// Single free param `D`, result is a fixed dimension.
    #[must_use]
    pub fn free_to_fixed(name: &str, output: Dimension) -> Self {
        let d = dim_var_d();
        expect_signature(
            vec![d.clone()],
            vec![param(name, ValueKind::Scalar(DimMonomial::var(d)))],
            ValueKind::scalar(output),
        )
    }

    /// Single free param `D`, result is `D^power`.
    #[must_use]
    pub fn free_to_pow(name: &str, power: Rational) -> Self {
        let d = dim_var_d();
        expect_signature(
            vec![d.clone()],
            vec![param(name, ValueKind::Scalar(DimMonomial::var(d.clone())))],
            ValueKind::Scalar(DimMonomial::var_pow(d, power)),
        )
    }

    /// N params all of the same dimension `D`, result is `D`.
    #[must_use]
    pub fn same_dim(names: &[&str]) -> Self {
        let d = dim_var_d();
        expect_signature(
            vec![d.clone()],
            names
                .iter()
                .map(|&n| param(n, ValueKind::Scalar(DimMonomial::var(d.clone()))))
                .collect(),
            ValueKind::Scalar(DimMonomial::var(d)),
        )
    }

    /// N params all of the same dimension `D`, result is a fixed dimension.
    #[must_use]
    pub fn same_dim_to_fixed(names: &[&str], output: Dimension) -> Self {
        let d = dim_var_d();
        expect_signature(
            vec![d.clone()],
            names
                .iter()
                .map(|&n| param(n, ValueKind::Scalar(DimMonomial::var(d.clone()))))
                .collect(),
            ValueKind::scalar(output),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::BaseDimId;

    fn var(name: &str) -> DimVarName {
        DimVarName::expect_valid(name)
    }

    fn length() -> Dimension {
        Dimension::base(BaseDimId::Prelude("Length".to_string()))
    }

    fn time() -> Dimension {
        Dimension::base(BaseDimId::Prelude("Time".to_string()))
    }

    #[test]
    fn cross_variable_monomial_result() {
        // (D1, D2) -> D1 * D2, the `torque(force, arm)` shape.
        let sig = FunctionSignature::try_new(
            vec![var("D1"), var("D2")],
            vec![
                param("force", ValueKind::Scalar(DimMonomial::var(var("D1")))),
                param("arm", ValueKind::Scalar(DimMonomial::var(var("D2")))),
            ],
            ValueKind::Scalar(DimMonomial {
                vars: vec![
                    DimVarPower {
                        var: var("D1"),
                        power: Rational::ONE,
                    },
                    DimVarPower {
                        var: var("D2"),
                        power: Rational::ONE,
                    },
                ],
                fixed: Dimension::dimensionless(),
            }),
        )
        .unwrap();

        let l = length();
        let t = time();
        let bindings = [(var("D1"), l.clone()), (var("D2"), t.clone())];
        let ValueKind::Scalar(result) = sig.result() else {
            panic!("expected scalar result");
        };
        let dim = result
            .eval(|v| bindings.iter().find(|(bv, _)| bv == v).map(|(_, d)| d))
            .unwrap();
        assert_eq!(dim, l.checked_mul(&t).unwrap());
    }

    #[test]
    fn use_before_binding_is_rejected() {
        // (x: D^2, y: D) -> D would require solving D from D^2 — rejected.
        let err = FunctionSignature::try_new(
            vec![var("D")],
            vec![
                param(
                    "x",
                    ValueKind::Scalar(DimMonomial::var_pow(var("D"), Rational::try_new(2, 1).unwrap())),
                ),
                param("y", ValueKind::Scalar(DimMonomial::var(var("D")))),
            ],
            ValueKind::Scalar(DimMonomial::var(var("D"))),
        )
        .unwrap_err();
        assert!(matches!(err, SignatureError::UseBeforeBinding { .. }));
    }

    #[test]
    fn undeclared_var_is_rejected() {
        let err = FunctionSignature::try_new(
            Vec::new(),
            vec![param("x", ValueKind::Scalar(DimMonomial::var(var("D"))))],
            ValueKind::dimensionless(),
        )
        .unwrap_err();
        assert!(matches!(err, SignatureError::UndeclaredDimVar { .. }));
    }

    #[test]
    fn declared_but_never_bound_var_is_rejected() {
        let err = FunctionSignature::try_new(
            vec![var("D")],
            vec![param("x", ValueKind::dimensionless())],
            ValueKind::dimensionless(),
        )
        .unwrap_err();
        assert!(matches!(err, SignatureError::DimVarNeverBound { .. }));
    }

    #[test]
    fn bool_and_int_kinds_carry_no_dims() {
        let sig = FunctionSignature::try_new(
            Vec::new(),
            vec![param("flag", ValueKind::Bool), param("n", ValueKind::Int)],
            ValueKind::Int,
        )
        .unwrap();
        assert_eq!(sig.arity(), 2);
    }

    #[test]
    fn format_with_renders_binders_params_and_result() {
        let sig = FunctionSignature::free_to_pow("x", Rational::HALF);
        let rendered = sig.format_with(|dim| format!("{dim:?}"));
        assert_eq!(rendered, "<D>(x: D) -> D^(1/2)");
    }

    #[test]
    fn builtin_shapes_are_valid() {
        let _ = FunctionSignature::all_dimensionless(&["x", "base"]);
        let _ = FunctionSignature::fixed_to_fixed("x", length(), time());
        let _ = FunctionSignature::passthrough("x");
        let _ = FunctionSignature::free_to_fixed("x", length());
        let _ = FunctionSignature::free_to_pow("x", Rational::HALF);
        let _ = FunctionSignature::same_dim(&["a", "b"]);
        let _ = FunctionSignature::same_dim_to_fixed(&["y", "x"], length());
    }
}
