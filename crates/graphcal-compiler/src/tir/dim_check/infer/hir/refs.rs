//! Inference of unit literals and of declaration, constant, and constructor references.

use crate::hir::expr::{ConstRef, ResolvedUnitExpr};
use crate::resolved_name::ResolvedDeclName;
use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::dimension::Dimension;
use crate::registry::declared_type::StructTypeRef;
use crate::registry::error::GraphcalError;
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::Span;
use crate::syntax::type_name::GenericParamName;

use crate::tir::dim_check::InferredType;
use crate::tir::dim_check::infer::rules;

use super::context::{Infer, InferEnv};
use super::override_deps::TypeNominalUse;

pub(super) fn infer_hir_quantity_literal(
    unit: &ResolvedUnitExpr,
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let dim = rules::resolve_unit_dimension_or_diagnose(unit, tir, src)?;
    Ok(InferredType::Quantity(dim))
}

impl InferEnv<'_> {
    pub(super) fn infer_resolved_decl_ref_type(
        &self,
        target: &ResolvedDeclName,
        span: Span,
    ) -> Result<InferredType, GraphcalError> {
        // HIR references preserve their definition-time owner. A concrete semantic
        // instance is the authoritative boundary that maps those references to the
        // corresponding runtime declaration before any type lookup.
        let runtime_target = self.dag.runtime_decl_identity(target);
        let checked =
            self.tir
                .decl_type(&runtime_target)
                .ok_or_else(|| GraphcalError::UnknownGraphRef {
                    name: ScopedName::local(runtime_target.to_unowned_def_name()),
                    src: self.src.clone(),
                    span: span.into(),
                })?;
        let dim_sub = HashMap::new();
        let index_sub =
            HashMap::<GenericParamName, crate::registry::declared_type::IndexTypeRef>::new();
        let nat_sub = HashMap::new();
        crate::tir::typed::substitute_resolved_type(
            checked.resolved(),
            &dim_sub,
            &index_sub,
            &nat_sub,
            self.src,
        )
    }
}

impl Infer<'_> {
    pub(super) fn infer_hir_const_ref(
        &self,
        target: &crate::syntax::span::Spanned<ConstRef>,
    ) -> Result<InferredType, GraphcalError> {
        match &target.value {
            ConstRef::Decl(resolved) => {
                self.env.infer_resolved_decl_ref_type(resolved, target.span)
            }
            ConstRef::Builtin(_) => Ok(InferredType::Quantity(Dimension::dimensionless())),
            ConstRef::Constructor(constructor) => {
                let target_def = self.env.resolved_constructor(constructor, target.span)?;
                self.check_type_override_dependency(
                    target_def.owning_type(),
                    TypeNominalUse::Constructor {
                        constructor,
                        span: target.span,
                    },
                )?;
                if !target_def.variant().fields().is_empty() {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "constructor `{}` requires field arguments",
                            target_def.name()
                        ),
                        src: self.env.src.clone(),
                        span: target.span.into(),
                    });
                }
                let type_args = self.env.resolve_applied_generic_args(
                    target_def.owning_type(),
                    target_def.definition(),
                    &[],
                    target.span,
                )?;
                Ok(InferredType::Struct(
                    StructTypeRef::from_resolved(target_def.owning_type().clone()),
                    type_args,
                ))
            }
        }
    }
}
