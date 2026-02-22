use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::Expr;
use graphcal_syntax::dimension::{Dimension, Rational};

use crate::builtins::DimSignature;
use crate::error::GraphcalError;
use crate::registry::Registry;

pub(super) fn infer_fn_dim(
    sig: DimSignature,
    arg_dims: &[Dimension],
    args: &[Expr],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<Dimension, GraphcalError> {
    use graphcal_syntax::dimension::BaseDimId;

    match sig {
        DimSignature::AllDimensionless => {
            for (dim, arg) in arg_dims.iter().zip(args) {
                if !dim.is_dimensionless() {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Dimensionless".to_string(),
                        found: registry.dimensions.format_dimension(dim),
                        src: src.clone(),
                        span: arg.span.into(),
                        help: "this function requires Dimensionless arguments".to_string(),
                    });
                }
            }
            Ok(Dimension::dimensionless())
        }
        DimSignature::AngleToDimensionless => {
            let angle = Dimension::base(BaseDimId::Prelude("Angle".to_string()));
            if arg_dims[0] != angle {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Angle".to_string(),
                    found: registry.dimensions.format_dimension(&arg_dims[0]),
                    src: src.clone(),
                    span: args[0].span.into(),
                    help: "trigonometric functions require an Angle argument".to_string(),
                });
            }
            Ok(Dimension::dimensionless())
        }
        DimSignature::DimensionlessToAngle => {
            if !arg_dims[0].is_dimensionless() {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Dimensionless".to_string(),
                    found: registry.dimensions.format_dimension(&arg_dims[0]),
                    src: src.clone(),
                    span: args[0].span.into(),
                    help: "inverse trigonometric functions require a Dimensionless argument"
                        .to_string(),
                });
            }
            Ok(Dimension::base(BaseDimId::Prelude("Angle".to_string())))
        }
        DimSignature::Sqrt => {
            // Result dimension is arg^(1/2)
            Ok(arg_dims[0].pow(Rational::new(1, 2)))
        }
        DimSignature::Passthrough => Ok(arg_dims[0].clone()),
        DimSignature::SameDimension => {
            if arg_dims[0] != arg_dims[1] {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(&arg_dims[0]),
                    found: registry.dimensions.format_dimension(&arg_dims[1]),
                    src: src.clone(),
                    span: args[1].span.into(),
                    help: "both arguments must have the same dimension".to_string(),
                });
            }
            Ok(arg_dims[0].clone())
        }
        DimSignature::SameDimensionToAngle => {
            if arg_dims[0] != arg_dims[1] {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(&arg_dims[0]),
                    found: registry.dimensions.format_dimension(&arg_dims[1]),
                    src: src.clone(),
                    span: args[1].span.into(),
                    help: "both arguments must have the same dimension".to_string(),
                });
            }
            Ok(Dimension::base(BaseDimId::Prelude("Angle".to_string())))
        }
    }
}
