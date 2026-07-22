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
use crate::syntax::index_name::IndexVarName;

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
    pub(crate) const fn dimensionless() -> Self {
        Self::fixed(Dimension::dimensionless())
    }

    /// A bare dimension variable: `var^1`.
    #[must_use]
    pub fn var(var: DimVarName) -> Self {
        Self::var_pow(var, Rational::ONE)
    }

    /// A single dimension-variable power: `var^power`.
    #[must_use]
    fn var_pow(var: DimVarName, power: Rational) -> Self {
        Self {
            vars: vec![DimVarPower { var, power }],
            fixed: Dimension::dimensionless(),
        }
    }

    /// Returns the variable when this monomial is exactly one bare variable
    /// (`var^1` with a dimensionless fixed factor) — the only shape that can
    /// *bind* a dimension variable at a call site.
    #[must_use]
    pub(crate) fn as_bare_var(&self) -> Option<&DimVarName> {
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
    fn referenced_vars(&self) -> impl Iterator<Item = &DimVarName> {
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
    pub(crate) fn eval<'a>(
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
    /// An array of scalars over a declared index variable: `element[index]`.
    ///
    /// Elements are scalars whose dimension is given by the monomial; the
    /// index variable is bound by an argument's concrete index at the call
    /// site. A result array must reuse an index variable bound by some
    /// parameter — a function can never invent its output length.
    Indexed {
        /// The element dimension monomial.
        element: DimMonomial,
        /// The index variable naming the array's index.
        index: IndexVarName,
    },
    /// A record value described by its flattened field shape.
    ///
    /// Result-only: [`FunctionSignature::try_new`] rejects struct
    /// parameters (callers pass fields as separate arguments). The shape is
    /// structural — the manifest never learns the graphcal type's name; the
    /// declaration site binds the shape to a record type in scope.
    Struct(StructShape),
}

/// The flattened field layout of a record return: named fields of concrete
/// scalar, boolean, or integer kinds, in declaration order.
///
/// Constructed through [`StructShape::try_new`], which rejects empty shapes
/// and duplicate field names. Field names and order are part of the calling
/// contract (they are labels users access, not binders), so structural
/// equivalence compares them verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructShape {
    fields: Vec<StructShapeField>,
}

/// One named field of a [`StructShape`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructShapeField {
    /// The field name (part of the contract).
    pub name: crate::syntax::type_name::FieldName,
    /// The field's value kind.
    pub kind: StructFieldKind,
}

/// The kind of one struct field: concrete scalars only — dimension
/// variables cannot appear in struct returns in this phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructFieldKind {
    /// A scalar with a concrete fixed dimension.
    Scalar(Dimension),
    /// A boolean value.
    Bool,
    /// An integer value.
    Int,
}

impl StructShape {
    /// Build a validated shape.
    ///
    /// # Errors
    ///
    /// Returns a [`SignatureError`] when the shape has no fields or repeats
    /// a field name.
    pub fn try_new(fields: Vec<StructShapeField>) -> Result<Self, SignatureError> {
        if fields.is_empty() {
            return Err(SignatureError::EmptyStructShape);
        }
        let mut seen: HashSet<&crate::syntax::type_name::FieldName> = HashSet::new();
        for field in &fields {
            if !seen.insert(&field.name) {
                return Err(SignatureError::DuplicateStructField {
                    field: field.name.clone(),
                });
            }
        }
        Ok(Self { fields })
    }

    /// The fields, in declaration order.
    #[must_use]
    pub fn fields(&self) -> &[StructShapeField] {
        &self.fields
    }
}

impl ValueKind {
    /// A dimensionless scalar.
    #[must_use]
    pub const fn dimensionless() -> Self {
        Self::Scalar(DimMonomial::dimensionless())
    }

