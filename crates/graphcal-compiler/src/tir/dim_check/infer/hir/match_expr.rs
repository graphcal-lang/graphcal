//! Inference of `match` expressions.

use crate::hir::expr::{Expr, MatchArm, MatchPattern, PatternBinding};
use crate::outcome::Outcome;
use crate::semantic_error::evaluation::EvaluationError;
use crate::semantic_error::index::IndexError;
use crate::semantic_error::structure::StructError;
use crate::source_id::SourceId;

use crate::display::formatting_registry::FormattingRegistry;
use crate::semantic::checked_type::{IndexTypeRef, Symbolic};
use crate::semantic_error::SemanticError;
use crate::syntax::type_name::FieldName;

use crate::semantic::checked_type::{CheckedGenericArg, CheckedType};
use crate::tir::dim_check::helpers::{format_checked_type, nominal_for_inferred};
use crate::tir::dim_check::infer::rules;

use super::context::{Infer, InferEnv};
use super::override_deps::{IndexNominalUse, TypeNominalUse};
use crate::tir::dim_check::generic_substitution::applied_field_type;
use crate::tir::typed::NominalMember;

impl InferEnv<'_> {
    fn constructor_field_type(
        &self,
        field: &crate::syntax::span::Spanned<FieldName>,
        member: NominalMember<'_>,
        scrutinee_type_args: &[CheckedGenericArg<Symbolic>],
    ) -> Result<CheckedType<Symbolic>, SemanticError> {
        let field_semantics = member.field(&field.value).ok_or_else(|| {
            SemanticError::located(
                self.src,
                field.span,
                StructError::UnknownField {
                    type_name: member.nominal().definition().name(),
                    member: crate::semantic_error::structure::NominalMember::Field(
                        field.value.clone(),
                    ),
                },
            )
        })?;
        applied_field_type(field_semantics, scrutinee_type_args, self.src, field.span)
            .map(|ty| ty.to_symbolic())
    }
}

