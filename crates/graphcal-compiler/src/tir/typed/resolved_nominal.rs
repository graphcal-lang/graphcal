//! One nominal type definition together with the resolved semantics of each
//! of its fields, aligned with the definition.

use crate::semantic_error::domain::{DomainSubject, NominalFieldPath};
use std::sync::Arc;

use crate::hir::nominal::{NominalConstructor, NominalField, NominalTypeDef, ResolvedConstructor};
use crate::resolved_name::ResolvedStructTypeName;
use crate::source_id::SourceId;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::Span;
use crate::syntax::type_name::FieldName;

use super::resolved_type::ResolvedDeclType;

/// A `min:`/`max:` domain bound with its expression lowered to HIR.
///
/// Declaration bounds cross into HIR with their owning type annotation. TIR
/// moves them into this checked semantic record so dimension checking and
/// evaluation consume the same canonical expression tree.
#[derive(Debug, Clone)]
pub struct ResolvedDomainBound {
    /// Whether this is a `min:` or `max:` bound.
    pub kind: crate::syntax::ast::DomainBoundKind,
    /// The bound expression, strictly lowered.
    pub value: crate::hir::expr::CheckedExpr,
    /// Span of the whole bound.
    pub span: Span,
    /// Source file whose bytes are indexed by `span` and the expression spans.
    pub src: SourceId,
}

/// Resolved semantic facts for one nominal field.
///
/// Keeping the target type and its bounds in one value prevents a constrained
/// field from existing without the type needed to validate those bounds.
#[derive(Debug, Clone)]
pub struct ResolvedStructFieldSemantics {
    resolved_type: ResolvedDeclType,
    /// The field's domain bounds; `None` when the field is unconstrained.
    domain_bounds: Option<NonEmpty<ResolvedDomainBound>>,
}

impl ResolvedStructFieldSemantics {
    /// A field of `resolved_type`, constrained when `domain_bounds` is not
    /// empty.
    #[must_use]
    pub fn new(resolved_type: ResolvedDeclType, domain_bounds: Vec<ResolvedDomainBound>) -> Self {
        Self {
            resolved_type,
            domain_bounds: NonEmpty::try_from_vec(domain_bounds).ok(),
        }
    }

    /// Return the field annotation resolved in its owning generic scope.
    #[must_use]
    pub const fn resolved_type(&self) -> &ResolvedDeclType {
        &self.resolved_type
    }

    /// Return domain bounds lowered in the same owning generic scope, when
    /// the field is constrained.
    #[must_use]
    pub const fn domain_bounds(&self) -> Option<&NonEmpty<ResolvedDomainBound>> {
        self.domain_bounds.as_ref()
    }
}

/// One nominal type definition with the resolved semantics of every field.
///
/// The semantics are resolved by walking the definition's own constructors
/// and fields, so every field of the definition has exactly one semantic
/// record and no record exists without its field. Consumers reach a field's
/// semantics through the definition instead of a side-table lookup.
#[derive(Debug, Clone)]
pub struct ResolvedNominal {
    definition: Arc<NominalTypeDef>,
    /// `fields[m][f]` belongs to field `f` of union member `m`, both in the
    /// definition's order. A required (bodiless) type has no members.
    fields: Vec<Vec<ResolvedStructFieldSemantics>>,
}

impl ResolvedNominal {
    /// Resolve the semantics of every field of `definition` with `resolve`,
    /// visiting constructors and their fields in definition order.
    ///
    /// # Errors
    ///
    /// Returns the first error `resolve` reports.
    pub(crate) fn try_resolve<E>(
        definition: Arc<NominalTypeDef>,
        mut resolve: impl FnMut(
            &NominalConstructor,
            &NominalField,
        ) -> Result<ResolvedStructFieldSemantics, E>,
    ) -> Result<Self, E> {
        let fields = definition
            .union_members()
            .into_iter()
            .flatten()
            .map(|member| {
                member
                    .fields()
                    .iter()
                    .map(|field| resolve(member, field))
                    .collect::<Result<Vec<_>, E>>()
            })
            .collect::<Result<Vec<_>, E>>()?;
        Ok(Self { definition, fields })
    }

    /// The shared definition handle.
    #[must_use]
    pub const fn definition(&self) -> &Arc<NominalTypeDef> {
        &self.definition
    }

    /// The type's canonical identity.
    #[must_use]
    pub fn identity(&self) -> &ResolvedStructTypeName {
        self.definition.identity()
    }