    /// A scalar with a concrete fixed dimension.
    #[must_use]
    const fn scalar(dim: Dimension) -> Self {
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

/// A typed, serializable function signature: declared dimension and index
/// variables, named parameters, and the result kind.
///
/// Construction goes through [`FunctionSignature::try_new`], which enforces
/// the invariants that make call-site checking decidable:
///
/// - Declared dimension variables are distinct; declared index variables are
///   distinct.
/// - Monomial factors carry no zero exponents and no duplicate variables.
/// - Every referenced dimension variable is declared, and every declared
///   dimension variable has a *binding occurrence*: a parameter whose scalar
///   or array-element monomial is exactly that bare variable (`var^1`),
///   appearing before (or as) the variable's first use in any compound
///   monomial. Binding occurrences are what unify a variable with a concrete
///   argument dimension at a call site; compound monomials are then checked
///   by direct evaluation, never by solving equations.
/// - Every referenced index variable is declared, and every declared index
///   variable indexes at least one array parameter. A result array reuses an
///   index variable some parameter binds — output lengths always come from
///   inputs (the dynamic-index fence stays closed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSignature {
    dim_vars: Vec<DimVarName>,
    index_vars: Vec<IndexVarName>,
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
        index_vars: Vec<IndexVarName>,
        params: Vec<FunctionParam>,
        result: ValueKind,
    ) -> Result<Self, SignatureError> {
        let mut declared: HashSet<&DimVarName> = HashSet::new();
        for var in &dim_vars {
            if !declared.insert(var) {
                return Err(SignatureError::DuplicateDimVar { var: var.clone() });
            }
        }
        let mut declared_indexes: HashSet<&IndexVarName> = HashSet::new();
        for var in &index_vars {
            if !declared_indexes.insert(var) {
                return Err(SignatureError::DuplicateIndexVar { var: var.clone() });
            }
        }

        let mut bound: HashSet<&DimVarName> = HashSet::new();
        let mut used_indexes: HashSet<&IndexVarName> = HashSet::new();
        for param in &params {
            let monomial = match &param.kind {
                ValueKind::Scalar(monomial) => monomial,
                ValueKind::Indexed { element, index } => {
                    if !declared_indexes.contains(index) {
                        return Err(SignatureError::UndeclaredIndexVar { var: index.clone() });
                    }
                    used_indexes.insert(index);
                    element
                }
                ValueKind::Struct(_) => {
                    return Err(SignatureError::StructParam {
                        param: param.name.clone(),
                    });
                }
                ValueKind::Bool | ValueKind::Int => continue,
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

        let result_monomial = match &result {
            ValueKind::Scalar(monomial) => Some(monomial),
            ValueKind::Indexed { element, index } => {
                if !declared_indexes.contains(index) {
                    return Err(SignatureError::UndeclaredIndexVar { var: index.clone() });
                }
                if !used_indexes.contains(index) {
                    return Err(SignatureError::UnboundResultIndexVar { var: index.clone() });
                }
                Some(element)
            }
            // Struct fields are concrete (validated by StructShape::try_new);
            // no variables to bind.
            ValueKind::Bool | ValueKind::Int | ValueKind::Struct(_) => None,
        };
        if let Some(monomial) = result_monomial {
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
        if let Some(var) = index_vars.iter().find(|var| !used_indexes.contains(var)) {
            return Err(SignatureError::IndexVarNeverUsed { var: var.clone() });
        }

        Ok(Self {
            dim_vars,
            index_vars,
            params,
            result,
        })
    }

    /// The declared dimension variables, in declaration order.
    #[must_use]
    pub fn dim_vars(&self) -> &[DimVarName] {
        &self.dim_vars
    }

    /// The declared index variables, in declaration order.
    #[must_use]
    pub fn index_vars(&self) -> &[IndexVarName] {
        &self.index_vars
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

    /// Render this signature as `<D1: Dim, I: Index>(name: kind, ...) -> kind`,
    /// using `format_dim` to render concrete dimensions.
    ///
    /// This is a display boundary (hover, signature help, diagnostics); the
    /// checker and evaluator pattern-match the typed parts instead.
    #[must_use]
    pub fn format_with(&self, mut format_dim: impl FnMut(&Dimension) -> String) -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        if !self.dim_vars.is_empty() || !self.index_vars.is_empty() {
            let vars: Vec<String> = self
                .dim_vars
                .iter()
                .map(|var| format!("{}: Dim", var.as_str()))
                .chain(
                    self.index_vars
                        .iter()
                        .map(|var| format!("{}: Index", var.as_str())),
                )
                .collect();
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

// ---------------------------------------------------------------------------
// Structural equivalence
// ---------------------------------------------------------------------------

impl FunctionSignature {
    /// Whether `self` and `other` denote the same calling contract.
    ///
    /// Two signatures are structurally equivalent when their parameter and
    /// result kinds match up to a bijective renaming of dimension variables
    /// and reordering of the factors within each monomial. Parameter names do
    /// not participate: they are documentation, and the extern declaration
    /// (not a plugin manifest) is their authoritative source. The order of
    /// the binder list itself is likewise cosmetic; what matters is which
    /// parameters share a variable and with what powers.
    ///
    /// This is the comparison the plugin loader uses to verify an extern
    /// declaration against the signature embedded in a plugin's manifest
    /// (Phase B of #25).
    #[must_use]
    pub fn structurally_equivalent(&self, other: &Self) -> bool {
        self.canonical_form() == other.canonical_form()
    }

    /// Rewrite this signature with dimension and index variables numbered by
    /// first occurrence across the parameter list and monomial factors sorted
    /// by that numbering.
    ///
    /// The use-before-binding invariant makes first occurrence well-defined
    /// and rename-invariant: a dimension variable's first appearance in
    /// parameter order is always its bare binding occurrence.
    fn canonical_form(&self) -> CanonicalSignature {
        let mut order = CanonicalOrder::default();
        let params = self
            .params
            .iter()
            .map(|param| canonical_kind(&param.kind, &mut order))
            .collect();
        let result = canonical_kind(&self.result, &mut order);
        CanonicalSignature {
            dim_var_count: self.dim_vars.len(),
            index_var_count: self.index_vars.len(),
            params,
            result,
        }
    }
}

/// A [`FunctionSignature`] with dimension and index variables replaced by
/// occurrence indices; equality on this form is structural equivalence.
#[derive(PartialEq, Eq)]
struct CanonicalSignature {
    dim_var_count: usize,
    index_var_count: usize,
    params: Vec<CanonicalValueKind>,
    result: CanonicalValueKind,
}

/// [`ValueKind`] in canonical form.
#[derive(PartialEq, Eq)]
enum CanonicalValueKind {
    Scalar(CanonicalMonomial),
    Bool,
    Int,
    Indexed {
        element: CanonicalMonomial,
        index: usize,
    },
    /// Struct shapes carry no variables; field names, order, and kinds are
    /// the contract and compare verbatim.
    Struct(StructShape),
}

/// [`DimMonomial`] in canonical form: variable factors as
/// `(occurrence index, power)` sorted by index.
#[derive(PartialEq, Eq)]
struct CanonicalMonomial {
    vars: Vec<(usize, Rational)>,
    fixed: Dimension,
}

/// First-occurrence numbering state shared across a signature's kinds.
#[derive(Default)]
struct CanonicalOrder<'a> {
    dims: Vec<&'a DimVarName>,
    indexes: Vec<&'a IndexVarName>,
}

fn canonical_kind<'a>(kind: &'a ValueKind, order: &mut CanonicalOrder<'a>) -> CanonicalValueKind {
    match kind {
        ValueKind::Bool => CanonicalValueKind::Bool,
        ValueKind::Int => CanonicalValueKind::Int,
        ValueKind::Scalar(monomial) => {
            CanonicalValueKind::Scalar(canonical_monomial(monomial, order))
        }
        ValueKind::Indexed { element, index } => CanonicalValueKind::Indexed {
            element: canonical_monomial(element, order),
            index: occurrence_index(&mut order.indexes, index),
        },
        ValueKind::Struct(shape) => CanonicalValueKind::Struct(shape.clone()),
    }
}

fn canonical_monomial<'a>(
    monomial: &'a DimMonomial,
    order: &mut CanonicalOrder<'a>,
) -> CanonicalMonomial {
    let mut vars: Vec<(usize, Rational)> = monomial
        .vars
        .iter()
        .map(|factor| (occurrence_index(&mut order.dims, &factor.var), factor.power))
        .collect();
    vars.sort_unstable_by_key(|(index, _)| *index);
    CanonicalMonomial {
        vars,
        fixed: monomial.fixed.clone(),
    }
}

fn occurrence_index<'a, T: PartialEq>(order: &mut Vec<&'a T>, item: &'a T) -> usize {
    order
        .iter()
        .position(|seen| *seen == item)
        .unwrap_or_else(|| {
            order.push(item);
            order.len() - 1
        })
}

fn format_value_kind(
    kind: &ValueKind,
    format_dim: &mut impl FnMut(&Dimension) -> String,
) -> String {
    match kind {
        ValueKind::Bool => "Bool".to_string(),
        ValueKind::Int => "Int".to_string(),
        ValueKind::Scalar(monomial) => format_monomial(monomial, format_dim),
        ValueKind::Indexed { element, index } => {
            format!("{}[{index}]", format_monomial(element, format_dim))
        }
        ValueKind::Struct(shape) => {
            let fields: Vec<String> = shape
                .fields()
                .iter()
                .map(|field| {
                    let kind = match &field.kind {
                        StructFieldKind::Bool => "Bool".to_string(),
                        StructFieldKind::Int => "Int".to_string(),
                        StructFieldKind::Scalar(dim) => {
                            let rendered = format_dim(dim);
                            if rendered.is_empty() {
                                "Dimensionless".to_string()
                            } else {
                                rendered
                            }
                        }
                    };
                    format!("{}: {kind}", field.name)
                })
                .collect();
            format!("{{ {} }}", fields.join(", "))
        }
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
    /// The same index variable was declared twice.
    #[error("index variable `{var}` is declared more than once")]
    DuplicateIndexVar {
        /// The duplicated variable.
        var: IndexVarName,
    },
    /// An array kind referenced an index variable that was not declared.
    #[error("index variable `{var}` is not declared by this signature")]
    UndeclaredIndexVar {
        /// The undeclared variable.
        var: IndexVarName,
    },
    /// The result array's index variable indexes no parameter.
    #[error(
        "result array is indexed by `{var}`, which no array parameter uses; a function cannot invent its output length"
    )]
    UnboundResultIndexVar {
        /// The unbound index variable.
        var: IndexVarName,
    },
    /// A declared index variable indexes no array parameter.
    #[error("index variable `{var}` is declared but indexes no array parameter")]
    IndexVarNeverUsed {
        /// The never-used variable.
        var: IndexVarName,
    },
    /// A struct appeared in parameter position.
    #[error(
        "parameter `{param}` has a struct type; struct values only cross the plugin boundary as results — pass the fields as separate parameters"
    )]
    StructParam {
        /// The offending parameter.
        param: FnParamName,
    },
    /// A struct shape declared no fields.
    #[error("a struct return must have at least one field")]
    EmptyStructShape,
    /// A struct shape repeated a field name.
    #[error("struct field `{field}` is declared more than once")]
    DuplicateStructField {
        /// The repeated field.
        field: crate::syntax::type_name::FieldName,
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
    FunctionSignature::try_new(dim_vars, Vec::new(), params, result)
        .expect("built-in signature shape must be valid")
}

