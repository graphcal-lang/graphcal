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

use crate::tir::dim_check::infer::rules;
use crate::tir::dim_check::{DeclaredType, InferredType};

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
        let local_name = ScopedName::local(runtime_target.to_unowned_def_name());

        if runtime_target.owner() == &self.dag.dag_id
            && let Some(inferred) =
                infer_bound_decl_type(&local_name, self.declared_types, self.dag, self.src)?
        {
            return Ok(inferred);
        }

        for name in self
            .dag
            .semantic
            .decl_bindings
            .iter()
            .filter_map(|(name, resolved)| (resolved == &runtime_target).then_some(name))
        {
            if let Some(inferred) =
                infer_bound_decl_type(name, self.declared_types, self.dag, self.src)?
            {
                return Ok(inferred);
            }
        }

        if let Some(target_dag) = self.tir.dag_containing_declaration(&runtime_target)
            && let Some(inferred) = infer_bound_decl_type(
                &local_name,
                &target_dag.build_declared_types(self.src)?,
                target_dag,
                self.src,
            )?
        {
            return Ok(inferred);
        }

        Err(GraphcalError::UnknownGraphRef {
            name: local_name,
            src: self.src.clone(),
            span: span.into(),
        })
    }
}

fn infer_bound_decl_type(
    name: &ScopedName,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    dag: &crate::tir::typed::DagTIR,
    src: &NamedSource<Arc<String>>,
) -> Result<Option<InferredType>, GraphcalError> {
    if let Some(resolved_type) = dag.resolved_decl_types.get(name) {
        let dim_sub = HashMap::new();
        let index_sub =
            HashMap::<GenericParamName, crate::registry::declared_type::IndexTypeRef>::new();
        let nat_sub = HashMap::new();
        return crate::tir::typed::substitute_resolved_type(
            resolved_type,
            &dim_sub,
            &index_sub,
            &nat_sub,
            src,
        )
        .map(Some);
    }

    Ok(declared_types.get(name).map(InferredType::from))
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
