//! Lowering of `type` declarations to canonical HIR nominal definitions.
//!
//! This is the single validation point of a nominal declaration: generic
//! parameter lists, constructor names, and payload fields are checked here,
//! while the definition's canonical identity and signatures are built. A type
//! that a selective include projects under Static bindings is lowered from
//! its template declaration in the template's own scope and then specialized
//! through the include's canonical substitution; no source spelling is
//! rewritten.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::desugared_ast::{self as ast, TypeDecl, TypeDeclBody};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::graphcal_error::GraphcalError;
use crate::ir::static_substitution::{InstanceIndexBindingTarget, StaticSubstitution};
use crate::nat::{NatOverflowError, NatPolyForm};
use crate::resolve::ModuleResolver;
use crate::resolve::namespace::Namespace;
use crate::resolve::reserved_name::validate_reserved_name;
use crate::resolved_name::ResolvedStructTypeName;
use crate::semantic::time_zone::TimeZoneRegistry;
use crate::syntax::names::NameAtom;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::GenericParamName;

use super::nominal::{
    NominalConstructor, NominalField, NominalGenericParam, NominalTypeDef, NominalTypeError,
    NominalTypeKind,
};
use super::types::{
    DeclType, DimArg, DimExpr, DimExprItem, DimTermRef, DimTermTarget, GenericArg, GenericParamId,
    GenericParamOwner, IndexRef, TypeAnnotation, ValueType, ValueTypeKind,
};

/// Services one nominal lowering run needs.
#[derive(Debug, Clone, Copy)]
pub struct NominalLowering<'a> {
    pub resolver: &'a ModuleResolver,
    pub cancellation: &'a crate::cancellation::CancellationToken,
}

/// Lower one `type` declaration to its canonical HIR definition.
///
/// Every name in the declaration resolves in the scope of `identity`'s owner.
/// `span` is the binding site recorded on the definition.
///
/// # Errors
///
/// Returns a [`GraphcalError`] for an invalid generic parameter list, a
/// duplicate constructor or payload field, or an unresolvable signature.
pub fn lower_type_declaration(
    declaration: &TypeDecl,
    identity: ResolvedStructTypeName,
    span: Span,
    src: &NamedSource<Arc<String>>,
    lowering: NominalLowering<'_>,
) -> Result<NominalTypeDef, GraphcalError> {
    validate_generic_params(declaration, src)?;
    let (generic_params, generic_scope) =
        lower_generic_params(declaration, &identity, src, lowering)?;
    match &declaration.body {
        TypeDeclBody::Required => Ok(NominalTypeDef::required(
            identity,
            generic_params,
            src.clone(),
            span,
        )),
        TypeDeclBody::Constructors(members) => {
            let lowered = members
                .iter()
                .map(|member| {
                    lowering.cancellation.checkpoint()?;
                    let payload = member.payload.as_deref().unwrap_or(&[]);
                    let fields = payload
                        .iter()
                        .map(|field| {
                            lower_nominal_field(field, &identity, &generic_scope, src, lowering)
                        })
                        .collect::<Result<Vec<_>, GraphcalError>>()?;
                    NominalConstructor::try_new(
                        identity.constructor(member.name.value.clone()),
                        fields,
                    )
                    .map_err(|error| member_error(error, declaration, payload, src))
                })
                .collect::<Result<Vec<_>, GraphcalError>>()?;
            NominalTypeDef::try_union(identity, generic_params, lowered, src.clone(), span)
                .map_err(|error| member_error(error, declaration, &[], src))
        }
    }
}

