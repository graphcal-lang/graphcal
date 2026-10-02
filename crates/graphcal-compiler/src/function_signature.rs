//! Function-signature IR: the typed dimensional calling convention shared by
//! built-in and externally-provided (plugin) functions.
//!
//! A [`FunctionSignature`] describes how a typed kernel function interacts
//! with the type system: which dimension and index variables it declares, what value
//! kind each named parameter requires, and how the result kind is computed
//! from the bound dimension variables. Param and result dimensions are
//! [`DimMonomial`]s — products of dimension-variable powers and a fixed
//! [`Dimension`] — generalizing single-variable forms like `D -> D^(1/2)`
//! to cross-variable algebra such as `(D1, D2) -> D1 * D2`.
//!
//! Signature parts are written with binders referenced by name (the
//! `Named*` aliases); [`FunctionSignature::try_from_parts`] resolves every
//! reference to a [`DimBinder`] / [`IndexBinder`], which identifies the binder
//! by its declaration position. Binder identity is therefore positional, and
//! structural equivalence is a comparison of binder positions up to a
//! renumbering rather than a comparison of spellings.
//!
//! This module owns only the pure signature algebra. Interpreting a signature
//! against inferred argument types (producing diagnostics) lives in the
//! dimension checker; evaluating the kernel lives in the evaluator. The model
//! is plain data end-to-end so boundaries (Phase B plugin manifests) can
//! serialize it without the core ever carrying strings.

use std::collections::{HashMap, HashSet};
use std::fmt;

use thiserror::Error;

use crate::dimension::{Dimension, Rational};
use crate::ratio::{ExponentStyle, RatioError};
use crate::sparse_monomial::{MonomialFactorError, SparseMonomial};
use crate::syntax::dimension::DimVarName;
use crate::syntax::function_name::FnParamName;
use crate::syntax::index_name::IndexVarName;
use crate::syntax::non_empty::NonEmpty;

/// A display callback rendering a concrete [`Dimension`].
pub type DimFormatter<'a> = dyn FnMut(&Dimension) -> String + 'a;

/// A dimension variable declared by a signature, identified by its position
/// in [`FunctionSignature::dim_vars`]. The name is carried for display only.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DimBinder {
    index: usize,
    name: DimVarName,
}

impl DimBinder {
    /// Declaration position.
    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }

    /// Declared spelling (display only).
    #[must_use]
    pub const fn name(&self) -> &DimVarName {
        &self.name
    }
}

impl fmt::Display for DimBinder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.name, f)
    }
}

/// An index variable declared by a signature, identified by its position in
/// [`FunctionSignature::index_vars`]. The name is carried for display only.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IndexBinder {
    index: usize,
    name: IndexVarName,
}

impl IndexBinder {
    /// Declaration position.
    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }

    /// Declared spelling (display only).
    #[must_use]
    pub const fn name(&self) -> &IndexVarName {
        &self.name
    }
}

impl fmt::Display for IndexBinder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.name, f)
    }
}

/// A dimension monomial: a product of dimension-variable powers and a fixed
/// dimension, e.g. `D1 * D2^2 * Length^-1`.
///
/// `V` is the variable reference: a [`DimVarName`] while a signature is being
/// written, a [`DimBinder`] once it is validated. Variable factors are kept
/// sorted, distinct, and non-zero by [`SparseMonomial`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimMonomial<V = DimBinder> {
    vars: SparseMonomial<V, Rational>,
    fixed: Dimension,
}

/// A [`DimMonomial`] as written, with variables referenced by name.
pub type NamedDimMonomial = DimMonomial<DimVarName>;

impl<V> DimMonomial<V> {
    /// A monomial with no variable factors: just a concrete dimension.
    #[must_use]
    pub const fn fixed(dim: Dimension) -> Self {
        Self {
            vars: SparseMonomial::one(),
            fixed: dim,
        }
    }

    /// The dimensionless monomial (no variables, dimensionless fixed factor).
    #[must_use]
    pub const fn dimensionless() -> Self {
        Self::fixed(Dimension::dimensionless())
    }

    /// The concrete dimension factor ([`Dimension::dimensionless`] when absent).
    #[must_use]
    pub const fn fixed_factor(&self) -> &Dimension {
        &self.fixed
    }

    /// The variable factors, sorted by variable.
    pub fn var_factors(&self) -> impl Iterator<Item = (&V, Rational)> {
        self.vars.iter().map(|(var, &power)| (var, power))
    }

    /// Returns whether this monomial references no dimension variables.
    #[must_use]
    pub fn is_concrete(&self) -> bool {
        self.vars.is_empty()
    }

    /// Returns the variable when this monomial is exactly one bare variable
    /// (`var^1` with a dimensionless fixed factor) — the only shape that can
    /// *bind* a dimension variable at a call site.
    #[must_use]
    pub fn as_bare_var(&self) -> Option<&V> {
        match self.vars.as_single() {
            Some((var, &Rational::ONE)) if self.fixed.is_dimensionless() => Some(var),
            _ => None,
        }
    }
}

impl<V: Ord> DimMonomial<V> {
    /// A monomial from explicitly written variable factors and a fixed factor.
    ///
    /// # Errors
    ///
    /// Returns [`MonomialFactorError`] when a variable repeats or carries a
    /// zero exponent.
    pub fn try_new(
        vars: impl IntoIterator<Item = (V, Rational)>,
        fixed: Dimension,
    ) -> Result<Self, MonomialFactorError<V>> {
        Ok(Self {
            vars: SparseMonomial::try_from_factors(vars)?,
            fixed,
        })
    }

    /// A bare dimension variable: `var^1`.
    #[must_use]
    pub fn var(var: V) -> Self {
        Self::var_pow(var, Rational::ONE)
    }

    /// A single dimension-variable power: `var^power`.
    #[must_use]
    fn var_pow(var: V, power: Rational) -> Self {
        Self {
            vars: SparseMonomial::single(var, power),
            fixed: Dimension::dimensionless(),
        }
    }
}

impl DimMonomial {
    /// Compute the concrete dimension of this monomial under `lookup`, which
    /// maps each referenced binder to its bound dimension.
    ///
    /// # Errors
    ///
    /// Returns [`DimMonomialEvalError::UnboundVar`] when `lookup` has no
    /// binding for a referenced binder, and
    /// [`DimMonomialEvalError::Overflow`] when exponent arithmetic overflows.
    pub(crate) fn eval<'a>(
        &self,
        mut lookup: impl FnMut(&DimBinder) -> Option<&'a Dimension>,
    ) -> Result<Dimension, DimMonomialEvalError> {
        let mut result = self.fixed.clone();
        for (var, power) in self.var_factors() {
            let bound =
                lookup(var).ok_or_else(|| DimMonomialEvalError::UnboundVar { var: var.clone() })?;
            let powered = bound.pow(power)?;
            result = result.checked_mul(&powered)?;
        }
        Ok(result)
    }
}

