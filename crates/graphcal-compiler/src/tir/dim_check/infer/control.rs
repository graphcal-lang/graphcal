//! Type inference for control flow: If, Match.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::syntax::ast::{Expr, MatchArm};
use crate::syntax::names::{FieldName, IndexName, StructTypeName};

use crate::gcl_err;
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
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let cond_type = infer_type(
        condition,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        src,
    )?;
    if cond_type != InferredType::Bool {
        return Err(gcl_err!(DimensionMismatch {
            expected: "Bool".to_string(),
            found: format_inferred_type(&cond_type, registry),
            help: "if/else condition must be Bool".to_string(),
        } @ src, condition.span));
    }

    let then_type = infer_type(
        then_branch,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        src,
    )?;
    let else_type = infer_type(
        else_branch,
        declared_types,
        local_types,
        registry,
        builtin_fns,
        src,
    )?;

    if then_type != else_type {
        return Err(gcl_err!(DimensionMismatch {
            expected: format_inferred_type(&then_type, registry),
            found: format_inferred_type(&else_type, registry),
            help: "both branches of if/else must have the same dimension".to_string(),
        } @ src, else_branch.span));
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
    declared_types: &HashMap<String, DeclaredType>,
    local_types: &HashMap<String, InferredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    // Infer scrutinee type — must be a struct/tagged union value.
    let scrutinee_type = infer_type(
        scrutinee,
        declared_types,
        local_types,
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
                .ok_or_else(|| gcl_err!(UnknownIndex {
                    name: index_name.clone(),
                } @ src, scrutinee.span))?;

            let variants = match &index_def.kind {
                crate::registry::types::IndexKind::Named { variants } => variants.clone(),
                crate::registry::types::IndexKind::RequiredNamed => vec![],
                _ => {
                    return Err(gcl_err!(EvalError {
                        message: format!(
                            "cannot match on range index `{index_name}`; only named indexes can be matched"
                        ),
                    } @ src, scrutinee.span));
                }
            };

            let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut arm_types: Vec<InferredType> = Vec::new();

            for arm in arms {
                let variant_name_str = arm.pattern.variant_name.value.as_str();

                // For label patterns, qualified_index must match the index name
                if let Some(qualified) = &arm.pattern.qualified_index
                    && qualified.value.as_str() != index_name.as_str()
                {
                    return Err(gcl_err!(IndexMismatch {
                        expected: index_name.clone(),
                        found: qualified.value.clone(),
                    } @ src, qualified.span));
                }

                // Check variant belongs to this index
                if !variants.iter().any(|v| v.as_str() == variant_name_str) {
                    return Err(gcl_err!(UnknownField {
                        type_name: StructTypeName::new(index_name.as_str()),
                        field_name: FieldName::new(variant_name_str),
                    } @ src, arm.pattern.variant_name.span));
                }

                // Check for duplicate arms
                if !covered.insert(variant_name_str.to_string()) {
                    return Err(gcl_err!(EvalError {
                        message: format!("duplicate match arm for variant `{variant_name_str}`"),
                    } @ src, arm.pattern.span));
                }

                // Labels are fieldless — no bindings allowed
                if !arm.pattern.bindings.is_empty() {
                    return Err(gcl_err!(EvalError {
                        message: format!(
                            "index label variant `{index_name}::{variant_name_str}` has no fields to bind"
                        ),
                    } @ src, arm.pattern.span));
                }

                // Infer arm body type
                let arm_type = infer_type(
                    &arm.body,
                    declared_types,
                    local_types,
                    registry,
                    builtin_fns,
                    src,
                )?;
                arm_types.push(arm_type);
            }

            // Check exhaustiveness: all variants must be covered
            for variant in variants {
                if !covered.contains(variant.as_str()) {
                    return Err(gcl_err!(EvalError {
                        message: format!(
                            "non-exhaustive match: variant `{index_name}::{}` not covered",
                            variant.as_str()
                        ),
                    } @ src, expr.span));
                }
            }

            // All arm types must match
            check_arm_types_match(&arm_types, arms, registry, src, expr)?
        }

        InferredType::Struct(type_name, scrutinee_type_args) => {
            let type_def = registry.types.get_type(type_name.as_str()).ok_or_else(|| {
                gcl_err!(UnknownStructType {
                    name: type_name.clone(),
                } @ src, scrutinee.span)
            })?;

            let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut arm_types: Vec<InferredType> = Vec::new();

            for arm in arms {
                let variant_name_str = arm.pattern.variant_name.value.as_str();
                if let Some(qualified) = &arm.pattern.qualified_index
                    && qualified.value.as_str() != type_name.as_str()
                {
                    return Err(gcl_err!(IndexMismatch {
                        expected: IndexName::new(type_name.as_str()),
                        found: qualified.value.clone(),
                    } @ src, qualified.span));
                }

                // Check variant/member belongs to this type
                let member_type_def = if type_def.is_union() {
                    // Union type: look up the member type
                    if !registry
                        .types
                        .is_member_of_union(variant_name_str, type_name.as_str())
                    {
                        return Err(gcl_err!(UnknownField {
                            type_name: type_name.clone(),
                            field_name: FieldName::new(variant_name_str),
                        } @ src, arm.pattern.variant_name.span));
                    }
                    registry.types.get_type(variant_name_str).ok_or_else(|| {
                        gcl_err!(UnknownStructType {
                            name: StructTypeName::new(variant_name_str),
                        } @ src, arm.pattern.variant_name.span)
                    })?
                } else {
                    // Non-union struct: the only valid pattern is the type itself
                    type_def
                };

                // Check for duplicate arms
                if !covered.insert(variant_name_str.to_string()) {
                    return Err(gcl_err!(EvalError {
                        message: format!("duplicate match arm for `{variant_name_str}`"),
                    } @ src, arm.pattern.span));
                }

                // Bind pattern variables as locals
                let mut arm_locals = local_types.clone();
                for binding in &arm.pattern.bindings {
                    match binding {
                        crate::syntax::ast::PatternBinding::Bind { field, var } => {
                            let field_def = member_type_def
                                .fields()
                                .iter()
                                .find(|f| f.name.as_str() == field.value.as_str())
                                .ok_or_else(|| gcl_err!(UnknownField {
                                    type_name: type_name.clone(),
                                    field_name: field.value.clone(),
                                } @ src, field.span))?;
                            let field_type = resolve_field_type(
                                &field_def.type_ann,
                                type_def,
                                scrutinee_type_args,
                                registry,
                                src,
                            )?;
                            arm_locals.insert(var.name.clone(), field_type);
                        }
                        crate::syntax::ast::PatternBinding::Wildcard { .. } => {
                            // Wildcard: no binding needed
                        }
                    }
                }

                // Infer arm body type
                let arm_type = infer_type(
                    &arm.body,
                    declared_types,
                    &arm_locals,
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
                        return Err(gcl_err!(EvalError {
                            message: format!(
                                "non-exhaustive match: member `{}` not covered",
                                member.name.as_str()
                            ),
                        } @ src, expr.span));
                    }
                }
            } else {
                // Non-union struct: single arm matching the type itself
                if !covered.contains(type_name.as_str()) {
                    return Err(gcl_err!(EvalError {
                        message: format!("non-exhaustive match: type `{type_name}` not covered"),
                    } @ src, expr.span));
                }
            }

            // All arm types must match
            check_arm_types_match(&arm_types, arms, registry, src, expr)?
        }

        _ => {
            return Err(gcl_err!(EvalError {
                message: format!(
                    "cannot match on type `{}`; expected a tagged union or label value",
                    format_inferred_type(&scrutinee_type, registry)
                ),
            } @ src, scrutinee.span));
        }
    })
}
