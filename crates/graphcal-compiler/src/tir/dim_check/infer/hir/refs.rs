//! Inference of unit literals and of declaration, constant, and constructor references.

use crate::hir::expr::LocalDecl;
use crate::hir::expr::{ConstRef, ResolvedUnitExpr};
use crate::source_id::SourceId;

use crate::dimension::Dimension;
use crate::graphcal_error::GraphcalError;
use crate::semantic::checked_type::{StructTypeRef, Symbolic};
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::Span;

use crate::semantic::checked_type::CheckedType;
use crate::tir::dim_check::infer::rules;

use super::context::{Infer, InferEnv};
use super::override_deps::TypeNominalUse;

pub(super) fn infer_hir_quantity_literal(
    unit: &ResolvedUnitExpr,
    tir: &dyn crate::tir::typed::TirRead,
    src: SourceId,
) -> Result<CheckedType<Symbolic>, GraphcalError> {
    let dim = rules::resolve_unit_dimension_or_diagnose(unit, tir, src)?;
    Ok(CheckedType::Quantity(dim))
}

impl InferEnv<'_> {
    pub(super) fn infer_resolved_decl_ref_type(
        &self,
        target: &LocalDecl,
        span: Span,
    ) -> Result<CheckedType<Symbolic>, GraphcalError> {
        // The body is shared with its template; the frame of the DAG checked
        // here names the declaration the reference reads.
        let runtime_target = self.dag.frame().resolve(target);
        let checked =
            self.tir
                .decl_type(&runtime_target)
                .ok_or_else(|| GraphcalError::UnknownGraphRef {
                    name: ScopedName::local(runtime_target.to_unowned_def_name()),
                    src: self.src,
                    span: span.into(),
                })?;
        Ok(checked.declared().to_symbolic())
    }
}

impl Infer<'_> {
    pub(super) fn infer_hir_const_ref(
        &self,
        target: &crate::syntax::span::Spanned<ConstRef>,
    ) -> Result<CheckedType<Symbolic>, GraphcalError> {
        match &target.value {
            ConstRef::Decl(resolved) => {
                self.env.infer_resolved_decl_ref_type(resolved, target.span)
            }
            ConstRef::Builtin(_) => Ok(CheckedType::Quantity(Dimension::dimensionless())),
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
                        src: self.env.src,
                        span: target.span.into(),
                    });
                }
                let type_args = self.env.resolve_applied_generic_args(
                    target_def.definition(),
                    &[],
                    target.span,
                )?;
                Ok(CheckedType::Struct(
                    StructTypeRef::from_resolved(target_def.owning_type().clone()),
                    type_args,
                ))
            }
        }
    }
}