/// Render a rejected constructor list at its source declaration.
fn member_error(
    error: NominalTypeError,
    declaration: &TypeDecl,
    payload: &[ast::FieldDecl],
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    match error {
        NominalTypeError::DuplicateConstructorField {
            constructor,
            field,
            first_index,
            duplicate_index,
        } => match (payload.get(first_index), payload.get(duplicate_index)) {
            (Some(first), Some(duplicate)) => GraphcalError::DuplicateConstructorField {
                type_name: declaration.name.value.clone(),
                constructor,
                field,
                src: src.clone(),
                duplicate: duplicate.name.span.into(),
                first: first.name.span.into(),
            },
            _ => invariant_error(
                format!("duplicate field `{field}` has no source declaration"),
                src,
                declaration.name.span,
            ),
        },
        NominalTypeError::DuplicateConstructor { constructor } => {
            let mut members = match &declaration.body {
                TypeDeclBody::Constructors(members) => members
                    .iter()
                    .filter(|member| member.name.value == constructor)
                    .map(|member| member.name.span)
                    .collect::<Vec<_>>(),
                TypeDeclBody::Required => Vec::new(),
            }
            .into_iter();
            let first = members.next().unwrap_or(declaration.name.span);
            let duplicate = members.next().unwrap_or(first);
            GraphcalError::DuplicateName {
                name: constructor.to_string(),
                src: src.clone(),
                duplicate: duplicate.into(),
                first: first.into(),
            }
        }
        error @ (NominalTypeError::ConstructorOwnerMismatch { .. }
        | NominalTypeError::DuplicateType { .. }
        | NominalTypeError::DuplicateCanonicalConstructor { .. }
        | NominalTypeError::NatOverflow(_)) => invariant_error(
            format!(
                "invalid HIR nominal type `{}`: {error}",
                declaration.name.value
            ),
            src,
            declaration.name.span,
        ),
    }
}

/// Reject duplicate generic parameters and defaults that are out of order.
///
/// A default may reference only earlier parameters, and a parameter without
/// a default cannot follow a defaulted one.
fn validate_generic_params(
    declaration: &TypeDecl,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let positions = declaration.generic_params.iter().enumerate().try_fold(
        HashMap::new(),
        |mut positions, (index, param)| match positions.insert(
            param.name.value.atom().clone(),
            (param.name.value.clone(), index, param.name.span),
        ) {
            Some((name, _, _)) => Err(GraphcalError::EvalError {
                message: format!("duplicate generic parameter `{name}`"),
                src: src.clone(),
                span: param.name.span.into(),
            }),
            None => Ok(positions),
        },
    )?;

    let mut first_defaulted: Option<&GenericParamName> = None;
    for (index, param) in declaration.generic_params.iter().enumerate() {
        match &param.default {
            Some(default) => {
                first_defaulted.get_or_insert(&param.name.value);
                if let Some((referenced, span)) =
                    find_non_earlier_generic_reference(default, index, &positions)
                {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "default for generic parameter `{}` may reference only earlier generic parameters; `{referenced}` is not earlier",
                            param.name.value
                        ),
                        src: src.clone(),
                        span: span.into(),
                    });
                }
            }
            None => {
                if let Some(first_defaulted) = first_defaulted {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "generic parameter `{}` without a default cannot follow defaulted parameter `{first_defaulted}`",
                            param.name.value
                        ),
                        src: src.clone(),
                        span: param.name.span.into(),
                    });
                }
            }
        }
    }
    Ok(())
}

type GenericParamPositions = HashMap<NameAtom, (GenericParamName, usize, Span)>;

fn find_non_earlier_generic_reference(
    arg: &ast::GenericArg,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    match arg {
        ast::GenericArg::Type(type_expr) => {
            find_non_earlier_type_reference(type_expr, current_index, positions)
        }
        ast::GenericArg::Index(index) => {
            find_non_earlier_index_reference(index, current_index, positions)
        }
        ast::GenericArg::Nat(nat_expr) => {
            find_non_earlier_nat_reference(nat_expr, current_index, positions)
        }
        ast::GenericArg::Ambiguous(ambiguous) => {
            find_non_earlier_ambiguous_reference(ambiguous, current_index, positions)
        }
    }
}

fn non_earlier_generic_reference(
    name: &NameAtom,
    span: Span,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    positions
        .get(name)
        .filter(|(_, index, _)| *index >= current_index)
        .map(|(name, _, _)| (name.clone(), span))
}

