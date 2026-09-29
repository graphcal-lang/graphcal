//! Inference of DAG calls, whose Static bindings specialize the called
//! template's interface like an include.

use crate::hir::expr::{Expr, ParamBinding};
use crate::ir::static_substitution::StaticSubstitution;
use crate::resolved_name::ResolvedDeclName;
use std::collections::HashMap;

use crate::registry::error::GraphcalError;
use crate::tir::typed::specialization::specialize_type;

use crate::registry::checked_type::{CheckedType, Symbolic};
use crate::tir::dim_check::helpers::format_checked_type;

use super::context::Infer;

impl Infer<'_> {
    pub(super) fn infer_hir_dag_call(
        &self,
        expr: &Expr,
        target: &crate::syntax::span::Spanned<crate::dag_id::DagId>,
        args: &[ParamBinding],
        static_bindings: &StaticSubstitution,
        output: &crate::syntax::span::Spanned<ResolvedDeclName>,
    ) -> Result<CheckedType<Symbolic>, GraphcalError> {
        let display_path = target.value.to_string();
        let dag_tir = self
            .env
            .tir
            .dag(&target.value)
            .ok_or_else(|| GraphcalError::UnknownDag {
                name: display_path.clone(),
                src: self.env.src.clone(),
                span: target.span.into(),
            })?;

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
                GraphcalError::UnknownDagParam {
                    name: target_key.as_str().to_string(),
                    dag_name: display_path.clone(),
                    src: self.env.src.clone(),
                    span: binding.target.span.into(),
                }
            })?;
            let found = self.infer_hir_type(&binding.value)?;
            let expected = specialize_type(
                expected,
                static_bindings,
                self.env.tir.project_type_store(),
                self.env.src,
            )?;
            // A parameter type that still mentions a generic parameter has no
            // checked type and therefore matches no argument.
            if !expected
                .to_checked_type(self.env.src)
                .is_ok_and(|expected| expected.to_symbolic() == found)
            {
                return Err(GraphcalError::DagArgTypeMismatch {
                    param_name: target_key.as_str().to_string(),
                    expected: expected.format(self.env.registry),
                    found: format_checked_type(&found, self.env.registry),
                    src: self.env.src.clone(),
                    span: binding.value.span.into(),
                });
            }
        }

        let mut missing: Vec<String> = required_param_keys
            .iter()
            .filter(|param| !bound_resolved_names.contains(*param))
            .map(|param| param.as_str().to_string())
            .collect();
        if !missing.is_empty() {
            missing.sort();
            return Err(GraphcalError::MissingDagBindings {
                missing,
                dag_name: display_path.clone(),
                src: self.env.src.clone(),
                span: expr.span.into(),
            });
        }

        let output_key = &output.value;
        let output_decl = node_decl_types_by_key
            .get(output_key)
            .or_else(|| param_decl_types_by_key.get(output_key))
            .ok_or_else(|| GraphcalError::UnknownDagOutput {
                name: output_key.as_str().to_string(),
                dag_name: display_path.clone(),
                src: self.env.src.clone(),
                span: output.span.into(),
            })?;
        let output_name = output_key.as_str();
        if !dag_tir
            .projectable_outputs
            .contains(&output_key.to_unowned_def_name())
        {
            return Err(GraphcalError::ImportPrivateItem {
                name: output_name.to_string(),
                file_path: display_path,
                src: self.env.src.clone(),
                span: output.span.into(),
            });
        }
        let output_decl = specialize_type(
            output_decl,
            static_bindings,
            self.env.tir.project_type_store(),
            self.env.src,
        )?;
        output_decl
            .to_checked_type(self.env.src)
            .map(|ty| ty.to_symbolic())
    }
}