impl FunctionSignature {
    /// All params dimensionless scalars, dimensionless scalar result.
    #[must_use]
    pub(crate) fn all_dimensionless(names: &[&str]) -> Self {
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
    pub(crate) fn passthrough(name: &str) -> Self {
        let d = dim_var_d();
        expect_signature(
            vec![d.clone()],
            vec![param(name, ValueKind::Scalar(DimMonomial::var(d.clone())))],
            ValueKind::Scalar(DimMonomial::var(d)),
        )
    }

    /// Single free param `D`, result is a fixed dimension.
    #[must_use]
    pub(crate) fn free_to_fixed(name: &str, output: Dimension) -> Self {
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
    pub(crate) fn same_dim(names: &[&str]) -> Self {
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
    pub(crate) fn same_dim_to_fixed(names: &[&str], output: Dimension) -> Self {
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
            Vec::new(),
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
            Vec::new(),
            vec![
                param(
                    "x",
                    ValueKind::Scalar(DimMonomial::var_pow(
                        var("D"),
                        Rational::try_new(2, 1).unwrap(),
                    )),
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
            Vec::new(),
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
        assert_eq!(rendered, "<D: Dim>(x: D) -> D^(1/2)");
    }

    /// `<vars>(params) -> result` shorthand for equivalence tests.
    fn sig(dim_vars: &[&str], params: &[ValueKind], result: ValueKind) -> FunctionSignature {
        FunctionSignature::try_new(
            dim_vars.iter().map(|v| var(v)).collect(),
            Vec::new(),
            params
                .iter()
                .enumerate()
                .map(|(i, kind)| param(&format!("p{i}"), kind.clone()))
                .collect(),
            result,
        )
        .unwrap()
    }

    fn bare(name: &str) -> ValueKind {
        ValueKind::Scalar(DimMonomial::var(var(name)))
    }

    #[test]
    fn equivalence_ignores_variable_names_and_param_names() {
        let declared = FunctionSignature::try_new(
            vec![var("D")],
            Vec::new(),
            vec![
                param("a", bare("D")),
                param("b", bare("D")),
                param("t", ValueKind::dimensionless()),
            ],
            bare("D"),
        )
        .unwrap();
        let manifest = FunctionSignature::try_new(
            vec![var("T")],
            Vec::new(),
            vec![
                param("lo", bare("T")),
                param("hi", bare("T")),
                param("frac", ValueKind::dimensionless()),
            ],
            bare("T"),
        )
        .unwrap();
        assert!(declared.structurally_equivalent(&manifest));
    }

    #[test]
    fn equivalence_ignores_monomial_factor_order_and_binder_order() {
        let product = |first: &str, second: &str| {
            ValueKind::Scalar(DimMonomial {
                vars: vec![
                    DimVarPower {
                        var: var(first),
                        power: Rational::ONE,
                    },
                    DimVarPower {
                        var: var(second),
                        power: Rational::ONE,
                    },
                ],
                fixed: Dimension::dimensionless(),
            })
        };
        let a = sig(
            &["D1", "D2"],
            &[bare("D1"), bare("D2")],
            product("D1", "D2"),
        );
        let b = sig(
            &["E2", "E1"],
            &[bare("E2"), bare("E1")],
            product("E1", "E2"),
        );
        assert!(a.structurally_equivalent(&b));
    }

    #[test]
    fn equivalence_distinguishes_variable_identification() {
        // (x: D, y: D) forces both dimensions equal; (x: D1, y: D2) does not.
        let same = sig(&["D"], &[bare("D"), bare("D")], bare("D"));
        let free = sig(&["D1", "D2"], &[bare("D1"), bare("D2")], bare("D1"));
        assert!(!same.structurally_equivalent(&free));
        assert!(!free.structurally_equivalent(&same));
    }

    #[test]
    fn equivalence_distinguishes_kinds_powers_dims_and_arity() {
        let passthrough = FunctionSignature::passthrough("x");
        assert!(
            !passthrough
                .structurally_equivalent(&FunctionSignature::free_to_pow("x", Rational::HALF))
        );
        assert!(
            !FunctionSignature::fixed_to_fixed("x", length(), time()).structurally_equivalent(
                &FunctionSignature::fixed_to_fixed("x", length(), length())
            )
        );
        assert!(
            !sig(&[], &[ValueKind::Bool], ValueKind::Int).structurally_equivalent(&sig(
                &[],
                &[ValueKind::Int],
                ValueKind::Int
            ))
        );
        assert!(
            !FunctionSignature::same_dim(&["a", "b"])
                .structurally_equivalent(&FunctionSignature::passthrough("x"))
        );
    }

    #[test]
    fn equivalence_accepts_identical_signatures() {
        let sig = FunctionSignature::same_dim_to_fixed(&["y", "x"], length());
        assert!(sig.structurally_equivalent(&sig.clone()));
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

    // -- Index-variable (array) invariants ---------------------------------

    fn ivar(name: &str) -> IndexVarName {
        IndexVarName::expect_valid(name)
    }

    fn array(dim_var: &str, index_var: &str) -> ValueKind {
        ValueKind::Indexed {
            element: DimMonomial::var(var(dim_var)),
            index: ivar(index_var),
        }
    }

    /// `smooth<D: Dim, I: Index>(xs: D[I], window: Dimensionless) -> D[I]`.
    fn smooth_signature() -> FunctionSignature {
        FunctionSignature::try_new(
            vec![var("D")],
            vec![ivar("I")],
            vec![
                param("xs", array("D", "I")),
                param("window", ValueKind::dimensionless()),
            ],
            array("D", "I"),
        )
        .unwrap()
    }

    #[test]
    fn array_element_binds_dimension_variable() {
        // `xs: D[I]` is a binding occurrence for `D`: the checker reads the
        // element dimension straight off the indexed argument.
        let sig = smooth_signature();
        assert_eq!(sig.dim_vars().len(), 1);
        assert_eq!(sig.index_vars().len(), 1);
    }

    #[test]
    fn duplicate_index_var_is_rejected() {
        let err = FunctionSignature::try_new(
            vec![var("D")],
            vec![ivar("I"), ivar("I")],
            vec![param("xs", array("D", "I"))],
            ValueKind::dimensionless(),
        )
        .unwrap_err();
        assert!(matches!(err, SignatureError::DuplicateIndexVar { .. }));
    }

    #[test]
    fn undeclared_index_var_is_rejected() {
        let err = FunctionSignature::try_new(
            vec![var("D")],
            Vec::new(),
            vec![param("xs", array("D", "I"))],
            ValueKind::dimensionless(),
        )
        .unwrap_err();
        assert!(matches!(err, SignatureError::UndeclaredIndexVar { .. }));
    }

    #[test]
    fn result_index_var_must_index_a_parameter() {
        // A function inventing its output length is the dynamic-index
        // problem — the result must reuse an input index variable.
        let err = FunctionSignature::try_new(
            vec![var("D")],
            vec![ivar("I")],
            vec![param("x", bare("D"))],
            array("D", "I"),
        )
        .unwrap_err();
        assert!(matches!(err, SignatureError::UnboundResultIndexVar { .. }));
    }

    #[test]
    fn declared_but_never_used_index_var_is_rejected() {
        let err = FunctionSignature::try_new(
            vec![var("D")],
            vec![ivar("I")],
            vec![param("x", bare("D"))],
            bare("D"),
        )
        .unwrap_err();
        assert!(matches!(err, SignatureError::IndexVarNeverUsed { .. }));
    }

    #[test]
    fn compound_array_element_requires_prior_binding() {
        let err = FunctionSignature::try_new(
            vec![var("D")],
            vec![ivar("I")],
            vec![param(
                "xs",
                ValueKind::Indexed {
                    element: DimMonomial::var_pow(var("D"), Rational::try_new(2, 1).unwrap()),
                    index: ivar("I"),
                },
            )],
            ValueKind::dimensionless(),
        )
        .unwrap_err();
        assert!(matches!(err, SignatureError::UseBeforeBinding { .. }));
    }

    #[test]
    fn array_equivalence_is_alpha_on_index_vars() {
        let a = smooth_signature();
        let b = FunctionSignature::try_new(
            vec![var("T")],
            vec![ivar("J")],
            vec![
                param("data", array("T", "J")),
                param("w", ValueKind::dimensionless()),
            ],
            array("T", "J"),
        )
        .unwrap();
        assert!(a.structurally_equivalent(&b));
    }

    #[test]
    fn array_equivalence_distinguishes_index_identification() {
        // (xs: D[I], ys: D[I]) forces both indexes equal; (xs: D[I], ys: D[J])
        // does not — the distinction must survive canonicalization.
        let same = FunctionSignature::try_new(
            vec![var("D")],
            vec![ivar("I")],
            vec![param("xs", array("D", "I")), param("ys", array("D", "I"))],
            array("D", "I"),
        )
        .unwrap();
        let free = FunctionSignature::try_new(
            vec![var("D")],
            vec![ivar("I"), ivar("J")],
            vec![param("xs", array("D", "I")), param("ys", array("D", "J"))],
            array("D", "I"),
        )
        .unwrap();
        assert!(!same.structurally_equivalent(&free));
        assert!(!free.structurally_equivalent(&same));
    }

    #[test]
    fn array_is_not_equivalent_to_scalar() {
        let arr = FunctionSignature::try_new(
            vec![var("D")],
            vec![ivar("I")],
            vec![param("xs", array("D", "I"))],
            bare("D"),
        )
        .unwrap();
        let scalar = FunctionSignature::passthrough("xs");
        assert!(!arr.structurally_equivalent(&scalar));
    }

    // -- Struct-return shapes ----------------------------------------------

    fn shape(fields: &[(&str, StructFieldKind)]) -> StructShape {
        StructShape::try_new(
            fields
                .iter()
                .map(|(name, kind)| StructShapeField {
                    name: crate::syntax::type_name::FieldName::expect_valid(*name),
                    kind: kind.clone(),
                })
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn struct_shapes_reject_empty_and_duplicate_fields() {
        assert!(matches!(
            StructShape::try_new(Vec::new()).unwrap_err(),
            SignatureError::EmptyStructShape
        ));
        let field = StructShapeField {
            name: crate::syntax::type_name::FieldName::expect_valid("x"),
            kind: StructFieldKind::Int,
        };
        assert!(matches!(
            StructShape::try_new(vec![field.clone(), field]).unwrap_err(),
            SignatureError::DuplicateStructField { .. }
        ));
    }

    #[test]
    fn struct_params_are_rejected() {
        let err = FunctionSignature::try_new(
            Vec::new(),
            Vec::new(),
            vec![param(
                "x",
                ValueKind::Struct(shape(&[("lo", StructFieldKind::Int)])),
            )],
            ValueKind::Int,
        )
        .unwrap_err();
        assert!(matches!(err, SignatureError::StructParam { .. }));
    }

    #[test]
    fn struct_results_compare_field_names_and_order() {
        let lo_hi = |names: (&str, &str)| {
            FunctionSignature::try_new(
                vec![var("D")],
                vec![ivar("I")],
                vec![param("xs", array("D", "I"))],
                ValueKind::Struct(shape(&[
                    (names.0, StructFieldKind::Scalar(length())),
                    (names.1, StructFieldKind::Scalar(length())),
                ])),
            )
            .unwrap()
        };
        let declared = lo_hi(("lo", "hi"));
        assert!(declared.structurally_equivalent(&lo_hi(("lo", "hi"))));
        // Field names are labels users access — part of the contract.
        assert!(!declared.structurally_equivalent(&lo_hi(("minimum", "maximum"))));
        // So is their order.
        assert!(!declared.structurally_equivalent(&lo_hi(("hi", "lo"))));
    }

    #[test]
    fn format_with_renders_struct_shapes() {
        let sig = FunctionSignature::try_new(
            vec![var("D")],
            vec![ivar("I")],
            vec![param("xs", array("D", "I"))],
            ValueKind::Struct(shape(&[
                ("lo", StructFieldKind::Scalar(Dimension::dimensionless())),
                ("ok", StructFieldKind::Bool),
            ])),
        )
        .unwrap();
        let rendered = sig.format_with(|dim| {
            if dim.is_dimensionless() {
                String::new()
            } else {
                format!("{dim:?}")
            }
        });
        assert_eq!(
            rendered,
            "<D: Dim, I: Index>(xs: D[I]) -> { lo: Dimensionless, ok: Bool }"
        );
    }

    #[test]
    fn format_with_renders_array_kinds() {
        // Mirror real callers: a dimensionless fixed factor renders empty and
        // falls back to the `Dimensionless` spelling.
        let rendered = smooth_signature().format_with(|dim| {
            if dim.is_dimensionless() {
                String::new()
            } else {
                format!("{dim:?}")
            }
        });
        assert_eq!(
            rendered,
            "<D: Dim, I: Index>(xs: D[I], window: Dimensionless) -> D[I]"
        );
    }
}