impl<V: fmt::Display> DimMonomial<V> {
    /// Render as `.gcl` dimension-expression syntax (`D1 * D2^2 * Length`),
    /// using `format_dim` for the fixed factor; `Dimensionless` when empty.
    #[must_use]
    pub fn format_with(&self, format_dim: &mut dyn FnMut(&Dimension) -> String) -> String {
        let mut parts: Vec<String> = self
            .var_factors()
            .map(|(var, power)| {
                if power == Rational::ONE {
                    var.to_string()
                } else {
                    format!("{var}{}", power.fmt_exponent(ExponentStyle::Source))
                }
            })
            .collect();
        if !self.fixed.is_dimensionless() {
            parts.push(format_dim(&self.fixed));
        }
        if parts.is_empty() {
            non_empty_dimension(format_dim(&self.fixed))
        } else {
            parts.join(" * ")
        }
    }
}

fn non_empty_dimension(rendered: String) -> String {
    if rendered.is_empty() {
        "Dimensionless".to_string()
    } else {
        rendered
    }
}

/// Error from evaluating a [`DimMonomial`] against variable bindings.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DimMonomialEvalError {
    /// A referenced dimension variable had no binding.
    #[error("dimension variable `{var}` is unbound")]
    UnboundVar {
        /// The unbound variable.
        var: DimBinder,
    },
    /// Exponent arithmetic overflowed.
    #[error(transparent)]
    Overflow(#[from] RatioError),
}

/// One scalar value kind supported by a function signature.
///
/// The same closed set is used for standalone values and indexed leaves, so
/// `Bool[I]` and `Int[I]` retain their semantic kinds instead of being
/// disguised as dimensionless quantities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScalarValueKind<V = DimBinder> {
    /// A quantity with the dimension given by the monomial.
    Quantity(DimMonomial<V>),
    /// A boolean value.
    Bool,
    /// An integer value.
    Int,
}

/// The kind of a parameter value in a function signature.
///
/// Parameters are flat: a struct cannot be passed (callers pass its fields
/// as separate arguments), so there is no struct variant here. Results may
/// additionally be structs; see [`ResultKind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamKind<V = DimBinder, I = IndexBinder> {
    /// A standalone scalar value.
    Scalar(ScalarValueKind<V>),
    /// An indexed scalar collection over declared axis variables:
    /// `element[I, J]`.
    ///
    /// Each axis variable is bound by an argument's concrete typed index at
    /// the call site. Every result axis must reuse a variable bound by some
    /// parameter — a function can reorder axes but cannot invent an output
    /// extent.
    Indexed {
        /// The semantic scalar element kind.
        element: ScalarValueKind<V>,
        /// Index variables naming the array's axes, in row-major order.
        indexes: NonEmpty<I>,
    },
}

/// A [`ParamKind`] as written, with binders referenced by name.
pub type NamedParamKind = ParamKind<DimVarName, IndexVarName>;

/// The kind of a result value in a function signature.
///
/// `S` is the struct-result payload. A plugin manifest only knows the
/// structural [`StructShape`]; an extern declaration additionally binds that
/// shape to a nominal record type, so its payload carries both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultKind<S = StructShape, V = DimBinder, I = IndexBinder> {
    /// A value of any kind a parameter can have.
    Value(ParamKind<V, I>),
    /// A record value described by its flattened field shape.
    Struct(S),
}

/// A [`ResultKind`] as written, with binders referenced by name.
pub type NamedResultKind<S = StructShape> = ResultKind<S, DimVarName, IndexVarName>;

impl<S: Clone, V: Clone, I> ResultKind<S, V, I> {
    /// The same result kind with each index variable of an indexed result,
    /// in row-major axis order, replaced by `index` of its axis position.
    ///
    /// # Errors
    ///
    /// Returns the first error of `index`.
    pub fn try_map_indexes<J, E>(
        &self,
        mut index: impl FnMut(usize, &I) -> Result<J, E>,
    ) -> Result<ResultKind<S, V, J>, E> {
        Ok(match self {
            Self::Value(ParamKind::Scalar(scalar)) => {
                ResultKind::Value(ParamKind::Scalar(scalar.clone()))
            }
            Self::Value(ParamKind::Indexed { element, indexes }) => {
                let mut depth = 0..;
                ResultKind::Value(ParamKind::Indexed {
                    element: element.clone(),
                    indexes: indexes
                        .try_map_ref(|binder| index(depth.next().unwrap_or(usize::MAX), binder))?,
                })
            }
            Self::Struct(payload) => ResultKind::Struct(payload.clone()),
        })
    }
}

impl<S, V, I> From<ParamKind<V, I>> for ResultKind<S, V, I> {
    fn from(kind: ParamKind<V, I>) -> Self {
        Self::Value(kind)
    }
}

/// A struct-result payload: anything that exposes the flattened field shape
/// that crosses the plugin boundary.
pub trait StructResult {
    /// The structural field layout of the record.
    fn shape(&self) -> &StructShape;
}

impl StructResult for StructShape {
    fn shape(&self) -> &StructShape {
        self
    }
}

/// The flattened field layout of a record return: named fields of concrete
/// quantity, boolean, or integer kinds, in declaration order.
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

/// The kind of one struct field: concrete quantities only — dimension
/// variables cannot appear in struct returns in this phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructFieldKind {
    /// A quantity with a concrete fixed dimension.
    Quantity(Dimension),
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

impl<V, I> ParamKind<V, I> {
    /// A boolean scalar.
    #[must_use]
    pub const fn bool() -> Self {
        Self::Scalar(ScalarValueKind::Bool)
    }

    /// An integer scalar.
    #[must_use]
    pub const fn int() -> Self {
        Self::Scalar(ScalarValueKind::Int)
    }

    /// A quantity scalar with the given dimension monomial.
    #[must_use]
    pub const fn quantity_monomial(monomial: DimMonomial<V>) -> Self {
        Self::Scalar(ScalarValueKind::Quantity(monomial))
    }

    /// A dimensionless quantity.
    #[must_use]
    pub const fn dimensionless() -> Self {
        Self::quantity_monomial(DimMonomial::dimensionless())
    }

    /// A quantity with a concrete fixed dimension.
    #[must_use]
    const fn quantity(dim: Dimension) -> Self {
        Self::quantity_monomial(DimMonomial::fixed(dim))
    }
}

/// A named parameter with its value-kind constraint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionParam<V = DimBinder, I = IndexBinder> {
    /// Parameter name, for diagnostics, hover, and signature help.
    pub name: FnParamName,
    /// The value kind this parameter requires.
    pub kind: ParamKind<V, I>,
}

