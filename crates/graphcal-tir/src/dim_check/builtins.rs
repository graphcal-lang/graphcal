use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::Expr;
use graphcal_syntax::dimension::Dimension;

use graphcal_registry::builtins::{DimSignature, ParamDim, ResultDim};
use graphcal_registry::error::GraphcalError;
use graphcal_registry::registry::Registry;

pub(super) fn infer_fn_dim(
    sig: &DimSignature,
    arg_dims: &[Dimension],
    args: &[Expr],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<Dimension, GraphcalError> {
    let mut bindings: HashMap<&str, &Dimension> = HashMap::new();

    for (i, param) in sig.params.iter().enumerate() {
        match &param.dim {
            ParamDim::Fixed(expected) => {
                if arg_dims[i] != *expected {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: registry.dimensions.format_dimension(expected),
                        found: registry.dimensions.format_dimension(&arg_dims[i]),
                        src: src.clone(),
                        span: args[i].span.into(),
                        help: format!(
                            "parameter `{}` requires {}",
                            param.name,
                            registry.dimensions.format_dimension(expected),
                        ),
                    });
                }
            }
            ParamDim::Bind(var) => {
                bindings.insert(var, &arg_dims[i]);
            }
            ParamDim::Ref(var) => {
                let bound = bindings[var.as_str()];
                if arg_dims[i] != *bound {
                    let bind_param_name = sig
                        .params
                        .iter()
                        .find(|p| matches!(&p.dim, ParamDim::Bind(v) if v == var))
                        .map_or("?", |p| &p.name);
                    return Err(GraphcalError::DimensionMismatch {
                        expected: registry.dimensions.format_dimension(bound),
                        found: registry.dimensions.format_dimension(&arg_dims[i]),
                        src: src.clone(),
                        span: args[i].span.into(),
                        help: format!(
                            "parameter `{}` must have the same dimension as `{}`",
                            param.name, bind_param_name,
                        ),
                    });
                }
            }
        }
    }

    match &sig.result {
        ResultDim::Fixed(dim) => Ok(dim.clone()),
        ResultDim::Var(name) => Ok(bindings[name.as_str()].clone()),
        ResultDim::VarPow(name, power) => Ok(bindings[name.as_str()].pow(*power)),
    }
}
