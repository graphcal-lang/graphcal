//! Checked type of a declaration or expression, and the type-level references it carries.

use crate::dag_id::DagId;
use crate::dimension::Dimension;
use crate::resolved_name::{ResolvedIndexName, ResolvedName};
use crate::syntax::index_name::{IndexName, IndexNameNamespace};
use crate::syntax::names::{NameDef, NameNamespace};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::type_name::StructTypeNameNamespace;

use crate::nat::NatPolyForm;
use crate::registry::time_scale::TimeScale;
use crate::registry::types::{
    DimensionFormattingRegistry, FiniteIndex, IndexCardinality, IndexCardinalityError,
};
use crate::tir::materialized_shape::{MaterializedShape, MaterializedShapeError};

/// A type-level reference to a named compiler entity.
///
/// Every semantic type reference has a canonical owner. Leaf-only names belong
/// at syntax/display boundaries; once a value crosses into the functional core,
/// it must carry a [`ResolvedName`].
#[derive(Debug, Clone)]
pub struct TypeNameRef<Ns: NameNamespace> {
    name: NameDef<Ns>,
    resolved: ResolvedName<Ns>,
}

impl<Ns: NameNamespace> TypeNameRef<Ns> {
    /// Create a module-aware reference from a canonical resolved name.
    #[must_use]
    pub fn from_resolved(resolved: ResolvedName<Ns>) -> Self {
        Self {
            name: resolved.to_unowned_def_name(),
            resolved,
        }
    }

    /// Create a reference with a display leaf that differs from the canonical
    /// owner-qualified identity.
    ///
    /// This is used at value-display boundaries such as tagged-union
    /// constructor values, where the semantic type is the owning union but the
    /// rendered value should show the constructor leaf.
    #[must_use]
    pub const fn with_display_leaf(name: NameDef<Ns>, resolved: ResolvedName<Ns>) -> Self {
        Self { name, resolved }
    }

    /// Resolve a definition-site leaf into the given owner.
    #[must_use]
    pub fn with_owner(owner: DagId, name: NameDef<Ns>) -> Self {
        Self::from_resolved(ResolvedName::from_def(owner, name))
    }

    /// The leaf definition name used by registries and diagnostics.
    #[must_use]
    pub const fn name(&self) -> &NameDef<Ns> {
        &self.name
    }

    /// The canonical owner/name identity.
    #[must_use]
    pub const fn resolved(&self) -> &ResolvedName<Ns> {
        &self.resolved
    }

    /// Compare this reference against another type reference by owner-qualified identity.
    #[must_use]
    pub fn matches_ref(&self, other: &Self) -> bool {
        self.resolved() == other.resolved()
    }

    /// Borrow the leaf string for diagnostic/display-only formatting.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.name.as_str()
    }

    /// Clone the leaf definition name for diagnostic/display boundaries.
    #[must_use]
    fn to_unowned_name(&self) -> NameDef<Ns> {
        self.name.clone()
    }
}

impl<Ns: NameNamespace> PartialEq for TypeNameRef<Ns> {
    fn eq(&self, other: &Self) -> bool {
        self.resolved == other.resolved
    }
}

impl<Ns: NameNamespace> Eq for TypeNameRef<Ns> {}

impl<Ns: NameNamespace> std::hash::Hash for TypeNameRef<Ns> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.resolved.hash(state);
    }
}

impl<Ns: NameNamespace> From<ResolvedName<Ns>> for TypeNameRef<Ns> {
    fn from(resolved: ResolvedName<Ns>) -> Self {
        Self::from_resolved(resolved)
    }
}

impl<Ns: NameNamespace> std::fmt::Display for TypeNameRef<Ns> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.name.fmt(f)
    }
}

/// Type-level reference to a compiler-generated structural `Fin(N)` index.
///
/// The representation is private and canonical: a constant cardinality is
/// always a validated concrete identity, and only a genuinely symbolic Nat
/// form stays symbolic. Equality is therefore structural identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FiniteIndexRef(FiniteIndexRepr);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum FiniteIndexRepr {
    Concrete(FiniteIndex),
    Symbolic(NatPolyForm),
}

impl FiniteIndexRef {
    /// Reference a validated concrete structural identity.
    #[must_use]
    pub const fn concrete(index: FiniteIndex) -> Self {
        Self(FiniteIndexRepr::Concrete(index))
    }

    /// Create a finite structural reference from a normalized Nat form.
    ///
    /// # Errors
    ///
    /// Returns an error when the form is a constant that is not a valid
    /// finite cardinality.
    pub(crate) fn from_form(form: NatPolyForm) -> Result<Self, IndexCardinalityError> {
        if form.is_constant() {
            FiniteIndex::try_from_u64(form.constant()).map(Self::concrete)
        } else {
            Ok(Self(FiniteIndexRepr::Symbolic(form)))
        }
    }

    /// Return the concrete finite structural identity, if this reference is concrete.
    #[must_use]
    pub(crate) const fn concrete_index(&self) -> Option<FiniteIndex> {
        match &self.0 {
            FiniteIndexRepr::Concrete(index) => Some(*index),
            FiniteIndexRepr::Symbolic(_) => None,
        }
    }