impl FunctionParam {
    /// Render as `name: kind`, the parameter spelling used by
    /// [`FunctionSignature::format_with_result`], using `format_dim` for
    /// concrete dimensions.
    ///
    /// This is a display boundary (signature help parameter labels).
    #[must_use]
    pub fn format_with(&self, format_dim: &mut DimFormatter<'_>) -> String {
        format!(
            "{}: {}",
            self.name,
            format_param_kind(&self.kind, format_dim)
        )
    }
}

/// A [`FunctionParam`] as written, with binders referenced by name.
pub type NamedFunctionParam = FunctionParam<DimVarName, IndexVarName>;

/// The diagnostic spelling of one function signature.
///
/// Spelled only through [`FunctionSignature::spelling`], so a diagnostic
/// payload holding a `SignatureSpelling` always names a signature, never free
/// text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureSpelling(String);

impl fmt::Display for SignatureSpelling {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A typed, serializable function signature: declared dimension and index
/// variables, named parameters, and the result kind.
///
/// Construction goes through [`FunctionSignature::try_from_parts`], which
/// resolves binder names to positions and enforces the invariants that make
/// call-site checking decidable:
///
/// - Declared dimension variables are distinct; declared index variables are
///   distinct.
/// - Parameter names are distinct.
/// - Every referenced dimension variable is declared, and every declared
///   dimension variable has a *binding occurrence*: a parameter whose quantity
///   or array-element monomial is exactly that bare variable (`var^1`),
///   appearing before (or as) the variable's first use in any compound
///   monomial. Binding occurrences are what unify a variable with a concrete
///   argument dimension at a call site; compound monomials are then checked
///   by direct evaluation, never by solving equations.
/// - Every referenced index variable is declared, and every declared index
///   variable indexes at least one array parameter. A result array reuses an
///   index variables that parameters bind — output extents always come from
///   inputs (the dynamic-index fence stays closed).
///
/// Monomial factors carry no zero exponents and no duplicate variables by
/// construction ([`DimMonomial::try_new`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSignature<S = StructShape> {
    dim_vars: Vec<DimVarName>,
    index_vars: Vec<IndexVarName>,
    params: Vec<FunctionParam>,
    result: ResultKind<S>,
}

impl FunctionSignature {
    /// Build a validated signature whose struct result, if any, is purely
    /// structural.
    ///
    /// # Errors
    ///
    /// Returns a [`SignatureError`] describing the first violated invariant.
    pub fn try_new(
        dim_vars: Vec<DimVarName>,
        index_vars: Vec<IndexVarName>,
        params: Vec<NamedFunctionParam>,
        result: NamedResultKind,
    ) -> Result<Self, SignatureError> {
        Self::try_from_parts(dim_vars, index_vars, params, result)
    }
}

/// Name-to-position resolution for one signature's declared binders.
struct BinderScope<'a> {
    dims: &'a [DimVarName],
    indexes: &'a [IndexVarName],
}

impl BinderScope<'_> {
    fn dim(&self, name: &DimVarName) -> Result<DimBinder, SignatureError> {
        self.dims
            .iter()
            .position(|declared| declared == name)
            .map(|index| DimBinder {
                index,
                name: name.clone(),
            })
            .ok_or_else(|| SignatureError::UndeclaredDimVar { var: name.clone() })
    }

    fn index(&self, name: &IndexVarName) -> Result<IndexBinder, SignatureError> {
        self.indexes
            .iter()
            .position(|declared| declared == name)
            .map(|index| IndexBinder {
                index,
                name: name.clone(),
            })
            .ok_or_else(|| SignatureError::UndeclaredIndexVar { var: name.clone() })
    }

    fn monomial(&self, monomial: &NamedDimMonomial) -> Result<DimMonomial, SignatureError> {
        let vars = monomial
            .var_factors()
            .map(|(var, power)| Ok((self.dim(var)?, power)))
            .collect::<Result<Vec<_>, SignatureError>>()?;
        DimMonomial::try_new(vars, monomial.fixed.clone())
            .map_err(|error| SignatureError::from(error.map_key(|var| var.name)))
    }

    fn scalar(
        &self,
        kind: &ScalarValueKind<DimVarName>,
    ) -> Result<ScalarValueKind, SignatureError> {
        Ok(match kind {
            ScalarValueKind::Quantity(monomial) => {
                ScalarValueKind::Quantity(self.monomial(monomial)?)
            }
            ScalarValueKind::Bool => ScalarValueKind::Bool,
            ScalarValueKind::Int => ScalarValueKind::Int,
        })
    }

    fn indexes(
        &self,
        indexes: &NonEmpty<IndexVarName>,
    ) -> Result<NonEmpty<IndexBinder>, SignatureError> {
        indexes.try_map_ref(|index| self.index(index))
    }
}

/// Which declared binders have a binding occurrence so far, in parameter order.
struct BindingState {
    bound: Vec<bool>,
    used_indexes: Vec<bool>,
}

impl BindingState {
    fn is_bound(&self, var: &DimBinder) -> bool {
        self.bound.get(var.index).copied().unwrap_or(false)
    }

    fn is_used(&self, index: &IndexBinder) -> bool {
        self.used_indexes.get(index.index).copied().unwrap_or(false)
    }

    /// Resolve one parameter, recording its binding occurrences and rejecting
    /// a compound use of a variable no earlier parameter binds.
    fn param(
        &mut self,
        scope: &BinderScope<'_>,
        param: NamedFunctionParam,
    ) -> Result<FunctionParam, SignatureError> {
        let kind = match &param.kind {
            ParamKind::Scalar(scalar) => ParamKind::Scalar(scope.scalar(scalar)?),
            ParamKind::Indexed { element, indexes } => {
                let indexes = scope.indexes(indexes)?;
                for index in &indexes {
                    if let Some(used) = self.used_indexes.get_mut(index.index) {
                        *used = true;
                    }
                }
                ParamKind::Indexed {
                    element: scope.scalar(element)?,
                    indexes,
                }
            }
        };
        let (ParamKind::Scalar(scalar)
        | ParamKind::Indexed {
            element: scalar, ..
        }) = &kind;
        if let ScalarValueKind::Quantity(monomial) = scalar {
            match monomial.as_bare_var() {
                Some(var) => {
                    if let Some(bound) = self.bound.get_mut(var.index) {
                        *bound = true;
                    }
                }
                None => {
                    if let Some((var, _)) =
                        monomial.var_factors().find(|(var, _)| !self.is_bound(var))
                    {
                        return Err(SignatureError::UseBeforeBinding {
                            var: var.name.clone(),
                            param: param.name,
                        });
                    }
                }
            }
        }
        Ok(FunctionParam {
            name: param.name,
            kind,
        })
    }

