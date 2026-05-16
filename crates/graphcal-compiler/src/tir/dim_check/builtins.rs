use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::resolved_ast::Expr;
use crate::registry::builtins::{DimSignature, ParamDim, ResultDim};
use crate::registry::error::GraphcalError;
use crate::registry::types::Registry;
use crate::syntax::dimension::Dimension;
use crate::syntax::names::DimVarName;

pub(super) fn infer_fn_dim(
    fn_name: &str,
    sig: &DimSignature,
    arg_dims: &[Dimension],
    args: &[Expr],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<Dimension, GraphcalError> {
    let mut bindings: HashMap<DimVarName, &Dimension> = HashMap::new();

    for (i, param) in sig.params.iter().enumerate() {
        match &param.dim {
            ParamDim::Fixed(expected) => {
                if arg_dims[i] != *expected {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: registry.dimensions.format_dimension(expected),
                        found: registry.dimensions.format_dimension(&arg_dims[i]),
                        help: format!(
                            "parameter `{}` requires {}",
                            param.name,
                            registry.dimensions.format_dimension(expected),
                        ),
                        src: src.clone(),
                        span: args[i].span.into(),
                    });
                }
            }
            ParamDim::Bind(var) => {
                bindings.insert(var.clone(), &arg_dims[i]);
            }
            ParamDim::Ref(var) => {
                let bound = lookup_binding(&bindings, var, fn_name, src, args[i].span)?;
                if arg_dims[i] != *bound {
                    let bind_param_name = sig
                        .params
                        .iter()
                        .find(|p| matches!(&p.dim, ParamDim::Bind(v) if v == var))
                        .map_or("?", |p| &p.name);
                    return Err(GraphcalError::DimensionMismatch {
                        expected: registry.dimensions.format_dimension(bound),
                        found: registry.dimensions.format_dimension(&arg_dims[i]),
                        help: format!(
                            "parameter `{}` must have the same dimension as `{}`",
                            param.name, bind_param_name,
                        ),
                        src: src.clone(),
                        span: args[i].span.into(),
                    });
                }
            }
        }
    }

    // For result dims that reference a bind variable, the binding must already
    // have been populated above. A missing binding here means the signature is
    // malformed (a `Ref`/`Var`/`VarPow` without a matching `Bind`) — surface
    // this as an internal error rather than panicking.
    let result_span = args
        .first()
        .map_or(crate::syntax::span::Span::new(0, 0), |a| a.span);
    match &sig.result {
        ResultDim::Fixed(dim) => Ok(dim.clone()),
        ResultDim::Var(name) => {
            Ok(lookup_binding(&bindings, name, fn_name, src, result_span)?.clone())
        }
        ResultDim::VarPow(name, power) => {
            lookup_binding(&bindings, name, fn_name, src, result_span)?
                .pow(*power)
                .map_err(|_| GraphcalError::DimensionOverflow {
                    src: src.clone(),
                    span: result_span.into(),
                })
        }
    }
}

fn lookup_binding<'a>(
    bindings: &HashMap<DimVarName, &'a Dimension>,
    var: &DimVarName,
    fn_name: &str,
    src: &NamedSource<Arc<String>>,
    span: crate::syntax::span::Span,
) -> Result<&'a Dimension, GraphcalError> {
    bindings
        .get(var)
        .copied()
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!(
                "builtin `{fn_name}` references unbound dim variable `{var}` in its signature"
            ),
            src: src.clone(),
            span: span.into(),
        })
}
