//! Inference of `match` expressions.

use crate::hir::expr::{Expr, MatchArm, MatchPattern, PatternBinding};
use crate::hir::nominal::{NominalConstructor, NominalTypeDef};
use crate::resolved_name::ResolvedStructTypeName;
use std::sync::Arc;

use miette::NamedSource;

use crate::registry::checked_type::IndexTypeRef;
use crate::registry::error::GraphcalError;
use crate::registry::types::FormattingRegistry;
use crate::syntax::type_name::FieldName;

use crate::registry::checked_type::{CheckedGenericArg, CheckedType};
use crate::tir::dim_check::helpers::{format_checked_type, struct_type_def_for_inferred};
use crate::tir::dim_check::infer::rules;

use super::context::{Infer, InferEnv};
use super::generics::{resolved_field_type, resolved_type_field_key};
use super::override_deps::{IndexNominalUse, TypeNominalUse};

impl InferEnv<'_> {
    fn constructor_field_type(
        &self,
        field: &crate::syntax::span::Spanned<FieldName>,
        variant: &NominalConstructor,
        owning_type: &ResolvedStructTypeName,
        type_def: &NominalTypeDef,
        scrutinee_type_args: &[CheckedGenericArg],
    ) -> Result<CheckedType, GraphcalError> {
        if !variant
            .fields()
            .iter()
            .any(|field_def| field_def.name() == &field.value)
        {
            return Err(GraphcalError::UnknownField {
                type_name: type_def.name(),
                member: crate::registry::error::NominalMember::Field(field.value.clone()),
                src: self.src.clone(),
                span: field.span.into(),
            });
        }
        resolved_field_type(
            &resolved_type_field_key(owning_type, variant, &field.value),
            type_def,
            scrutinee_type_args,
            self.dag,
            self.src,
            field.span,
        )
    }
}

