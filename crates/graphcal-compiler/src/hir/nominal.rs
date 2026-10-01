//! Canonical HIR definitions for user-declared nominal types.
//!
//! A [`NominalTypeDef`] is lowered once from its `type` declaration (by the
//! `nominal_lower` module): its identity, generic defaults, field types, and
//! field bounds are all canonical HIR and must never be lowered again by TIR.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;
use thiserror::Error;

use crate::ir::static_substitution::StaticSubstitution;
use crate::resolved_name::{ResolvedConstructorName, ResolvedStructTypeName};
use crate::syntax::ast::GenericConstraint;
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorName, FieldName, GenericParamName, StructTypeName};

use super::type_annotation::TypeAnnotation;
use super::types::{GenericArg, GenericParamId};

/// One constructor field whose complete signature has crossed into HIR.
#[derive(Debug, Clone)]
pub struct NominalField {
    name: FieldName,
    type_annotation: TypeAnnotation,
}

impl NominalField {
    #[must_use]
    pub(crate) const fn new(name: FieldName, type_annotation: TypeAnnotation) -> Self {
        Self {
            name,
            type_annotation,
        }
    }

    #[must_use]
    pub const fn name(&self) -> &FieldName {
        &self.name
    }

    /// Canonical field type and its HIR domain bounds.
    #[must_use]
    pub const fn type_annotation(&self) -> &TypeAnnotation {
        &self.type_annotation
    }
}

/// One canonically owned tagged-union constructor.
#[derive(Debug, Clone)]
pub struct NominalConstructor {
    identity: ResolvedConstructorName,
    fields: Vec<NominalField>,
}

impl NominalConstructor {
    pub(crate) fn try_new(
        identity: ResolvedConstructorName,
        fields: Vec<NominalField>,
    ) -> Result<Self, NominalTypeError> {
        let mut first_positions = HashMap::new();
        for (duplicate_index, field) in fields.iter().enumerate() {
            match first_positions.entry(field.name.clone()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(duplicate_index);
                }
                std::collections::hash_map::Entry::Occupied(entry) => {
                    return Err(NominalTypeError::DuplicateConstructorField {
                        constructor: identity.to_unowned_def_name(),
                        field: field.name.clone(),
                        first_index: *entry.get(),
                        duplicate_index,
                    });
                }
            }
        }
        Ok(Self { identity, fields })
    }

    /// Canonical constructor identity, including its defining DAG.
    #[must_use]
    pub const fn identity(&self) -> &ResolvedConstructorName {
        &self.identity
    }

    #[must_use]
    pub fn name(&self) -> ConstructorName {
        self.identity.to_unowned_def_name()
    }

    #[must_use]
    pub fn fields(&self) -> &[NominalField] {
        &self.fields
    }
}

/// The checked shape category available at the HIR boundary.
#[derive(Debug, Clone)]
pub enum NominalTypeKind {
    /// A bindable type declaration without a body.
    Required,
    /// A tagged union; a one-constructor union may have record semantics.
    Union { members: Vec<NominalConstructor> },
}

/// One lexical generic parameter and its already-sorted HIR default.
#[derive(Debug, Clone)]
pub struct NominalGenericParam {
    id: GenericParamId,
    constraint: GenericConstraint,
    default: Option<GenericArg>,
    span: Span,
}

impl NominalGenericParam {
    #[must_use]
    pub(crate) const fn new(
        id: GenericParamId,
        constraint: GenericConstraint,
        default: Option<GenericArg>,
        span: Span,
    ) -> Self {
        Self {
            id,
            constraint,
            default,
            span,
        }
    }

    /// Lexical identity, including the canonical owning type.
    #[must_use]
    pub const fn id(&self) -> &GenericParamId {
        &self.id
    }

    #[must_use]
    pub const fn name(&self) -> &GenericParamName {
        &self.id.name
    }

    #[must_use]
    pub const fn constraint(&self) -> GenericConstraint {
        self.constraint
    }

    #[must_use]
    pub const fn default(&self) -> Option<&GenericArg> {
        self.default.as_ref()
    }

    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }
}

/// One complete canonical nominal definition before semantic type checking.
#[derive(Debug, Clone)]
pub struct NominalTypeDef {
    identity: ResolvedStructTypeName,
    generic_params: Vec<NominalGenericParam>,
    kind: NominalTypeKind,
    source: NamedSource<Arc<String>>,
    span: Span,
    /// For a definition an include projects from its template, the include's
    /// canonical substitution. Signature names are already substituted; the
    /// dimensions a named template dimension is defined over are substituted
    /// when its field types are resolved.
    instance_substitution: Option<StaticSubstitution>,
}

