//! Converting decoded plugin manifests into the compiler's typed
//! [`FunctionSignature`] IR.
//!
//! This is the boundary where untrusted manifest strings become typed
//! compiler values: names go through the fallible name constructors,
//! dimension names resolve against the prelude *base* dimension alphabet,
//! and the assembled signature passes through
//! [`FunctionSignature::try_new`], which enforces the binding-discipline
//! invariants. Nothing downstream of this module handles manifest strings.

use graphcal_compiler::dimension::{Dimension, Rational, RationalError};
use graphcal_compiler::function_signature::{
    DimMonomial, DimVarPower, FunctionParam, FunctionSignature, SignatureError, StructFieldKind,
    StructShape, StructShapeField, ValueKind,
};
use graphcal_compiler::registry::prelude::{PRELUDE_BASE_DIMENSION_NAMES, prelude_base_dimension};
use graphcal_compiler::syntax::dimension::DimVarName;
use graphcal_compiler::syntax::function_name::{FnName, FnParamName};
use graphcal_compiler::syntax::index_name::IndexVarName;
use graphcal_compiler::syntax::names::NameAtomError;
use graphcal_plugin_abi::{
    ManifestField, ManifestFieldKind, ManifestFunction, ManifestMonomial, ManifestRational,
    ManifestValueKind, PluginManifest,
};
use thiserror::Error;

/// Convert every function in a decoded manifest to its typed signature.
///
/// # Errors
///
/// Returns [`ManifestConvertError`] naming the offending function when any
/// name is invalid, a dimension is not a prelude base dimension, exponent
/// arithmetic overflows, or the signature violates the binding invariants.
pub fn convert_manifest(
    manifest: &PluginManifest,
) -> Result<Vec<(FnName, FunctionSignature)>, ManifestConvertError> {
    manifest.functions.iter().map(convert_function).collect()
}

