use std::collections::HashMap;

use crate::desugar::desugared_ast::{GenericConstraint, TypeExpr};
use crate::syntax::type_name::{ConstructorName, FieldName, GenericParamName, StructTypeName};

#[derive(Debug, Clone)]
pub struct StructField {
    pub name: FieldName,
    pub type_ann: TypeExpr,
}

/// A member (constructor) of a tagged-union type.
///
/// The compiler treats every `type T { ... }` declaration as an n-variant
/// tagged union — including single-variant cases. Each variant carries
/// its payload fields inline; there are no per-variant standalone types.
#[derive(Debug, Clone)]
pub struct UnionMemberDef {
    /// Constructor name.
    pub name: ConstructorName,
    /// Payload fields for this constructor. An empty `Vec` means a unit
    /// constructor (`Coast`).
    pub fields: Vec<StructField>,
}

/// The kind of a type definition.
///
/// The functional core only distinguishes two shapes: a *required* type
/// stub (no body, awaits binding via include) and an *n-variant union*
/// — single-variant or multi-variant alike. Record-shaped types are
/// represented as a single-variant union whose sole constructor's name
/// matches the type's name (e.g.,
/// `type Position { Position(x: Length, y: Length) }`).
#[derive(Debug, Clone)]
pub enum TypeDefKind {
    /// A required type with no body: `type Element;`. Bound from outside
    /// via parameterized include.
    Required,
    /// A tagged union: `type Maneuver { Impulsive(delta_v: Velocity), Coast }`
    /// or, as a single-variant special case,
    /// `type Position { Position(x: Length, y: Length) }`.
    Union { members: Vec<UnionMemberDef> },
}

/// The constraint on a generic parameter of a type definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeGenericConstraint {
    /// `D: Dim` — the generic stands for a dimension.
    Dim,
    /// `I: Index` — the generic stands for an index.
    Index,
    /// `N: Nat` — the generic stands for a natural number (type-level).
    Nat,
    /// `F: Type` — unconstrained phantom type parameter.
    Unconstrained,
}

impl From<GenericConstraint> for TypeGenericConstraint {
    fn from(c: GenericConstraint) -> Self {
        match c {
            GenericConstraint::Dim => Self::Dim,
            GenericConstraint::Index => Self::Index,
            GenericConstraint::Nat => Self::Nat,
            GenericConstraint::Type => Self::Unconstrained,
        }
    }
}

/// A generic parameter on a type definition.
#[derive(Debug, Clone)]
pub struct TypeGenericParam {
    pub name: GenericParamName,
    pub constraint: TypeGenericConstraint,
    /// Optional default type expression, e.g. `F: Type = Unframed`.
    pub default: Option<crate::desugar::desugared_ast::TypeExpr>,
}

/// A registered type definition: either a required type stub or a tagged union.
#[derive(Debug, Clone)]
pub struct TypeDef {
    pub name: StructTypeName,
    pub generic_params: Vec<TypeGenericParam>,
    pub kind: TypeDefKind,
}

impl TypeDef {
    /// Returns the union members if this is a tagged union.
    ///
    /// Returns `None` only for a required (unbound) type stub.
    #[must_use]
    pub fn union_members(&self) -> Option<&[UnionMemberDef]> {
        match &self.kind {
            TypeDefKind::Union { members } => Some(members),
            TypeDefKind::Required => None,
        }
    }

    /// Returns `true` if this is a tagged union — single-variant or
    /// multi-variant.
    #[must_use]
    pub const fn is_union(&self) -> bool {
        matches!(self.kind, TypeDefKind::Union { .. })
    }

    /// Returns `true` if this is a required type stub awaiting binding.
    #[must_use]
    pub const fn is_required(&self) -> bool {
        matches!(self.kind, TypeDefKind::Required)
    }

    /// If this is a single-variant union whose sole constructor's name
    /// equals the type's name, returns that variant's payload fields.
    /// This is the record-like shape: field access and brace
    /// construction work directly on it.
    ///
    /// For multi-variant unions or single-variant unions whose
    /// constructor name differs from the type name, returns `None` —
    /// callers must dispatch through the constructor namespace and / or
    /// `match`.
    #[must_use]
    pub fn record_fields(&self) -> Option<&[StructField]> {
        let TypeDefKind::Union { members } = &self.kind else {
            return None;
        };
        let [only] = members.as_slice() else {
            return None;
        };
        (only.name.as_str() == self.name.as_str()).then_some(only.fields.as_slice())
    }
}

/// Type registry: maps type names to `TypeDef` and provides
/// constructor-namespace lookup.
///
/// The constructor namespace is *separate from* the type namespace: a
/// single lexeme can name both a type (`Position` — the n-variant
/// union) and a constructor (`Position` — the sole constructor of that
/// union). [`lookup_ctor`](Self::lookup_ctor) walks the constructor
/// side; [`get_type`](Self::get_type) walks the type side.
#[derive(Debug, Clone)]
pub struct TypeRegistry {
    pub(crate) types: HashMap<StructTypeName, TypeDef>,
    /// Constructor namespace: each constructor name resolves to the
    /// union it belongs to. With no module system, the namespace is
    /// flat. Duplicate names are rejected upstream during name
    /// resolution; like every `register_*` entry point, insertion here
    /// is last-wins defense-in-depth, not a validation layer.
    pub(crate) ctors: HashMap<ConstructorName, StructTypeName>,
}

impl TypeRegistry {
    /// Look up a type definition by type name.
    #[must_use]
    pub fn get_type(&self, name: &str) -> Option<&TypeDef> {
        self.types.get(name)
    }

    /// Look up the union that owns a constructor name, plus the
    /// constructor's payload fields. Returns `None` if the name is not
    /// a registered constructor.
    #[must_use]
    pub fn lookup_ctor(&self, ctor: &ConstructorName) -> Option<(&TypeDef, &UnionMemberDef)> {
        let union_name = self.ctors.get(ctor)?;
        let td = self.types.get(union_name)?;
        let members = td.union_members()?;
        let member = members.iter().find(|m| m.name == *ctor)?;
        Some((td, member))
    }

    /// Iterate over all registered type definitions.
    pub fn all_types(&self) -> impl Iterator<Item = &TypeDef> {
        self.types.values()
    }
}
