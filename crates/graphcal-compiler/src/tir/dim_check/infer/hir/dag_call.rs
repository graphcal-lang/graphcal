//! Inference of DAG calls, whose Static bindings specialize the called
//! template's interface like an include.

use crate::hir::expr::{Expr, ParamBinding};
use crate::ir::static_substitution::StaticSubstitution;
use crate::outcome::Outcome;
use crate::resolved_name::ResolvedDeclName;
use crate::semantic_error::graph::{DagReference, GraphError};
use crate::semantic_error::visibility::VisibilityError;
use std::collections::HashMap;

use crate::semantic_error::SemanticError;
use crate::tir::typed::complete_substitution::CompleteSubstitution;
use crate::tir::typed::specialization::specialize_type;

use crate::semantic::checked_type::{CheckedType, Symbolic};

use super::context::Infer;

impl Infer<'_> {
    pub(super) fn infer_hir_dag_call(
        &self,
        expr: &Expr,
        target: &crate::syntax::span::Spanned<crate::dag_id::DagId>,
        args: &[ParamBinding],
        static_bindings: &StaticSubstitution,
        output: &crate::syntax::span::Spanned<ResolvedDeclName>,
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        let dag_tir = self.env.tir.dag(&target.value).ok_or_else(|| {
            SemanticError::located(
                self.env.src,
                target.span,
                GraphError::UnknownDag {
                    name: target.value.clone(),
                },
            )
        })?;

        let substitution =
            CompleteSubstitution::try_new(static_bindings, self.env.tir.project_type_store())
                .map_err(|error| error.into_graphcal(self.env.src))?;
        let mut required_param_keys = std::collections::HashSet::new();
        let param_decl_types_by_key: HashMap<
            ResolvedDeclName,
            &crate::tir::typed::ResolvedDeclType,
        > = dag_tir
            .params()
            .map(|param| {
                let key = param.identity();
                if param.default.is_none() {
                    required_param_keys.insert(key.clone());
                }
                (key, param.type_ann.checked().resolved())
            })
            .collect();
        let node_decl_types_by_key: HashMap<
            ResolvedDeclName,
            &crate::tir::typed::ResolvedDeclType,
        > = dag_tir
            .nodes()
            .map(|node| (node.identity(), node.type_ann.checked().resolved()))
            .collect();

        let mut bound_resolved_names: std::collections::HashSet<ResolvedDeclName> =
            std::collections::HashSet::with_capacity(args.len());
        for binding in args {
            let target_key = &binding.target.value;
            bound_resolved_names.insert(target_key.clone());
            let expected = param_decl_types_by_key.get(target_key).ok_or_else(|| {
                SemanticError::located(
                    self.env.src,
                    binding.target.span,
                    GraphError::UnknownDagParam {
                        name: target_key.to_unowned_def_name(),
                        dag_name: target.value.clone(),
                    },
                )
            })?;
            let found = self.infer_hir_type(&binding.value)?;
            let expected = specialize_type(expected, &substitution, self.env.src)?;
            // A parameter type that still mentions a generic parameter has no
            // checked type and therefore matches no argument.
            if !expected
                .to_checked_type(self.env.src)
                .is_ok_and(|expected| expected.to_symbolic() == found)
            {
                return Err(SemanticError::located(
                    self.env.src,
                    binding.value.span,
                    GraphError::DagArgTypeMismatch {
                        param_name: target_key.to_unowned_def_name(),
                        expected: expected.spelling(self.env.registry),
                        found: found.spelling(&self.env.registry.dimensions),
                    },
                )
                .into());
            }
        }

        let mut missing: Vec<_> = required_param_keys
            .iter()
            .filter(|param| !bound_resolved_names.contains(*param))
            .map(ResolvedDeclName::to_unowned_def_name)
            .collect();
        if !missing.is_empty() {
            missing.sort_by_key(ToString::to_string);
            return Err(SemanticError::located(
                self.env.src,
                expr.span,
                GraphError::MissingDagBindings {
                    missing,
                    dag_name: DagReference::Dag(target.value.clone()),
                },
            )
            .into());
        }

        let output_key = &output.value;
        let output_decl = node_decl_types_by_key
            .get(output_key)
            .or_else(|| param_decl_types_by_key.get(output_key))
            .ok_or_else(|| {
                SemanticError::located(
                    self.env.src,
                    output.span,
                    GraphError::UnknownDagOutput {
                        name: output_key.to_unowned_def_name(),
                        dag_name: target.value.clone(),
                    },
                )
            })?;
        if !dag_tir
            .projectable_outputs
            .contains(&output_key.to_unowned_def_name())
        {
            return Err(SemanticError::located(
                self.env.src,
                output.span,
                VisibilityError::ImportPrivateItem {
                    name: output_key.atom().clone(),
                    file_path: DagReference::Dag(target.value.clone()),
                },
            )
            .into());
        }
        let output_decl = specialize_type(output_decl, &substitution, self.env.src)?;
        output_decl
            .to_checked_type(self.env.src)
            .map(|ty| ty.to_symbolic())
            .map_err(Outcome::Failed)
    }
}
