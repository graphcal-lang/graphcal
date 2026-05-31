//! Type inference for control flow: If, Match.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::resolved_ast::{Expr, MatchArm, MatchPattern};
use crate::registry::error::GraphcalError;
use crate::registry::types::{IndexDef, Registry, TypeDef, UnionMemberDef};
use crate::syntax::names::{
    FieldName, IndexName, NamePath, ResolvedIndexVariant, ScopedName, StructTypeName,
};
use crate::syntax::span::Span;
use crate::tir::typed::{ResolvedConstructorPattern, ResolvedPatternBinding};

use super::super::helpers::{
    check_arm_types_match, format_inferred_type, resolve_field_type, struct_type_def_for_inferred,
};
use super::super::{DeclaredType, InferredIndex, InferredType};
use super::infer_type;

/// Collapse a syntactic index path to a leaf-only name at syntax boundaries.
///
/// Module-aware label matching must use `ResolvedCollectionRefs`; this adapter
/// is only for callers that still receive a syntax-only pattern.
fn standalone_index_name_from_path(path: &NamePath) -> IndexName {
    IndexName::from(path.leaf().clone())
}

fn resolved_match_label_variant<'a>(
    dag: Option<&'a crate::tir::typed::DagTIR>,
    pattern: &MatchPattern,
) -> Option<&'a ResolvedIndexVariant> {
    let refs = dag.map(|dag| &dag.semantic.collection_refs)?;
    refs.match_label_variants
        .get(&pattern.span())
        .or_else(|| match pattern {
            MatchPattern::IndexLabel { index, variant, .. } => refs
                .match_label_variants
                .get(&index.span.merge(variant.span)),
            MatchPattern::Path { path, .. } => refs.match_label_variants.get(&path.span()),
            MatchPattern::Constructor { .. } => None,
        })
}

fn index_def_for_label_index<'a>(
    index: &InferredIndex,
    dag: Option<&'a crate::tir::typed::DagTIR>,
    _registry: &'a Registry,
) -> Option<&'a IndexDef> {
    let resolved = index.declared_resolved()?;
    dag.map(|dag| &dag.semantic.collection_refs)
        .and_then(|refs| refs.index_defs.get(resolved))
}

/// Infer the type of an if/else expression.
pub(super) fn infer_if(
    condition: &Expr,
    then_branch: &Expr,
    else_branch: &Expr,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let cond_type = infer_type(
        condition,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    if cond_type != InferredType::Bool {
        return Err(GraphcalError::DimensionMismatch {
            expected: "Bool".to_string(),
            found: format_inferred_type(&cond_type, registry),
            help: "if/else condition must be Bool".to_string(),
            src: src.clone(),
            span: condition.span.into(),
        });
    }

    let then_type = infer_type(
        then_branch,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    let else_type = infer_type(
        else_branch,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;

    if then_type != else_type {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_inferred_type(&then_type, registry),
            found: format_inferred_type(&else_type, registry),
            help: "both branches of if/else must have the same dimension".to_string(),
            src: src.clone(),
            span: else_branch.span.into(),
        });
    }

    Ok(then_type)
}

#[derive(Clone, Copy)]
enum ConstructorPatternBindings<'a> {
    Syntax(&'a [crate::desugar::resolved_ast::PatternBinding]),
    Resolved(&'a [ResolvedPatternBinding]),
}

fn constructor_pattern_lookup_span(pattern: &MatchPattern) -> Option<Span> {
    match pattern {
        MatchPattern::Constructor { name, .. } => Some(name.span),
        MatchPattern::Path { path, .. } => Some(path.span()),
        MatchPattern::IndexLabel { .. } => None,
    }
}

fn resolved_constructor_pattern<'a>(
    dag: Option<&'a crate::tir::typed::DagTIR>,
    pattern: &MatchPattern,
) -> Option<&'a ResolvedConstructorPattern> {
    let span = constructor_pattern_lookup_span(pattern)?;
    dag.map(|dag| &dag.semantic.constructor_refs)
        .and_then(|refs| refs.match_pattern_constructors.get(&span))
}

#[expect(
    clippy::too_many_arguments,
    reason = "threads match-pattern binding context through one compatibility boundary"
)]
fn bind_constructor_pattern_locals(
    arm_locals: &mut HashMap<String, InferredType>,
    bindings: ConstructorPatternBindings<'_>,
    variant_def: &UnionMemberDef,
    type_name: &StructTypeName,
    type_def: &TypeDef,
    scrutinee_type_args: &[InferredType],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match bindings {
        ConstructorPatternBindings::Syntax(bindings) => {
            for binding in bindings {
                match binding {
                    crate::desugar::resolved_ast::PatternBinding::Bind { field, var } => {
                        let field_type = constructor_field_type(
                            field,
                            variant_def,
                            type_name,
                            type_def,
                            scrutinee_type_args,
                            registry,
                            src,
                        )?;
                        arm_locals.insert(var.name.to_string(), field_type);
                    }
                    crate::desugar::resolved_ast::PatternBinding::Wildcard { .. } => {}
                }
            }
        }
        ConstructorPatternBindings::Resolved(bindings) => {
            for binding in bindings {
                match binding {
                    ResolvedPatternBinding::Bind { field, local } => {
                        let field_type = constructor_field_type(
                            field,
                            variant_def,
                            type_name,
                            type_def,
                            scrutinee_type_args,
                            registry,
                            src,
                        )?;
                        // The HIR local ID has already proven which lexical binding this field
                        // introduces; expression inference still consumes the syntax name-keyed
                        // local map until HIR expressions become authoritative end-to-end.
                        arm_locals.insert(local.name.to_string(), field_type);
                    }
                    ResolvedPatternBinding::Wildcard { .. } => {}
                }
            }
        }
    }
    Ok(())
}

