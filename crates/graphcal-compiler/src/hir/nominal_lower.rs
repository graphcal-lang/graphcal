//! Lowering of `type` declarations to canonical HIR nominal definitions.
//!
//! This is the single validation point of a nominal declaration: generic
//! parameter lists, constructor names, and payload fields are checked here,
//! while the definition's canonical identity and signatures are built. A type
//! that a selective include projects under Static bindings is lowered from
//! its template declaration in the template's own scope and then specialized
//! through the include's canonical substitution; no source spelling is
//! rewritten.

use crate::semantic_error::name::DuplicateDeclaration;
use crate::semantic_error::structure::StructError;
use std::collections::HashMap;

use crate::desugar::desugared_ast::{self as ast, TypeDecl, TypeDeclBody};
use crate::ir::static_substitution::{InstanceIndexBindingTarget, StaticSubstitution};
use crate::nat::{NatOverflowError, NatPolyForm};
use crate::resolve::ModuleResolver;
use crate::resolve::namespace::Namespace;
use crate::resolve::reserved_name::validate_reserved_name;
use crate::resolved_name::ResolvedStructTypeName;
use crate::semantic::time_zone::TimeZoneRegistry;
use crate::semantic_error::SemanticError;
use crate::semantic_error::name::NameError;
use crate::source_id::SourceId;
use crate::syntax::names::NameAtom;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::GenericParamName;

use super::nominal::{
    NominalConstructor, NominalField, NominalGenericParam, NominalTypeDef, NominalTypeError,
};
use super::type_annotation::TypeAnnotation;
use super::types::{
    DeclType, DimArg, DimExpr, DimExprItem, DimTermRef, DimTermTarget, GenericArg, GenericParamId,
    GenericParamOwner, IndexRef, ValueType, ValueTypeKind,
};
use crate::outcome::Outcome;

/// Services one nominal lowering run needs.
#[derive(Debug, Clone, Copy)]
pub struct NominalLowering<'a> {
    pub resolver: &'a ModuleResolver,
    /// The module that declares the type, whose scope its generic
    /// parameters must not shadow.
    pub module: crate::resolve::ModuleRef<'a>,
    pub cancellation: &'a crate::cancellation::CancellationToken,
}

/// Lower one `type` declaration to its canonical HIR definition.
///
/// Every name in the declaration resolves in the scope of `identity`'s owner.
/// `span` is the binding site recorded on the definition.
///
/// # Errors
///
/// Returns a [`SemanticError`] for an invalid generic parameter list, a
/// duplicate constructor or payload field, or an unresolvable signature.
pub fn lower_type_declaration(
    declaration: &TypeDecl,
    identity: ResolvedStructTypeName,
    span: Span,
    src: SourceId,
    lowering: NominalLowering<'_>,
) -> Result<NominalTypeDef, Outcome<SemanticError>> {
    validate_generic_params(declaration, src)?;
    let (generic_params, generic_scope) =
        lower_generic_params(declaration, &identity, src, lowering)?;
    match &declaration.body {
        TypeDeclBody::Required => Ok(NominalTypeDef::required(
            identity,
            generic_params,
            src,
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
                                .map(|lowered| Spanned::new(lowered, field.name.span))
                        })
                        .collect::<Result<Vec<_>, SemanticError>>()?;
                    NominalConstructor::try_new(
                        identity.constructor(member.name.value.clone()),
                        fields,
                    )
                    .map_err(|error| member_error(error, declaration, src).into())
                })
                .collect::<Result<Vec<_>, Outcome<SemanticError>>>()?;
            NominalTypeDef::try_union(identity, generic_params, lowered, src, span)
                .map_err(|error| member_error(error, declaration, src).into())
        }
    }
}