    /// Return the normalized Nat form for this reference.
    #[must_use]
    pub(crate) fn form(&self) -> NatPolyForm {
        match &self.0 {
            FiniteIndexRepr::Concrete(index) => NatPolyForm::from_constant(index.size_u64()),
            FiniteIndexRepr::Symbolic(form) => form.clone(),
        }
    }
}

/// Renders the source spelling `Fin(N)` or `Fin(<Nat expression>)`.
impl std::fmt::Display for FiniteIndexRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        IndexDisplayName::Finite(self.form()).fmt(f)
    }
}

/// Source-facing spelling of an index for diagnostics and value display.
///
/// Declared indexes render by their definition-site leaf name; structural
/// axes keep their Nat cardinality form and render as `Fin(...)` only through
/// `Display`, never as a fabricated [`IndexName`]. The form is display-only
/// and may describe a cardinality that failed validation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IndexDisplayName {
    Declared(IndexName),
    Finite(NatPolyForm),
}

impl IndexDisplayName {
    /// The declared leaf name, when this spells a declared index.
    #[must_use]
    pub const fn declared_name(&self) -> Option<&IndexName> {
        match self {
            Self::Declared(name) => Some(name),
            Self::Finite(_) => None,
        }
    }
}

impl From<IndexName> for IndexDisplayName {
    fn from(name: IndexName) -> Self {
        Self::Declared(name)
    }
}

impl std::fmt::Display for IndexDisplayName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declared(name) => name.fmt(f),
            Self::Finite(form) => write!(f, "Fin({})", form.format()),
        }
    }
}

mod sealed {
    pub trait Sealed {}
}

/// Whether a checked type may still mention unbound `Nat` generic parameters.
///
/// [`Concrete`] types are fully bound: every structural `Fin(N)` axis is a
/// validated [`FiniteIndex`] and every `Nat` argument is a number. They are
/// what declarations carry and what evaluation consumes. [`Symbolic`] types
/// are what inference produces inside generic and template bodies, where an
/// axis may be `Fin(N + 1)`. [`CheckedType::instantiate`] is the only way from
/// the latter to the former.
pub trait Concreteness:
    sealed::Sealed + std::fmt::Debug + Clone + Copy + PartialEq + Eq + std::hash::Hash + 'static
{
    /// The identity of a structural `Fin(N)` axis.
    type Finite: FiniteAxis;
    /// A `Nat`-sorted generic argument.
    type Nat: NatArgument;
}

/// Fully bound checked types (see [`Concreteness`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Concrete {}

/// Checked types that may mention unbound `Nat` parameters (see [`Concreteness`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Symbolic {}

impl sealed::Sealed for Concrete {}
impl sealed::Sealed for Symbolic {}

impl Concreteness for Concrete {
    type Finite = FiniteIndex;
    type Nat = u64;
}

impl Concreteness for Symbolic {
    type Finite = FiniteIndexRef;
    type Nat = NatPolyForm;
}

/// The identity of a structural `Fin(N)` axis at some [`Concreteness`].
pub trait FiniteAxis:
    std::fmt::Debug + Clone + PartialEq + Eq + std::hash::Hash + std::fmt::Display + From<FiniteIndex>
{
    /// The validated identity, when the cardinality is a constant.
    fn concrete_index(&self) -> Option<FiniteIndex>;

    /// The normalized Nat cardinality form.
    fn form(&self) -> NatPolyForm;
}

impl FiniteAxis for FiniteIndex {
    fn concrete_index(&self) -> Option<FiniteIndex> {
        Some(*self)
    }

    fn form(&self) -> NatPolyForm {
        NatPolyForm::from_constant(self.size_u64())
    }
}

impl FiniteAxis for FiniteIndexRef {
    fn concrete_index(&self) -> Option<FiniteIndex> {
        Self::concrete_index(self)
    }

    fn form(&self) -> NatPolyForm {
        Self::form(self)
    }
}

impl From<FiniteIndex> for FiniteIndexRef {
    fn from(index: FiniteIndex) -> Self {
        Self::concrete(index)
    }
}

/// A `Nat`-sorted generic argument at some [`Concreteness`].
pub trait NatArgument: std::fmt::Debug + Clone + PartialEq + Eq + std::hash::Hash {
    /// The normalized Nat form.
    fn form(&self) -> NatPolyForm;
}

impl NatArgument for u64 {
    fn form(&self) -> NatPolyForm {
        NatPolyForm::from_constant(*self)
    }
}

impl NatArgument for NatPolyForm {
    fn form(&self) -> NatPolyForm {
        self.clone()
    }
}

/// Why a [`Symbolic`] type could not be instantiated as a [`Concrete`] one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstantiationError<E> {
    /// A `Nat` form could not be closed to a constant.
    Binding(E),
    /// A `Fin(N)` axis closed to an invalid cardinality.
    Cardinality(IndexCardinalityError),
}

/// Close the `Nat` forms of a symbolic type with one caller-supplied rule.
struct NatBinder<'a, E> {
    close: &'a mut dyn FnMut(&NatPolyForm) -> Result<u64, E>,
}