fn find_non_earlier_path_reference(
    path: &Spanned<crate::syntax::names::NamePath>,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    path.value.as_bare()?;
    non_earlier_generic_reference(path.value.leaf(), path.span, current_index, positions)
}

fn find_non_earlier_ambiguous_reference(
    arg: &ast::AmbiguousGenericArg,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    match arg {
        ast::AmbiguousGenericArg::Name(ident) => {
            non_earlier_generic_reference(ident.name.atom(), ident.span, current_index, positions)
        }
        ast::AmbiguousGenericArg::Mul(operands, _) => operands.iter().find_map(|operand| {
            find_non_earlier_ambiguous_reference(operand, current_index, positions)
        }),
    }
}

fn find_non_earlier_nat_reference(
    expr: &ast::NatExpr,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    match expr {
        ast::NatExpr::Literal(..) => None,
        ast::NatExpr::Var(ident) => {
            non_earlier_generic_reference(ident.name.atom(), ident.span, current_index, positions)
        }
        ast::NatExpr::Add(operands, _) | ast::NatExpr::Mul(operands, _) => operands
            .iter()
            .find_map(|operand| find_non_earlier_nat_reference(operand, current_index, positions)),
    }
}

fn find_non_earlier_index_reference(
    index: &ast::IndexExpr,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    match index {
        ast::IndexExpr::Name(path) => {
            find_non_earlier_path_reference(path, current_index, positions)
        }
        ast::IndexExpr::Finite { cardinality, .. } | ast::IndexExpr::BareNat(cardinality) => {
            find_non_earlier_nat_reference(cardinality, current_index, positions)
        }
    }
}

fn find_non_earlier_type_reference(
    type_expr: &ast::TypeExpr,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    use ast::TypeExprKind;

    match &type_expr.kind {
        TypeExprKind::IndexLabel { .. }
        | TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => None,
        TypeExprKind::DimExpr(dim_expr) => dim_expr.terms.iter().find_map(|item| {
            find_non_earlier_path_reference(&item.term.name, current_index, positions)
        }),
        TypeExprKind::Indexed { base, indexes } => {
            find_non_earlier_type_reference(base, current_index, positions).or_else(|| {
                indexes.iter().find_map(|index| {
                    find_non_earlier_index_reference(index, current_index, positions)
                })
            })
        }
        TypeExprKind::TypeApplication { generic_args, .. } => generic_args
            .iter()
            .find_map(|arg| find_non_earlier_generic_reference(arg, current_index, positions)),
        TypeExprKind::ComplexApplication { generic_args }
        | TypeExprKind::KeyApplication { generic_args } => generic_args
            .iter()
            .find_map(|arg| find_non_earlier_generic_reference(arg, current_index, positions)),
        TypeExprKind::DatetimeApplication { type_args } => type_args.iter().find_map(|type_arg| {
            find_non_earlier_type_reference(type_arg, current_index, positions)
        }),
    }
}

fn lower_generic_params(
    declaration: &TypeDecl,
    identity: &ResolvedStructTypeName,
    src: &NamedSource<Arc<String>>,
    lowering: NominalLowering<'_>,
) -> Result<(Vec<NominalGenericParam>, super::lower::GenericScope), GraphcalError> {
    let generic_owner = GenericParamOwner::Type(identity.clone());
    declaration.generic_params.iter().try_fold(
        (
            Vec::with_capacity(declaration.generic_params.len()),
            super::lower::GenericScope::new(),
        ),
        |(mut lowered, mut scope), param| {
            lowering.cancellation.checkpoint()?;
            let id = GenericParamId::new(generic_owner.clone(), param.name.value.clone());
            let default = lower_generic_default(param, identity, &scope, src, lowering)?;
            lowered.push(NominalGenericParam::new(
                id.clone(),
                param.constraint,
                default,
                param.name.span,
            ));
            let atom = param.name.value.atom();
            let visible = lowering
                .resolver
                .visible_span(identity.owner(), Namespace::Static, atom)
                .map_err(|error| {
                    GraphcalError::internal_error(
                        format!("failed to inspect Static scope for `{atom}`: {error}"),
                        src,
                        DiagnosticAnchor::Source(param.name.span),
                    )
                })?;
            if validate_reserved_name(Namespace::Static, atom).is_err() || visible.is_some() {
                return Err(super::diagnostics::hir_lower_error_to_graphcal(
                    &super::lower::HirLowerError::GenericParamShadowsStatic {
                        name: param.name.value.clone(),
                        original: visible,
                        duplicate: param.name.span,
                    },
                    src,
                ));
            }
            scope
                .insert_binding(super::lower::GenericParamBinding::new(
                    id,
                    param.constraint,
                    param.name.span,
                ))
                .map_err(|error| super::diagnostics::hir_lower_error_to_graphcal(&error, src))?;
            Ok((lowered, scope))
        },
    )
}

