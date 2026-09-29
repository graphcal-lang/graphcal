//! Inference of DAG calls with specialized generic interfaces.

use crate::hir::expr::{DagCallIndexBinding, DagCallStaticBindings, Expr, ParamBinding};
use crate::resolved_name::ResolvedDeclName;
use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::dimension::{BaseDimId, Dimension};
use crate::registry::error::GraphcalError;
use crate::syntax::span::Span;
use crate::tir::typed::{
    NatPolyForm, ResolvedDimArg, ResolvedDimTerm, ResolvedGenericArg, ResolvedIndex,
    ResolvedTypeExpr,
};

use crate::tir::dim_check::InferredType;
use crate::tir::dim_check::helpers::{format_inferred_type, resolved_type_matches_inferred};

use super::context::Infer;
use super::generics::{GenericSubstitutions, substitute_resolved_type_with_type_params};

fn specialize_dag_call_dimension(
    dimension: &Dimension,
    bindings: &DagCallStaticBindings,
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Dimension, GraphcalError> {
    dimension.iter().try_fold(
        Dimension::dimensionless(),
        |acc, (base, exponent)| {
            let factor = match base {
                BaseDimId::UserDefined(name) => bindings
                    .dimensions
                    .get(name)
                    .map_or_else(
                        || Ok(Dimension::base(base.clone())),
                        |target| {
                            tir.dimension(target).cloned().ok_or_else(|| {
                                GraphcalError::InternalError {
                                    message: format!(
                                        "DAG-call dimension binding target `{target}` is absent from the semantic registry"
                                    ),
                                    src: src.clone(),
                                    span: span.into(),
                                }
                            })
                        },
                    )?,
                BaseDimId::Prelude(_) => Dimension::base(base.clone()),
            };
            let factor = factor.pow(*exponent).map_err(|error| {
                GraphcalError::InternalError {
                    message: format!("DAG-call dimension substitution overflowed: {error}"),
                    src: src.clone(),
                    span: span.into(),
                }
            })?;
            acc.checked_mul(&factor).map_err(|error| {
                GraphcalError::InternalError {
                    message: format!("DAG-call dimension substitution overflowed: {error}"),
                    src: src.clone(),
                    span: span.into(),
                }
            })
        },
    )
}

fn specialize_dag_call_index(
    index: &ResolvedIndex,
    bindings: &DagCallStaticBindings,
) -> ResolvedIndex {
    match index {
        ResolvedIndex::Concrete(name, span) => bindings.indexes.get(name).map_or_else(
            || index.clone(),
            |target| match target {
                DagCallIndexBinding::Declared(target) => {
                    ResolvedIndex::Concrete(target.clone(), *span)
                }
                DagCallIndexBinding::Finite(target) => {
                    ResolvedIndex::Finite(NatPolyForm::from_constant(target.size_u64()), *span)
                }
            },
        ),
        ResolvedIndex::GenericParam(_, _) | ResolvedIndex::Finite(_, _) => index.clone(),
    }
}

fn specialize_dag_call_dim_arg(
    dimension: &ResolvedDimArg,
    bindings: &DagCallStaticBindings,
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<ResolvedDimArg, GraphcalError> {
    match dimension {
        ResolvedDimArg::Concrete(dimension) => {
            { specialize_dag_call_dimension(dimension, bindings, tir, src, span) }
                .map(ResolvedDimArg::Concrete)
        }
        ResolvedDimArg::Expr { terms, span } => terms
            .iter()
            .map(|term| match term {
                ResolvedDimTerm::Concrete { dim, power, op } => {
                    specialize_dag_call_dimension(dim, bindings, tir, src, *span).map(|dim| {
                        ResolvedDimTerm::Concrete {
                            dim,
                            power: *power,
                            op: *op,
                        }
                    })
                }
                ResolvedDimTerm::GenericParam { .. } => Ok(term.clone()),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|terms| ResolvedDimArg::Expr { terms, span: *span }),
        ResolvedDimArg::Dimensionless | ResolvedDimArg::GenericParam(_, _) => Ok(dimension.clone()),
    }
}

fn specialize_dag_call_type(
    resolved: &ResolvedTypeExpr,
    bindings: &DagCallStaticBindings,
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let recurse =
        |resolved: &ResolvedTypeExpr| specialize_dag_call_type(resolved, bindings, tir, src, span);
    match resolved {
        ResolvedTypeExpr::Quantity(dimension) => {
            { specialize_dag_call_dimension(dimension, bindings, tir, src, span) }
                .map(ResolvedTypeExpr::Quantity)
        }
        ResolvedTypeExpr::Complex {
            dimension,
            span: type_span,
        } => specialize_dag_call_dim_arg(dimension, bindings, tir, src, span).map(|dimension| {
            ResolvedTypeExpr::Complex {
                dimension,
                span: *type_span,
            }
        }),
        ResolvedTypeExpr::Key {
            index,
            span: type_span,
        } => Ok(ResolvedTypeExpr::Key {
            index: specialize_dag_call_index(index, bindings),
            span: *type_span,
        }),
        ResolvedTypeExpr::Struct(name, type_span) => Ok(ResolvedTypeExpr::Struct(
            bindings.types.get(name).unwrap_or(name).clone(),
            *type_span,
        )),
        ResolvedTypeExpr::GenericStruct {
            name,
            generic_args,
            span: type_span,
        } => {
            let generic_args = generic_args
                .iter()
                .map(|argument| match argument {
                    ResolvedGenericArg::Dim(dimension) => {
                        { specialize_dag_call_dim_arg(dimension, bindings, tir, src, span) }
                            .map(ResolvedGenericArg::Dim)
                    }
                    ResolvedGenericArg::Index(index) => Ok(ResolvedGenericArg::Index(
                        specialize_dag_call_index(index, bindings),
                    )),
                    ResolvedGenericArg::Nat(_, _) => Ok(argument.clone()),
                    ResolvedGenericArg::Type(resolved) => {
                        recurse(resolved).map(ResolvedGenericArg::Type)
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ResolvedTypeExpr::GenericStruct {
                name: bindings.types.get(name).unwrap_or(name).clone(),
                generic_args,
                span: *type_span,
            })
        }
        ResolvedTypeExpr::GenericDimExpr {
            terms,
            span: type_span,
        } => {
            let terms = terms
                .iter()
                .map(|term| match term {
                    ResolvedDimTerm::Concrete { dim, power, op } => {
                        specialize_dag_call_dimension(dim, bindings, tir, src, span).map(|dim| {
                            ResolvedDimTerm::Concrete {
                                dim,
                                power: *power,
                                op: *op,
                            }
                        })
                    }
                    ResolvedDimTerm::GenericParam { .. } => Ok(term.clone()),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ResolvedTypeExpr::GenericDimExpr {
                terms,
                span: *type_span,
            })
        }
        ResolvedTypeExpr::Indexed { base, indexes } => Ok(ResolvedTypeExpr::Indexed {
            base: Box::new(recurse(base)?),
            indexes: indexes
                .iter()
                .map(|index| specialize_dag_call_index(index, bindings))
                .collect(),
        }),
        ResolvedTypeExpr::Dimensionless
        | ResolvedTypeExpr::Bool
        | ResolvedTypeExpr::Int
        | ResolvedTypeExpr::Datetime(_)
        | ResolvedTypeExpr::GenericDimParam(_, _)
        | ResolvedTypeExpr::GenericTypeParam(_, _) => Ok(resolved.clone()),
    }
}

impl Infer<'_> {
    pub(super) fn infer_hir_dag_call(
        &self,
        expr: &Expr,
        target: &crate::syntax::span::Spanned<crate::dag_id::DagId>,
        args: &[ParamBinding],
        static_bindings: &DagCallStaticBindings,
        output: &crate::syntax::span::Spanned<ResolvedDeclName>,
    ) -> Result<InferredType, GraphcalError> {
        let display_path = target.value.to_string();
        let dag_tir =
            self.env
                .tir
                .dags
                .get(&target.value)
                .ok_or_else(|| GraphcalError::UnknownDag {
                    name: display_path.clone(),
                    src: self.env.src.clone(),
                    span: target.span.into(),
                })?;

        let mut required_param_keys = std::collections::HashSet::new();
        let param_decl_types_by_key: HashMap<
            ResolvedDeclName,
            &crate::tir::typed::ResolvedTypeExpr,
        > = dag_tir
            .params
            .iter()
            .map(|param| {
                let key = param.identity();
                if param.default.is_none() {
                    required_param_keys.insert(key.clone());
                }
                let resolved = dag_tir
                    .resolved_decl_types
                    .get(&param.name)
                    .ok_or_else(|| GraphcalError::InternalError {
                        message: format!(
                            "semantic type missing for DAG-call param `{}`",
                            param.name
                        ),
                        src: self.env.src.clone(),
                        span: param.type_ann.span.into(),
                    })?;
                Ok((key, resolved))
            })
            .collect::<Result<_, GraphcalError>>()?;
        let node_decl_types_by_key: HashMap<
            ResolvedDeclName,
            &crate::tir::typed::ResolvedTypeExpr,
        > = dag_tir
            .nodes
            .iter()
            .map(|node| {
                let key = node.identity();
                let resolved = dag_tir.resolved_decl_types.get(&node.name).ok_or_else(|| {
                    GraphcalError::InternalError {
                        message: format!("semantic type missing for DAG-call node `{}`", node.name),
                        src: self.env.src.clone(),
                        span: node.type_ann.span.into(),
                    }
                })?;
                Ok((key, resolved))
            })
            .collect::<Result<_, GraphcalError>>()?;

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
            let expected = specialize_dag_call_type(
                expected,
                static_bindings,
                self.env.tir,
                self.env.src,
                binding.target.span,
            )?;
            if !resolved_type_matches_inferred(&expected, &found) {
                return Err(GraphcalError::DagArgTypeMismatch {
                    param_name: target_key.as_str().to_string(),
                    expected: expected.format(self.env.registry),
                    found: format_inferred_type(&found, self.env.registry),
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
        let output_decl = specialize_dag_call_type(
            output_decl,
            static_bindings,
            self.env.tir,
            self.env.src,
            output.span,
        )?;
        substitute_resolved_type_with_type_params(
            &output_decl,
            &GenericSubstitutions::default(),
            self.env.src,
        )
    }
}