    /// Resolve the result kind, whose variables must all be bound by
    /// parameters.
    fn result<S>(
        &self,
        scope: &BinderScope<'_>,
        result: NamedResultKind<S>,
    ) -> Result<ResultKind<S>, SignatureError> {
        let kind = match result {
            ResultKind::Struct(payload) => return Ok(ResultKind::Struct(payload)),
            ResultKind::Value(kind) => kind,
        };
        let (indexes, element) = match &kind {
            ParamKind::Scalar(scalar) => (None, scope.scalar(scalar)?),
            ParamKind::Indexed { element, indexes } => {
                (Some(scope.indexes(indexes)?), scope.scalar(element)?)
            }
        };
        if let Some(index) = indexes.iter().flatten().find(|index| !self.is_used(index)) {
            return Err(SignatureError::UnboundResultIndexVar {
                var: index.name.clone(),
            });
        }
        if let ScalarValueKind::Quantity(monomial) = &element
            && let Some((var, _)) = monomial.var_factors().find(|(var, _)| !self.is_bound(var))
        {
            return Err(SignatureError::UnboundResultVar {
                var: var.name.clone(),
            });
        }
        Ok(ResultKind::Value(match indexes {
            None => ParamKind::Scalar(element),
            Some(indexes) => ParamKind::Indexed { element, indexes },
        }))
    }
}

impl From<MonomialFactorError<DimVarName>> for SignatureError {
    fn from(error: MonomialFactorError<DimVarName>) -> Self {
        match error {
            MonomialFactorError::ZeroExponent(var) => Self::ZeroExponent { var },
            MonomialFactorError::DuplicateKey(var) => Self::DuplicateMonomialVar { var },
        }
    }
}