impl<E> NatBinder<'_, E> {
    fn nat(&mut self, form: &NatPolyForm) -> Result<u64, InstantiationError<E>> {
        (self.close)(form).map_err(InstantiationError::Binding)
    }

    fn index(
        &mut self,
        index: &IndexTypeRef<Symbolic>,
    ) -> Result<IndexTypeRef<Concrete>, InstantiationError<E>> {
        match index {
            IndexTypeRef::Declared(reference) => Ok(IndexTypeRef::Declared(reference.clone())),
            IndexTypeRef::Finite(reference) => {
                let size = match reference.concrete_index() {
                    Some(index) => return Ok(IndexTypeRef::Finite(index)),
                    None => self.nat(&reference.form())?,
                };
                FiniteIndex::try_from_u64(size)
                    .map(IndexTypeRef::Finite)
                    .map_err(InstantiationError::Cardinality)
            }
        }
    }

    fn generic_arg(
        &mut self,
        arg: &CheckedGenericArg<Symbolic>,
    ) -> Result<CheckedGenericArg<Concrete>, InstantiationError<E>> {
        Ok(match arg {
            CheckedGenericArg::Dim(dimension) => CheckedGenericArg::Dim(dimension.clone()),
            CheckedGenericArg::Index(index) => CheckedGenericArg::Index(self.index(index)?),
            CheckedGenericArg::Nat(form) => CheckedGenericArg::Nat(self.nat(form)?),
            CheckedGenericArg::Type(ty) => CheckedGenericArg::Type(self.ty(ty)?),
        })
    }

    fn ty(
        &mut self,
        ty: &CheckedType<Symbolic>,
    ) -> Result<CheckedType<Concrete>, InstantiationError<E>> {
        Ok(match ty {
            CheckedType::Quantity(dimension) => CheckedType::Quantity(dimension.clone()),
            CheckedType::Complex(dimension) => CheckedType::Complex(dimension.clone()),
            CheckedType::Bool => CheckedType::Bool,
            CheckedType::Int => CheckedType::Int,
            CheckedType::Datetime(scale) => CheckedType::Datetime(*scale),
            CheckedType::Key(index) => CheckedType::Key(self.index(index)?),
            CheckedType::Struct(identity, args) => CheckedType::Struct(
                identity.clone(),
                args.iter()
                    .map(|arg| self.generic_arg(arg))
                    .collect::<Result<_, _>>()?,
            ),
            CheckedType::Indexed { element, index } => CheckedType::Indexed {
                element: Box::new(self.ty(element)?),
                index: self.index(index)?,
            },
        })
    }
}

/// No `Nat` parameter is bound: only constant forms close.
fn constant_only(form: &NatPolyForm) -> Result<u64, ()> {
    form.constant_value().ok_or(())
}

/// Type-level reference to an index definition.
///
/// Declared indexes are owner-qualified names. Compiler-generated `Fin(N)`
/// axes are typed structural identities and have no declared resolved names;
/// whether their cardinality may still be symbolic is fixed by `V`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IndexTypeRef<V: Concreteness = Concrete> {
    Declared(TypeNameRef<IndexNameNamespace>),
    Finite(V::Finite),
}

impl<V: Concreteness> IndexTypeRef<V> {
    /// Create a module-aware reference from a canonical resolved declared-index name.
    #[must_use]
    pub fn from_resolved(resolved: ResolvedIndexName) -> Self {
        Self::Declared(TypeNameRef::from_resolved(resolved))
    }

    /// Resolve a declared-index definition-site leaf into the given owner.
    #[must_use]
    pub fn with_owner(owner: DagId, name: IndexName) -> Self {
        Self::Declared(TypeNameRef::with_owner(owner, name))
    }

    /// Create a reference with a display leaf that differs from the canonical
    /// owner-qualified identity.
    #[must_use]
    pub const fn with_display_leaf(name: IndexName, resolved: ResolvedIndexName) -> Self {
        Self::Declared(TypeNameRef::with_display_leaf(name, resolved))
    }

    /// Create a concrete compiler-generated finite structural reference.
    #[must_use]
    pub fn from_finite_index(index: FiniteIndex) -> Self {
        Self::Finite(index.into())
    }

    /// The declared leaf name, when this is a declared index.
    #[must_use]
    pub const fn declared_name(&self) -> Option<&IndexName> {
        match self {
            Self::Declared(reference) => Some(reference.name()),
            Self::Finite(_) => None,
        }
    }

    /// The canonical declared owner/name identity, when this is a declared index.
    #[must_use]
    pub const fn declared_resolved(&self) -> Option<&ResolvedIndexName> {
        match self {
            Self::Declared(reference) => Some(reference.resolved()),
            Self::Finite(_) => None,
        }
    }

    /// Return the typed concrete finite structural identity, if this reference has one.
    #[must_use]
    pub fn finite_index(&self) -> Option<FiniteIndex> {
        match self {
            Self::Finite(reference) => reference.concrete_index(),
            Self::Declared(_) => None,
        }
    }

