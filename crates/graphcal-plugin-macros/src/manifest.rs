//! Building the manifest JSON from the validated signature IR.
//!
//! The macro constructs the same [`PluginManifest`] model the host
//! decodes (via `graphcal-plugin-abi`) and serializes it with the ABI
//! crate's own codec, so the embedded bytes cannot drift from the wire
//! format by construction.

use graphcal_plugin_abi::{
    ManifestDimPower, ManifestFunction, ManifestMonomial, ManifestParam, ManifestParamKind,
    ManifestRational, ManifestResultKind, ManifestVarPower, PluginManifest,
};
use proc_macro2::Span;

use crate::dims;
use crate::lower::{FieldKindIr, FunctionIr, MonomialIr, ParamKindIr, PluginIr, ResultKindIr};
use crate::rational::Rational;

/// Serialize the signature IR as the manifest JSON payload.
pub fn build_manifest_json(ir: &PluginIr) -> syn::Result<String> {
    let functions = ir
        .functions
        .iter()
        .map(function_to_manifest)
        .collect::<syn::Result<Vec<_>>>()?;
    let manifest = PluginManifest {
        abi_version: graphcal_plugin_abi::ABI_VERSION,
        functions,
    };
    manifest.to_json().map_err(|err| {
        syn::Error::new(
            Span::call_site(),
            format!("internal error: failed to encode the plugin manifest: {err}"),
        )
    })
}

fn function_to_manifest(function: &FunctionIr) -> syn::Result<ManifestFunction> {
    let params = function
        .params
        .iter()
        .map(|param| {
            Ok(ManifestParam {
                name: param.name.to_string(),
                kind: param_kind_to_manifest(&param.kind, param.name.span())?,
            })
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let manifest = ManifestFunction {
        name: function.name.to_string(),
        dim_vars: function.dim_vars.iter().map(ToString::to_string).collect(),
        index_vars: function
            .index_vars
            .iter()
            .map(ToString::to_string)
            .collect(),
        params,
        result: result_kind_to_manifest(&function.result, function.name.span())?,
    };
    let slots = manifest.abi_parameter_slots().unwrap_or(usize::MAX);
    if slots > graphcal_plugin_abi::MAX_ABI_FUNCTION_PARAMS {
        return Err(syn::Error::new(
            function.name.span(),
            format!(
                "function `{}` requires {slots} raw ABI parameter slots, exceeding the limit of {}",
                function.name,
                graphcal_plugin_abi::MAX_ABI_FUNCTION_PARAMS
            ),
        ));
    }
    Ok(manifest)
}

fn param_kind_to_manifest(
    kind: &ParamKindIr,
    fallback_span: Span,
) -> syn::Result<ManifestParamKind> {
    Ok(match kind {
        ParamKindIr::Bool => ManifestParamKind::Bool,
        ParamKindIr::Int => ManifestParamKind::Int,
        ParamKindIr::Quantity(monomial) => {
            ManifestParamKind::Quantity(monomial_to_manifest(monomial, fallback_span)?)
        }
        ParamKindIr::Array { element, indexes } => ManifestParamKind::Array {
            element: monomial_to_manifest(element, fallback_span)?,
            indexes: indexes.iter().map(ToString::to_string).collect(),
        },
    })
}

fn result_kind_to_manifest(
    kind: &ResultKindIr,
    fallback_span: Span,
) -> syn::Result<ManifestResultKind> {
    Ok(match kind {
        ResultKindIr::Bool => ManifestResultKind::Bool,
        ResultKindIr::Int => ManifestResultKind::Int,
        ResultKindIr::Quantity(monomial) => {
            ManifestResultKind::Quantity(monomial_to_manifest(monomial, fallback_span)?)
        }
        ResultKindIr::Array { element, indexes } => ManifestResultKind::Array {
            element: monomial_to_manifest(element, fallback_span)?,
            indexes: indexes.iter().map(ToString::to_string).collect(),
        },
        ResultKindIr::Struct(fields) => ManifestResultKind::Struct {
            fields: fields
                .iter()
                .map(|field| {
                    Ok(graphcal_plugin_abi::ManifestField {
                        name: field.name.to_string(),
                        kind: match &field.kind {
                            FieldKindIr::Bool => graphcal_plugin_abi::ManifestFieldKind::Bool,
                            FieldKindIr::Int => graphcal_plugin_abi::ManifestFieldKind::Int,
                            FieldKindIr::Quantity(monomial) => {
                                graphcal_plugin_abi::ManifestFieldKind::Quantity(
                                    monomial_to_manifest(monomial, field.name.span())?,
                                )
                            }
                        },
                    })
                })
                .collect::<syn::Result<Vec<_>>>()?,
        },
    })
}

fn monomial_to_manifest(
    monomial: &MonomialIr,
    fallback_span: Span,
) -> syn::Result<ManifestMonomial> {
    let vars = monomial
        .vars
        .iter()
        .map(|factor| {
            Ok(ManifestVarPower {
                var: factor.name.clone(),
                pow: rational_to_manifest(factor.power, factor.span)?,
            })
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let fixed = monomial
        .fixed
        .iter()
        .zip(dims::BASE_DIMENSION_NAMES)
        .filter(|(power, _)| !power.is_zero())
        .map(|(power, dim)| {
            Ok(ManifestDimPower {
                dim: dim.to_string(),
                pow: rational_to_manifest(*power, fallback_span)?,
            })
        })
        .collect::<syn::Result<Vec<_>>>()?;
    Ok(ManifestMonomial { vars, fixed })
}

fn rational_to_manifest(power: Rational, span: Span) -> syn::Result<ManifestRational> {
    let out_of_range = |part: &str| {
        syn::Error::new(
            span,
            format!(
                "dimension exponent {part} {num}/{den} does not fit the manifest's i32 range",
                num = power.num(),
                den = power.den()
            ),
        )
    };
    Ok(ManifestRational {
        num: i32::try_from(power.num()).map_err(|_| out_of_range("numerator"))?,
        den: i32::try_from(power.den()).map_err(|_| out_of_range("denominator"))?,
    })
}
