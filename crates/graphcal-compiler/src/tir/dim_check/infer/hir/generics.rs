//! Generic-argument inference and substitution for nominal types.

use crate::hir::types::{
    BuiltinType, DimArg, DimExpr, DimTermTarget, GenericArg, IndexRef, ValueType, ValueTypeKind,
};
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::structure::StructError;
use crate::semantic_error::structure::{GenericSort, UnboundGeneric};
use crate::source_id::SourceId;

use crate::dimension::Dimension;
use crate::semantic::checked_type::{IndexTypeRef, StructTypeRef, Symbolic};
use crate::semantic_error::SemanticError;
use crate::syntax::names::NamePath;
use crate::syntax::span::Span;

use crate::semantic::checked_type::{CheckedGenericArg, CheckedType};

use super::context::InferEnv;
use super::nat_forms::finite_index_error;
use crate::tir::dim_check::generic_substitution::{
    SortedGenericArg, generic_arg_internal_sort_error, generic_substitution_prefix,
    instantiate_concrete_generic_arg,
};

impl InferEnv<'_> {
    fn infer_hir_generic_type_arg(
        &self,
        value_type: &ValueType,
    ) -> Result<CheckedType<Symbolic>, SemanticError> {
        match &value_type.kind {
            ValueTypeKind::Builtin(BuiltinType::Dimensionless) => {
                Ok(CheckedType::Quantity(Dimension::dimensionless()))
            }
            ValueTypeKind::Builtin(BuiltinType::Bool) => Ok(CheckedType::Bool),
            ValueTypeKind::Builtin(BuiltinType::Int) => Ok(CheckedType::Int),
            ValueTypeKind::Builtin(BuiltinType::Datetime(scale)) => {
                Ok(CheckedType::Datetime(*scale))
            }
            ValueTypeKind::DimExpr(dim_expr) => {
                infer_hir_dim_expr_arg(dim_expr, self.tir, self.src).map(CheckedType::Quantity)
            }
            ValueTypeKind::Complex(dimension) => match dimension {
                DimArg::Dimensionless(_) => Ok(CheckedType::Complex(Dimension::dimensionless())),
                DimArg::Expr(dim_expr) => {
                    infer_hir_dim_expr_arg(dim_expr, self.tir, self.src).map(CheckedType::Complex)
                }
            },
            ValueTypeKind::Key(index) => Ok(CheckedType::Key(inferred_index_from_type_arg(
                index, self.src,
            )?)),
            ValueTypeKind::Struct(name) => Ok(CheckedType::Struct(
                StructTypeRef::from_resolved(name.value.clone()),
                vec![],
            )),
            ValueTypeKind::GenericTypeParam(param) => Err(SemanticError::located(
                self.src,
                param.span,
                StructError::UnboundGenericInConcreteType {
                    generic: Box::new(UnboundGeneric::NotConcretelyBound {
                        sort: GenericSort::Type,
                        name: param.value.name.clone(),
                    }),
                },
            )),
            ValueTypeKind::TypeApplication { name, generic_args } => {
                let nominal = self
                    .dag
                    .semantic
                    .type_defs
                    .nominal(&name.value)
                    .ok_or_else(|| {
                        SemanticError::internal_error(
                            format!(
                                "semantic type metadata missing generic type `{}`",
                                name.value
                            ),
                            self.src,
                            crate::diagnostic_anchor::DiagnosticAnchor::Source(name.span),
                        )
                    })?;
                Ok(CheckedType::Struct(
                    StructTypeRef::from_resolved(name.value.clone()),
                    self.resolve_applied_generic_args(nominal, generic_args, name.span)?,
                ))
            }
        }
    }

    fn infer_hir_sorted_generic_arg(
        &self,
        arg: &GenericArg,
    ) -> Result<CheckedGenericArg<Symbolic>, SemanticError> {
        match arg {
            GenericArg::Dim(DimArg::Dimensionless(_)) => {
                Ok(CheckedGenericArg::Dim(Dimension::dimensionless()))
            }
            GenericArg::Dim(DimArg::Expr(dim_expr)) => {
                infer_hir_dim_expr_arg(dim_expr, self.tir, self.src).map(CheckedGenericArg::Dim)
            }
            GenericArg::Index(index) => {
                inferred_index_from_type_arg(index, self.src).map(CheckedGenericArg::Index)
            }
            GenericArg::Nat(nat) => Ok(CheckedGenericArg::Nat(nat.value.clone())),
            GenericArg::Type(value_type) => self
                .infer_hir_generic_type_arg(value_type)
                .map(CheckedGenericArg::Type),
        }
    }
}