impl<S: StructResult> FunctionSignature<S> {
    /// Build a validated signature with any struct-result payload.
    ///
    /// # Errors
    ///
    /// Returns a [`SignatureError`] describing the first violated invariant.
    pub fn try_from_parts(
        dim_vars: Vec<DimVarName>,
        index_vars: Vec<IndexVarName>,
        params: Vec<NamedFunctionParam>,
        result: NamedResultKind<S>,
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
        validate_unique_param_names(&params)?;

        let scope = BinderScope {
            dims: &dim_vars,
            indexes: &index_vars,
        };
        let mut binding = BindingState {
            bound: vec![false; dim_vars.len()],
            used_indexes: vec![false; index_vars.len()],
        };
        let params = params
            .into_iter()
            .map(|param| binding.param(&scope, param))
            .collect::<Result<Vec<_>, _>>()?;
        let result = binding.result(&scope, result)?;
        let BindingState {
            bound,
            used_indexes,
        } = binding;

        if let Some((var, _)) = dim_vars.iter().zip(&bound).find(|(_, bound)| !**bound) {
            return Err(SignatureError::DimVarNeverBound { var: var.clone() });
        }
        if let Some((var, _)) = index_vars
            .iter()
            .zip(&used_indexes)
            .find(|(_, used)| !**used)
        {
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
    pub const fn result(&self) -> &ResultKind<S> {
        &self.result
    }

    /// The number of parameters.
    #[must_use]
    pub const fn arity(&self) -> usize {
        self.params.len()
    }

    /// Render this signature as `<D1: Dim, I: Index>(name: kind, ...) -> kind`,
    /// using `format_dim` to render concrete dimensions and spelling a struct
    /// result as its field list.
    ///
    /// This is a display boundary (hover, signature help, diagnostics); the
    /// checker and evaluator pattern-match the typed parts instead.
    #[must_use]
    pub fn format_with(&self, mut format_dim: impl FnMut(&Dimension) -> String) -> String {
        self.format_with_result(&mut format_dim, &mut |payload, format_dim| {
            format_struct_shape(payload.shape(), format_dim)
        })
    }

    /// The diagnostic spelling of this signature: [`Self::format_with`] as a
    /// typed payload.
    #[must_use]
    pub fn spelling(&self, format_dim: impl FnMut(&Dimension) -> String) -> SignatureSpelling {
        SignatureSpelling(self.format_with(format_dim))
    }

    /// Render like [`Self::format_with`], but spell a struct result with
    /// `format_struct` (for example, as a nominal type name).
    #[must_use]
    pub fn format_with_result(
        &self,
        format_dim: &mut DimFormatter<'_>,
        format_struct: &mut dyn FnMut(&S, &mut DimFormatter<'_>) -> String,
    ) -> String {
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
            out.push_str(&param.format_with(format_dim));
        }
        out.push_str(") -> ");
        match &self.result {
            ResultKind::Value(kind) => out.push_str(&format_param_kind(kind, format_dim)),
            ResultKind::Struct(payload) => out.push_str(&format_struct(payload, format_dim)),
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Structural equivalence
// ---------------------------------------------------------------------------

/// Binder positions renumbered by first occurrence across a signature's
/// parameter list.
///
/// The use-before-binding invariant makes first occurrence well-defined and
/// rename-invariant: a dimension variable's first appearance in parameter
/// order is always its bare binding occurrence, and every declared index
/// variable indexes some parameter.
struct FirstOccurrence {
    dims: Vec<Option<usize>>,
    indexes: Vec<Option<usize>>,
}

impl FirstOccurrence {
    fn of<S>(signature: &FunctionSignature<S>) -> Self {
        let mut order = Self {
            dims: vec![None; signature.dim_vars.len()],
            indexes: vec![None; signature.index_vars.len()],
        };
        let mut next_dim = 0;
        let mut next_index = 0;
        for param in &signature.params {
            let (element, indexes) = match &param.kind {
                ParamKind::Scalar(scalar) => (scalar, None),
                ParamKind::Indexed { element, indexes } => (element, Some(indexes)),
            };
            if let ScalarValueKind::Quantity(monomial) = element {
                for (var, _) in monomial.var_factors() {
                    number(&mut order.dims, var.index, &mut next_dim);
                }
            }
            for index in indexes.into_iter().flatten() {
                number(&mut order.indexes, index.index, &mut next_index);
            }
        }
        order
    }

    fn monomial(&self, monomial: &DimMonomial) -> (Vec<(Option<usize>, Rational)>, Dimension) {
        let mut vars: Vec<_> = monomial
            .var_factors()
            .map(|(var, power)| (self.dims.get(var.index).copied().flatten(), power))
            .collect();
        vars.sort_unstable_by_key(|(position, _)| *position);
        (vars, monomial.fixed.clone())
    }

    fn scalar_equivalent(
        &self,
        kind: &ScalarValueKind,
        other_order: &Self,
        other: &ScalarValueKind,
    ) -> bool {
        match (kind, other) {
            (ScalarValueKind::Quantity(left), ScalarValueKind::Quantity(right)) => {
                self.monomial(left) == other_order.monomial(right)
            }
            (ScalarValueKind::Bool, ScalarValueKind::Bool)
            | (ScalarValueKind::Int, ScalarValueKind::Int) => true,
            _ => false,
        }
    }

    fn param_equivalent(&self, kind: &ParamKind, other_order: &Self, other: &ParamKind) -> bool {
        match (kind, other) {
            (ParamKind::Scalar(left), ParamKind::Scalar(right)) => {
                self.scalar_equivalent(left, other_order, right)
            }
            (
                ParamKind::Indexed {
                    element: left,
                    indexes: left_indexes,
                },
                ParamKind::Indexed {
                    element: right,
                    indexes: right_indexes,
                },
            ) => {
                self.scalar_equivalent(left, other_order, right)
                    && left_indexes.len() == right_indexes.len()
                    && left_indexes.iter().zip(right_indexes).all(|(left, right)| {
                        self.indexes.get(left.index) == other_order.indexes.get(right.index)
                    })
            }
            _ => false,
        }
    }
}

fn number(order: &mut [Option<usize>], binder: usize, next: &mut usize) {
    if let Some(slot @ None) = order.get_mut(binder) {
        *slot = Some(*next);
        *next += 1;
    }
}

impl<S: StructResult> FunctionSignature<S> {
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
    pub fn structurally_equivalent<T: StructResult>(&self, other: &FunctionSignature<T>) -> bool {
        let order = FirstOccurrence::of(self);
        let other_order = FirstOccurrence::of(other);
        let result_equivalent = match (&self.result, &other.result) {
            (ResultKind::Value(left), ResultKind::Value(right)) => {
                order.param_equivalent(left, &other_order, right)
            }
            // Struct shapes carry no variables; field names, order, and
            // kinds are the contract and compare verbatim.
            (ResultKind::Struct(left), ResultKind::Struct(right)) => left.shape() == right.shape(),
            _ => false,
        };
        self.dim_vars.len() == other.dim_vars.len()
            && self.index_vars.len() == other.index_vars.len()
            && self.params.len() == other.params.len()
            && self
                .params
                .iter()
                .zip(&other.params)
                .all(|(left, right)| order.param_equivalent(&left.kind, &other_order, &right.kind))
            && result_equivalent
    }
}

fn format_param_kind(kind: &ParamKind, format_dim: &mut dyn FnMut(&Dimension) -> String) -> String {
    match kind {
        ParamKind::Scalar(scalar) => format_scalar_value_kind(scalar, format_dim),
        ParamKind::Indexed { element, indexes } => {
            let indexes = indexes
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{}[{indexes}]",
                format_scalar_value_kind(element, format_dim)
            )
        }
    }
}

fn format_struct_shape(
    shape: &StructShape,
    format_dim: &mut dyn FnMut(&Dimension) -> String,
) -> String {
    let fields: Vec<String> = shape
        .fields()
        .iter()
        .map(|field| {
            let kind = match &field.kind {
                StructFieldKind::Bool => "Bool".to_string(),
                StructFieldKind::Int => "Int".to_string(),
                StructFieldKind::Quantity(dim) => non_empty_dimension(format_dim(dim)),
            };
            format!("{}: {kind}", field.name)
        })
        .collect();
    format!("{{ {} }}", fields.join(", "))
}

fn format_scalar_value_kind(
    kind: &ScalarValueKind,
    format_dim: &mut dyn FnMut(&Dimension) -> String,
) -> String {
    match kind {
        ScalarValueKind::Quantity(monomial) => monomial.format_with(format_dim),
        ScalarValueKind::Bool => "Bool".to_string(),
        ScalarValueKind::Int => "Int".to_string(),
    }
}

fn validate_unique_param_names<V, I>(params: &[FunctionParam<V, I>]) -> Result<(), SignatureError> {
    let mut declared: HashMap<&FnParamName, usize> = HashMap::new();
    for (duplicate, param) in params.iter().enumerate() {
        if let Some(first) = declared.insert(&param.name, duplicate) {
            return Err(SignatureError::DuplicateParamName {
                name: param.name.clone(),
                first,
                duplicate,
            });
        }
    }
    Ok(())
}

/// Error from [`FunctionSignature::try_from_parts`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SignatureError {
    /// The same parameter name was declared twice.
    #[error("parameter `{name}` is declared more than once")]
    DuplicateParamName {
        /// The duplicated parameter name.
        name: FnParamName,
        /// Position of the first declaration in the parameter list.
        first: usize,
        /// Position of the repeated declaration in the parameter list.
        duplicate: usize,
    },
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
    /// One result-array axis variable indexes no parameter.
    #[error(
        "result array axis `{var}` is not used by any array parameter; a function cannot invent its output extent"
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
// All-quantity signatures
// ---------------------------------------------------------------------------

/// A signature whose parameters and result are all scalar quantities, the
/// shape of every scalar built-in: a call is checked by dimension alone.
///
/// [`QuantitySignature::try_new`] is the only construction point; afterwards
/// every parameter and the result are dimension monomials by type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuantitySignature {
    signature: FunctionSignature,
    params: Vec<QuantityParam>,
    result: DimMonomial,
}

/// One scalar quantity parameter of a [`QuantitySignature`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuantityParam {
    name: FnParamName,
    monomial: DimMonomial,
}

impl QuantityParam {
    /// The parameter name.
    #[must_use]
    pub const fn name(&self) -> &FnParamName {
        &self.name
    }

    /// The parameter's dimension monomial.
    #[must_use]
    pub const fn monomial(&self) -> &DimMonomial {
        &self.monomial
    }
}

/// A signature with a parameter or result that is not a scalar quantity.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QuantitySignatureError {
    #[error("parameter `{param}` is not a scalar quantity")]
    Param { param: FnParamName },
    #[error("the result is not a scalar quantity")]
    Result,
}

impl QuantitySignature {
    /// View `signature` as all-quantity.
    ///
    /// # Errors
    ///
    /// Returns the first parameter, or the result, that is not a scalar
    /// quantity.
    pub fn try_new(signature: FunctionSignature) -> Result<Self, QuantitySignatureError> {
        let params = signature
            .params()
            .iter()
            .map(|param| match &param.kind {
                ParamKind::Scalar(ScalarValueKind::Quantity(monomial)) => Ok(QuantityParam {
                    name: param.name.clone(),
                    monomial: monomial.clone(),
                }),
                ParamKind::Scalar(ScalarValueKind::Bool | ScalarValueKind::Int)
                | ParamKind::Indexed { .. } => Err(QuantitySignatureError::Param {
                    param: param.name.clone(),
                }),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let ResultKind::Value(ParamKind::Scalar(ScalarValueKind::Quantity(result))) =
            signature.result()
        else {
            return Err(QuantitySignatureError::Result);
        };
        let result = result.clone();
        Ok(Self {
            signature,
            params,
            result,
        })
    }

    /// The underlying signature.
    #[must_use]
    pub const fn signature(&self) -> &FunctionSignature {
        &self.signature
    }

    /// The quantity parameters, in declaration order.
    #[must_use]
    pub fn params(&self) -> &[QuantityParam] {
        &self.params
    }

    /// The result monomial.
    #[must_use]
    pub const fn result(&self) -> &DimMonomial {
        &self.result
    }

    /// Number of parameters.
    #[must_use]
    pub const fn arity(&self) -> usize {
        self.params.len()
    }
}

// ---------------------------------------------------------------------------
// Built-in signature shapes
// ---------------------------------------------------------------------------

fn dim_var_d() -> DimVarName {
    DimVarName::expect_valid("D")
}

fn param(name: &str, kind: NamedParamKind) -> NamedFunctionParam {
    FunctionParam {
        name: FnParamName::expect_valid(name),
        kind,
    }
}

const fn typed_param(name: FnParamName, kind: NamedParamKind) -> NamedFunctionParam {
    FunctionParam { name, kind }
}

#[expect(
    clippy::expect_used,
    reason = "built-in signature shapes are validated by construction; a failure is a compiler bug caught by tests"
)]
fn expect_signature(
    dim_vars: Vec<DimVarName>,
    params: Vec<NamedFunctionParam>,
    result: NamedParamKind,
) -> FunctionSignature {
    FunctionSignature::try_new(dim_vars, Vec::new(), params, result.into())
        .expect("built-in signature shape must be valid")
}

impl FunctionSignature {
    /// All params dimensionless quantities, dimensionless quantity result.
    #[must_use]
    pub(crate) fn all_dimensionless(names: &[&str]) -> Self {
        expect_signature(
            Vec::new(),
            names
                .iter()
                .map(|&n| param(n, ParamKind::dimensionless()))
                .collect(),
            ParamKind::dimensionless(),
        )
    }