impl Infer<'_> {
    #[expect(clippy::too_many_lines, reason = "exhaustive handling of match arms")]
    pub(super) fn infer_hir_match(
        &self,
        expr: &Expr,
        scrutinee: &Expr,
        arms: &[MatchArm],
    ) -> Result<CheckedType, GraphcalError> {
        let scrutinee_type = self.infer_hir_type(scrutinee)?;
        match &scrutinee_type {
            CheckedType::Key(index_identity) => {
                if index_identity.finite_index_form().is_some() {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "cannot match on `Key<{index_identity}>`; only named-axis keys support label matching"
                        ),
                        src: self.env.src.clone(),
                        span: scrutinee.span.into(),
                    });
                }
                let index_def = crate::tir::dim_check::infer::index_def_for_inferred(
                    index_identity,
                    self.env.tir,
                )
                .ok_or_else(|| GraphcalError::UnknownIndex {
                    name: index_identity.display_name(),
                    src: self.env.src.clone(),
                    span: scrutinee.span.into(),
                })?;
                let variants = match &index_def.kind {
                    crate::registry::types::IndexKind::Concrete(
                        crate::registry::types::ConcreteIndexKind::Named { variants },
                    ) => variants.as_slice().to_vec(),
                    crate::registry::types::IndexKind::Required(
                        crate::registry::types::RequiredIndexKind::Named,
                    ) => vec![],
                    _ => {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "cannot match on coordinate index `{index_identity}`; only named indexes can be matched"
                            ),
                            src: self.env.src.clone(),
                            span: scrutinee.span.into(),
                        });
                    }
                };
                let mut covered = std::collections::HashSet::new();
                let mut arm_types = Vec::new();
                for arm in arms {
                    let MatchPattern::IndexLabel { variant, span } = &arm.pattern else {
                        return Err(GraphcalError::EvalError {
                            message: "label match arms must use index-label patterns".to_string(),
                            src: self.env.src.clone(),
                            span: arm.span.into(),
                        });
                    };
                    self.check_index_override_dependency(
                        &IndexTypeRef::from_resolved(variant.variant.index().clone()),
                        IndexNominalUse::Label(variant.variant.variant()),
                    )?;
                    if index_identity.declared_resolved() != Some(variant.variant.index()) {
                        return Err(GraphcalError::IndexMismatch {
                            expected: index_identity.display_name(),
                            found: variant.variant.index().to_unowned_def_name().into(),
                            src: self.env.src.clone(),
                            span: (*span).into(),
                        });
                    }
                    let variant_name = variant.variant.variant();
                    if !variants.iter().any(|v| v == variant_name) {
                        return Err(GraphcalError::UnknownVariant {
                            index_name: index_identity.display_name(),
                            variant_name: variant_name.clone(),
                            src: self.env.src.clone(),
                            span: variant.path_span().into(),
                        });
                    }
                    if !covered.insert(variant_name.clone()) {
                        return Err(GraphcalError::EvalError {
                            message: format!("duplicate match arm for variant `{variant_name}`"),
                            src: self.env.src.clone(),
                            span: (*span).into(),
                        });
                    }
                    arm_types.push(self.infer_hir_type(&arm.body)?);
                }
                for variant in variants {
                    if !covered.contains(&variant) {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "non-exhaustive match: variant `{index_identity}#{variant}` not covered"
                            ),
                            src: self.env.src.clone(),
                            span: expr.span.into(),
                        });
                    }
                }
                hir_arm_types_match(&arm_types, arms, self.env.registry, self.env.src, expr)
            }
            CheckedType::Struct(type_name, scrutinee_type_args) => {
                let type_def =
                    struct_type_def_for_inferred(type_name, Some(self.env.dag), self.env.registry)
                        .ok_or_else(|| GraphcalError::UnknownStructType {
                            name: type_name.to_string(),
                            src: self.env.src.clone(),
                            span: scrutinee.span.into(),
                        })?;
                let mut covered = std::collections::HashSet::new();
                let mut arm_types = Vec::new();
                for arm in arms {
                    let MatchPattern::Constructor {
                        constructor,
                        bindings,
                        span,
                    } = &arm.pattern
                    else {
                        return Err(GraphcalError::EvalError {
                            message: "union match arms must use constructor patterns".to_string(),
                            src: self.env.src.clone(),
                            span: arm.span.into(),
                        });
                    };
                    let target = self
                        .env
                        .resolved_constructor(&constructor.value, constructor.span)?;
                    self.check_type_override_dependency(
                        target.owning_type(),
                        TypeNominalUse::Constructor {
                            constructor: &constructor.value,
                            span: constructor.span,
                        },
                    )?;
                    if bindings.is_explicit_empty() && target.variant().fields().is_empty() {
                        return Err(GraphcalError::EmptyParenthesizedConstructor {
                            constructor: target.variant().name(),
                            src: self.env.src.clone(),
                            span: (*span).into(),
                        });
                    }
                    if type_name.resolved() != target.owning_type() {
                        return Err(GraphcalError::UnknownField {
                            type_name: type_name.name().clone(),
                            member: crate::registry::error::NominalMember::Constructor(
                                target.name(),
                            ),
                            src: self.env.src.clone(),
                            span: constructor.span.into(),
                        });
                    }
                    if !covered.insert(target.variant().name().clone()) {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "duplicate match arm for `{}`",
                                target.variant().name()
                            ),
                            src: self.env.src.clone(),
                            span: (*span).into(),
                        });
                    }
                    let mut arm_locals = self.locals.child(Vec::new());
                    let mut seen_pattern_fields = std::collections::HashSet::new();
                    for binding in bindings {
                        let field = match binding {
                            PatternBinding::Bind { field, .. }
                            | PatternBinding::Wildcard { field, .. } => field,
                        };
                        if !seen_pattern_fields.insert(field.value.clone()) {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "duplicate pattern binding for field `{}` in `{}`",
                                    field.value,
                                    target.variant().name()
                                ),
                                src: self.env.src.clone(),
                                span: field.span.into(),
                            });
                        }
                        let field_type = self.env.constructor_field_type(
                            field,
                            target.variant(),
                            target.owning_type(),
                            target.definition(),
                            scrutinee_type_args,
                        )?;
                        match binding {
                            PatternBinding::Bind { local, .. } => {
                                arm_locals.bind(local.id, field_type);
                            }
                            PatternBinding::Wildcard { .. } => {}
                        }
                    }
                    let missing = target
                        .variant()
                        .fields()
                        .iter()
                        .filter(|field| !seen_pattern_fields.contains(field.name()))
                        .map(|field| field.name().clone())
                        .collect::<Vec<_>>();
                    if !missing.is_empty() {
                        return Err(GraphcalError::MissingPatternFields {
                            constructor: target.variant().name(),
                            missing,
                            src: self.env.src.clone(),
                            span: (*span).into(),
                        });
                    }
                    arm_types.push(self.with_locals(&arm_locals).infer_hir_type(&arm.body)?);
                }
                if let Some(members) = type_def.union_members() {
                    for member in members {
                        if !covered.contains(&member.name()) {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "non-exhaustive match: member `{}` not covered",
                                    member.name()
                                ),
                                src: self.env.src.clone(),
                                span: expr.span.into(),
                            });
                        }
                    }
                }
                hir_arm_types_match(&arm_types, arms, self.env.registry, self.env.src, expr)
            }
            _ => Err(GraphcalError::EvalError {
                message: format!(
                    "cannot match on type `{}`; expected a tagged union or label value",
                    format_checked_type(&scrutinee_type, self.env.registry)
                ),
                src: self.env.src.clone(),
                span: scrutinee.span.into(),
            }),
        }
    }
}

fn hir_arm_types_match(
    arm_types: &[CheckedType],
    arms: &[MatchArm],
    registry: &FormattingRegistry,
    src: &NamedSource<Arc<String>>,
    expr: &Expr,
) -> Result<CheckedType, GraphcalError> {
    rules::match_arms_rule(arm_types, |i| arms[i].body.span, expr.span, registry, src)
}