impl NominalTypeDef {
    #[must_use]
    pub(crate) const fn required(
        identity: ResolvedStructTypeName,
        generic_params: Vec<NominalGenericParam>,
        source: NamedSource<Arc<String>>,
        span: Span,
    ) -> Self {
        Self {
            identity,
            generic_params,
            kind: NominalTypeKind::Required,
            source,
            span,
            instance_substitution: None,
        }
    }

    pub(crate) fn try_union(
        identity: ResolvedStructTypeName,
        generic_params: Vec<NominalGenericParam>,
        members: Vec<NominalConstructor>,
        source: NamedSource<Arc<String>>,
        span: Span,
    ) -> Result<Self, NominalTypeError> {
        let mut constructors = HashSet::new();
        for member in &members {
            if member.identity.owner() != identity.owner() {
                return Err(NominalTypeError::ConstructorOwnerMismatch {
                    constructor: member.identity.clone(),
                    owning_type: identity,
                });
            }
            if !constructors.insert(member.identity.clone()) {
                return Err(NominalTypeError::DuplicateConstructor {
                    constructor: member.name(),
                });
            }
        }
        Ok(Self {
            identity,
            generic_params,
            kind: NominalTypeKind::Union { members },
            source,
            span,
            instance_substitution: None,
        })
    }

    /// Mark this definition as projected from a template through an
    /// include's canonical `substitution`.
    #[must_use]
    pub(crate) fn with_instance_substitution(mut self, substitution: StaticSubstitution) -> Self {
        self.instance_substitution = Some(substitution);
        self
    }

    /// The include substitution this definition was projected through, if any.
    #[must_use]
    pub const fn instance_substitution(&self) -> Option<&StaticSubstitution> {
        self.instance_substitution.as_ref()
    }

    /// Canonical type identity, including its defining DAG.
    #[must_use]
    pub const fn identity(&self) -> &ResolvedStructTypeName {
        &self.identity
    }

    #[must_use]
    pub fn name(&self) -> StructTypeName {
        self.identity.to_unowned_def_name()
    }

    #[must_use]
    pub fn generic_params(&self) -> &[NominalGenericParam] {
        &self.generic_params
    }

    #[must_use]
    pub const fn kind(&self) -> &NominalTypeKind {
        &self.kind
    }

    #[must_use]
    pub fn union_members(&self) -> Option<&[NominalConstructor]> {
        match &self.kind {
            NominalTypeKind::Required => None,
            NominalTypeKind::Union { members } => Some(members),
        }
    }

    #[must_use]
    pub const fn is_required(&self) -> bool {
        matches!(self.kind, NominalTypeKind::Required)
    }

    /// Return the payload of a one-constructor record-shaped definition.
    #[must_use]
    pub fn record_fields(&self) -> Option<&[NominalField]> {
        let NominalTypeKind::Union { members } = &self.kind else {
            return None;
        };
        let [only] = members.as_slice() else {
            return None;
        };
        (only.identity.atom() == self.identity.atom()).then_some(only.fields.as_slice())
    }

    #[must_use]
    pub const fn source(&self) -> &NamedSource<Arc<String>> {
        &self.source
    }

    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }
}

/// One canonical constructor resolved to the nominal definition that owns it.
///
/// Values are only enumerated from a definition's own union members, so the
/// variant always belongs to the shared definition handle. The variant's field
/// annotations carry the field domain bounds, so a constructor application
/// knows its field constraints without a side table.
#[derive(Debug, Clone)]
pub struct ResolvedConstructor {
    def: Arc<NominalTypeDef>,
    variant: NominalConstructor,
}