/// Render a rejected constructor list at its source declaration.
fn member_error(error: NominalTypeError, declaration: &TypeDecl, src: SourceId) -> SemanticError {
    match error {
        NominalTypeError::DuplicateConstructorField {
            constructor,
            field,
            first,
            duplicate,
        } => SemanticError::located(
            src,
            duplicate,
            NameError::DuplicateConstructorField {
                type_name: declaration.name.value.clone(),
                constructor,
                field,
                first,
            },
        ),
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
            SemanticError::located(
                src,
                duplicate,
                NameError::DuplicateName {
                    name: DuplicateDeclaration::Name(constructor.atom().clone()),
                    first,
                },
            )
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
fn validate_generic_params(declaration: &TypeDecl, src: SourceId) -> Result<(), SemanticError> {
    let positions = declaration.generic_params.iter().enumerate().try_fold(
        HashMap::new(),
        |mut positions, (index, param)| match positions.insert(
            param.name.value.atom().clone(),
            (param.name.value.clone(), index, param.name.span),
        ) {
            Some((name, _, _)) => Err(SemanticError::located(
                src,
                param.name.span,
                NameError::DuplicateGenericParam { name },
            )),
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
                    return Err(SemanticError::located(
                        src,
                        span,
                        StructError::GenericDefaultForwardReference {
                            param: param.name.value.clone(),
                            referenced,
                        },
                    ));
                }
            }
            None => {
                if let Some(first_defaulted) = first_defaulted {
                    return Err(SemanticError::located(
                        src,
                        param.name.span,
                        StructError::RequiredGenericAfterDefault {
                            param: param.name.value.clone(),
                            first_defaulted: first_defaulted.clone(),
                        },
                    ));
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

    let element = match &type_expr.element.kind {
        TypeExprKind::IndexLabel { .. }
        | TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => None,
        TypeExprKind::DimExpr(dim_expr) => dim_expr.terms.iter().find_map(|item| {
            find_non_earlier_path_reference(&item.term.name, current_index, positions)
        }),
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
    };
    element.or_else(|| {
        type_expr
            .indexes
            .iter()
            .flatten()
            .find_map(|index| find_non_earlier_index_reference(index, current_index, positions))
    })
}

fn lower_generic_params(
    declaration: &TypeDecl,
    identity: &ResolvedStructTypeName,
    src: SourceId,
    lowering: NominalLowering<'_>,
) -> Result<(Vec<NominalGenericParam>, super::lower::GenericScope), Outcome<SemanticError>> {
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
            let visible = lowering.module.visible_span(Namespace::Static, atom);
            if validate_reserved_name(Namespace::Static, atom).is_err() || visible.is_some() {
                return Err(super::diagnostics::hir_lower_error_to_graphcal(
                    &super::lower::HirLowerError::GenericParamShadowsStatic {
                        name: param.name.value.clone(),
                        original: visible,
                        duplicate: param.name.span,
                    },
                    src,
                )
                .into());
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
    src: SourceId,
    lowering: NominalLowering<'_>,
) -> Result<Option<GenericArg>, SemanticError> {
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
    src: SourceId,
    lowering: NominalLowering<'_>,
) -> Result<NominalField, SemanticError> {
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
            Ok(super::type_annotation::DomainBound {
                kind: bound.kind,
                value: super::expr_lower::lower::lower_expr(&bound.value, expr_ctx).map_err(
                    |error| super::diagnostics::expr_lower_error_to_semantic(&error, src),
                )?,
                span: bound.span,
            })
        })
        .collect::<Result<Vec<_>, SemanticError>>()?;
    Ok(NominalField::new(
        field.name.value.clone(),
        TypeAnnotation {
            decl_type,
            domain_bounds,
            span: field.type_ann.span,
        },
    ))
}

fn invariant_error(message: String, src: SourceId, span: Span) -> SemanticError {
    SemanticError::internal_error(
        message,
        src,
        crate::diagnostic_anchor::DiagnosticAnchor::Source(span),
    )
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
/// Returns a [`NatOverflowError`] if re-owning a template Nat expression
/// overflows.
pub fn specialize_nominal_type(
    template: &NominalTypeDef,
    identity: &ResolvedStructTypeName,
    substitution: &StaticSubstitution,
    source: SourceId,
    span: Span,
) -> Result<NominalTypeDef, NatOverflowError> {
    let specializer = Specializer {
        template: template.identity(),
        identity,
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
    template.try_project(
        identity.clone(),
        generic_params,
        substitution.clone(),
        (source, span),
        |annotation| {
            Ok(TypeAnnotation {
                decl_type: specializer.decl_type(&annotation.decl_type)?,
                domain_bounds: annotation.domain_bounds.clone(),
                span: annotation.span,
            })
        },
    )
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
    use crate::semantic_error::SemanticErrorKind;
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
    fn lower_first(source: &str) -> Result<NominalTypeDef, SemanticError> {
        let owner = DagId::root_in_package("test", "main");
        let file = parse(source);
        let mut modules = TestModules::default();
        modules.add(owner.clone(), &file.declarations);
        let resolver = modules.build().unwrap();
        let src = crate::source_registry::SourceRegistry::new()
            .register("main.gcl", std::sync::Arc::new(source.to_string()));
        let declaration = first_type(&file);
        let module = resolver.module(resolver.module_handle(&owner).unwrap());
        crate::outcome::without_cancellation(|cancellation| {
            lower_type_declaration(
                declaration,
                ResolvedStructTypeName::for_test(owner, declaration.name.value.clone()),
                declaration.name.span,
                src,
                NominalLowering {
                    resolver: &resolver,
                    module,
                    cancellation,
                },
            )
        })
    }

    fn eval_message(result: Result<NominalTypeDef, SemanticError>) -> String {
        match result {
            Err(SemanticError::Located(diagnostic)) => diagnostic.kind.to_string(),
            other => panic!("expected a located diagnostic, got {other:?}"),
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
                SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(NameError::DuplicateConstructorField { type_name, field, .. }), .. })
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
    fn template_box(template_id: &DagId) -> (NominalTypeDef, SourceId) {
        let source = "pub(bind) dim Q;\n\
                      pub(bind) index Axis;\n\
                      pub(bind) type Slot;\n\
                      pub type Box<T: Type = Slot, N: Nat = 2> \
                      { Box(v: Q, s: Slot, xs: Q[Axis], t: T, ys: Q[Fin(N + 1)]) }";
        let file = parse(source);
        let mut modules = TestModules::default();
        modules.add(template_id.clone(), &file.declarations);
        let resolver = modules.build().unwrap();
        let src = crate::source_registry::SourceRegistry::new()
            .register("lib.gcl", std::sync::Arc::new(source.to_string()));
        let declaration = type_named(&file, "Box");
        let module = resolver.module(resolver.module_handle(template_id).unwrap());
        let template = lower_type_declaration(
            declaration,
            ResolvedStructTypeName::for_test(template_id.clone(), declaration.name.value.clone()),
            declaration.name.span,
            src,
            NominalLowering {
                resolver: &resolver,
                module,
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
        let specialized =
            specialize_nominal_type(&template, &identity, &substitution, src, Span::new(0, 1))
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
