//! Frontend type-table construction from desugared declarations.
//!
//! Dimensions, units, and indexes are evaluated canonically by
//! [`super::static_definitions`]; only syntax-backed nominal type
//! definitions are still collected into a per-module table here.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::desugared_ast::{DeclKind, File, TypeExpr};
use crate::registry::error::GraphcalError;
use crate::registry::types::{self, TypeRegistry};
use crate::syntax::names::{NameAtom, NamePath};
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::{GenericParamName, StructTypeName};

/// Register every type declaration of a file into its frontend type table.
///
/// # Errors
///
/// Returns a [`GraphcalError`] for an invalid generic parameter list or a
/// duplicate constructor or payload field.
pub(super) fn register_file_types(
    file: &File,
    types: &mut TypeRegistry,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    register_types_impl(file, types, src, None)
}

/// Register only the named type declarations of a dependency file.
///
/// # Errors
///
/// Returns a [`GraphcalError`] for an invalid selected type declaration.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn register_selected_types(
    file: &File,
    types: &mut TypeRegistry,
    src: &NamedSource<Arc<String>>,
    names: &HashSet<StructTypeName>,
) -> Result<(), GraphcalError> {
    register_types_impl(file, types, src, Some(names))
}

fn register_types_impl(
    file: &File,
    types: &mut TypeRegistry,
    src: &NamedSource<Arc<String>>,
    filter: Option<&HashSet<StructTypeName>>,
) -> Result<(), GraphcalError> {
    for decl in &file.declarations {
        if let DeclKind::Type(t) = &decl.kind
            && filter.is_none_or(|names| names.contains(&t.name.value))
        {
            register_type_decl(t, types, src)?;
        }
    }
    Ok(())
}

fn register_type_decl(
    t: &crate::desugar::desugared_ast::TypeDecl,
    types: &mut TypeRegistry,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    validate_type_generic_params(t, src)?;
    let generic_params: Vec<types::TypeGenericParam> = t
        .generic_params
        .iter()
        .map(|g| types::TypeGenericParam {
            name: g.name.value.clone(),
            constraint: g.constraint,
            default: g.default.clone(),
            span: g.name.span,
        })
        .collect();

    let type_def = match &t.body {
        crate::desugar::desugared_ast::TypeDeclBody::Required => {
            types::TypeDef::required(t.name.value.clone(), generic_params)
        }
        crate::desugar::desugared_ast::TypeDeclBody::Constructors(type_members) => {
            // Every constructor carries its payload inline; no per-constructor
            // TypeDef is synthesized. Checked construction keeps duplicate
            // payload fields out of the registry even for non-parser AST callers.
            let members = type_members
                .iter()
                .map(|member| {
                    let payload = match &member.payload {
                        Some(fields) => fields.as_slice(),
                        None => &[],
                    };
                    let fields = payload
                        .iter()
                        .map(|field| {
                            types::StructField::new(
                                field.name.value.clone(),
                                field.type_ann.clone(),
                            )
                        })
                        .collect();
                    types::UnionMemberDef::try_new(
                        member.name.value.clone(),
                        fields,
                    )
                    .map_err(|error| match error {
                        types::TypeDefError::DuplicateConstructorField {
                            constructor,
                            field,
                            first_index,
                            duplicate_index,
                        } => GraphcalError::DuplicateConstructorField {
                            type_name: t.name.value.clone(),
                            constructor,
                            field,
                            src: src.clone(),
                            duplicate: payload[duplicate_index].name.span.into(),
                            first: payload[first_index].name.span.into(),
                        },
                        types::TypeDefError::DuplicateConstructor { constructor } => {
                            GraphcalError::InternalError {
                                message: format!(
                                    "checked union member unexpectedly reported duplicate constructor `{constructor}`"
                                ),
                                src: src.clone(),
                                span: member.name.span.into(),
                            }
                        }
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            types::TypeDef::try_union(t.name.value.clone(), generic_params, members).map_err(
                |error| match error {
                    types::TypeDefError::DuplicateConstructor { constructor } => {
                        let mut declarations = type_members
                            .iter()
                            .filter(|member| member.name.value.as_str() == constructor.as_str());
                        let first = declarations
                            .next()
                            .map_or(t.name.span, |member| member.name.span);
                        let duplicate =
                            declarations.next().map_or(first, |member| member.name.span);
                        GraphcalError::DuplicateName {
                            name: constructor.to_string(),
                            src: src.clone(),
                            duplicate: duplicate.into(),
                            first: first.into(),
                        }
                    }
                    types::TypeDefError::DuplicateConstructorField { .. } => {
                        GraphcalError::InternalError {
                            message: "validated union member lost its field-uniqueness invariant"
                                .to_string(),
                            src: src.clone(),
                            span: t.name.span.into(),
                        }
                    }
                },
            )?
        }
    };

    types.register_type(type_def);
    Ok(())
}

fn validate_type_generic_params(
    t: &crate::desugar::desugared_ast::TypeDecl,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let positions = t.generic_params.iter().enumerate().try_fold(
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
    for (index, param) in t.generic_params.iter().enumerate() {
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
    arg: &crate::desugar::desugared_ast::GenericArg,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    match arg {
        crate::syntax::ast::GenericArg::Type(type_expr) => {
            find_non_earlier_type_reference(type_expr, current_index, positions)
        }
        crate::syntax::ast::GenericArg::Index(index) => {
            find_non_earlier_index_reference(index, current_index, positions)
        }
        crate::syntax::ast::GenericArg::Nat(nat_expr) => {
            find_non_earlier_nat_reference(nat_expr, current_index, positions)
        }
        crate::syntax::ast::GenericArg::Ambiguous(ambiguous) => {
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
    path: &Spanned<NamePath>,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    path.value.as_bare()?;
    non_earlier_generic_reference(path.value.leaf(), path.span, current_index, positions)
}

fn find_non_earlier_ambiguous_reference(
    arg: &crate::syntax::ast::AmbiguousGenericArg,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    match arg {
        crate::syntax::ast::AmbiguousGenericArg::Name(ident) => {
            non_earlier_generic_reference(ident.name.atom(), ident.span, current_index, positions)
        }
        crate::syntax::ast::AmbiguousGenericArg::Mul(operands, _) => {
            operands.iter().find_map(|operand| {
                find_non_earlier_ambiguous_reference(operand, current_index, positions)
            })
        }
    }
}

fn find_non_earlier_nat_reference(
    expr: &crate::syntax::ast::NatExpr,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    match expr {
        crate::syntax::ast::NatExpr::Literal(..) => None,
        crate::syntax::ast::NatExpr::Var(ident) => {
            non_earlier_generic_reference(ident.name.atom(), ident.span, current_index, positions)
        }
        crate::syntax::ast::NatExpr::Add(operands, _)
        | crate::syntax::ast::NatExpr::Mul(operands, _) => operands
            .iter()
            .find_map(|operand| find_non_earlier_nat_reference(operand, current_index, positions)),
    }
}

fn find_non_earlier_index_reference(
    index: &crate::syntax::ast::IndexExpr,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    match index {
        crate::syntax::ast::IndexExpr::Name(path) => {
            find_non_earlier_path_reference(path, current_index, positions)
        }
        crate::syntax::ast::IndexExpr::Finite { cardinality, .. }
        | crate::syntax::ast::IndexExpr::BareNat(cardinality) => {
            find_non_earlier_nat_reference(cardinality, current_index, positions)
        }
    }
}

fn find_non_earlier_type_reference(
    type_expr: &TypeExpr,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    use crate::desugar::desugared_ast::TypeExprKind;

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