fn lower_generic_default(
    param: &ast::GenericParam,
    identity: &ResolvedStructTypeName,
    scope: &super::lower::GenericScope,
    src: &NamedSource<Arc<String>>,
    lowering: NominalLowering<'_>,
) -> Result<Option<GenericArg>, GraphcalError> {
    param
        .default
        .as_ref()
        .map(|default| {
            if let ast::GenericArg::Type(type_expr) = default {
                super::diagnostics::validate_type_annotation(type_expr, src)?;
            }
            super::lower::lower_generic_arg_for_constraint(
                default,
                param.constraint,
                &param.name.value,
                super::lower::ModuleScope::new(identity.owner(), lowering.resolver, scope),
            )
            .map_err(|error| super::diagnostics::hir_lower_error_to_graphcal(&error, src))
        })
        .transpose()
}

fn lower_nominal_field(
    field: &ast::FieldDecl,
    identity: &ResolvedStructTypeName,
    generic_scope: &super::lower::GenericScope,
    src: &NamedSource<Arc<String>>,
    lowering: NominalLowering<'_>,
) -> Result<NominalField, GraphcalError> {
    super::diagnostics::validate_type_annotation(&field.type_ann, src)?;
    let scope = super::lower::ModuleScope::new(identity.owner(), lowering.resolver, generic_scope);
    let decl_type = super::lower::lower_decl_type(&field.type_ann, scope)
        .map_err(|error| super::diagnostics::type_lower_error_to_graphcal(&error, src))?;
    let time_zones = TimeZoneRegistry::bundled();
    let expr_ctx = super::expr_lower::context::ExprLoweringContext::new(scope, &time_zones);
    let domain_bounds = field
        .type_ann
        .domain_bounds()
        .iter()
        .map(|bound| {
            Ok(super::types::DomainBound {
                kind: bound.kind,
                value: super::expr_lower::lower::lower_expr(&bound.value, expr_ctx).map_err(
                    |error| super::diagnostics::expr_lower_error_to_graphcal(&error, src),
                )?,
                span: bound.span,
            })
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    Ok(NominalField::new(
        field.name.value.clone(),
        TypeAnnotation {
            decl_type,
            domain_bounds,
            span: field.type_ann.span,
        },
    ))
}

fn invariant_error(message: String, src: &NamedSource<Arc<String>>, span: Span) -> GraphcalError {
    GraphcalError::InternalError {
        message,
        src: src.clone(),
        span: span.into(),
    }
}

/// Specialize a template's nominal definition as the importer-owned
/// definition `identity` through `substitution`: the include's Static
/// bindings plus the template declarations the same include projects as
/// importer-owned declarations.
///
/// Constructors and generic parameters are re-owned by `identity`; every
/// signature reference the substitution names is replaced. Domain bounds
/// keep the template's lowering.
///
/// # Errors
///
/// Returns a [`NominalTypeError`] only if the template definition was invalid.
pub fn specialize_nominal_type(
    template: &NominalTypeDef,
    identity: ResolvedStructTypeName,
    substitution: &StaticSubstitution,
    source: NamedSource<Arc<String>>,
    span: Span,
) -> Result<NominalTypeDef, NominalTypeError> {
    let specializer = Specializer {
        template: template.identity(),
        identity: &identity,
        substitution,
    };
    let generic_params = template
        .generic_params()
        .iter()
        .map(|param| {
            Ok(NominalGenericParam::new(
                specializer.generic_param(param.id()),
                param.constraint(),
                param
                    .default()
                    .map(|arg| specializer.generic_arg(arg))
                    .transpose()?,
                param.span(),
            ))
        })
        .collect::<Result<_, NatOverflowError>>()?;
    match template.kind() {
        NominalTypeKind::Required => {
            Ok(
                NominalTypeDef::required(identity, generic_params, source, span)
                    .with_instance_substitution(substitution.clone()),
            )
        }
        NominalTypeKind::Union { members } => {
            let members = members
                .iter()
                .map(|member| {
                    NominalConstructor::try_new(
                        identity.constructor(member.name()),
                        member
                            .fields()
                            .iter()
                            .map(|field| {
                                let annotation = field.type_annotation();
                                Ok(NominalField::new(
                                    field.name().clone(),
                                    TypeAnnotation {
                                        decl_type: specializer.decl_type(&annotation.decl_type)?,
                                        domain_bounds: annotation.domain_bounds.clone(),
                                        span: annotation.span,
                                    },
                                ))
                            })
                            .collect::<Result<_, NatOverflowError>>()?,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            NominalTypeDef::try_union(identity, generic_params, members, source, span)
                .map(|definition| definition.with_instance_substitution(substitution.clone()))
        }
    }
}

/// One specialization of a template's nominal signatures.
struct Specializer<'a> {
    template: &'a ResolvedStructTypeName,
    identity: &'a ResolvedStructTypeName,
    substitution: &'a StaticSubstitution,
}

impl Specializer<'_> {
    fn struct_type(
        &self,
        name: &Spanned<ResolvedStructTypeName>,
    ) -> Spanned<ResolvedStructTypeName> {
        let target = if &name.value == self.template {
            self.identity
        } else {
            self.substitution
                .types
                .get(&name.value)
                .unwrap_or(&name.value)
        };
        Spanned::new(target.clone(), name.span)
    }

    fn generic_param(&self, id: &GenericParamId) -> GenericParamId {
        match id.owner() {
            GenericParamOwner::Type(owner) if owner == self.template => GenericParamId::new(
                GenericParamOwner::Type(self.identity.clone()),
                id.name.clone(),
            ),
            GenericParamOwner::Type(_) | GenericParamOwner::ExternFn(_) => id.clone(),
        }
    }

    fn spanned_param(&self, id: &Spanned<GenericParamId>) -> Spanned<GenericParamId> {
        Spanned::new(self.generic_param(&id.value), id.span)
    }

    fn decl_type(&self, decl_type: &DeclType) -> Result<DeclType, NatOverflowError> {
        Ok(match decl_type {
            DeclType::Value(value) => DeclType::Value(self.value_type(value)?),
            DeclType::Indexed {
                element,
                indexes,
                span,
            } => DeclType::Indexed {
                element: self.value_type(element)?,
                indexes: indexes.try_map_ref(|index| self.index(index))?,
                span: *span,
            },
        })
    }

    fn value_type(&self, value: &ValueType) -> Result<ValueType, NatOverflowError> {
        let kind = match &value.kind {
            ValueTypeKind::Builtin(builtin) => ValueTypeKind::Builtin(*builtin),
            ValueTypeKind::DimExpr(expr) => ValueTypeKind::DimExpr(self.dim_expr(expr)),
            ValueTypeKind::Struct(name) => ValueTypeKind::Struct(self.struct_type(name)),
            ValueTypeKind::GenericTypeParam(id) => {
                ValueTypeKind::GenericTypeParam(self.spanned_param(id))
            }
            ValueTypeKind::Complex(arg) => ValueTypeKind::Complex(self.dim_arg(arg)),
            ValueTypeKind::Key(index) => ValueTypeKind::Key(self.index(index)?),
            ValueTypeKind::TypeApplication { name, generic_args } => {
                ValueTypeKind::TypeApplication {
                    name: self.struct_type(name),
                    generic_args: generic_args
                        .iter()
                        .map(|arg| self.generic_arg(arg))
                        .collect::<Result<_, _>>()?,
                }
            }
        };
        Ok(ValueType::new(kind, value.span))
    }

    fn generic_arg(&self, arg: &GenericArg) -> Result<GenericArg, NatOverflowError> {
        Ok(match arg {
            GenericArg::Dim(arg) => GenericArg::Dim(self.dim_arg(arg)),
            GenericArg::Index(index) => GenericArg::Index(self.index(index)?),
            GenericArg::Nat(nat) => GenericArg::Nat(self.nat(nat)?),
            GenericArg::Type(value) => GenericArg::Type(self.value_type(value)?),
        })
    }

    fn dim_arg(&self, arg: &DimArg) -> DimArg {
        match arg {
            DimArg::Dimensionless(span) => DimArg::Dimensionless(*span),
            DimArg::Expr(expr) => DimArg::Expr(self.dim_expr(expr)),
        }
    }

    fn dim_expr(&self, expr: &DimExpr) -> DimExpr {
        DimExpr {
            terms: expr
                .terms
                .iter()
                .map(|item| DimExprItem {
                    op: item.op,
                    term: DimTermRef {
                        target: match &item.term.target {
                            DimTermTarget::Dimension(name) => {
                                DimTermTarget::Dimension(Spanned::new(
                                    self.substitution
                                        .dimensions
                                        .get(&name.value)
                                        .unwrap_or(&name.value)
                                        .clone(),
                                    name.span,
                                ))
                            }
                            DimTermTarget::GenericParam(id) => {
                                DimTermTarget::GenericParam(self.spanned_param(id))
                            }
                        },
                        power: item.term.power,
                        span: item.term.span,
                    },
                })
                .collect(),
            span: expr.span,
        }
    }

    fn index(&self, index: &IndexRef) -> Result<IndexRef, NatOverflowError> {
        Ok(match index {
            IndexRef::Concrete(name) => match self.substitution.indexes.get(&name.value) {
                Some(InstanceIndexBindingTarget::Declared(target)) => {
                    IndexRef::Concrete(Spanned::new(target.clone(), name.span))
                }
                Some(InstanceIndexBindingTarget::Finite(finite)) => IndexRef::Finite(Spanned::new(
                    NatPolyForm::from_constant(finite.size_u64()),
                    name.span,
                )),
                None => IndexRef::Concrete(name.clone()),
            },
            IndexRef::GenericParam(id) => IndexRef::GenericParam(self.spanned_param(id)),
            IndexRef::Finite(nat) => IndexRef::Finite(self.nat(nat)?),
        })
    }

    /// Re-own the template's Nat parameters in a normalized form.
    fn nat(&self, nat: &Spanned<NatPolyForm>) -> Result<Spanned<NatPolyForm>, NatOverflowError> {
        let reowned = nat
            .value
            .variables()
            .into_iter()
            .map(|id| {
                let target = NatPolyForm::from_var(self.generic_param(&id));
                (id, target)
            })
            .collect();
        Ok(Spanned::new(
            nat.value.substitute_forms(&reowned)?,
            nat.span,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_id::DagId;
    use crate::resolve::builder::TestModules;
    use crate::resolved_name::{ResolvedDimName, ResolvedIndexName};
    use crate::semantic::index_def::FiniteIndex;
    use crate::syntax::parser::Parser;
    use crate::syntax::type_name::StructTypeName;
    use std::collections::BTreeMap;

    fn parse(source: &str) -> ast::File {
        ast::File::from(Parser::new(source).parse_file().unwrap())
    }

    fn first_type(file: &ast::File) -> &TypeDecl {
        file.declarations
            .iter()
            .find_map(|declaration| match &declaration.kind {
                ast::DeclKind::Type(type_decl) => Some(type_decl),
                _ => None,
            })
            .unwrap()
    }

    fn type_named<'f>(file: &'f ast::File, name: &str) -> &'f TypeDecl {
        file.declarations
            .iter()
            .find_map(|declaration| match &declaration.kind {
                ast::DeclKind::Type(type_decl) if type_decl.name.value.as_str() == name => {
                    Some(type_decl)
                }
                _ => None,
            })
            .unwrap()
    }

    /// Lower the first type declaration of a one-module project.
    fn lower_first(source: &str) -> Result<NominalTypeDef, GraphcalError> {
        let owner = DagId::root_in_package("test", "main");
        let file = parse(source);
        let mut modules = TestModules::default();
        modules.add(owner.clone(), &file.declarations);
        let resolver = modules.build().unwrap();
        let src = NamedSource::new("main.gcl", Arc::new(source.to_string()));
        let declaration = first_type(&file);
        lower_type_declaration(
            declaration,
            ResolvedStructTypeName::for_test(owner, declaration.name.value.clone()),
            declaration.name.span,
            &src,
            NominalLowering {
                resolver: &resolver,
                cancellation: &crate::cancellation::CancellationToken::unbounded(),
            },
        )
    }

    fn eval_message(result: Result<NominalTypeDef, GraphcalError>) -> String {
        match result {
            Err(GraphcalError::EvalError { message, .. }) => message,
            other => panic!("expected an evaluation diagnostic, got {other:?}"),
        }
    }

    #[test]
    fn generic_parameter_lists_are_validated_once() {
        assert_eq!(
            eval_message(lower_first("type Pair<T: Type, T: Type> { Pair(a: T) }")),
            "duplicate generic parameter `T`"
        );
        assert!(
            eval_message(lower_first("type Box<N: Nat = M, M: Nat = 1> { Box }"))
                .contains("may reference only earlier generic parameters; `M` is not earlier")
        );
        assert!(
            eval_message(lower_first("type Box<N: Nat = 1, M: Nat> { Box }"))
                .contains("without a default cannot follow defaulted parameter `N`")
        );
    }

    #[test]
    fn duplicate_payload_fields_are_reported_at_their_declarations() {
        let error =
            lower_first("type Pair { Pair(value: Length, other: Bool, value: Time) }").unwrap_err();
        assert!(
            matches!(
                &error,
                GraphcalError::DuplicateConstructorField { type_name, field, .. }
                    if type_name.as_str() == "Pair" && field.as_str() == "value"
            ),
            "{error:?}"
        );
    }

    #[test]
    fn declarations_lower_to_owner_qualified_constructors() {
        let definition = lower_first("type Choice { Left(value: Length), Right }").unwrap();
        let members = definition.union_members().unwrap();
        assert_eq!(members.len(), 2);
        assert!(
            members
                .iter()
                .all(|member| member.identity().owner() == definition.identity().owner())
        );
        assert!(definition.record_fields().is_none());
    }

    /// The template `Box` of the specialization test, owned by `template_id`.
    fn template_box(template_id: &DagId) -> (NominalTypeDef, NamedSource<Arc<String>>) {
        let source = "pub(bind) dim Q;\n\
                      pub(bind) index Axis;\n\
                      pub(bind) type Slot;\n\
                      pub type Box<T: Type = Slot, N: Nat = 2> \
                      { Box(v: Q, s: Slot, xs: Q[Axis], t: T, ys: Q[Fin(N + 1)]) }";
        let file = parse(source);
        let mut modules = TestModules::default();
        modules.add(template_id.clone(), &file.declarations);
        let resolver = modules.build().unwrap();
        let src = NamedSource::new("lib.gcl", Arc::new(source.to_string()));
        let declaration = type_named(&file, "Box");
        let template = lower_type_declaration(
            declaration,
            ResolvedStructTypeName::for_test(template_id.clone(), declaration.name.value.clone()),
            declaration.name.span,
            &src,
            NominalLowering {
                resolver: &resolver,
                cancellation: &crate::cancellation::CancellationToken::unbounded(),
            },
        )
        .unwrap();
        (template, src)
    }

    #[test]
    fn specialization_reowns_and_substitutes_signatures() {
        let template_id = DagId::root_in_package("test", "lib");
        let importer_id = DagId::root_in_package("test", "main");
        let (template, src) = template_box(&template_id);

        let identity = ResolvedStructTypeName::for_test(
            importer_id.clone(),
            StructTypeName::expect_valid("Box"),
        );
        let slot = ResolvedStructTypeName::for_test(
            template_id.clone(),
            StructTypeName::expect_valid("Slot"),
        );
        let concrete = ResolvedStructTypeName::for_test(
            importer_id.clone(),
            StructTypeName::expect_valid("Concrete"),
        );
        let port = ResolvedDimName::for_test(
            template_id.clone(),
            crate::syntax::dimension::DimName::expect_valid("Q"),
        );
        let length = crate::resolve::prelude::prelude_type_scope()
            .resolve_dimension_path(&crate::syntax::names::NamePath::expect_local("Length"))
            .unwrap();
        let axis = ResolvedIndexName::for_test(
            template_id,
            crate::syntax::index_name::IndexName::expect_valid("Axis"),
        );
        let substitution = StaticSubstitution {
            types: BTreeMap::from([(slot, concrete.clone())]),
            dimensions: BTreeMap::from([(port, length.clone())]),
            indexes: BTreeMap::from([(
                axis,
                InstanceIndexBindingTarget::Finite(FiniteIndex::try_from_u64(3).unwrap()),
            )]),
        };
        let specialized = specialize_nominal_type(
            &template,
            identity.clone(),
            &substitution,
            src,
            Span::new(0, 1),
        )
        .unwrap();

        assert_eq!(specialized.identity(), &identity);
        assert_eq!(specialized.instance_substitution(), Some(&substitution));
        assert_eq!(template.instance_substitution(), None);
        let [param, nat_param] = specialized.generic_params() else {
            panic!("Box keeps two generic parameters");
        };
        assert_eq!(
            nat_param.id().owner(),
            &GenericParamOwner::Type(identity.clone())
        );
        assert_eq!(
            param.id().owner(),
            &GenericParamOwner::Type(identity.clone())
        );
        assert!(matches!(
            param.default(),
            Some(GenericArg::Type(ValueType { kind: ValueTypeKind::Struct(name), .. }))
                if name.value == concrete
        ));
        let [constructor] = specialized.union_members().unwrap() else {
            panic!("Box keeps one constructor");
        };
        assert_eq!(constructor.identity().owner(), &importer_id);
        let [v, s, xs, t, ys] = constructor.fields() else {
            panic!("Box keeps five fields");
        };
        // The Nat form is re-owned by the specialized identity.
        let DeclType::Indexed { indexes, .. } = &ys.type_annotation().decl_type else {
            panic!("ys stays indexed");
        };
        let [IndexRef::Finite(cardinality)] = indexes.as_slice() else {
            panic!("ys keeps one finite axis");
        };
        assert_eq!(
            cardinality.value,
            NatPolyForm::from_var(nat_param.id().clone())
                .add(&NatPolyForm::from_constant(1))
                .unwrap()
        );
        assert!(matches!(
            &v.type_annotation().decl_type,
            DeclType::Value(ValueType { kind: ValueTypeKind::DimExpr(expr), .. })
                if matches!(&expr.terms[0].term.target, DimTermTarget::Dimension(name) if name.value == length)
        ));
        assert!(matches!(
            &s.type_annotation().decl_type,
            DeclType::Value(ValueType { kind: ValueTypeKind::Struct(name), .. }) if name.value == concrete
        ));
        assert!(matches!(
            &xs.type_annotation().decl_type,
            DeclType::Indexed { indexes, .. }
                if matches!(indexes.first(), IndexRef::Finite(n) if n.value.constant_value() == Some(3))
        ));
        assert!(matches!(
            &t.type_annotation().decl_type,
            DeclType::Value(ValueType { kind: ValueTypeKind::GenericTypeParam(id), .. })
                if id.value.owner() == &GenericParamOwner::Type(identity)
        ));
    }
}
