//! Inference of field access and constructor calls on nominal types.

use crate::hir::expr::{Expr, FieldInit};
use crate::hir::types::GenericArg;
use crate::outcome::Outcome;
use crate::resolved_name::ResolvedConstructorName;
use crate::semantic_error::structure::StructError;
use crate::semantic_error::structure::{FieldlessOperand, UnknownStructTypeName};

use crate::semantic::checked_type::{StructTypeRef, Symbolic};
use crate::semantic_error::SemanticError;
use crate::syntax::type_name::FieldName;

use crate::semantic::checked_type::CheckedType;
use crate::tir::dim_check::helpers::nominal_for_inferred;

use super::context::Infer;
use super::override_deps::TypeNominalUse;
use crate::tir::dim_check::generic_substitution::{applied_field_type, recorded_member};

impl Infer<'_> {
    pub(super) fn infer_hir_field_access(
        &self,
        inner: &Expr,
        field: &crate::syntax::span::Spanned<FieldName>,
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        let inner_type = self.infer_hir_type(inner)?;
        let CheckedType::Struct(type_name, type_args) = &inner_type else {
            return Err(SemanticError::located(
                self.env.src,
                inner.span,
                StructError::NotAStruct {
                    name: FieldlessOperand::NonStruct(
                        inner_type.spelling(&self.env.registry.dimensions),
                    ),
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
        let nominal = nominal_for_inferred(type_name, self.env.dag).ok_or_else(|| {
            SemanticError::located(
                self.env.src,
                inner.span,
                StructError::UnknownStructType {
                    name: UnknownStructTypeName::Checked(type_name.clone()),
                },
            )
        })?;
        let member = nominal.record_member().ok_or_else(|| {
            let detail = if nominal.definition().is_required() {
                FieldlessOperand::RequiredType(type_name.clone())
            } else {
                FieldlessOperand::Union(type_name.clone())
            };
            SemanticError::located(
                self.env.src,
                inner.span,
                StructError::NotAStruct { name: detail },
            )
        })?;
        let field_semantics = member.field(&field.value).ok_or_else(|| {
            SemanticError::located(
                self.env.src,
                field.span,
                StructError::UnknownField {
                    type_name: type_name.name().clone(),
                    member: crate::semantic_error::structure::NominalMember::Field(
                        field.value.clone(),
                    ),
                },
            )
        })?;
        applied_field_type(field_semantics, type_args, self.env.src, field.span)
            .map(|ty| ty.to_symbolic())
            .map_err(Outcome::Failed)
    }

    pub(super) fn infer_hir_constructor_call(
        &self,
        expr: &Expr,
        callee: &crate::syntax::span::Spanned<ResolvedConstructorName>,
        constructor_generic_args: &[GenericArg],
        fields: &[FieldInit],
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
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

        let member = recorded_member(self.env.dag, target, self.env.src, callee.span)?;
        let resolved_type_args = self.env.resolve_applied_generic_args(
            member.nominal(),
            constructor_generic_args,
            callee.span,
        )?;

        let provided_names: Vec<&FieldName> =
            fields.iter().map(|field| &field.name.value).collect();
        let mut seen_fields = std::collections::HashSet::new();
        for field in fields {
            if !seen_fields.insert(field.name.value.clone()) {
                return Err(SemanticError::located(
                    self.env.src,
                    field.name.span,
                    StructError::DuplicateConstructionField {
                        field: field.name.value.clone(),
                        constructor: variant.name(),
                    },
                )
                .into());
            }
        }
        // Each initializer is paired with the field it names; one naming no
        // field is extra.
        let mut initialized = Vec::with_capacity(fields.len());
        let mut extra: Vec<FieldName> = Vec::new();
        for field_init in fields {
            match member.field(&field_init.name.value) {
                Some(field_def) => initialized.push((field_def, field_init)),
                None => extra.push(field_init.name.value.clone()),
            }
        }
        if !extra.is_empty() {
            return Err(SemanticError::located(
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
            return Err(SemanticError::located(
                self.env.src,
                expr.span,
                StructError::MissingFields {
                    type_name: owning_type_name,
                    missing,
                },
            )
            .into());
        }

        for (field_def, field_init) in initialized {
            let value_type = self.infer_hir_type(&field_init.value)?;
            let expected = applied_field_type(
                field_def,
                &resolved_type_args,
                self.env.src,
                field_init.name.span,
            )?
            .to_symbolic();
            if value_type != expected {
                let (expected, found) = CheckedType::distinct_spellings(
                    &expected,
                    &value_type,
                    &self.env.registry.dimensions,
                );
                return Err(SemanticError::located(
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