fn inferred_index_from_type_arg(
    index: &IndexRef,
    src: SourceId,
) -> Result<IndexTypeRef<Symbolic>, SemanticError> {
    match index {
        IndexRef::Concrete(name) => Ok(IndexTypeRef::from_resolved(name.value.clone())),
        IndexRef::GenericParam(param) => Err(SemanticError::located(
            src,
            param.span,
            StructError::UnboundGenericInConcreteType {
                generic: Box::new(UnboundGeneric::NotConcretelyBound {
                    sort: GenericSort::Index,
                    name: param.value.name.clone(),
                }),
            },
        )),
        IndexRef::Finite(nat_expr) => IndexTypeRef::from_finite_index_form(nat_expr.value.clone())
            .map_err(|err| finite_index_error(err, src, nat_expr.span)),
    }
}

fn infer_hir_dim_expr_arg(
    dim_expr: &DimExpr,
    tir: &dyn crate::tir::typed::TirRead,
    src: SourceId,
) -> Result<Dimension, SemanticError> {
    dim_expr
        .terms
        .iter()
        .try_fold(Dimension::dimensionless(), |acc, item| {
            let (dim, power, span) = match &item.term.target {
                DimTermTarget::Dimension(target) => {
                    let dim = tir.dimension(&target.value).cloned().ok_or_else(|| {
                        SemanticError::located(
                            src,
                            target.span,
                            DimensionError::UnknownDimension {
                                name: NamePath::from(target.value.atom().clone()),
                            },
                        )
                    })?;
                    (dim, item.term.power, item.term.span)
                }
                DimTermTarget::Dimensionless => return Ok(acc),
                DimTermTarget::GenericParam(param) => {
                    return Err(SemanticError::located(
                        src,
                        param.span,
                        StructError::UnboundGenericInConcreteType {
                            generic: Box::new(UnboundGeneric::NotConcretelyBound {
                                sort: GenericSort::Dimension,
                                name: param.value.name.clone(),
                            }),
                        },
                    ));
                }
            };
            let powered = dim.pow(power).map_err(|_| {
                SemanticError::located(src, span, DimensionError::DimensionOverflow)
            })?;
            match item.op {
                crate::desugar::desugared_ast::MulDivOp::Mul => {
                    acc.checked_mul(&powered).map_err(|_| {
                        SemanticError::located(src, span, DimensionError::DimensionOverflow)
                    })
                }
                crate::desugar::desugared_ast::MulDivOp::Div => {
                    acc.checked_div(&powered).map_err(|_| {
                        SemanticError::located(src, span, DimensionError::DimensionOverflow)
                    })
                }
            }
        })
}

impl InferEnv<'_> {
    pub(super) fn resolve_applied_generic_args(
        &self,
        nominal: &crate::tir::typed::ResolvedNominal,
        applied_generic_args: &[GenericArg],
        span: Span,
    ) -> Result<Vec<CheckedGenericArg<Symbolic>>, SemanticError> {
        let type_def = nominal.definition();
        if applied_generic_args.is_empty() && type_def.generic_params().is_empty() {
            return Ok(Vec::new());
        }
        let total_params = type_def.generic_params().len();
        let defaulted = nominal
            .defaulted_generic_tail(applied_generic_args.len())
            .map_err(|expected| {
                SemanticError::located(
                    self.src,
                    span,
                    StructError::GenericArgCount {
                        type_name: type_def.name(),
                        expected,
                        got: applied_generic_args.len(),
                    },
                )
            })?;
        let mut args = Vec::with_capacity(total_params);
        for (param, arg) in type_def.generic_params().iter().zip(applied_generic_args) {
            let inferred = self.infer_hir_sorted_generic_arg(arg)?;
            if SortedGenericArg::of(param, &inferred).is_none() {
                return Err(generic_arg_internal_sort_error(param, self.src, arg.span()));
            }
            args.push(inferred);
        }
        for (_, resolved_default) in defaulted {
            let subs = generic_substitution_prefix(type_def, &args, self.src, span)?;
            args.push(
                instantiate_concrete_generic_arg(resolved_default, &subs, self.src)?.to_symbolic(),
            );
        }
        Ok(args)
    }
}