    /// Return the normalized Nat form, when this is a finite structural reference.
    #[must_use]
    pub(crate) fn finite_index_form(&self) -> Option<NatPolyForm> {
        match self {
            Self::Finite(reference) => Some(reference.form()),
            Self::Declared(_) => None,
        }
    }

    /// The display-only spelling for diagnostics and formatting.
    #[must_use]
    pub fn display_name(&self) -> IndexDisplayName {
        match self {
            Self::Declared(reference) => IndexDisplayName::Declared(reference.to_unowned_name()),
            Self::Finite(reference) => IndexDisplayName::Finite(reference.form()),
        }
    }

    /// Compare this reference against another type reference.
    #[must_use]
    pub fn matches_ref(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Declared(lhs), Self::Declared(rhs)) => lhs.matches_ref(rhs),
            (Self::Finite(lhs), Self::Finite(rhs)) => lhs == rhs,
            _ => false,
        }
    }
}

impl IndexTypeRef<Symbolic> {
    /// Create a finite structural reference from a normalized Nat form.
    ///
    /// # Errors
    ///
    /// Returns an error when the form is a concrete invalid finite structural size.
    pub(crate) fn from_finite_index_form(form: NatPolyForm) -> Result<Self, IndexCardinalityError> {
        FiniteIndexRef::from_form(form).map(Self::Finite)
    }

    /// Wrap an already validated finite structural reference.
    #[must_use]
    pub const fn from_finite_index_ref(reference: FiniteIndexRef) -> Self {
        Self::Finite(reference)
    }

    /// Return the finite structural reference, when this is a compiler-generated finite structural.
    #[must_use]
    pub(crate) const fn finite_index_ref(&self) -> Option<&FiniteIndexRef> {
        match self {
            Self::Declared(_) => None,
            Self::Finite(reference) => Some(reference),
        }
    }

    /// Close the axis cardinality with `close`.
    ///
    /// # Errors
    ///
    /// Returns an [`InstantiationError`] when the form cannot be closed or
    /// closes to an invalid cardinality.
    pub fn instantiate<E>(
        &self,
        mut close: impl FnMut(&NatPolyForm) -> Result<u64, E>,
    ) -> Result<IndexTypeRef<Concrete>, InstantiationError<E>> {
        NatBinder { close: &mut close }.index(self)
    }

    /// The concrete reference, when the cardinality mentions no `Nat` variable.
    #[must_use]
    pub fn to_concrete(&self) -> Option<IndexTypeRef<Concrete>> {
        self.instantiate(constant_only).ok()
    }
}

impl IndexTypeRef<Concrete> {
    /// View a concrete reference at the symbolic level.
    #[must_use]
    pub fn to_symbolic(&self) -> IndexTypeRef<Symbolic> {
        match self {
            Self::Declared(reference) => IndexTypeRef::Declared(reference.clone()),
            Self::Finite(index) => IndexTypeRef::Finite(FiniteIndexRef::concrete(*index)),
        }
    }
}

impl<V: Concreteness> From<ResolvedIndexName> for IndexTypeRef<V> {
    fn from(resolved: ResolvedIndexName) -> Self {
        Self::from_resolved(resolved)
    }
}

impl<V: Concreteness> std::fmt::Display for IndexTypeRef<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declared(reference) => reference.fmt(f),
            Self::Finite(reference) => reference.fmt(f),
        }
    }
}