impl ResolvedConstructor {
    /// Every constructor of `def`, each sharing the same definition handle.
    /// A required (bodiless) type has no constructors.
    pub(crate) fn members_of(def: &Arc<NominalTypeDef>) -> impl Iterator<Item = Self> + '_ {
        def.union_members()
            .into_iter()
            .flatten()
            .map(|variant| Self {
                def: Arc::clone(def),
                variant: variant.clone(),
            })
    }

    /// Shared handle of the owning nominal definition.
    #[must_use]
    pub const fn definition(&self) -> &Arc<NominalTypeDef> {
        &self.def
    }

    /// Canonical identity of the owning nominal type.
    #[must_use]
    pub fn owning_type(&self) -> &ResolvedStructTypeName {
        self.def.identity()
    }

    /// The tagged-union member this constructor builds.
    #[must_use]
    pub const fn variant(&self) -> &NominalConstructor {
        &self.variant
    }

    /// Canonical constructor identity, including its defining DAG.
    #[must_use]
    pub const fn identity(&self) -> &ResolvedConstructorName {
        self.variant.identity()
    }

    #[must_use]
    pub fn name(&self) -> ConstructorName {
        self.variant.name()
    }

    /// Payload fields whose annotations declare domain bounds.
    pub fn constrained_fields(&self) -> impl Iterator<Item = &FieldName> {
        self.variant
            .fields()
            .iter()
            .filter(|field| !field.type_annotation().domain_bounds.is_empty())
            .map(NominalField::name)
    }

    /// Whether `field` carries a domain constraint in this constructor.
    #[must_use]
    pub fn constrains(&self, field: &FieldName) -> bool {
        self.constrained_fields()
            .any(|constrained| constrained == field)
    }
}

/// Canonical identities determine equality: one identity has one definition.
impl PartialEq for ResolvedConstructor {
    fn eq(&self, other: &Self) -> bool {
        self.owning_type() == other.owning_type() && self.identity() == other.identity()
    }
}

impl Eq for ResolvedConstructor {}

/// Failure to construct an invariant-preserving HIR nominal definition.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NominalTypeError {
    #[error("constructor `{constructor}` declares field `{field}` more than once")]
    DuplicateConstructorField {
        constructor: ConstructorName,
        field: FieldName,
        first_index: usize,
        duplicate_index: usize,
    },
    #[error("constructor `{constructor}` is declared more than once")]
    DuplicateConstructor { constructor: ConstructorName },
    #[error("constructor `{constructor}` does not belong to type `{owning_type}`")]
    ConstructorOwnerMismatch {
        constructor: ResolvedConstructorName,
        owning_type: ResolvedStructTypeName,
    },
    #[error("nominal type `{identity}` is already present in HIR")]
    DuplicateType { identity: ResolvedStructTypeName },
    #[error("constructor `{constructor}` is already owned by HIR type `{first_owner}`")]
    DuplicateCanonicalConstructor {
        constructor: ResolvedConstructorName,
        first_owner: ResolvedStructTypeName,
    },
    /// Re-owning a specialized definition's Nat forms overflowed.
    #[error(transparent)]
    NatOverflow(#[from] crate::nat::NatOverflowError),
}

/// Canonical nominal definitions owned by one HIR DAG.
///
/// Keys are always derived from the contained definitions. Construction also
/// enforces the per-DAG constructor namespace, so neither a mismatched map key
/// nor two owners for one canonical constructor can be represented.
#[derive(Debug, Clone, Default)]
pub struct NominalTypeRegistry {
    definitions: HashMap<ResolvedStructTypeName, Arc<NominalTypeDef>>,
    constructor_owners: HashMap<ResolvedConstructorName, ResolvedStructTypeName>,
}

impl NominalTypeRegistry {
    pub(crate) fn insert(&mut self, definition: NominalTypeDef) -> Result<(), NominalTypeError> {
        let identity = definition.identity.clone();
        if self.definitions.contains_key(&identity) {
            return Err(NominalTypeError::DuplicateType { identity });
        }
        if let Some(members) = definition.union_members() {
            for member in members {
                if let Some(first_owner) = self.constructor_owners.get(member.identity()) {
                    return Err(NominalTypeError::DuplicateCanonicalConstructor {
                        constructor: member.identity().clone(),
                        first_owner: first_owner.clone(),
                    });
                }
            }
            for member in members {
                self.constructor_owners
                    .insert(member.identity().clone(), identity.clone());
            }
        }
        self.definitions.insert(identity, Arc::new(definition));
        Ok(())
    }

    #[must_use]
    pub fn get(&self, identity: &ResolvedStructTypeName) -> Option<&Arc<NominalTypeDef>> {
        self.definitions.get(identity)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ResolvedStructTypeName, &Arc<NominalTypeDef>)> {
        self.definitions.iter()
    }

    pub fn values(&self) -> impl Iterator<Item = &Arc<NominalTypeDef>> {
        self.definitions.values()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}
