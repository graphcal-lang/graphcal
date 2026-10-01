//! Inference of extern (plugin) function calls against their signatures.

use crate::hir::expr::{Expr, ExternFnRef};
use crate::outcome::Outcome;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::name::NameError;
use crate::semantic_error::plugin::PluginError;
use std::collections::HashMap;

use crate::graphcal_error::GraphcalError;
use crate::semantic::checked_type::{IndexTypeRef, StructTypeRef, Symbolic};
use crate::syntax::span::Span;

use crate::semantic::checked_type::CheckedType;
use crate::tir::dim_check::helpers::{expect_quantity, format_checked_type};

use super::context::Infer;

impl Infer<'_> {
    /// Check an extern (plugin) function call against its declared
    /// [`crate::function_signature::FunctionSignature`], using the same
    /// bind/check dimension-variable walk as built-in signatures.
    pub(super) fn infer_extern_fn_call(
        &self,
        ext: &ExternFnRef,
        callee_span: Span,
        args: &[Expr],
    ) -> Result<CheckedType<Symbolic>, Outcome<GraphcalError>> {
        use crate::function_signature::{ParamKind, ResultKind, ScalarValueKind};

        use crate::tir::dim_check::builtins::SignatureDimWalk;

        let Some(function) = self.env.tir.extern_functions().get(&ext.key()) else {
            return Err(GraphcalError::located(
                self.env.src,
                callee_span,
                PluginError::UnknownExternFunction {
                    alias: ext.alias.clone(),
                    name: ext.name.clone(),
                },
            )
            .into());
        };
        let sig = &function.signature;
        if args.len() != sig.arity() {
            return Err(GraphcalError::located(
                self.env.src,
                callee_span,
                NameError::WrongArity {
                    name: crate::semantic_error::name::CalledFunction::Extern(ext.name.clone()),
                    expected: sig.arity(),
                    got: args.len(),
                },
            )
            .into());
        }

        // Boundary rendering for diagnostics only.
        let display_name = ext.to_string();
        let mut dim_walk =
            SignatureDimWalk::new(&display_name, sig, self.env.registry, self.env.src);
        let mut index_bindings: HashMap<
            crate::function_signature::IndexBinder,
            IndexTypeRef<Symbolic>,
        > = HashMap::new();
        for (param, arg) in sig.params().iter().zip(args) {
            let arg_type = self.infer_arg(arg)?;
            match &param.kind {
                ParamKind::Scalar(ScalarValueKind::Bool) => {
                    if !matches!(arg_type, CheckedType::Bool) {
                        return Err(GraphcalError::located(
                            self.env.src,
                            arg.span,
                            DimensionError::DimensionMismatch {
                                expected: "Bool".to_string(),
                                found: format_checked_type(&arg_type, self.env.registry),
                                help: format!("parameter `{}` requires Bool", param.name),
                            },
                        )
                        .into());
                    }
                }
                ParamKind::Scalar(ScalarValueKind::Int) => {
                    if arg_type != CheckedType::Int {
                        return Err(GraphcalError::located(
                            self.env.src,
                            arg.span,
                            DimensionError::DimensionMismatch {
                                expected: "Int".to_string(),
                                found: format_checked_type(&arg_type, self.env.registry),
                                help: format!("parameter `{}` requires Int", param.name),
                            },
                        )
                        .into());
                    }
                }
                ParamKind::Scalar(ScalarValueKind::Quantity(monomial)) => {
                    let arg_dim =
                        expect_quantity(&arg_type, self.env.registry, self.env.src, arg.span)?;
                    dim_walk.check_quantity_param(&param.name, monomial, &arg_dim, arg.span)?;
                }
                ParamKind::Indexed { element, indexes } => {
                    let mut current = &arg_type;
                    let mut arg_indexes = Vec::with_capacity(indexes.len());
                    for _ in indexes {
                        let CheckedType::Indexed {
                            element,
                            index: arg_index,
                        } = current
                        else {
                            return Err(GraphcalError::located(self.env.src, arg.span, DimensionError::DimensionMismatch { expected: format!("a rank-{} indexed collection", indexes.len()), found: format_checked_type(&arg_type, self.env.registry), help: format!(
                                    "parameter `{}` of `{display_name}` takes one axis for each declared index variable",
                                    param.name
                                ) }).into());
                        };
                        arg_indexes.push(arg_index);
                        current = element;
                    }
                    match element {
                        ScalarValueKind::Quantity(monomial) => {
                            let Some(arg_dim) = current.quantity_dimension().cloned() else {
                                return Err(GraphcalError::located(self.env.src, arg.span, DimensionError::DimensionMismatch { expected: format!(
                                        "a rank-{} indexed quantity collection",
                                        indexes.len()
                                    ), found: format_checked_type(&arg_type, self.env.registry), help: format!(
                                        "parameter `{}` of `{display_name}` requires quantity elements",
                                        param.name
                                    ) }).into());
                            };
                            dim_walk.check_quantity_param(
                                &param.name,
                                monomial,
                                &arg_dim,
                                arg.span,
                            )?;
                        }
                        scalar @ (ScalarValueKind::Bool | ScalarValueKind::Int) => {
                            let name = if matches!(scalar, ScalarValueKind::Bool) {
                                "Bool"
                            } else {
                                "Int"
                            };
                            let matches = matches!(
                                (scalar, current),
                                (ScalarValueKind::Bool, CheckedType::Bool)
                                    | (ScalarValueKind::Int, CheckedType::Int)
                            );
                            if !matches {
                                return Err(GraphcalError::located(self.env.src, arg.span, DimensionError::DimensionMismatch { expected: format!(
                                        "{name} with exactly {} indexed axes",
                                        indexes.len()
                                    ), found: format_checked_type(&arg_type, self.env.registry), help: format!(
                                        "parameter `{}` of `{display_name}` requires {name} elements",
                                        param.name
                                    ) }).into());
                            }
                        }
                    }
                    for (index, arg_index) in indexes.iter().zip(arg_indexes) {
                        match index_bindings.entry(index.clone()) {
                            std::collections::hash_map::Entry::Vacant(slot) => {
                                slot.insert(arg_index.clone());
                            }
                            std::collections::hash_map::Entry::Occupied(bound) => {
                                if bound.get() != arg_index {
                                    return Err(GraphcalError::located(self.env.src, arg.span, DimensionError::DimensionMismatch { expected: format!(
                                            "an axis over `{}` (index variable `{index}` was bound by an earlier argument)",
                                            bound.get()
                                        ), found: format_checked_type(&arg_type, self.env.registry), help: format!(
                                            "axes sharing index variable `{index}` of `{display_name}` must use the same typed index"
                                        ) }).into());
                                }
                            }
                        }
                    }
                }
            }
        }

        match sig.result() {
            ResultKind::Value(ParamKind::Scalar(ScalarValueKind::Bool)) => Ok(CheckedType::Bool),
            ResultKind::Value(ParamKind::Scalar(ScalarValueKind::Int)) => Ok(CheckedType::Int),
            ResultKind::Value(ParamKind::Scalar(ScalarValueKind::Quantity(monomial))) => {
                Ok(dim_walk
                    .result(monomial, callee_span)
                    .map(CheckedType::Quantity)?)
            }
            ResultKind::Value(ParamKind::Indexed { element, indexes }) => {
                let leaf = match element {
                    ScalarValueKind::Quantity(monomial) => dim_walk
                        .result(monomial, callee_span)
                        .map(CheckedType::Quantity)?,
                    ScalarValueKind::Bool => CheckedType::Bool,
                    ScalarValueKind::Int => CheckedType::Int,
                };
                indexes.iter().rev().try_fold(leaf, |element, index| {
                let Some(bound) = index_bindings.get(index) else {
                    // try_new guarantees every result index variable indexes
                    // some parameter, so this is a compiler bug.
                    return Err(GraphcalError::InternalError {
                        message: format!(
                            "result index variable `{index}` of `{display_name}` was not bound by any argument"
                        ),
                        src: self.env.src,
                        anchor: crate::diagnostic_anchor::DiagnosticAnchor::Source(callee_span),
                    });
                };
                Ok(CheckedType::Indexed {
                    element: Box::new(element),
                    index: bound.clone(),
                })
            }).map_err(Outcome::Failed)
            }
            // Extern struct returns are non-generic records, so the argument
            // list is empty.
            ResultKind::Struct(result_struct) => Ok(CheckedType::Struct(
                StructTypeRef::from_resolved(result_struct.record_type().clone()),
                Vec::new(),
            )),
        }
    }
}
