//! Canonical HIR definitions for user-declared nominal types.
//!
//! Frontend registries retain syntax-shaped definitions while include
//! elaboration is still allowed to rewrite names. A [`NominalTypeDef`] is the
//! post-elaboration boundary: its identity, generic defaults, field types, and
//! field bounds are all canonical HIR and must never be lowered again by TIR.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;
use thiserror::Error;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::registry::error::GraphcalError;
use crate::registry::type_def::{
    StructField as FrontendStructField, TypeDef as FrontendTypeDef, TypeDefKind,
    TypeGenericConstraint, TypeGenericParam,
};
use crate::registry::types::Registry;
use crate::syntax::module_resolve::ModuleResolver;
use crate::syntax::span::Span;
use crate::syntax::type_name::{
    ConstructorName, FieldName, GenericParamName, ResolvedConstructorName, ResolvedStructTypeName,
    StructTypeName,
};

use super::{GenericArg, GenericParamId, TypeAnnotation};

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
    constraint: TypeGenericConstraint,
    default: Option<GenericArg>,
    span: Span,
}

impl NominalGenericParam {
    #[must_use]
    pub(crate) const fn new(
        id: GenericParamId,
        constraint: TypeGenericConstraint,
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
    pub const fn constraint(&self) -> TypeGenericConstraint {
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
        })
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

struct NominalLoweringContext<'a> {
    registry: &'a Registry,
    resolver: &'a ModuleResolver,
    src: &'a NamedSource<Arc<String>>,
    cancellation: &'a crate::cancellation::CancellationToken,
}

