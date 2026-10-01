//! Inference of field access and constructor calls on nominal types.

use crate::hir::expr::{Expr, FieldInit};
use crate::hir::nominal::{NominalConstructor, NominalField, NominalTypeDef};
use crate::hir::types::GenericArg;
use crate::outcome::Outcome;
use crate::resolved_name::ResolvedConstructorName;
use crate::semantic_error::evaluation::EvaluationError;
use crate::semantic_error::structure::StructError;

use crate::graphcal_error::GraphcalError;
use crate::semantic::checked_type::{StructTypeRef, Symbolic};
use crate::syntax::type_name::FieldName;

use crate::semantic::checked_type::CheckedType;
use crate::tir::dim_check::helpers::{
    format_checked_type, format_distinct_types, struct_type_def_for_inferred,
};

use super::context::Infer;
use super::override_deps::TypeNominalUse;
use crate::tir::dim_check::generic_substitution::{resolved_field_type, resolved_type_field_key};

fn record_member(type_def: &NominalTypeDef) -> Option<&NominalConstructor> {
    let members = type_def.union_members()?;
    let [only] = members else {
        return None;
    };
    (only.name().as_str() == type_def.name().as_str()).then_some(only)
}

impl Infer<'_> {
    pub(super) fn infer_hir_field_access(
        &self,
        inner: &Expr,
        field: &crate::syntax::span::Spanned<FieldName>,
    ) -> Result<CheckedType<Symbolic>, Outcome<GraphcalError>> {
        let inner_type = self.infer_hir_type(inner)?;
        let CheckedType::Struct(type_name, type_args) = &inner_type else {
            return Err(GraphcalError::located(
                self.env.src,
                inner.span,
                StructError::NotAStruct {
                    name: format_checked_type(&inner_type, self.env.registry),
                },
            )
            .into());
        };
        self.check_type_override_dependency(
            type_name.resolved(),
            TypeNominalUse::Field {
                field: &field.value,
                span: field.span,
            },
        )?;
        let type_def =
            struct_type_def_for_inferred(type_name, Some(self.env.dag), self.env.registry)
                .ok_or_else(|| {
                    GraphcalError::located(
                        self.env.src,
                        inner.span,
                        StructError::UnknownStructType {
                            name: type_name.to_string(),
                        },
                    )
                })?;
        let member = record_member(type_def).ok_or_else(|| {
            let detail = if type_def.is_required() {
                format!("required type `{}` has no fields", type_name.name())
            } else {
                format!(
                    "union type `{}` (use `match` to access fields)",
                    type_name.name()
                )
            };
            GraphcalError::located(
                self.env.src,
                inner.span,
                StructError::NotAStruct { name: detail },
            )
        })?;
        if !member
            .fields()
            .iter()
            .any(|field_def| field_def.name() == &field.value)
        {
            return Err(GraphcalError::located(
                self.env.src,
                field.span,
                StructError::UnknownField {
                    type_name: type_name.name().clone(),
                    member: crate::semantic_error::structure::NominalMember::Field(
                        field.value.clone(),
                    ),
                },
            )
            .into());
        }
        resolved_field_type(
            &resolved_type_field_key(type_name.resolved(), member, &field.value),
            type_def,
            type_args,
            self.env.dag,
            self.env.src,
            field.span,
        )
        .map(|ty| ty.to_symbolic())
        .map_err(Outcome::Failed)
    }

    pub(super) fn infer_hir_constructor_call(
        &self,
        expr: &Expr,
        callee: &crate::syntax::span::Spanned<ResolvedConstructorName>,
        constructor_generic_args: &[GenericArg],
        fields: &[FieldInit],
    ) -> Result<CheckedType<Symbolic>, Outcome<GraphcalError>> {
        let target = self.env.resolved_constructor(&callee.value, callee.span)?;
        self.check_type_override_dependency(
            target.owning_type(),
            TypeNominalUse::Constructor {
                constructor: &callee.value,
                span: callee.span,
            },
        )?;
        constructor_generic_args
            .iter()
            .try_for_each(|arg| self.check_hir_generic_arg_override_dependencies(arg))?;
        let type_def = target.definition();
        let variant = target.variant();
        let owning_type_identity = StructTypeRef::from_resolved(target.owning_type().clone());
        let owning_type_name = type_def.name();

        let resolved_type_args = self.env.resolve_applied_generic_args(
            type_def,
            constructor_generic_args,
            callee.span,
        )?;

        let def_field_names: std::collections::HashSet<&FieldName> =
            variant.fields().iter().map(NominalField::name).collect();
        let provided_names: Vec<&FieldName> =
            fields.iter().map(|field| &field.name.value).collect();
        let mut seen_fields = std::collections::HashSet::new();
        for field in fields {
            if !seen_fields.insert(field.name.value.clone()) {
                return Err(GraphcalError::located(
                    self.env.src,
                    field.name.span,
                    EvaluationError::Failed {
                        message: format!(
                            "duplicate field `{}` in constructor `{}`",
                            field.name.value,
                            variant.name()
                        ),
                    },
                )
                .into());
            }
        }
        let extra: Vec<FieldName> = provided_names
            .iter()
            .filter(|name| !def_field_names.contains(**name))
            .map(|name| (*name).clone())
            .collect();
        if !extra.is_empty() {
            return Err(GraphcalError::located(
                self.env.src,
                expr.span,
                StructError::ExtraFields {
                    type_name: owning_type_name,
                    extra,
                },
            )
            .into());
        }

        let provided_set: std::collections::HashSet<&FieldName> =
            provided_names.iter().copied().collect();
        let missing: Vec<FieldName> = variant
            .fields()
            .iter()
            .filter(|field| !provided_set.contains(field.name()))
            .map(|field| field.name().clone())
            .collect();
        if !missing.is_empty() {
            return Err(GraphcalError::located(
                self.env.src,
                expr.span,
                StructError::MissingFields {
                    type_name: owning_type_name,
                    missing,
                },
            )
            .into());
        }

        for field_init in fields {
            let field_def = variant
                .fields()
                .iter()
                .find(|field| field.name() == &field_init.name.value)
                .ok_or_else(|| {
                    GraphcalError::located(
                        self.env.src,
                        field_init.name.span,
                        EvaluationError::Failed {
                            message: format!(
                                "internal: unknown field `{}` in constructor `{}`",
                                field_init.name.value,
                                variant.name()
                            ),
                        },
                    )
                })?;
            let value_type = self.infer_hir_type(&field_init.value)?;
            let expected = resolved_field_type(
                &resolved_type_field_key(target.owning_type(), variant, field_def.name()),
                type_def,
                &resolved_type_args,
                self.env.dag,
                self.env.src,
                field_init.name.span,
            )?
            .to_symbolic();
            if value_type != expected {
                let (expected, found) =
                    format_distinct_types(&expected, &value_type, self.env.registry);
                return Err(GraphcalError::located(
                    self.env.src,
                    field_init.name.span,
                    StructError::FieldDimensionMismatch {
                        type_name: owning_type_name,
                        field_name: field_init.name.value.clone(),
                        expected,
                        found,
                    },
                )
                .into());
            }
        }

        Ok(CheckedType::Struct(
            owning_type_identity,
            resolved_type_args,
        ))
    }
}