/// Convert one manifest function to its typed signature.
///
/// # Errors
///
/// See [`convert_manifest`].
fn convert_function(
    function: &ManifestFunction,
) -> Result<(FnName, FunctionSignature), ManifestConvertError> {
    let in_function = |kind: ConvertErrorKind| ManifestConvertError {
        function: function.name.clone(),
        kind,
    };

    let name = FnName::try_new(function.name.clone()).map_err(|source| {
        in_function(ConvertErrorKind::InvalidFunctionName {
            name: function.name.clone(),
            source,
        })
    })?;
    let dim_vars = function
        .dim_vars
        .iter()
        .map(|var| convert_dim_var(var))
        .collect::<Result<Vec<_>, _>>()
        .map_err(&in_function)?;
    let index_vars = function
        .index_vars
        .iter()
        .map(|var| convert_index_var(var))
        .collect::<Result<Vec<_>, _>>()
        .map_err(&in_function)?;
    let params = function
        .params
        .iter()
        .map(|param| {
            let name = FnParamName::try_new(param.name.clone()).map_err(|source| {
                ConvertErrorKind::InvalidParamName {
                    name: param.name.clone(),
                    source,
                }
            })?;
            let kind = convert_kind(&param.kind)?;
            Ok(FunctionParam { name, kind })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(&in_function)?;
    let result = convert_kind(&function.result).map_err(&in_function)?;

    let signature = FunctionSignature::try_new(dim_vars, index_vars, params, result)
        .map_err(|source| in_function(ConvertErrorKind::Signature(source)))?;
    Ok((name, signature))
}

fn convert_dim_var(var: &str) -> Result<DimVarName, ConvertErrorKind> {
    DimVarName::try_new(var.to_string()).map_err(|source| ConvertErrorKind::InvalidDimVarName {
        name: var.to_string(),
        source,
    })
}

fn convert_index_var(var: &str) -> Result<IndexVarName, ConvertErrorKind> {
    IndexVarName::try_new(var.to_string()).map_err(|source| ConvertErrorKind::InvalidIndexVarName {
        name: var.to_string(),
        source,
    })
}

fn convert_kind(kind: &ManifestValueKind) -> Result<ValueKind, ConvertErrorKind> {
    match kind {
        ManifestValueKind::Bool => Ok(ValueKind::Bool),
        ManifestValueKind::Int => Ok(ValueKind::Int),
        ManifestValueKind::Scalar(monomial) => Ok(ValueKind::Scalar(convert_monomial(monomial)?)),
        ManifestValueKind::Array { element, index } => Ok(ValueKind::Indexed {
            element: convert_monomial(element)?,
            index: convert_index_var(index)?,
        }),
        ManifestValueKind::Struct { fields } => {
            let fields = fields
                .iter()
                .map(convert_struct_field)
                .collect::<Result<Vec<_>, _>>()?;
            let shape = StructShape::try_new(fields).map_err(ConvertErrorKind::Signature)?;
            Ok(ValueKind::Struct(shape))
        }
    }
}

fn convert_struct_field(field: &ManifestField) -> Result<StructShapeField, ConvertErrorKind> {
    let name = graphcal_compiler::syntax::type_name::FieldName::try_new(field.name.clone())
        .map_err(|source| ConvertErrorKind::InvalidFieldName {
            name: field.name.clone(),
            source,
        })?;
    let kind = match &field.kind {
        ManifestFieldKind::Bool => StructFieldKind::Bool,
        ManifestFieldKind::Int => StructFieldKind::Int,
        ManifestFieldKind::Scalar(monomial) => {
            // Wire validation already rejects dimension-variable factors in
            // struct fields, so the converted monomial is concrete.
            let monomial = convert_monomial(monomial)?;
            StructFieldKind::Scalar(monomial.fixed)
        }
    };
    Ok(StructShapeField { name, kind })
}

fn convert_monomial(monomial: &ManifestMonomial) -> Result<DimMonomial, ConvertErrorKind> {
    let vars = monomial
        .vars
        .iter()
        .map(|factor| {
            Ok(DimVarPower {
                var: convert_dim_var(&factor.var)?,
                power: convert_rational(factor.pow)?,
            })
        })
        .collect::<Result<Vec<_>, ConvertErrorKind>>()?;

    let mut fixed = Dimension::dimensionless();
    for factor in &monomial.fixed {
        let base = prelude_base_dimension(&factor.dim).ok_or_else(|| {
            ConvertErrorKind::UnknownBaseDimension {
                dim: factor.dim.clone(),
            }
        })?;
        let powered = base.pow(convert_rational(factor.pow)?)?;
        fixed = fixed.checked_mul(&powered)?;
    }

    Ok(DimMonomial { vars, fixed })
}

fn convert_rational(pow: ManifestRational) -> Result<Rational, ConvertErrorKind> {
    Ok(Rational::try_new(pow.num, pow.den)?)
}

/// Error converting a manifest function to a typed signature.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("plugin manifest function `{function}`: {kind}")]
pub struct ManifestConvertError {
    /// The manifest function that failed to convert.
    pub function: String,
    /// What went wrong.
    pub kind: ConvertErrorKind,
}

/// The ways a manifest function can fail to convert.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConvertErrorKind {
    /// The function's export name is not a valid graphcal function name.
    #[error("`{name}` is not a valid function name: {source}")]
    InvalidFunctionName {
        /// The rejected name.
        name: String,
        /// Why the name was rejected.
        source: NameAtomError,
    },
    /// A declared dimension variable is not a valid name.
    #[error("`{name}` is not a valid dimension variable name: {source}")]
    InvalidDimVarName {
        /// The rejected name.
        name: String,
        /// Why the name was rejected.
        source: NameAtomError,
    },
    /// A declared or referenced index variable is not a valid name.
    #[error("`{name}` is not a valid index variable name: {source}")]
    InvalidIndexVarName {
        /// The rejected name.
        name: String,
        /// Why the name was rejected.
        source: NameAtomError,
    },
    /// A struct field name is not a valid name.
    #[error("`{name}` is not a valid field name: {source}")]
    InvalidFieldName {
        /// The rejected name.
        name: String,
        /// Why the name was rejected.
        source: NameAtomError,
    },
    /// A parameter name is not a valid name.
    #[error("`{name}` is not a valid parameter name: {source}")]
    InvalidParamName {
        /// The rejected name.
        name: String,
        /// Why the name was rejected.
        source: NameAtomError,
    },
    /// A fixed dimension is not one of the prelude base dimensions.
    #[error(
        "`{dim}` is not a prelude base dimension; manifests may only use: {}",
        PRELUDE_BASE_DIMENSION_NAMES.join(", ")
    )]
    UnknownBaseDimension {
        /// The unknown dimension name.
        dim: String,
    },
    /// Exponent arithmetic overflowed while assembling a dimension.
    #[error(transparent)]
    Rational(#[from] RationalError),
    /// The assembled signature violates the binding-discipline invariants.
    #[error(transparent)]
    Signature(#[from] SignatureError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_plugin_abi::{ManifestDimPower, ManifestParam, ManifestVarPower};

    fn rational(num: i32, den: i32) -> ManifestRational {
        ManifestRational { num, den }
    }

    fn scalar_var(var: &str, num: i32, den: i32) -> ManifestValueKind {
        ManifestValueKind::Scalar(ManifestMonomial {
            vars: vec![ManifestVarPower {
                var: var.to_string(),
                pow: rational(num, den),
            }],
            fixed: Vec::new(),
        })
    }

    fn fixed_dim(dim: &str, num: i32, den: i32) -> ManifestValueKind {
        ManifestValueKind::Scalar(ManifestMonomial {
            vars: Vec::new(),
            fixed: vec![ManifestDimPower {
                dim: dim.to_string(),
                pow: rational(num, den),
            }],
        })
    }

    fn param(name: &str, kind: ManifestValueKind) -> ManifestParam {
        ManifestParam {
            name: name.to_string(),
            kind,
        }
    }

    #[test]
    fn converts_a_dim_generic_function() {
        let function = ManifestFunction {
            name: "root".to_string(),
            dim_vars: vec!["D".to_string()],
            index_vars: Vec::new(),
            params: vec![param("x", scalar_var("D", 1, 1))],
            result: scalar_var("D", 1, 2),
        };
        let (name, signature) = convert_function(&function).unwrap();
        assert_eq!(name.as_str(), "root");
        assert!(
            signature
                .structurally_equivalent(&FunctionSignature::free_to_pow("value", Rational::HALF))
        );
    }

    #[test]
    fn converts_fixed_base_dimensions() {
        let function = ManifestFunction {
            name: "period".to_string(),
            dim_vars: Vec::new(),
            index_vars: Vec::new(),
            params: vec![param("length", fixed_dim("Length", 1, 1))],
            result: fixed_dim("Time", 1, 1),
        };
        let (_, signature) = convert_function(&function).unwrap();
        let expected = FunctionSignature::fixed_to_fixed(
            "x",
            prelude_base_dimension("Length").unwrap(),
            prelude_base_dimension("Time").unwrap(),
        );
        assert!(signature.structurally_equivalent(&expected));
    }

    #[test]
    fn rejects_derived_prelude_dimensions() {
        // `Velocity` exists in the prelude but is derived; manifests must
        // spell it structurally as Length * Time^-1.
        let function = ManifestFunction {
            name: "speed".to_string(),
            dim_vars: Vec::new(),
            index_vars: Vec::new(),
            params: vec![param("x", fixed_dim("Velocity", 1, 1))],
            result: ManifestValueKind::Scalar(ManifestMonomial::default()),
        };
        let err = convert_function(&function).unwrap_err();
        assert_eq!(err.function, "speed");
        assert!(matches!(
            err.kind,
            ConvertErrorKind::UnknownBaseDimension { dim } if dim == "Velocity"
        ));
    }

    #[test]
    fn rejects_invalid_names() {
        let function = ManifestFunction {
            name: "has.dot".to_string(),
            dim_vars: Vec::new(),
            index_vars: Vec::new(),
            params: Vec::new(),
            result: ManifestValueKind::Int,
        };
        assert!(matches!(
            convert_function(&function).unwrap_err().kind,
            ConvertErrorKind::InvalidFunctionName { .. }
        ));
    }

    #[test]
    fn rejects_use_before_binding_signatures() {
        // x: D^2 before any bare binding of D — the compiler invariant fires.
        let function = ManifestFunction {
            name: "sq".to_string(),
            dim_vars: vec!["D".to_string()],
            index_vars: Vec::new(),
            params: vec![param("x", scalar_var("D", 2, 1))],
            result: scalar_var("D", 1, 1),
        };
        assert!(matches!(
            convert_function(&function).unwrap_err().kind,
            ConvertErrorKind::Signature(SignatureError::UseBeforeBinding { .. })
        ));
    }
}