/// Type-level reference to a struct/tagged-union definition.
pub type StructTypeRef = TypeNameRef<StructTypeNameNamespace>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generic_param::test_support::type_param;
    use crate::syntax::type_name::StructTypeName;

    fn symbolic_n_plus_one() -> NatPolyForm {
        NatPolyForm::from_var(type_param("N"))
            .add(&NatPolyForm::from_constant(1))
            .unwrap()
    }

    #[test]
    fn finite_index_ref_is_canonical_for_constant_forms() {
        let three = FiniteIndex::try_from_u64(3).unwrap();
        let from_form = FiniteIndexRef::from_form(NatPolyForm::from_constant(3)).unwrap();
        assert_eq!(from_form, FiniteIndexRef::concrete(three));
        assert_eq!(from_form.concrete_index(), Some(three));
        assert_eq!(from_form.form(), NatPolyForm::from_constant(3));
        assert_eq!(from_form.to_string(), "Fin(3)");
        assert_eq!(
            FiniteIndexRef::from_form(NatPolyForm::from_constant(0)),
            Err(IndexCardinalityError::Empty)
        );
    }

    #[test]
    fn finite_index_ref_keeps_symbolic_forms() {
        let symbolic = FiniteIndexRef::from_form(symbolic_n_plus_one()).unwrap();
        assert_eq!(symbolic.concrete_index(), None);
        assert_eq!(symbolic.form(), symbolic_n_plus_one());
        assert_eq!(symbolic.to_string(), "Fin(N + 1)");
        assert_ne!(
            symbolic,
            FiniteIndexRef::concrete(FiniteIndex::try_from_u64(1).unwrap())
        );
    }

    #[test]
    fn index_display_names_render_declared_leaves_and_finite_forms() {
        let phase = IndexName::expect_valid("Phase");
        let declared = IndexDisplayName::from(phase.clone());
        assert_eq!(declared.declared_name(), Some(&phase));
        assert_eq!(declared.to_string(), "Phase");

        let finite = IndexDisplayName::Finite(symbolic_n_plus_one());
        assert_eq!(finite.declared_name(), None);
        assert_eq!(finite.to_string(), "Fin(N + 1)");
        // Display-only: an invalid cardinality still renders.
        assert_eq!(
            IndexDisplayName::Finite(NatPolyForm::from_constant(0)).to_string(),
            "Fin(0)"
        );
    }

    #[test]
    fn index_type_ref_display_matches_its_display_name() {
        let owner = DagId::root_in_package("test", "main");
        let declared = IndexTypeRef::with_owner(owner, IndexName::expect_valid("Phase"));
        assert_eq!(declared.to_string(), "Phase");
        assert_eq!(
            declared.display_name(),
            IndexDisplayName::Declared(IndexName::expect_valid("Phase"))
        );

        let finite = IndexTypeRef::from_finite_index(FiniteIndex::try_from_u64(4).unwrap());
        assert_eq!(finite.to_string(), "Fin(4)");
        assert_eq!(
            finite.display_name(),
            IndexDisplayName::Finite(NatPolyForm::from_constant(4))
        );
        assert!(finite.matches_ref(
            &IndexTypeRef::from_finite_index_form(NatPolyForm::from_constant(4)).unwrap()
        ));
        assert!(!finite.matches_ref(&declared));
    }

    fn n() -> crate::generic_param::GenericParamId {
        type_param("N")
    }

    fn fin_n_plus_one() -> IndexTypeRef<Symbolic> {
        IndexTypeRef::from_finite_index_form(symbolic_n_plus_one()).unwrap()
    }

    fn fin(size: u64) -> FiniteIndex {
        FiniteIndex::try_from_u64(size).unwrap()
    }

    #[test]
    fn instantiate_binds_symbolic_axes_and_nat_arguments() {
        let owner = DagId::root_in_package("test", "main");
        let vec = StructTypeRef::from_resolved(ResolvedName::from_def(
            owner,
            StructTypeName::expect_valid("Vec"),
        ));
        let symbolic = CheckedType::<Symbolic>::Indexed {
            element: Box::new(CheckedType::Struct(
                vec.clone(),
                vec![CheckedGenericArg::Nat(NatPolyForm::from_var(n()))],
            )),
            index: fin_n_plus_one(),
        };
        let bound = symbolic
            .instantiate(|form| {
                form.evaluate_with(|name| {
                    assert_eq!(name, &n());
                    Ok::<_, crate::nat::NatOverflowError>(2)
                })
            })
            .unwrap();
        assert_eq!(
            bound,
            CheckedType::Indexed {
                element: Box::new(CheckedType::Struct(vec, vec![CheckedGenericArg::Nat(2)])),
                index: IndexTypeRef::from_finite_index(fin(3)),
            }
        );
        assert_eq!(symbolic.to_concrete(), None);
        assert_eq!(bound.to_symbolic().to_concrete(), Some(bound.clone()));
        assert_eq!(
            bound.format(
                &crate::registry::types::FormattingRegistry::graphcal_prelude()
                    .unwrap()
                    .dimensions
            ),
            "Vec<2>[Fin(3)]"
        );
    }

    #[test]
    fn instantiate_reports_missing_bindings_and_invalid_cardinalities() {
        #[derive(Debug, PartialEq)]
        enum Missing {
            Name,
        }
        let key = CheckedType::<Symbolic>::Key(
            IndexTypeRef::from_finite_index_form(NatPolyForm::from_var(n())).unwrap(),
        );
        assert_eq!(
            key.instantiate(|_| Err(Missing::Name)),
            Err(InstantiationError::Binding(Missing::Name))
        );
        assert_eq!(
            key.instantiate(|_| Ok::<_, Missing>(0)),
            Err(InstantiationError::Cardinality(
                IndexCardinalityError::Empty
            ))
        );
        assert_eq!(
            fin_n_plus_one().instantiate(|form| {
                form.evaluate_with(|_| Ok::<_, crate::nat::NatOverflowError>(4))
            }),
            Ok(IndexTypeRef::from_finite_index(fin(5)))
        );
    }

    #[test]
    fn concrete_views_embed_without_change() {
        let owner = DagId::root_in_package("test", "main");
        let declared =
            IndexTypeRef::<Concrete>::with_owner(owner, IndexName::expect_valid("Phase"));
        assert_eq!(
            declared.to_symbolic().to_concrete().as_ref(),
            Some(&declared)
        );
        let finite = IndexTypeRef::<Concrete>::from_finite_index(fin(2));
        assert_eq!(
            finite.to_symbolic(),
            IndexTypeRef::from_finite_index_form(NatPolyForm::from_constant(2)).unwrap()
        );
        assert_eq!(fin_n_plus_one().to_concrete(), None);
    }

    #[test]
    fn indexes_collects_axes_keys_and_nominal_arguments() {
        let owner = DagId::root_in_package("test", "main");
        let phase =
            IndexTypeRef::<Concrete>::with_owner(owner.clone(), IndexName::expect_valid("Phase"));
        let finite = IndexTypeRef::from_finite_index(fin(2));
        let nested = CheckedType::Struct(
            StructTypeRef::from_resolved(ResolvedName::from_def(
                owner,
                StructTypeName::expect_valid("Pair"),
            )),
            vec![
                CheckedGenericArg::Index(phase.clone()),
                CheckedGenericArg::Type(CheckedType::Key(finite.clone())),
                CheckedGenericArg::Nat(1),
            ],
        );
        let ty = CheckedType::Indexed {
            element: Box::new(nested),
            index: finite.clone(),
        };
        assert_eq!(ty.indexes(), vec![&finite, &phase, &finite]);
        let (axes, element) = ty.peel_axes();
        assert_eq!(axes, vec![&finite]);
        assert!(matches!(element, CheckedType::Struct(..)));
    }

    #[test]
    fn materialized_shape_is_absent_for_scalars_and_unknown_axes() {
        let axis = IndexTypeRef::<Concrete>::from_finite_index(fin(3));
        let matrix = CheckedType::Indexed {
            element: Box::new(CheckedType::Indexed {
                element: Box::new(CheckedType::Int),
                index: axis.clone(),
            }),
            index: axis,
        };
        let known = |index: &IndexTypeRef| {
            Ok::<_, MaterializedShapeError>(index.finite_index().map(FiniteIndex::cardinality))
        };
        let shape = matrix.materialized_shape(known).unwrap().unwrap();
        assert_eq!(shape.rank(), 2);
        assert_eq!(shape.total().get(), 9);
        assert_eq!(
            CheckedType::<Concrete>::Int.materialized_shape(known),
            Ok(None)
        );
        assert_eq!(
            matrix.materialized_shape(|_| Ok::<_, MaterializedShapeError>(None)),
            Ok(None)
        );
        let huge = IndexTypeRef::<Concrete>::from_finite_index(fin(1_000_000));
        let too_large = CheckedType::Indexed {
            element: Box::new(CheckedType::Indexed {
                element: Box::new(CheckedType::Int),
                index: huge.clone(),
            }),
            index: huge,
        };
        assert!(matches!(
            too_large.materialized_shape(known),
            Err(MaterializedShapeError::ExceedsLimit { .. })
        ));
    }

    #[test]
    fn checked_type_projections_and_rank() {
        let length = Dimension::base(crate::dimension::BaseDimId::Prelude(
            crate::dimension::PreludeBaseDimension::Length,
        ));
        let axis =
            IndexTypeRef::<Concrete>::from_finite_index(FiniteIndex::try_from_u64(2).unwrap());
        let quantity = CheckedType::Quantity(length.clone());
        let complex = CheckedType::<Concrete>::Complex(length.clone());
        let matrix = CheckedType::Indexed {
            element: Box::new(CheckedType::Indexed {
                element: Box::new(quantity.clone()),
                index: axis.clone(),
            }),
            index: axis,
        };
        assert_eq!(quantity.quantity_dimension(), Some(&length));
        assert_eq!(quantity.complex_dimension(), None);
        assert_eq!(complex.complex_dimension(), Some(&length));
        assert_eq!(complex.quantity_dimension(), None);
        assert_eq!(quantity.indexed_rank(), 0);
        assert_eq!(matrix.indexed_rank(), 2);
        assert_eq!(matrix.quantity_dimension(), None);
    }

    #[test]
    fn type_name_ref_equality_uses_canonical_identity_not_display_leaf() {
        let owner = DagId::root_in_package("test", "main");
        let resolved = ResolvedName::from_def(owner, StructTypeName::expect_valid("Result"));
        let canonical = StructTypeRef::from_resolved(resolved.clone());
        let display_variant =
            StructTypeRef::with_display_leaf(StructTypeName::expect_valid("Success"), resolved);

        assert_eq!(canonical, display_variant);
        let mut set = std::collections::HashSet::new();
        set.insert(canonical);
        assert!(set.contains(&display_variant));
    }
}