fn constructor_field_type(
    field: &crate::syntax::span::Spanned<FieldName>,
    variant_def: &UnionMemberDef,
    type_name: &StructTypeName,
    type_def: &TypeDef,
    scrutinee_type_args: &[InferredType],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let field_def = variant_def
        .fields
        .iter()
        .find(|field_def| field_def.name.as_str() == field.value.as_str())
        .ok_or_else(|| GraphcalError::UnknownField {
            type_name: type_name.clone(),
            field_name: field.value.clone(),
            src: src.clone(),
            span: field.span.into(),
        })?;
    resolve_field_type(
        &field_def.type_ann,
        type_def,
        scrutinee_type_args,
        registry,
        src,
    )
}

/// Infer the type of a match expression.
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive handling of match arms for labels and structs"
)]
pub(super) fn infer_match(
    expr: &Expr,
    scrutinee: &Expr,
    arms: &[MatchArm],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    dag: Option<&crate::tir::typed::DagTIR>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    // Infer scrutinee type — must be a struct/tagged union value.
    let scrutinee_type = infer_type(
        scrutinee,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;

    Ok(match &scrutinee_type {
        InferredType::Label(index_identity) => {
            let index_name = index_identity.name();
            // Label scrutinee: match on index variants (fieldless, qualified syntax)
            let index_def =
                index_def_for_label_index(index_identity, dag, registry).ok_or_else(|| {
                    GraphcalError::UnknownIndex {
                        name: index_name.clone(),
                        src: src.clone(),
                        span: scrutinee.span.into(),
                    }
                })?;

            let variants = match &index_def.kind {
                crate::registry::types::IndexKind::Named { variants } => variants.clone(),
                crate::registry::types::IndexKind::RequiredNamed => vec![],
                _ => {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "cannot match on range index `{index_name}`; only named indexes can be matched"
                        ),
                        src: src.clone(),
                        span: scrutinee.span.into(),
                    });
                }
            };

            let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut arm_types: Vec<InferredType> = Vec::new();

            for arm in arms {
                let (variant_name_str, duplicate_span) = if let Some(resolved_variant) =
                    resolved_match_label_variant(dag, &arm.pattern)
                {
                    if !index_identity.matches_resolved(resolved_variant.index()) {
                        return Err(GraphcalError::IndexMismatch {
                            expected: index_name.clone(),
                            found: resolved_variant.index().to_unowned_def_name(),
                            src: src.clone(),
                            span: arm.pattern.span().into(),
                        });
                    }
                    (resolved_variant.variant().as_str(), arm.pattern.span())
                } else {
                    let crate::desugar::resolved_ast::MatchPattern::IndexLabel {
                        index,
                        variant,
                        span,
                    } = &arm.pattern
                    else {
                        return Err(GraphcalError::EvalError {
                            message: "label match arms must use qualified index-label patterns"
                                .to_string(),
                            src: src.clone(),
                            span: arm.pattern.span().into(),
                        });
                    };
                    let variant_name_str = variant.value.as_str();

                    if index.value.leaf().as_str() != index_name.as_str() {
                        return Err(GraphcalError::IndexMismatch {
                            expected: index_name.clone(),
                            found: standalone_index_name_from_path(&index.value),
                            src: src.clone(),
                            span: index.span.into(),
                        });
                    }
                    (variant_name_str, *span)
                };

                // Check variant belongs to this index
                if !variants.iter().any(|v| v.as_str() == variant_name_str) {
                    return Err(GraphcalError::UnknownField {
                        type_name: StructTypeName::new(index_name.as_str()),
                        field_name: FieldName::new(variant_name_str),
                        src: src.clone(),
                        span: arm.pattern.span().into(),
                    });
                }

                // Check for duplicate arms
                if !covered.insert(variant_name_str.to_string()) {
                    return Err(GraphcalError::EvalError {
                        message: format!("duplicate match arm for variant `{variant_name_str}`"),
                        src: src.clone(),
                        span: duplicate_span.into(),
                    });
                }

                // Infer arm body type
                let arm_type = infer_type(
                    &arm.body,
                    declared_types,
                    local_types,
                    dag,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?;
                arm_types.push(arm_type);
            }

            // Check exhaustiveness: all variants must be covered
            for variant in variants {
                if !covered.contains(variant.as_str()) {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "non-exhaustive match: variant `{}` not covered",
                            variant.qualified_by(&index_name)
                        ),
                        src: src.clone(),
                        span: expr.span.into(),
                    });
                }
            }

            // All arm types must match
            check_arm_types_match(&arm_types, arms, registry, src, expr)?
        }

        InferredType::Struct(type_name, scrutinee_type_args) => {
            let type_def =
                struct_type_def_for_inferred(type_name, dag, registry).ok_or_else(|| {
                    GraphcalError::UnknownStructType {
                        name: type_name.to_string(),
                        src: src.clone(),
                        span: scrutinee.span.into(),
                    }
                })?;

            let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut arm_types: Vec<InferredType> = Vec::new();
            let mut resolved_type_def = None;

            for arm in arms {
                let resolved_pattern = resolved_constructor_pattern(dag, &arm.pattern);
                let (variant_name_str, pattern_span, pattern_type_def, variant_def, bindings) =
                    if let Some(pattern) = resolved_pattern {
                        if !type_name.matches_resolved(&pattern.target.owning_type) {
                            return Err(GraphcalError::UnknownField {
                                type_name: type_name.name().clone(),
                                field_name: FieldName::new(pattern.target.variant.name.as_str()),
                                src: src.clone(),
                                span: constructor_pattern_lookup_span(&arm.pattern)
                                    .unwrap_or_else(|| arm.pattern.span())
                                    .into(),
                            });
                        }
                        resolved_type_def.get_or_insert(&pattern.target.type_def);
                        (
                            pattern.target.variant.name.as_str(),
                            arm.pattern.span(),
                            &pattern.target.type_def,
                            &pattern.target.variant,
                            ConstructorPatternBindings::Resolved(&pattern.bindings),
                        )
                    } else {
                        let MatchPattern::Constructor {
                            name,
                            bindings,
                            span,
                        } = &arm.pattern
                        else {
                            return Err(GraphcalError::EvalError {
                                message: "union match arms must use constructor patterns"
                                    .to_string(),
                                src: src.clone(),
                                span: arm.pattern.span().into(),
                            });
                        };
                        let variant_name_str = name.value.as_str();

                        // The match pattern names a constructor of `type_def`.
                        // Resolve it in the union's member list directly — there
                        // are no per-variant TypeDefs.
                        let members =
                            type_def
                                .union_members()
                                .ok_or_else(|| GraphcalError::EvalError {
                                    message: format!(
                                        "internal: cannot match on required (unbound) type `{}`",
                                        type_name.name()
                                    ),
                                    src: src.clone(),
                                    span: (*span).into(),
                                })?;
                        let variant_def = members
                            .iter()
                            .find(|m| m.name.as_str() == variant_name_str)
                            .ok_or_else(|| GraphcalError::UnknownField {
                                type_name: type_name.name().clone(),
                                field_name: FieldName::new(variant_name_str),
                                src: src.clone(),
                                span: name.span.into(),
                            })?;
                        (
                            variant_name_str,
                            *span,
                            type_def,
                            variant_def,
                            ConstructorPatternBindings::Syntax(bindings),
                        )
                    };

                // Check for duplicate arms
                if !covered.insert(variant_name_str.to_string()) {
                    return Err(GraphcalError::EvalError {
                        message: format!("duplicate match arm for `{variant_name_str}`"),
                        src: src.clone(),
                        span: pattern_span.into(),
                    });
                }

                // Bind pattern variables as locals
                let mut arm_locals = local_types.clone();
                bind_constructor_pattern_locals(
                    &mut arm_locals,
                    bindings,
                    variant_def,
                    type_name.name(),
                    pattern_type_def,
                    scrutinee_type_args,
                    registry,
                    src,
                )?;

                // Infer arm body type
                let arm_type = infer_type(
                    &arm.body,
                    declared_types,
                    &arm_locals,
                    dag,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?;
                arm_types.push(arm_type);
            }

            let exhaustiveness_type_def = resolved_type_def.unwrap_or(type_def);

            // Check exhaustiveness: all members/variants must be covered
            if let Some(members) = exhaustiveness_type_def.union_members() {
                for member in members {
                    if !covered.contains(member.name.as_str()) {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "non-exhaustive match: member `{}` not covered",
                                member.name.as_str()
                            ),
                            src: src.clone(),
                            span: expr.span.into(),
                        });
                    }
                }
            } else {
                // Non-union struct: single arm matching the type itself
                if !covered.contains(type_name.name().as_str()) {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "non-exhaustive match: type `{}` not covered",
                            type_name.name()
                        ),
                        src: src.clone(),
                        span: expr.span.into(),
                    });
                }
            }

            // All arm types must match
            check_arm_types_match(&arm_types, arms, registry, src, expr)?
        }

        _ => {
            return Err(GraphcalError::EvalError {
                message: format!(
                    "cannot match on type `{}`; expected a tagged union or label value",
                    format_inferred_type(&scrutinee_type, registry)
                ),
                src: src.clone(),
                span: scrutinee.span.into(),
            });
        }
    })
}