    /// Every constructor of the type, in definition order.
    pub fn members(&self) -> impl Iterator<Item = NominalMember<'_>> {
        self.definition
            .union_members()
            .into_iter()
            .flatten()
            .zip(&self.fields)
            .map(|(constructor, fields)| NominalMember {
                nominal: self,
                constructor,
                fields,
            })
    }

    /// The constructor `constructor` names, when it is a constructor of this
    /// type (of this very definition, not merely of a same-named one).
    #[must_use]
    pub fn member_of(&self, constructor: &ResolvedConstructor) -> Option<NominalMember<'_>> {
        if !Arc::ptr_eq(constructor.definition(), &self.definition) {
            return None;
        }
        self.members().nth(constructor.position())
    }

    /// The only constructor of a record-shaped type: one constructor named
    /// like the type.
    #[must_use]
    pub fn record_member(&self) -> Option<NominalMember<'_>> {
        let mut members = self.members();
        let only = members.next()?;
        (members.next().is_none()
            && only.constructor.name().as_str() == self.definition.name().as_str())
        .then_some(only)
    }

    /// Every field that carries domain bounds, with its bounds, in
    /// definition order.
    pub fn constrained_fields(&self) -> impl Iterator<Item = ConstrainedField<'_>> {
        self.members()
            .flat_map(NominalMember::fields)
            .filter_map(|field| {
                field
                    .semantics
                    .domain_bounds()
                    .map(|bounds| ConstrainedField { field, bounds })
            })
    }
}

/// One constructor of a [`ResolvedNominal`] with its fields' semantics.
#[derive(Debug, Clone, Copy)]
pub struct NominalMember<'a> {
    nominal: &'a ResolvedNominal,
    constructor: &'a NominalConstructor,
    fields: &'a [ResolvedStructFieldSemantics],
}

impl<'a> NominalMember<'a> {
    /// The type this constructor builds.
    #[must_use]
    pub const fn nominal(self) -> &'a ResolvedNominal {
        self.nominal
    }

    /// The constructor's definition.
    #[must_use]
    pub const fn constructor(self) -> &'a NominalConstructor {
        self.constructor
    }

    /// Every field of the constructor with its semantics, in definition
    /// order.
    pub fn fields(self) -> impl Iterator<Item = NominalFieldSemantics<'a>> {
        self.constructor
            .fields()
            .iter()
            .zip(self.fields)
            .map(move |(field, semantics)| NominalFieldSemantics {
                member: self,
                field,
                semantics,
            })
    }

    /// The field named `name`, when the constructor has one.
    #[must_use]
    pub fn field(self, name: &FieldName) -> Option<NominalFieldSemantics<'a>> {
        self.fields().find(|field| field.field.name() == name)
    }
}

/// One field of a [`NominalMember`] with its resolved semantics.
#[derive(Debug, Clone, Copy)]
pub struct NominalFieldSemantics<'a> {
    member: NominalMember<'a>,
    field: &'a NominalField,
    semantics: &'a ResolvedStructFieldSemantics,
}

impl<'a> NominalFieldSemantics<'a> {
    /// The constructor this field belongs to.
    #[must_use]
    pub const fn member(self) -> NominalMember<'a> {
        self.member
    }

    /// The field's definition.
    #[must_use]
    pub const fn field(self) -> &'a NominalField {
        self.field
    }

    /// The field's resolved semantics.
    #[must_use]
    pub const fn semantics(self) -> &'a ResolvedStructFieldSemantics {
        self.semantics
    }

    /// How diagnostics name the field: `Type.field` for a record-shaped
    /// constructor named like its type, `Type.Constructor.field` otherwise.
    #[must_use]
    pub fn domain_subject(self) -> DomainSubject {
        let definition = self.member.nominal.definition();
        let constructor = self.member.constructor.name();
        let record_shaped = constructor.as_str() == definition.name().as_str();
        DomainSubject::NominalField(Box::new(NominalFieldPath {
            type_name: definition.name(),
            constructor: (!record_shaped).then(|| constructor.clone()),
            field: self.field.name().clone(),
        }))
    }
}

/// A field of a [`ResolvedNominal`] that carries domain bounds.
#[derive(Debug, Clone, Copy)]
pub struct ConstrainedField<'a> {
    field: NominalFieldSemantics<'a>,
    bounds: &'a NonEmpty<ResolvedDomainBound>,
}

impl<'a> ConstrainedField<'a> {
    /// The field with its semantics.
    #[must_use]
    pub const fn field(self) -> NominalFieldSemantics<'a> {
        self.field
    }

    /// The field's domain bounds.
    #[must_use]
    pub const fn bounds(self) -> &'a NonEmpty<ResolvedDomainBound> {
        self.bounds
    }
}