    /// Single fixed-dimension param, fixed-dimension result.
    #[must_use]
    pub fn fixed_to_fixed(name: FnParamName, input: Dimension, output: Dimension) -> Self {
        expect_signature(
            Vec::new(),
            vec![typed_param(name, ParamKind::quantity(input))],
            ParamKind::quantity(output),
        )
    }

    /// Single free param `D`, result is `D` (a test fixture: no built-in has
    /// this shape since `abs` is typed by its complex overload rule).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn passthrough(name: &str) -> Self {
        let d = dim_var_d();
        expect_signature(
            vec![d.clone()],
            vec![param(
                name,
                ParamKind::quantity_monomial(DimMonomial::var(d.clone())),
            )],
            ParamKind::quantity_monomial(DimMonomial::var(d)),
        )
    }

    /// Single free param `D`, result is a fixed dimension.
    #[must_use]
    pub(crate) fn free_to_fixed(name: &str, output: Dimension) -> Self {
        let d = dim_var_d();
        expect_signature(
            vec![d.clone()],
            vec![param(
                name,
                ParamKind::quantity_monomial(DimMonomial::var(d)),
            )],
            ParamKind::quantity(output),
        )
    }

    /// Single free param `D`, result is `D^power`.
    #[must_use]
    pub fn free_to_pow(name: FnParamName, power: Rational) -> Self {
        let d = dim_var_d();
        expect_signature(
            vec![d.clone()],
            vec![typed_param(
                name,
                ParamKind::quantity_monomial(DimMonomial::var(d.clone())),
            )],
            ParamKind::quantity_monomial(DimMonomial::var_pow(d, power)),
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
                .map(|&n| param(n, ParamKind::quantity_monomial(DimMonomial::var(d.clone()))))
                .collect(),
            ParamKind::quantity_monomial(DimMonomial::var(d)),
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
                .map(|&n| param(n, ParamKind::quantity_monomial(DimMonomial::var(d.clone()))))
                .collect(),
            ParamKind::quantity(output),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::BaseDimId;

    // Tests write signatures by name, as builders do.
    type ParamKind = super::NamedParamKind;
    type ScalarValueKind = super::ScalarValueKind<DimVarName>;

    fn var(name: &str) -> DimVarName {
        DimVarName::expect_valid(name)
    }

    fn fn_param(name: &str) -> FnParamName {
        FnParamName::expect_valid(name)
    }

    fn length() -> Dimension {
        Dimension::base(BaseDimId::Prelude(
            crate::dimension::PreludeBaseDimension::Length,
        ))
    }

    fn time() -> Dimension {
        Dimension::base(BaseDimId::Prelude(
            crate::dimension::PreludeBaseDimension::Time,
        ))
    }

    #[test]
    fn quantity_signature_admits_only_all_quantity_shapes() {
        let all_quantity = super::FunctionSignature::same_dim(&["a", "b"]);
        let viewed = QuantitySignature::try_new(all_quantity.clone()).unwrap();
        assert_eq!(viewed.signature(), &all_quantity);
        assert_eq!(viewed.arity(), 2);
        assert_eq!(viewed.params()[1].name(), &fn_param("b"));
        assert_eq!(
            viewed.params()[1].monomial().as_bare_var().unwrap().index(),
            0
        );
        assert_eq!(viewed.result().as_bare_var().unwrap().index(), 0);

        let bool_param = super::FunctionSignature::try_new(
            Vec::new(),
            Vec::new(),
            vec![param("flag", ParamKind::bool())],
            ParamKind::dimensionless().into(),
        )
        .unwrap();
        assert_eq!(
            QuantitySignature::try_new(bool_param),
            Err(QuantitySignatureError::Param {
                param: fn_param("flag")
            })
        );
        let int_result = super::FunctionSignature::try_new(
            Vec::new(),
            Vec::new(),
            vec![param("x", ParamKind::dimensionless())],
            ParamKind::int().into(),
        )
        .unwrap();
        assert_eq!(
            QuantitySignature::try_new(int_result),
            Err(QuantitySignatureError::Result)
        );
    }

    #[test]
    fn cross_variable_monomial_result() {
        // (D1, D2) -> D1 * D2, the `torque(force, arm)` shape.
        let sig = FunctionSignature::try_new(
            vec![var("D1"), var("D2")],
            Vec::new(),
            vec![
                param(
                    "force",
                    ParamKind::quantity_monomial(DimMonomial::var(var("D1"))),
                ),
                param(
                    "arm",
                    ParamKind::quantity_monomial(DimMonomial::var(var("D2"))),
                ),
            ],
            ParamKind::quantity_monomial(
                DimMonomial::try_new(
                    [(var("D1"), Rational::ONE), (var("D2"), Rational::ONE)],
                    Dimension::dimensionless(),
                )
                .unwrap(),
            )
            .into(),
        )
        .unwrap();

        let l = length();
        let t = time();
        let bindings = [(var("D1"), l.clone()), (var("D2"), t.clone())];
        let ResultKind::Value(super::ParamKind::Scalar(super::ScalarValueKind::Quantity(result))) =
            sig.result()
        else {
            panic!("expected quantity result");
        };
        let dim = result
            .eval(|v| {
                bindings
                    .iter()
                    .find(|(bv, _)| bv == v.name())
                    .map(|(_, d)| d)
            })
            .unwrap();
        assert_eq!(dim, l.checked_mul(&t).unwrap());
    }

    #[test]
    fn written_monomials_reject_zero_and_repeated_variables() {
        assert_eq!(
            DimMonomial::try_new([(var("D"), Rational::ZERO)], Dimension::dimensionless())
                .map_err(SignatureError::from),
            Err(SignatureError::ZeroExponent { var: var("D") })
        );
        assert_eq!(
            DimMonomial::try_new(
                [(var("D"), Rational::ONE), (var("D"), Rational::HALF)],
                Dimension::dimensionless(),
            )
            .map_err(SignatureError::from),
            Err(SignatureError::DuplicateMonomialVar { var: var("D") })
        );
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
                    ParamKind::quantity_monomial(DimMonomial::var_pow(
                        var("D"),
                        Rational::try_new(2, 1).unwrap(),
                    )),
                ),
                param(
                    "y",
                    ParamKind::quantity_monomial(DimMonomial::var(var("D"))),
                ),
            ],
            ParamKind::quantity_monomial(DimMonomial::var(var("D"))).into(),
        )
        .unwrap_err();
        assert!(matches!(err, SignatureError::UseBeforeBinding { .. }));
    }

    #[test]
    fn undeclared_var_is_rejected() {
        let err = FunctionSignature::try_new(
            Vec::new(),
            Vec::new(),
            vec![param(
                "x",
                ParamKind::quantity_monomial(DimMonomial::var(var("D"))),
            )],
            ParamKind::dimensionless().into(),
        )
        .unwrap_err();
        assert!(matches!(err, SignatureError::UndeclaredDimVar { .. }));
    }

    #[test]
    fn declared_but_never_bound_var_is_rejected() {
        let err = FunctionSignature::try_new(
            vec![var("D")],
            Vec::new(),
            vec![param("x", ParamKind::dimensionless())],
            ParamKind::dimensionless().into(),
        )
        .unwrap_err();
        assert!(matches!(err, SignatureError::DimVarNeverBound { .. }));
    }

    #[test]
    fn bool_and_int_kinds_carry_no_dims() {
        let sig = FunctionSignature::try_new(
            Vec::new(),
            Vec::new(),
            vec![
                param("flag", ParamKind::bool()),
                param("n", ParamKind::int()),
            ],
            ParamKind::int().into(),
        )
        .unwrap();
        assert_eq!(sig.arity(), 2);
    }

    #[test]
    fn duplicate_param_name_is_rejected() {
        let err = FunctionSignature::try_new(
            Vec::new(),
            Vec::new(),
            vec![
                param("value", ParamKind::bool()),
                param("other", ParamKind::int()),
                param("value", ParamKind::int()),
            ],
            ParamKind::int().into(),
        )
        .unwrap_err();
        assert_eq!(
            err,
            SignatureError::DuplicateParamName {
                name: FnParamName::expect_valid("value"),
                first: 0,
                duplicate: 2,
            }
        );
    }

    #[test]
    fn format_with_renders_binders_params_and_result() {
        let sig = FunctionSignature::free_to_pow(fn_param("x"), Rational::HALF);
        let rendered = sig.format_with(|dim| format!("{dim:?}"));
        assert_eq!(rendered, "<D: Dim>(x: D) -> D^(1/2)");
    }

    /// `<vars>(params) -> result` shorthand for equivalence tests.
    fn sig(dim_vars: &[&str], params: &[ParamKind], result: ParamKind) -> FunctionSignature {
        FunctionSignature::try_new(
            dim_vars.iter().map(|v| var(v)).collect(),
            Vec::new(),
            params
                .iter()
                .enumerate()
                .map(|(i, kind)| param(&format!("p{i}"), kind.clone()))
                .collect(),
            result.into(),
        )
        .unwrap()
    }

    fn bare(name: &str) -> ParamKind {
        ParamKind::quantity_monomial(DimMonomial::var(var(name)))
    }

    #[test]
    fn equivalence_ignores_variable_names_and_param_names() {
        let declared = FunctionSignature::try_new(
            vec![var("D")],
            Vec::new(),
            vec![
                param("a", bare("D")),
                param("b", bare("D")),
                param("t", ParamKind::dimensionless()),
            ],
            bare("D").into(),
        )
        .unwrap();
        let manifest = FunctionSignature::try_new(
            vec![var("T")],
            Vec::new(),
            vec![
                param("lo", bare("T")),
                param("hi", bare("T")),
                param("frac", ParamKind::dimensionless()),
            ],
            bare("T").into(),
        )
        .unwrap();
        assert!(declared.structurally_equivalent(&manifest));
    }

    #[test]
    fn equivalence_ignores_monomial_factor_order_and_binder_order() {
        let product = |first: &str, second: &str| {
            ParamKind::quantity_monomial(
                DimMonomial::try_new(
                    [(var(first), Rational::ONE), (var(second), Rational::ONE)],
                    Dimension::dimensionless(),
                )
                .unwrap(),
            )
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
            !passthrough.structurally_equivalent(&FunctionSignature::free_to_pow(
                fn_param("x"),
                Rational::HALF
            ))
        );
        assert!(
            !FunctionSignature::fixed_to_fixed(fn_param("x"), length(), time())
                .structurally_equivalent(&FunctionSignature::fixed_to_fixed(
                    fn_param("x"),
                    length(),
                    length(),
                ))
        );
        assert!(
            !sig(&[], &[ParamKind::bool()], ParamKind::int()).structurally_equivalent(&sig(
                &[],
                &[ParamKind::int()],
                ParamKind::int()
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
        let _ = FunctionSignature::fixed_to_fixed(fn_param("x"), length(), time());
        let _ = FunctionSignature::passthrough("x");
        let _ = FunctionSignature::free_to_fixed("x", length());
        let _ = FunctionSignature::free_to_pow(fn_param("x"), Rational::HALF);
        let _ = FunctionSignature::same_dim(&["a", "b"]);
        let _ = FunctionSignature::same_dim_to_fixed(&["y", "x"], length());
    }

    // -- Index-variable (array) invariants ---------------------------------

    fn ivar(name: &str) -> IndexVarName {
        IndexVarName::expect_valid(name)
    }

    fn array(dim_var: &str, index_var: &str) -> ParamKind {
        ParamKind::Indexed {
            element: ScalarValueKind::Quantity(DimMonomial::var(var(dim_var))),
            indexes: NonEmpty::singleton(ivar(index_var)),
        }
    }

    /// `smooth<D: Dim, I: Index>(xs: D[I], window: Dimensionless) -> D[I]`.
    fn smooth_signature() -> FunctionSignature {
        FunctionSignature::try_new(
            vec![var("D")],
            vec![ivar("I")],
            vec![
                param("xs", array("D", "I")),
                param("window", ParamKind::dimensionless()),
            ],
            array("D", "I").into(),
        )
        .unwrap()
    }

    fn scalar_array(element: ScalarValueKind, index_var: &str) -> ParamKind {
        ParamKind::Indexed {
            element,
            indexes: NonEmpty::singleton(ivar(index_var)),
        }
    }

    #[test]
    fn bool_and_int_arrays_bind_indexes_without_binding_dimensions() {
        for element in [ScalarValueKind::Bool, ScalarValueKind::Int] {
            let signature = FunctionSignature::try_new(
                Vec::new(),
                vec![ivar("I")],
                vec![param("values", scalar_array(element.clone(), "I"))],
                scalar_array(element, "I").into(),
            )
            .unwrap();
            assert!(signature.dim_vars().is_empty());
        }

        let bool_signature = FunctionSignature::try_new(
            Vec::new(),
            vec![ivar("I")],
            vec![param("values", scalar_array(ScalarValueKind::Bool, "I"))],
            scalar_array(ScalarValueKind::Bool, "I").into(),
        )
        .unwrap();
        assert_eq!(
            bool_signature.format_with(|_| String::new()),
            "<I: Index>(values: Bool[I]) -> Bool[I]"
        );

        let int_signature = FunctionSignature::try_new(
            Vec::new(),
            vec![ivar("I"), ivar("J")],
            vec![param(
                "values",
                ParamKind::Indexed {
                    element: ScalarValueKind::Int,
                    indexes: NonEmpty::try_from_vec(vec![ivar("I"), ivar("J")]).unwrap(),
                },
            )],
            scalar_array(ScalarValueKind::Int, "I").into(),
        )
        .unwrap();
        assert_eq!(
            int_signature.format_with(|_| String::new()),
            "<I: Index, J: Index>(values: Int[I, J]) -> Int[I]"
        );
        let int_one_axis = FunctionSignature::try_new(
            Vec::new(),
            vec![ivar("K")],
            vec![param("values", scalar_array(ScalarValueKind::Int, "K"))],
            scalar_array(ScalarValueKind::Int, "K").into(),
        )
        .unwrap();
        assert!(!bool_signature.structurally_equivalent(&int_one_axis));
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
    fn multi_axis_results_may_reorder_bound_axes() {
        let matrix = |left: &str, right: &str| ParamKind::Indexed {
            element: ScalarValueKind::Quantity(DimMonomial::var(var("D"))),
            indexes: NonEmpty::try_from_vec(vec![ivar(left), ivar(right)]).unwrap(),
        };
        let signature = FunctionSignature::try_new(
            vec![var("D")],
            vec![ivar("I"), ivar("J")],
            vec![param("matrix", matrix("I", "J"))],
            matrix("J", "I").into(),
        )
        .unwrap();
        let ResultKind::Value(super::ParamKind::Indexed { indexes, .. }) = signature.result()
        else {
            panic!("expected an indexed result");
        };
        // Result axes resolve to the declared binders `J` (position 1) then
        // `I` (position 0).
        assert_eq!(
            indexes
                .iter()
                .map(|index| (index.index(), index.name().as_str()))
                .collect::<Vec<_>>(),
            vec![(1, "J"), (0, "I")]
        );
    }

    #[test]
    fn duplicate_index_var_is_rejected() {
        let err = FunctionSignature::try_new(
            vec![var("D")],
            vec![ivar("I"), ivar("I")],
            vec![param("xs", array("D", "I"))],
            ParamKind::dimensionless().into(),
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
            ParamKind::dimensionless().into(),
        )
        .unwrap_err();
        assert!(matches!(err, SignatureError::UndeclaredIndexVar { .. }));
    }

    #[test]
    fn result_index_var_must_index_a_parameter() {
        // A function inventing an output extent is the dynamic-index
        // problem — the result must reuse an input index variable.
        let err = FunctionSignature::try_new(
            vec![var("D")],
            vec![ivar("I")],
            vec![param("x", bare("D"))],
            array("D", "I").into(),
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
            bare("D").into(),
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
                ParamKind::Indexed {
                    element: ScalarValueKind::Quantity(DimMonomial::var_pow(
                        var("D"),
                        Rational::try_new(2, 1).unwrap(),
                    )),
                    indexes: NonEmpty::singleton(ivar("I")),
                },
            )],
            ParamKind::dimensionless().into(),
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
                param("w", ParamKind::dimensionless()),
            ],
            array("T", "J").into(),
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
            array("D", "I").into(),
        )
        .unwrap();
        let free = FunctionSignature::try_new(
            vec![var("D")],
            vec![ivar("I"), ivar("J")],
            vec![param("xs", array("D", "I")), param("ys", array("D", "J"))],
            array("D", "I").into(),
        )
        .unwrap();
        assert!(!same.structurally_equivalent(&free));
        assert!(!free.structurally_equivalent(&same));
    }

    #[test]
    fn array_is_not_equivalent_to_quantity() {
        let arr = FunctionSignature::try_new(
            vec![var("D")],
            vec![ivar("I")],
            vec![param("xs", array("D", "I"))],
            bare("D").into(),
        )
        .unwrap();
        let quantity = FunctionSignature::passthrough("xs");
        assert!(!arr.structurally_equivalent(&quantity));
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
    fn struct_results_compare_field_names_and_order() {
        let lo_hi = |names: (&str, &str)| {
            FunctionSignature::try_new(
                vec![var("D")],
                vec![ivar("I")],
                vec![param("xs", array("D", "I"))],
                ResultKind::Struct(shape(&[
                    (names.0, StructFieldKind::Quantity(length())),
                    (names.1, StructFieldKind::Quantity(length())),
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
            ResultKind::Struct(shape(&[
                ("lo", StructFieldKind::Quantity(Dimension::dimensionless())),
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
