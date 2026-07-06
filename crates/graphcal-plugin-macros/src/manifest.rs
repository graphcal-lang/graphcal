//! Building the manifest JSON from the validated signature IR.
//!
//! The macro constructs the same [`PluginManifest`] model the host
//! decodes (via `graphcal-plugin-abi`) and serializes it with the ABI
//! crate's own codec, so the embedded bytes cannot drift from the wire
//! format by construction.

use graphcal_plugin_abi::{
    ManifestDimPower, ManifestFunction, ManifestMonomial, ManifestParam, ManifestRational,
    ManifestValueKind, ManifestVarPower, PluginManifest,
};
use proc_macro2::Span;

use crate::dims;
use crate::lower::{FieldKindIr, FunctionIr, KindIr, MonomialIr, PluginIr};
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
                kind: kind_to_manifest(&param.kind, param.name.span())?,
            })
        })
        .collect::<syn::Result<Vec<_>>>()?;
    Ok(ManifestFunction {
        name: function.name.to_string(),
        dim_vars: function.dim_vars.iter().map(ToString::to_string).collect(),
        index_vars: function
            .index_vars
            .iter()
            .map(ToString::to_string)
            .collect(),
        params,
        result: kind_to_manifest(&function.result, function.name.span())?,
    })
}

fn kind_to_manifest(kind: &KindIr, fallback_span: Span) -> syn::Result<ManifestValueKind> {
    Ok(match kind {
        KindIr::Bool => ManifestValueKind::Bool,
        KindIr::Int => ManifestValueKind::Int,
        KindIr::Scalar(monomial) => {
            ManifestValueKind::Scalar(monomial_to_manifest(monomial, fallback_span)?)
        }
        KindIr::Array { element, index } => ManifestValueKind::Array {
            element: monomial_to_manifest(element, fallback_span)?,
            index: index.to_string(),
        },
        KindIr::Struct(fields) => ManifestValueKind::Struct {
            fields: fields
                .iter()
                .map(|field| {
                    Ok(graphcal_plugin_abi::ManifestField {
                        name: field.name.to_string(),
                        kind: match &field.kind {
                            FieldKindIr::Bool => graphcal_plugin_abi::ManifestFieldKind::Bool,
                            FieldKindIr::Int => graphcal_plugin_abi::ManifestFieldKind::Int,
                            FieldKindIr::Scalar(monomial) => {
                                graphcal_plugin_abi::ManifestFieldKind::Scalar(
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