/// A generic argument classified by its declared sort.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CheckedGenericArg<V: Concreteness = Concrete> {
    Dim(Dimension),
    Index(IndexTypeRef<V>),
    Nat(V::Nat),
    Type(CheckedType<V>),
}

impl CheckedGenericArg<Concrete> {
    /// View a concrete argument at the symbolic level.
    #[must_use]
    pub fn to_symbolic(&self) -> CheckedGenericArg<Symbolic> {
        match self {
            Self::Dim(dimension) => CheckedGenericArg::Dim(dimension.clone()),
            Self::Index(index) => CheckedGenericArg::Index(index.to_symbolic()),
            Self::Nat(value) => CheckedGenericArg::Nat(NatPolyForm::from_constant(*value)),
            Self::Type(ty) => CheckedGenericArg::Type(ty.to_symbolic()),
        }
    }
}

impl CheckedGenericArg<Symbolic> {
    /// The concrete argument, when it mentions no `Nat` variable.
    #[must_use]
    pub fn to_concrete(&self) -> Option<CheckedGenericArg<Concrete>> {
        NatBinder {
            close: &mut constant_only,
        }
        .generic_arg(self)
        .ok()
    }
}

#[derive(Clone, Copy)]
enum DiagnosticNameQualification {
    Leaf,
    OwnerQualified,
}

