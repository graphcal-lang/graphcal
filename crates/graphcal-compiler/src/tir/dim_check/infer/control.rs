//! Type inference for control flow: If, Match.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::resolved_ast::{Expr, MatchArm};
use crate::syntax::names::{FieldName, IndexName, ScopedName, StructTypeName};

use crate::registry::error::GraphcalError;
use crate::registry::types::Registry;

use super::super::helpers::{check_arm_types_match, format_inferred_type, resolve_field_type};
use super::super::{DeclaredType, InferredType};
use super::infer_type;

/// Infer the type of an if/else expression.
pub(super) fn infer_if(
    condition: &Expr,
    then_branch: &Expr,
    else_branch: &Expr,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let cond_type = infer_type(
        condition,
        declared_types,
        local_types,
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
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    let else_type = infer_type(
        else_branch,
        declared_types,
        local_types,
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
        tir,
        registry,
        builtin_fns,
        src,
    )?;

    Ok(match &scrutinee_type {
        InferredType::Label(index_name) => {
            // Label scrutinee: match on index variants (fieldless, qualified syntax)
            let index_def = registry
                .indexes
                .get_index(index_name.as_str())
                .ok_or_else(|| GraphcalError::UnknownIndex {
                    name: index_name.clone(),
                    src: src.clone(),
                    span: scrutinee.span.into(),
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

                if index.value.leaf_str() != index_name.as_str() {
                    return Err(GraphcalError::IndexMismatch {
                        expected: index_name.clone(),
                        found: IndexName::from_atom(index.value.leaf().clone()),
                        src: src.clone(),
                        span: index.span.into(),
                    });
                }

                // Check variant belongs to this index
                if !variants.iter().any(|v| v.as_str() == variant_name_str) {
                    return Err(GraphcalError::UnknownField {
                        type_name: StructTypeName::new(index_name.as_str()),
                        field_name: FieldName::new(variant_name_str),
                        src: src.clone(),
                        span: variant.span.into(),
                    });
                }

                // Check for duplicate arms
                if !covered.insert(variant_name_str.to_string()) {
                    return Err(GraphcalError::EvalError {
                        message: format!("duplicate match arm for variant `{variant_name_str}`"),
                        src: src.clone(),
                        span: (*span).into(),
                    });
                }

                // Infer arm body type
                let arm_type = infer_type(
                    &arm.body,
                    declared_types,
                    local_types,
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
                            variant.qualified_by(index_name)
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
            let type_def = registry.types.get_type(type_name.as_str()).ok_or_else(|| {
                GraphcalError::UnknownStructType {
                    name: type_name.to_string(),
                    src: src.clone(),
                    span: scrutinee.span.into(),
                }
            })?;

            let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut arm_types: Vec<InferredType> = Vec::new();

            for arm in arms {
                let crate::desugar::resolved_ast::MatchPattern::Constructor {
                    name,
                    bindings,
                    span,
                } = &arm.pattern
                else {
                    return Err(GraphcalError::EvalError {
                        message: "union match arms must use constructor patterns".to_string(),
                        src: src.clone(),
                        span: arm.pattern.span().into(),
                    });
                };
                let variant_name_str = name.value.as_str();

                // The match pattern names a constructor of `type_def`.
                // Resolve it in the union's member list directly — there
                // are no per-variant TypeDefs.
                let members = type_def
                    .union_members()
                    .ok_or_else(|| GraphcalError::EvalError {
                        message: format!(
                            "internal: cannot match on required (unbound) type `{type_name}`"
                        ),
                        src: src.clone(),
                        span: (*span).into(),
                    })?;
                let variant_def = members
                    .iter()
                    .find(|m| m.name.as_str() == variant_name_str)
                    .ok_or_else(|| GraphcalError::UnknownField {
                        type_name: type_name.clone(),
                        field_name: FieldName::new(variant_name_str),
                        src: src.clone(),
                        span: name.span.into(),
                    })?;

                // Check for duplicate arms
                if !covered.insert(variant_name_str.to_string()) {
                    return Err(GraphcalError::EvalError {
                        message: format!("duplicate match arm for `{variant_name_str}`"),
                        src: src.clone(),
                        span: (*span).into(),
                    });
                }

                // Bind pattern variables as locals
                let mut arm_locals = local_types.clone();
                for binding in bindings {
                    match binding {
                        crate::desugar::resolved_ast::PatternBinding::Bind { field, var } => {
                            let field_def = variant_def
                                .fields
                                .iter()
                                .find(|f| f.name.as_str() == field.value.as_str())
                                .ok_or_else(|| GraphcalError::UnknownField {
                                    type_name: type_name.clone(),
                                    field_name: field.value.clone(),
                                    src: src.clone(),
                                    span: field.span.into(),
                                })?;
                            let field_type = resolve_field_type(
                                &field_def.type_ann,
                                type_def,
                                scrutinee_type_args,
                                registry,
                                src,
                            )?;
                            arm_locals.insert(var.name.to_string(), field_type);
                        }
                        crate::desugar::resolved_ast::PatternBinding::Wildcard { .. } => {
                            // Wildcard: no binding needed
                        }
                    }
                }

                // Infer arm body type
                let arm_type = infer_type(
                    &arm.body,
                    declared_types,
                    &arm_locals,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?;
                arm_types.push(arm_type);
            }

            // Check exhaustiveness: all members/variants must be covered
            if let Some(members) = type_def.union_members() {
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
                if !covered.contains(type_name.as_str()) {
                    return Err(GraphcalError::EvalError {
                        message: format!("non-exhaustive match: type `{type_name}` not covered"),
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