/// Lower every nominal definition owned directly by one DAG.
pub(crate) fn lower_nominal_type_registry(
    owner: &crate::dag_id::DagId,
    registry: &Registry,
    resolver: &ModuleResolver,
    src: &NamedSource<Arc<String>>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<NominalTypeRegistry, GraphcalError> {
    let symbols = resolver.modules().get(owner).ok_or_else(|| {
        GraphcalError::internal_error(
            format!("module resolver is missing HIR DAG `{owner}`"),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })?;
    let mut declarations = symbols.struct_types().iter().collect::<Vec<_>>();
    declarations.sort_by_key(|(_, symbol)| symbol.span().offset());
    let ctx = NominalLoweringContext {
        registry,
        resolver,
        src,
        cancellation,
    };
    declarations.into_iter().try_fold(
        NominalTypeRegistry::default(),
        |mut lowered, (source_name, symbol)| {
            ctx.cancellation.checkpoint()?;
            let frontend = registry.types.get_type(source_name.as_str()).ok_or_else(|| {
                invariant_error(
                    format!(
                        "frontend registry is missing local type `{source_name}` owned by `{owner}`"
                    ),
                    src,
                    symbol.span(),
                )
            })?;
            let definition =
                lower_nominal_type_def(frontend, symbol.resolved().clone(), symbol.span(), &ctx)?;
            lowered.insert(definition).map_err(|error| {
                invariant_error(
                    format!("cannot register HIR nominal type: {error}"),
                    src,
                    symbol.span(),
                )
            })?;
            Ok(lowered)
        },
    )
}

fn lower_nominal_type_def(
    frontend: &FrontendTypeDef,
    identity: ResolvedStructTypeName,
    span: Span,
    ctx: &NominalLoweringContext<'_>,
) -> Result<NominalTypeDef, GraphcalError> {
    let (generic_params, generic_scope) = lower_generic_params(frontend, &identity, ctx)?;
    match frontend.kind() {
        TypeDefKind::Required => Ok(NominalTypeDef::required(
            identity,
            generic_params,
            ctx.src.clone(),
            span,
        )),
        TypeDefKind::Union { members } => {
            let members = members
                .iter()
                .map(|member| {
                    ctx.cancellation.checkpoint()?;
                    let fields = member
                        .fields()
                        .iter()
                        .map(|field| lower_nominal_field(field, &identity, &generic_scope, ctx))
                        .collect::<Result<Vec<_>, GraphcalError>>()?;
                    NominalConstructor::try_new(
                        ResolvedConstructorName::from_def(
                            identity.owner().clone(),
                            member.name().clone(),
                        ),
                        fields,
                    )
                    .map_err(|error| {
                        invariant_error(
                            format!("invalid HIR constructor in `{identity}`: {error}"),
                            ctx.src,
                            span,
                        )
                    })
                })
                .collect::<Result<Vec<_>, GraphcalError>>()?;
            NominalTypeDef::try_union(
                identity.clone(),
                generic_params,
                members,
                ctx.src.clone(),
                span,
            )
            .map_err(|error| {
                invariant_error(
                    format!("invalid HIR nominal type `{identity}`: {error}"),
                    ctx.src,
                    span,
                )
            })
        }
    }
}

fn lower_generic_params(
    frontend: &FrontendTypeDef,
    identity: &ResolvedStructTypeName,
    ctx: &NominalLoweringContext<'_>,
) -> Result<(Vec<NominalGenericParam>, super::GenericScope), GraphcalError> {
    let generic_owner = super::GenericParamOwner::Type(identity.clone());
    frontend.generic_params().iter().try_fold(
        (
            Vec::with_capacity(frontend.generic_params().len()),
            super::GenericScope::new(),
        ),
        |(mut lowered, mut scope), param| {
            ctx.cancellation.checkpoint()?;
            let constraint = generic_constraint_for_hir(param.constraint);
            let id = super::GenericParamId::new(generic_owner.clone(), param.name.clone());
            let default = lower_generic_default(param, identity, &scope, ctx)?;
            lowered.push(NominalGenericParam::new(
                id.clone(),
                param.constraint,
                default,
                param.span,
            ));
            scope
                .insert_binding(super::GenericParamBinding::new(id, constraint, param.span))
                .map_err(|error| {
                    super::diagnostics::hir_lower_error_to_graphcal(&error, ctx.src)
                })?;
            Ok((lowered, scope))
        },
    )
}

fn lower_generic_default(
    param: &TypeGenericParam,
    identity: &ResolvedStructTypeName,
    scope: &super::GenericScope,
    ctx: &NominalLoweringContext<'_>,
) -> Result<Option<super::GenericArg>, GraphcalError> {
    param
        .default
        .as_ref()
        .map(|default| {
            if let crate::desugar::desugared_ast::GenericArg::Type(type_expr) = default {
                super::diagnostics::validate_type_annotation(type_expr, ctx.src)?;
            }
            let prelude = super::PreludeTypeScope::graphcal();
            let type_ctx = super::TypeLoweringContext::new(identity.owner(), ctx.resolver, scope)
                .with_prelude(&prelude);
            super::lower::lower_generic_arg_for_constraint(
                default,
                generic_constraint_for_hir(param.constraint),
                &param.name,
                type_ctx,
            )
            .map_err(|error| super::diagnostics::hir_lower_error_to_graphcal(&error, ctx.src))
        })
        .transpose()
}

fn lower_nominal_field(
    field: &FrontendStructField,
    identity: &ResolvedStructTypeName,
    generic_scope: &super::GenericScope,
    ctx: &NominalLoweringContext<'_>,
) -> Result<NominalField, GraphcalError> {
    super::diagnostics::validate_type_annotation(field.type_ann(), ctx.src)?;
    let prelude = super::PreludeTypeScope::graphcal();
    let type_ctx = super::TypeLoweringContext::new(identity.owner(), ctx.resolver, generic_scope)
        .with_prelude(&prelude);
    let type_expr = super::lower_type_expr(field.type_ann(), type_ctx).map_err(|error| {
        super::diagnostics::type_lower_error_to_graphcal(&error, field.type_ann(), ctx.src)
    })?;
    let expr_ctx = super::ExprLoweringContext::new(
        identity.owner(),
        ctx.resolver,
        generic_scope,
        &ctx.registry.time_zones,
    )
    .with_prelude(&prelude)
    .with_unit_registry(&ctx.registry.units);
    let domain_bounds = field
        .type_ann()
        .domain_bounds()
        .iter()
        .map(|bound| {
            Ok(super::DomainBound {
                kind: bound.kind,
                value: super::lower_expr(&bound.value, expr_ctx).map_err(|error| {
                    super::diagnostics::expr_lower_error_to_graphcal(&error, ctx.src)
                })?,
                span: bound.span,
            })
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    Ok(NominalField::new(
        field.name().clone(),
        super::TypeAnnotation {
            type_expr,
            domain_bounds,
            span: field.type_ann().span,
        },
    ))
}

const fn generic_constraint_for_hir(
    constraint: TypeGenericConstraint,
) -> crate::syntax::ast::GenericConstraint {
    match constraint {
        TypeGenericConstraint::Dim => crate::syntax::ast::GenericConstraint::Dim,
        TypeGenericConstraint::Index => crate::syntax::ast::GenericConstraint::Index,
        TypeGenericConstraint::Nat => crate::syntax::ast::GenericConstraint::Nat,
        TypeGenericConstraint::Type => crate::syntax::ast::GenericConstraint::Type,
    }
}

fn invariant_error(message: String, src: &NamedSource<Arc<String>>, span: Span) -> GraphcalError {
    GraphcalError::InternalError {
        message,
        src: src.clone(),
        span: span.into(),
    }
}
