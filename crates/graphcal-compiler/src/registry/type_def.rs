use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::desugar::desugared_ast::{GenericConstraint, TypeExpr};
use crate::syntax::type_name::{ConstructorName, FieldName, GenericParamName, StructTypeName};

/// A typed field in a constructor payload.
///
/// Fields can only enter a [`UnionMemberDef`] through
/// [`UnionMemberDef::try_new`], which enforces uniqueness within that
/// constructor.
#[derive(Debug, Clone)]
pub struct StructField {
    name: FieldName,
    type_ann: TypeExpr,
}

impl StructField {
    #[must_use]
    pub const fn new(name: FieldName, type_ann: TypeExpr) -> Self {
        Self { name, type_ann }
    }

    #[must_use]
    pub const fn name(&self) -> &FieldName {
        &self.name
    }

    #[must_use]
    pub const fn type_ann(&self) -> &TypeExpr {
        &self.type_ann
    }
}

/// A member (constructor) of a tagged-union type.
///
/// The compiler treats every `type T { ... }` declaration as an n-variant
/// tagged union — including single-variant cases. Each variant carries
/// its payload fields inline; there are no per-variant standalone types.
#[derive(Debug, Clone)]
pub struct UnionMemberDef {
    /// Constructor name.
    name: ConstructorName,
    /// Payload fields for this constructor. An empty `Vec` means a unit
    /// constructor (`Coast`). Field names are unique by construction.
    fields: Vec<StructField>,
}

impl UnionMemberDef {
    /// Construct a union member while enforcing unique payload-field names.
    ///
    /// # Errors
    ///
    /// Returns [`TypeDefError::DuplicateConstructorField`] with both field
    /// positions when the payload repeats a name.
    pub fn try_new(name: ConstructorName, fields: Vec<StructField>) -> Result<Self, TypeDefError> {
        let mut first_positions: HashMap<FieldName, usize> = HashMap::new();
        for (duplicate_index, field) in fields.iter().enumerate() {
            match first_positions.entry(field.name.clone()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(duplicate_index);
                }
                std::collections::hash_map::Entry::Occupied(entry) => {
                    return Err(TypeDefError::DuplicateConstructorField {
                        constructor: name,
                        field: field.name.clone(),
                        first_index: *entry.get(),
                        duplicate_index,
                    });
                }
            }
        }
        Ok(Self { name, fields })
    }

    #[must_use]
    pub const fn name(&self) -> &ConstructorName {
        &self.name
    }

    #[must_use]
    pub fn fields(&self) -> &[StructField] {
        &self.fields
    }
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
    /// `F: Type` — the generic stands for a value type.
    Type,
}

impl From<GenericConstraint> for TypeGenericConstraint {
    fn from(c: GenericConstraint) -> Self {
        match c {
            GenericConstraint::Dim => Self::Dim,
            GenericConstraint::Index => Self::Index,
            GenericConstraint::Nat => Self::Nat,
            GenericConstraint::Type => Self::Type,
        }
    }
}

impl std::fmt::Display for TypeGenericConstraint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Dim => "Dim",
            Self::Index => "Index",
            Self::Nat => "Nat",
            Self::Type => "Type",
        })
    }
}

/// A generic parameter on a type definition.
#[derive(Debug, Clone)]
pub struct TypeGenericParam {
    pub name: GenericParamName,
    pub(crate) constraint: TypeGenericConstraint,
    /// Optional unresolved generic argument, e.g. `F: Type = Unframed` or
    /// `N: Nat = 3`. It is sorted against `constraint` at the HIR boundary.
    pub(crate) default: Option<crate::desugar::desugared_ast::GenericArg>,
    /// Definition-site span of the parameter name.
    pub(crate) span: crate::syntax::span::Span,
}

/// Failure to construct a semantically valid nominal type definition.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TypeDefError {
    /// One constructor payload repeated a field name.
    #[error("constructor `{constructor}` declares field `{field}` more than once")]
    DuplicateConstructorField {
        constructor: ConstructorName,
        field: FieldName,
        first_index: usize,
        duplicate_index: usize,
    },
    /// A tagged union repeated a constructor name.
    #[error("constructor `{constructor}` is declared more than once")]
    DuplicateConstructor { constructor: ConstructorName },
}

/// A registered type definition: either a required type stub or a tagged union.
///
/// Its fields are private so registry clients cannot bypass checked union-member
/// construction or replace a validated definition with malformed parts.
#[derive(Debug, Clone)]
pub struct TypeDef {
    name: StructTypeName,
    generic_params: Vec<TypeGenericParam>,
    kind: TypeDefKind,
}

impl TypeDef {
    /// Construct a required (unbound) type declaration.
    #[must_use]
    pub const fn required(name: StructTypeName, generic_params: Vec<TypeGenericParam>) -> Self {
        Self {
            name,
            generic_params,
            kind: TypeDefKind::Required,
        }
    }

    /// Construct a tagged union after checking constructor uniqueness.
    ///
    /// Payload-field uniqueness has already been enforced by
    /// [`UnionMemberDef::try_new`].
    ///
    /// # Errors
    ///
    /// Returns [`TypeDefError::DuplicateConstructor`] when two members repeat
    /// a constructor name.
    pub fn try_union(
        name: StructTypeName,
        generic_params: Vec<TypeGenericParam>,
        members: Vec<UnionMemberDef>,
    ) -> Result<Self, TypeDefError> {
        let mut constructors = HashSet::new();
        for member in &members {
            if !constructors.insert(member.name.clone()) {
                return Err(TypeDefError::DuplicateConstructor {
                    constructor: member.name.clone(),
                });
            }
        }
        Ok(Self {
            name,
            generic_params,
            kind: TypeDefKind::Union { members },
        })
    }

    #[must_use]
    pub const fn name(&self) -> &StructTypeName {
        &self.name
    }

    #[must_use]
    pub fn generic_params(&self) -> &[TypeGenericParam] {
        &self.generic_params
    }

    #[must_use]
    pub const fn kind(&self) -> &TypeDefKind {
        &self.kind
    }

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

    /// Returns `true` if this is a tagged union.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn is_union(&self) -> bool {
        matches!(self.kind, TypeDefKind::Union { .. })
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
        (only.name.atom() == self.name.atom()).then_some(only.fields.as_slice())
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