impl Infer<'_> {
    #[expect(clippy::too_many_lines, reason = "exhaustive handling of match arms")]
    pub(super) fn infer_hir_match(
        &self,
        expr: &Expr,
        scrutinee: &Expr,
        arms: &[MatchArm],
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        let scrutinee_type = self.infer_hir_type(scrutinee)?;
        match &scrutinee_type {
            CheckedType::Key(index_identity) => {
                if index_identity.finite_index_form().is_some() {
                    return Err(SemanticError::located(self.env.src, scrutinee.span, EvaluationError::Failed { message: format!(
                            "cannot match on `Key<{index_identity}>`; only named-axis keys support label matching"
                        ) }).into());
                }
                let index_def = crate::tir::dim_check::infer::index_def_for_inferred(
                    index_identity,
                    self.env.tir,
                )
                .ok_or_else(|| {
                    SemanticError::located(
                        self.env.src,
                        scrutinee.span,
                        IndexError::UnknownIndex {
                            name: index_identity.display_name(),
                        },
                    )
                })?;
                let variants = match &index_def.kind {
                    crate::semantic::index_def::IndexKind::Concrete(
                        crate::semantic::index_def::ConcreteIndexKind::Named { variants },
                    ) => variants.as_slice().to_vec(),
                    crate::semantic::index_def::IndexKind::Required(
                        crate::semantic::index_def::RequiredIndexKind::Named,
                    ) => vec![],
                    _ => {
                        return Err(SemanticError::located(self.env.src, scrutinee.span, EvaluationError::Failed { message: format!(
                                "cannot match on coordinate index `{index_identity}`; only named indexes can be matched"
                            ) }).into());
                    }
                };
                let mut covered = std::collections::HashSet::new();
                let mut arm_types = Vec::new();
                for arm in arms {
                    let MatchPattern::IndexLabel { variant, span } = &arm.pattern else {
                        return Err(SemanticError::located(
                            self.env.src,
                            arm.span,
                            EvaluationError::Failed {
                                message: "label match arms must use index-label patterns"
                                    .to_string(),
                            },
                        )
                        .into());
                    };
                    self.check_index_override_dependency(
                        &IndexTypeRef::from_resolved(variant.variant.index().clone()),
                        IndexNominalUse::Label(variant.variant.variant()),
                    )?;
                    if index_identity.declared_resolved() != Some(variant.variant.index()) {
                        return Err(SemanticError::located(
                            self.env.src,
                            *span,
                            IndexError::IndexMismatch {
                                expected: index_identity.display_name(),
                                found: variant.variant.index().to_unowned_def_name().into(),
                            },
                        )
                        .into());
                    }
                    let variant_name = variant.variant.variant();
                    if !variants.iter().any(|v| v == variant_name) {
                        return Err(SemanticError::located(
                            self.env.src,
                            variant.path_span(),
                            IndexError::UnknownVariant {
                                index_name: index_identity.display_name(),
                                variant_name: variant_name.clone(),
                            },
                        )
                        .into());
                    }
                    if !covered.insert(variant_name.clone()) {
                        return Err(SemanticError::located(
                            self.env.src,
                            *span,
                            EvaluationError::Failed {
                                message: format!(
                                    "duplicate match arm for variant `{variant_name}`"
                                ),
                            },
                        )
                        .into());
                    }
                    arm_types.push(self.infer_hir_type(&arm.body)?);
                }
                for variant in variants {
                    if !covered.contains(&variant) {
                        return Err(SemanticError::located(self.env.src, expr.span, EvaluationError::Failed { message: format!(
                                "non-exhaustive match: variant `{index_identity}#{variant}` not covered"
                            ) }).into());
                    }
                }
                hir_arm_types_match(&arm_types, arms, self.env.registry, self.env.src, expr)
                    .map_err(Outcome::Failed)
            }
            CheckedType::Struct(type_name, scrutinee_type_args) => {
                let nominal = nominal_for_inferred(type_name, self.env.dag).ok_or_else(|| {
                    SemanticError::located(
                        self.env.src,
                        scrutinee.span,
                        StructError::UnknownStructType {
                            name: type_name.to_string(),
                        },
                    )
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
                        return Err(SemanticError::located(
                            self.env.src,
                            arm.span,
                            EvaluationError::Failed {
                                message: "union match arms must use constructor patterns"
                                    .to_string(),
                            },
                        )
                        .into());
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
                        return Err(SemanticError::located(
                            self.env.src,
                            *span,
                            StructError::EmptyParenthesizedConstructor {
                                constructor: target.variant().name(),
                            },
                        )
                        .into());
                    }
                    let Some(member) = nominal.member_of(target) else {
                        return Err(SemanticError::located(
                            self.env.src,
                            constructor.span,
                            StructError::UnknownField {
                                type_name: type_name.name().clone(),
                                member:
                                    crate::semantic_error::structure::NominalMember::Constructor(
                                        target.name(),
                                    ),
                            },
                        )
                        .into());
                    };
                    if !covered.insert(target.variant().name().clone()) {
                        return Err(SemanticError::located(
                            self.env.src,
                            *span,
                            EvaluationError::Failed {
                                message: format!(
                                    "duplicate match arm for `{}`",
                                    target.variant().name()
                                ),
                            },
                        )
                        .into());
                    }
                    let mut arm_locals = self.locals.child(Vec::new());
                    let mut seen_pattern_fields = std::collections::HashSet::new();
                    for binding in bindings {
                        let field = match binding {
                            PatternBinding::Bind { field, .. }
                            | PatternBinding::Wildcard { field, .. } => field,
                        };
                        if !seen_pattern_fields.insert(field.value.clone()) {
                            return Err(SemanticError::located(
                                self.env.src,
                                field.span,
                                EvaluationError::Failed {
                                    message: format!(
                                        "duplicate pattern binding for field `{}` in `{}`",
                                        field.value,
                                        target.variant().name()
                                    ),
                                },
                            )
                            .into());
                        }
                        let field_type =
                            self.env
                                .constructor_field_type(field, member, scrutinee_type_args)?;
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
                        return Err(SemanticError::located(
                            self.env.src,
                            *span,
                            StructError::MissingPatternFields {
                                constructor: target.variant().name(),
                                missing,
                            },
                        )
                        .into());
                    }
                    arm_types.push(self.with_locals(&arm_locals).infer_hir_type(&arm.body)?);
                }
                for member in nominal.members() {
                    let name = member.constructor().name();
                    if !covered.contains(&name) {
                        return Err(SemanticError::located(
                            self.env.src,
                            expr.span,
                            EvaluationError::Failed {
                                message: format!(
                                    "non-exhaustive match: member `{name}` not covered"
                                ),
                            },
                        )
                        .into());
                    }
                }
                hir_arm_types_match(&arm_types, arms, self.env.registry, self.env.src, expr)
                    .map_err(Outcome::Failed)
            }
            _ => Err(SemanticError::located(
                self.env.src,
                scrutinee.span,
                EvaluationError::Failed {
                    message: format!(
                        "cannot match on type `{}`; expected a tagged union or label value",
                        format_checked_type(&scrutinee_type, self.env.registry)
                    ),
                },
            )
            .into()),
        }
    }
}

fn hir_arm_types_match(
    arm_types: &[CheckedType<Symbolic>],
    arms: &[MatchArm],
    registry: &FormattingRegistry,
    src: SourceId,
    expr: &Expr,
) -> Result<CheckedType<Symbolic>, SemanticError> {
    rules::match_arms_rule(arm_types, |i| arms[i].body.span, expr.span, registry, src)
}