impl DiagnosticNameQualification {
    fn dimension(self, dimension: &Dimension, dims: &DimensionFormattingRegistry) -> String {
        match self {
            Self::Leaf => dims.format_dimension(dimension),
            Self::OwnerQualified => dimension.owner_qualified().to_string(),
        }
    }

    fn index<V: Concreteness>(self, index: &IndexTypeRef<V>) -> String {
        match (self, index.declared_resolved()) {
            (Self::OwnerQualified, Some(resolved)) => resolved.to_string(),
            (Self::Leaf | Self::OwnerQualified, None) | (Self::Leaf, Some(_)) => index.to_string(),
        }
    }

    fn struct_type(self, name: &StructTypeRef) -> String {
        match self {
            Self::Leaf => name.to_string(),
            Self::OwnerQualified => name.resolved().to_string(),
        }
    }
}

impl<V: Concreteness> CheckedGenericArg<V> {
    fn format(
        &self,
        dims: &DimensionFormattingRegistry,
        qualification: DiagnosticNameQualification,
    ) -> String {
        match self {
            Self::Dim(dimension) => qualification.dimension(dimension, dims),
            Self::Index(index) => qualification.index(index),
            Self::Nat(value) => value.form().format(),
            Self::Type(type_expr) => type_expr.format_with(dims, qualification),
        }
    }
}

/// A checked type: the semantic type of a declaration or an expression.
///
/// Declarations and inferred expressions share this one representation, so a
/// declaration matches its body exactly when the two types are equal. Index
/// arguments are never types; generic struct metadata carries them as
/// [`CheckedGenericArg::Index`]. `V` fixes whether structural `Fin(N)` axes
/// and `Nat` arguments may still be symbolic (see [`Concreteness`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CheckedType<V: Concreteness = Concrete> {
    Quantity(Dimension),
    /// A complex quantity whose real and imaginary components share one dimension.
    Complex(Dimension),
    Bool,
    Int,
    /// A datetime instant in a specific time scale. `Datetime(UTC)` is the default for civil use.
    Datetime(TimeScale),
    /// An index-key value type `Key<I>`: element keys of axis `I`.
    Key(IndexTypeRef<V>),
    /// A struct type, optionally with sorted generic arguments.
    Struct(StructTypeRef, Vec<CheckedGenericArg<V>>),
    Indexed {
        element: Box<Self>,
        index: IndexTypeRef<V>,
    },
}

impl<V: Concreteness> CheckedType<V> {
    /// The shared component dimension of a complex type.
    #[must_use]
    pub(crate) const fn complex_dimension(&self) -> Option<&Dimension> {
        match self {
            Self::Complex(dimension) => Some(dimension),
            _ => None,
        }
    }

    /// The dimension of a scalar quantity type.
    #[must_use]
    pub(crate) const fn quantity_dimension(&self) -> Option<&Dimension> {
        match self {
            Self::Quantity(dimension) => Some(dimension),
            _ => None,
        }
    }

    /// Number of index axes carried by this type.
    #[must_use]
    pub(crate) fn indexed_rank(&self) -> usize {
        match self {
            Self::Indexed { element, .. } => element.indexed_rank().saturating_add(1),
            _ => 0,
        }
    }

    /// The index axes of an indexed type, outermost first, and its element type.
    #[must_use]
    pub fn peel_axes(&self) -> (Vec<&IndexTypeRef<V>>, &Self) {
        let mut axes = Vec::new();
        let mut element = self;
        while let Self::Indexed {
            element: inner,
            index,
        } = element
        {
            axes.push(index);
            element = inner;
        }
        (axes, element)
    }

    /// Every index this type mentions: its axes, keys, and nominal index
    /// arguments, recursively.
    pub fn indexes(&self) -> Vec<&IndexTypeRef<V>> {
        let mut indexes = Vec::new();
        self.collect_indexes(&mut indexes);
        indexes
    }

    fn collect_indexes<'a>(&'a self, indexes: &mut Vec<&'a IndexTypeRef<V>>) {
        match self {
            Self::Indexed { element, index } => {
                indexes.push(index);
                element.collect_indexes(indexes);
            }
            Self::Key(index) => indexes.push(index),
            Self::Struct(_, args) => {
                for arg in args {
                    match arg {
                        CheckedGenericArg::Index(index) => indexes.push(index),
                        CheckedGenericArg::Type(ty) => ty.collect_indexes(indexes),
                        CheckedGenericArg::Dim(_) | CheckedGenericArg::Nat(_) => {}
                    }
                }
            }
            Self::Quantity(_) | Self::Complex(_) | Self::Bool | Self::Int | Self::Datetime(_) => {}
        }
    }

    /// The eagerly materialized shape of an indexed value.
    ///
    /// Returns `None` for a scalar type, or while an axis cardinality is not
    /// yet known (`cardinality` returns `None`).
    ///
    /// # Errors
    ///
    /// Propagates the `Err` of `cardinality`, and returns
    /// [`MaterializedShapeError`] when the total exceeds the eager-allocation
    /// policy.
    pub fn materialized_shape<E: From<MaterializedShapeError>>(
        &self,
        mut cardinality: impl FnMut(&IndexTypeRef<V>) -> Result<Option<IndexCardinality>, E>,
    ) -> Result<Option<MaterializedShape>, E> {
        let (axes, _) = self.peel_axes();
        let sizes = axes
            .into_iter()
            .map(&mut cardinality)
            .collect::<Result<Vec<_>, _>>()?;
        let Some(sizes) = sizes.into_iter().collect::<Option<Vec<_>>>() else {
            return Ok(None);
        };
        NonEmpty::try_from_vec(sizes)
            .ok()
            .map(|sizes| MaterializedShape::try_new(sizes).map_err(E::from))
            .transpose()
    }

    /// Format as a human-readable string for diagnostics (e.g. `"Length / Time"`, `"Bool"`).
    #[must_use]
    pub fn format(&self, dims: &DimensionFormattingRegistry) -> String {
        self.format_with(dims, DiagnosticNameQualification::Leaf)
    }

    /// Format every nominal identity with its canonical owner.
    ///
    /// Used only when two unequal semantic types would otherwise render with
    /// the same leaf-only diagnostic spelling.
    #[must_use]
    pub(crate) fn format_owner_qualified(&self, dims: &DimensionFormattingRegistry) -> String {
        self.format_with(dims, DiagnosticNameQualification::OwnerQualified)
    }

    fn format_with(
        &self,
        dims: &DimensionFormattingRegistry,
        qualification: DiagnosticNameQualification,
    ) -> String {
        match self {
            Self::Quantity(d) => qualification.dimension(d, dims),
            Self::Complex(d) => format!("Complex<{}>", qualification.dimension(d, dims)),
            Self::Bool => "Bool".to_string(),
            Self::Int => "Int".to_string(),
            Self::Datetime(scale) => {
                if scale.is_utc() {
                    "Datetime".to_string()
                } else {
                    format!("Datetime<{scale}>")
                }
            }
            Self::Key(index) => format!("Key<{}>", qualification.index(index)),
            Self::Struct(name, args) => {
                let name = qualification.struct_type(name);
                if args.is_empty() {
                    name
                } else {
                    let args_str: Vec<String> = args
                        .iter()
                        .map(|arg| arg.format(dims, qualification))
                        .collect();
                    format!("{name}<{}>", args_str.join(", "))
                }
            }
            Self::Indexed { .. } => {
                // Multi-axis source syntax uses one bracket list (`T[I, J]`),
                // while the concrete type stores one outer-to-inner layer per
                // axis. Flatten those layers at the diagnostic boundary rather
                // than exposing the invalid internal spelling `T[J][I]`.
                let (axes, base) = self.peel_axes();
                let indexes = axes
                    .into_iter()
                    .map(|index| qualification.index(index))
                    .collect::<Vec<_>>();
                format!(
                    "{}[{}]",
                    base.format_with(dims, qualification),
                    indexes.join(", ")
                )
            }
        }
    }
}

impl CheckedType<Symbolic> {
    /// Close every `Nat` form with `close`, producing the concrete type.
    ///
    /// This is the one traversal from symbolic to concrete types; a generic
    /// instantiation supplies its bindings through `close` (see
    /// `Substitution::instantiate`).
    ///
    /// # Errors
    ///
    /// Returns an [`InstantiationError`] when a form cannot be closed or a
    /// `Fin(N)` axis closes to an invalid cardinality.
    pub fn instantiate<E>(
        &self,
        mut close: impl FnMut(&NatPolyForm) -> Result<u64, E>,
    ) -> Result<CheckedType<Concrete>, InstantiationError<E>> {
        NatBinder { close: &mut close }.ty(self)
    }

    /// The concrete type, when no axis or argument mentions a `Nat` variable.
    #[must_use]
    pub fn to_concrete(&self) -> Option<CheckedType<Concrete>> {
        self.instantiate(constant_only).ok()
    }
}

impl CheckedType<Concrete> {
    /// View a concrete type at the symbolic level.
    #[must_use]
    pub fn to_symbolic(&self) -> CheckedType<Symbolic> {
        match self {
            Self::Quantity(dimension) => CheckedType::Quantity(dimension.clone()),
            Self::Complex(dimension) => CheckedType::Complex(dimension.clone()),
            Self::Bool => CheckedType::Bool,
            Self::Int => CheckedType::Int,
            Self::Datetime(scale) => CheckedType::Datetime(*scale),
            Self::Key(index) => CheckedType::Key(index.to_symbolic()),
            Self::Struct(identity, args) => CheckedType::Struct(
                identity.clone(),
                args.iter().map(CheckedGenericArg::to_symbolic).collect(),
            ),
            Self::Indexed { element, index } => CheckedType::Indexed {
                element: Box::new(element.to_symbolic()),
                index: index.to_symbolic(),
            },
        }
    }
}
