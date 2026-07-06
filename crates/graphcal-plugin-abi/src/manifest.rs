//! The plugin manifest data model and its JSON codec.
//!
//! A [`PluginManifest`] is the wire form of the function signatures a plugin
//! provides: for each function, its declared dimension variables, named
//! parameters, and result, with dimensions written as monomials — products
//! of dimension-variable powers and fixed prelude base-dimension powers with
//! rational exponents. It mirrors the shape of the compiler's
//! `FunctionSignature` IR; the host converts between the two at the binary
//! boundary and the compiler's constructor enforces the semantic invariants
//! (variable binding discipline, known dimension names).
//!
//! This module validates only *wire shape*: JSON well-formedness, a known
//! [`ABI_VERSION`](crate::ABI_VERSION), non-empty names, positive
//! denominators, non-zero powers, and no duplicate keys. Everything the
//! compiler can check better is left to the compiler.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::section;

/// A plugin's embedded description of the functions it provides.
///
/// Serialized as JSON into the [`MANIFEST_SECTION`](crate::MANIFEST_SECTION)
/// custom section. Field order and array order are meaningful (declaration
/// order); the codec never reorders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    /// The ABI version the plugin was built against. Must equal
    /// [`ABI_VERSION`](crate::ABI_VERSION) to decode.
    pub abi_version: u32,
    /// The functions the plugin exports, in declaration order.
    pub functions: Vec<ManifestFunction>,
}

/// One plugin function's signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestFunction {
    /// Export name of the function inside the wasm module.
    pub name: String,
    /// Declared dimension variables, in declaration order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dim_vars: Vec<String>,
    /// Declared index variables, in declaration order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub index_vars: Vec<String>,
    /// Named parameters, in declaration order.
    pub params: Vec<ManifestParam>,
    /// The result kind.
    pub result: ManifestValueKind,
}

/// A named parameter and its value kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestParam {
    /// Parameter name (documentation and diagnostics on the host side).
    pub name: String,
    /// The value kind the parameter requires.
    pub kind: ManifestValueKind,
}

/// The kind of a parameter or result value.
///
/// JSON encoding is externally tagged: `{"scalar": {…}}`, `"bool"`, `"int"`,
/// `{"array": {"element": {…}, "index": "I"}}`,
/// `{"struct": {"fields": [{"name": "root", "kind": {"scalar": {}}}]}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestValueKind {
    /// A scalar with the dimension described by the monomial.
    Scalar(ManifestMonomial),
    /// A boolean value (crosses the ABI as `1.0`/`0.0`).
    Bool,
    /// An integer value (crosses the ABI as an exactly-representable `f64`).
    Int,
    /// An array of scalars over an index variable (crosses the ABI as a
    /// dense `f64` buffer in index order).
    Array {
        /// The element dimension monomial.
        element: ManifestMonomial,
        /// The index variable, matching an entry in
        /// [`ManifestFunction::index_vars`].
        index: String,
    },
    /// A record described by its flattened field layout (result-only;
    /// crosses the ABI as one `f64` slot per field, in declaration order).
    /// The shape is structural — the record's graphcal type name never
    /// enters the manifest.
    Struct {
        /// The named fields, in declaration order.
        fields: Vec<ManifestField>,
    },
}

/// One field of a struct result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestField {
    /// The field name (part of the calling contract).
    pub name: String,
    /// The field's kind.
    pub kind: ManifestFieldKind,
}

/// The kind of one struct-result field: concrete scalars only (no
/// dimension variables), or `Bool`/`Int` with the usual slot encodings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestFieldKind {
    /// A scalar whose dimension is fixed (the monomial must have no
    /// dimension-variable factors).
    Scalar(ManifestMonomial),
    /// A boolean slot (`1.0`/`0.0`).
    Bool,
    /// An integer slot (exactly-representable `f64`).
    Int,
}

/// A dimension monomial: a product of dimension-variable powers and fixed
/// prelude base-dimension powers, e.g. `D1 * D2^(1/2) * Length^-1`.
///
/// The dimensionless monomial is the empty product: `{"scalar": {}}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestMonomial {
    /// Dimension-variable factors, in declaration order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vars: Vec<ManifestVarPower>,
    /// Fixed base-dimension factors. Dimension names must be prelude *base*
    /// dimensions; the host rejects anything else when converting.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed: Vec<ManifestDimPower>,
}

/// One dimension-variable factor: `var^pow`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestVarPower {
    /// The dimension variable, matching an entry in
    /// [`ManifestFunction::dim_vars`].
    pub var: String,
    /// The rational exponent. Never zero.
    pub pow: ManifestRational,
}

/// One fixed base-dimension factor: `dim^pow`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestDimPower {
    /// A prelude base dimension name (e.g. `Length`).
    pub dim: String,
    /// The rational exponent. Never zero.
    pub pow: ManifestRational,
}

/// A rational exponent `num/den` with `den > 0`.
///
/// The wire form is not required to be reduced; the host normalizes on
/// conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestRational {
    /// Numerator; carries the sign.
    pub num: i32,
    /// Denominator; must be positive.
    pub den: i32,
}

/// Version-only probe decoded before the full manifest, so that a manifest
/// written for a different (possibly shape-incompatible) ABI version reports
/// [`ManifestDecodeError::UnsupportedAbiVersion`] instead of a shape error.
#[derive(Deserialize)]
struct VersionProbe {
    abi_version: u32,
}

impl PluginManifest {
    /// Encode this manifest as compact JSON, the byte payload of the
    /// [`MANIFEST_SECTION`](crate::MANIFEST_SECTION) custom section.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestEncodeError`] if JSON serialization fails.
    pub fn to_json(&self) -> Result<String, ManifestEncodeError> {
        serde_json::to_string(self).map_err(|err| ManifestEncodeError::Json {
            message: err.to_string(),
        })
    }

    /// Decode and shape-validate a manifest from its JSON payload.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestDecodeError`] when the payload is not valid JSON for
    /// the manifest shape, was written for a different ABI version, or
    /// violates a wire-shape rule.
    pub fn from_json(payload: &[u8]) -> Result<Self, ManifestDecodeError> {
        let probe: VersionProbe =
            serde_json::from_slice(payload).map_err(|err| ManifestDecodeError::Json {
                message: err.to_string(),
            })?;
        if probe.abi_version != crate::ABI_VERSION {
            return Err(ManifestDecodeError::UnsupportedAbiVersion {
                found: probe.abi_version,
                supported: crate::ABI_VERSION,
            });
        }
        let manifest: Self =
            serde_json::from_slice(payload).map_err(|err| ManifestDecodeError::Json {
                message: err.to_string(),
            })?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Extract and decode the manifest embedded in a `.wasm` binary.
    ///
    /// Reads only the module's section layout — no instantiation, no
    /// bytecode validation.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestFromWasmError`] when the binary is not a wasm
    /// module, carries no (or more than one) manifest section, or the
    /// payload fails to decode.
    pub fn from_wasm(wasm: &[u8]) -> Result<Self, ManifestFromWasmError> {
        let payload = section::extract_manifest(wasm)?;
        Ok(Self::from_json(payload)?)
    }

    /// Append this manifest to a `.wasm` binary as the
    /// [`MANIFEST_SECTION`](crate::MANIFEST_SECTION) custom section.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestEmbedError`] when encoding fails, the binary is not
    /// a wasm module, or it already contains a manifest section.
    pub fn embed_into(&self, wasm: &[u8]) -> Result<Vec<u8>, ManifestEmbedError> {
        let payload = self.to_json()?;
        Ok(section::embed_manifest(wasm, payload.as_bytes())?)
    }

    /// Enforce the wire-shape rules documented on [`ManifestValidationError`].
    fn validate(&self) -> Result<(), ManifestValidationError> {
        if self.functions.is_empty() {
            return Err(ManifestValidationError::NoFunctions);
        }
        let mut function_names: Vec<&str> = Vec::new();
        for function in &self.functions {
            if function.name.is_empty() {
                return Err(ManifestValidationError::EmptyName {
                    function: None,
                    role: NameRole::Function,
                });
            }
            if function_names.contains(&function.name.as_str()) {
                return Err(ManifestValidationError::DuplicateFunction {
                    name: function.name.clone(),
                });
            }
            function_names.push(&function.name);
            validate_function(function)?;
        }
        Ok(())
    }
}

fn validate_function(function: &ManifestFunction) -> Result<(), ManifestValidationError> {
    let mut seen_vars: Vec<&str> = Vec::new();
    for var in &function.dim_vars {
        if var.is_empty() {
            return Err(ManifestValidationError::EmptyName {
                function: Some(function.name.clone()),
                role: NameRole::DimVar,
            });
        }
        if seen_vars.contains(&var.as_str()) {
            return Err(ManifestValidationError::DuplicateDimVar {
                function: function.name.clone(),
                var: var.clone(),
            });
        }
        seen_vars.push(var);
    }

    let mut seen_index_vars: Vec<&str> = Vec::new();
    for var in &function.index_vars {
        if var.is_empty() {
            return Err(ManifestValidationError::EmptyName {
                function: Some(function.name.clone()),
                role: NameRole::IndexVar,
            });
        }
        if seen_index_vars.contains(&var.as_str()) {
            return Err(ManifestValidationError::DuplicateIndexVar {
                function: function.name.clone(),
                var: var.clone(),
            });
        }
        seen_index_vars.push(var);
    }

    let mut seen_params: Vec<&str> = Vec::new();
    for param in &function.params {
        if param.name.is_empty() {
            return Err(ManifestValidationError::EmptyName {
                function: Some(function.name.clone()),
                role: NameRole::Param,
            });
        }
        if seen_params.contains(&param.name.as_str()) {
            return Err(ManifestValidationError::DuplicateParam {
                function: function.name.clone(),
                param: param.name.clone(),
            });
        }
        seen_params.push(&param.name);
        validate_kind(function, &param.kind)?;
    }

    validate_kind(function, &function.result)
}

fn validate_kind(
    function: &ManifestFunction,
    kind: &ManifestValueKind,
) -> Result<(), ManifestValidationError> {
    let monomial = match kind {
        ManifestValueKind::Scalar(monomial) => monomial,
        ManifestValueKind::Array { element, index } => {
            if index.is_empty() {
                return Err(ManifestValidationError::EmptyName {
                    function: Some(function.name.clone()),
                    role: NameRole::IndexVar,
                });
            }
            element
        }
        ManifestValueKind::Struct { fields } => return validate_struct(function, fields),
        ManifestValueKind::Bool | ManifestValueKind::Int => return Ok(()),
    };

    validate_monomial(function, monomial)
}

fn validate_struct(
    function: &ManifestFunction,
    fields: &[ManifestField],
) -> Result<(), ManifestValidationError> {
    if fields.is_empty() {
        return Err(ManifestValidationError::EmptyStruct {
            function: function.name.clone(),
        });
    }
    let mut seen_fields: Vec<&str> = Vec::new();
    for field in fields {
        if field.name.is_empty() {
            return Err(ManifestValidationError::EmptyName {
                function: Some(function.name.clone()),
                role: NameRole::Field,
            });
        }
        if seen_fields.contains(&field.name.as_str()) {
            return Err(ManifestValidationError::DuplicateStructField {
                function: function.name.clone(),
                field: field.name.clone(),
            });
        }
        seen_fields.push(&field.name);
        match &field.kind {
            ManifestFieldKind::Bool | ManifestFieldKind::Int => {}
            ManifestFieldKind::Scalar(monomial) => {
                if !monomial.vars.is_empty() {
                    return Err(ManifestValidationError::StructFieldWithDimVars {
                        function: function.name.clone(),
                        field: field.name.clone(),
                    });
                }
                validate_monomial(function, monomial)?;
            }
        }
    }
    Ok(())
}

fn validate_monomial(
    function: &ManifestFunction,
    monomial: &ManifestMonomial,
) -> Result<(), ManifestValidationError> {
    let mut seen_vars: Vec<&str> = Vec::new();
    for factor in &monomial.vars {
        if factor.var.is_empty() {
            return Err(ManifestValidationError::EmptyName {
                function: Some(function.name.clone()),
                role: NameRole::MonomialVar,
            });
        }
        if seen_vars.contains(&factor.var.as_str()) {
            return Err(ManifestValidationError::DuplicateMonomialVar {
                function: function.name.clone(),
                var: factor.var.clone(),
            });
        }
        seen_vars.push(&factor.var);
        validate_power(function, factor.pow)?;
    }

    let mut seen_dims: Vec<&str> = Vec::new();
    for factor in &monomial.fixed {
        if factor.dim.is_empty() {
            return Err(ManifestValidationError::EmptyName {
                function: Some(function.name.clone()),
                role: NameRole::FixedDim,
            });
        }
        if seen_dims.contains(&factor.dim.as_str()) {
            return Err(ManifestValidationError::DuplicateFixedDim {
                function: function.name.clone(),
                dim: factor.dim.clone(),
            });
        }
        seen_dims.push(&factor.dim);
        validate_power(function, factor.pow)?;
    }
    Ok(())
}

fn validate_power(
    function: &ManifestFunction,
    pow: ManifestRational,
) -> Result<(), ManifestValidationError> {
    if pow.den <= 0 {
        return Err(ManifestValidationError::NonPositiveDenominator {
            function: function.name.clone(),
            num: pow.num,
            den: pow.den,
        });
    }
    if pow.num == 0 {
        return Err(ManifestValidationError::ZeroPower {
            function: function.name.clone(),
        });
    }
    Ok(())
}

/// Which name slot a wire-shape violation occurred in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameRole {
    /// A function's export name.
    Function,
    /// A declared dimension variable.
    DimVar,
    /// A declared or referenced index variable.
    IndexVar,
    /// A struct-result field name.
    Field,
    /// A parameter name.
    Param,
    /// A dimension variable referenced by a monomial factor.
    MonomialVar,
    /// A fixed base-dimension name in a monomial factor.
    FixedDim,
}

impl std::fmt::Display for NameRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Function => "function name",
            Self::DimVar => "dimension variable",
            Self::IndexVar => "index variable",
            Self::Field => "struct field name",
            Self::Param => "parameter name",
            Self::MonomialVar => "monomial dimension variable",
            Self::FixedDim => "fixed dimension name",
        })
    }
}

/// Error from [`PluginManifest::to_json`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ManifestEncodeError {
    /// JSON serialization failed.
    #[error("failed to encode plugin manifest as JSON: {message}")]
    Json {
        /// The serializer's error message.
        message: String,
    },
}

/// Error from [`PluginManifest::from_json`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ManifestDecodeError {
    /// The payload is not valid JSON for the manifest shape.
    #[error("plugin manifest is not valid manifest JSON: {message}")]
    Json {
        /// The deserializer's error message.
        message: String,
    },
    /// The manifest was written for a different ABI version.
    #[error(
        "plugin manifest uses ABI version {found}, but this graphcal supports version {supported}"
    )]
    UnsupportedAbiVersion {
        /// The version recorded in the manifest.
        found: u32,
        /// The version this crate speaks.
        supported: u32,
    },
    /// The manifest decoded but violates a wire-shape rule.
    #[error(transparent)]
    Invalid(#[from] ManifestValidationError),
}

/// Wire-shape violation inside a decoded manifest.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ManifestValidationError {
    /// The manifest declares no functions at all.
    #[error("plugin manifest declares no functions")]
    NoFunctions,
    /// Two functions share one export name.
    #[error("plugin manifest declares function `{name}` more than once")]
    DuplicateFunction {
        /// The repeated function name.
        name: String,
    },
    /// A name slot holds an empty string.
    #[error("plugin manifest contains an empty {role}{}", function_context(function.as_deref()))]
    EmptyName {
        /// The declaring function, when the slot is inside one.
        function: Option<String>,
        /// Which name slot was empty.
        role: NameRole,
    },
    /// A function declares the same dimension variable twice.
    #[error("function `{function}` declares dimension variable `{var}` more than once")]
    DuplicateDimVar {
        /// The declaring function.
        function: String,
        /// The repeated variable.
        var: String,
    },
    /// A function declares the same index variable twice.
    #[error("function `{function}` declares index variable `{var}` more than once")]
    DuplicateIndexVar {
        /// The declaring function.
        function: String,
        /// The repeated variable.
        var: String,
    },
    /// A struct result declares no fields.
    #[error("function `{function}` declares a struct result with no fields")]
    EmptyStruct {
        /// The declaring function.
        function: String,
    },
    /// A struct result repeats a field name.
    #[error("function `{function}` declares struct field `{field}` more than once")]
    DuplicateStructField {
        /// The declaring function.
        function: String,
        /// The repeated field name.
        field: String,
    },
    /// A struct field's monomial references dimension variables.
    #[error(
        "function `{function}` struct field `{field}` references dimension variables; struct-result fields must have fixed dimensions"
    )]
    StructFieldWithDimVars {
        /// The declaring function.
        function: String,
        /// The offending field.
        field: String,
    },
    /// A function declares the same parameter name twice.
    #[error("function `{function}` declares parameter `{param}` more than once")]
    DuplicateParam {
        /// The declaring function.
        function: String,
        /// The repeated parameter name.
        param: String,
    },
    /// One monomial references the same dimension variable twice.
    #[error("function `{function}` repeats dimension variable `{var}` within one monomial")]
    DuplicateMonomialVar {
        /// The declaring function.
        function: String,
        /// The repeated variable.
        var: String,
    },
    /// One monomial references the same fixed dimension twice.
    #[error("function `{function}` repeats fixed dimension `{dim}` within one monomial")]
    DuplicateFixedDim {
        /// The declaring function.
        function: String,
        /// The repeated dimension name.
        dim: String,
    },
    /// A rational exponent has a zero or negative denominator.
    #[error("function `{function}` uses exponent {num}/{den}, whose denominator is not positive")]
    NonPositiveDenominator {
        /// The declaring function.
        function: String,
        /// The exponent's numerator.
        num: i32,
        /// The exponent's non-positive denominator.
        den: i32,
    },
    /// A monomial factor carries a zero exponent.
    #[error("function `{function}` contains a monomial factor with a zero exponent")]
    ZeroPower {
        /// The declaring function.
        function: String,
    },
}

fn function_context(function: Option<&str>) -> String {
    function.map_or_else(String::new, |name| format!(" in function `{name}`"))
}

/// Error from [`PluginManifest::from_wasm`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ManifestFromWasmError {
    /// The binary's section layout was unreadable or held no unique manifest.
    #[error(transparent)]
    Section(#[from] crate::SectionError),
    /// The manifest payload failed to decode.
    #[error(transparent)]
    Decode(#[from] ManifestDecodeError),
}

/// Error from [`PluginManifest::embed_into`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ManifestEmbedError {
    /// The manifest failed to encode as JSON.
    #[error(transparent)]
    Encode(#[from] ManifestEncodeError),
    /// The target binary was not a wasm module or already holds a manifest.
    #[error(transparent)]
    Section(#[from] crate::SectionError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rational(num: i32, den: i32) -> ManifestRational {
        ManifestRational { num, den }
    }

    fn scalar_var(var: &str) -> ManifestValueKind {
        ManifestValueKind::Scalar(ManifestMonomial {
            vars: vec![ManifestVarPower {
                var: var.to_string(),
                pow: rational(1, 1),
            }],
            fixed: Vec::new(),
        })
    }

    fn lerp_manifest() -> PluginManifest {
        PluginManifest {
            abi_version: crate::ABI_VERSION,
            functions: vec![ManifestFunction {
                name: "lerp".to_string(),
                dim_vars: vec!["D".to_string()],
                index_vars: Vec::new(),
                params: vec![
                    ManifestParam {
                        name: "a".to_string(),
                        kind: scalar_var("D"),
                    },
                    ManifestParam {
                        name: "b".to_string(),
                        kind: scalar_var("D"),
                    },
                    ManifestParam {
                        name: "t".to_string(),
                        kind: ManifestValueKind::Scalar(ManifestMonomial::default()),
                    },
                ],
                result: scalar_var("D"),
            }],
        }
    }

    #[test]
    fn roundtrips_through_json() {
        let manifest = PluginManifest {
            abi_version: crate::ABI_VERSION,
            functions: vec![ManifestFunction {
                name: "density".to_string(),
                dim_vars: Vec::new(),
                index_vars: Vec::new(),
                params: vec![
                    ManifestParam {
                        name: "pressure".to_string(),
                        kind: ManifestValueKind::Scalar(ManifestMonomial {
                            vars: Vec::new(),
                            fixed: vec![
                                ManifestDimPower {
                                    dim: "Mass".to_string(),
                                    pow: rational(1, 1),
                                },
                                ManifestDimPower {
                                    dim: "Length".to_string(),
                                    pow: rational(-1, 1),
                                },
                                ManifestDimPower {
                                    dim: "Time".to_string(),
                                    pow: rational(-2, 1),
                                },
                            ],
                        }),
                    },
                    ManifestParam {
                        name: "steps".to_string(),
                        kind: ManifestValueKind::Int,
                    },
                    ManifestParam {
                        name: "clamp".to_string(),
                        kind: ManifestValueKind::Bool,
                    },
                ],
                result: ManifestValueKind::Scalar(ManifestMonomial {
                    vars: Vec::new(),
                    fixed: vec![
                        ManifestDimPower {
                            dim: "Mass".to_string(),
                            pow: rational(1, 1),
                        },
                        ManifestDimPower {
                            dim: "Length".to_string(),
                            pow: rational(-3, 1),
                        },
                    ],
                }),
            }],
        };
        let json = manifest.to_json().unwrap();
        let decoded = PluginManifest::from_json(json.as_bytes()).unwrap();
        assert_eq!(decoded, manifest);
    }

    #[test]
    fn dimensionless_scalar_is_the_empty_monomial() {
        let json = r#"{"abi_version":2,"functions":[
            {"name":"tanh","params":[{"name":"x","kind":{"scalar":{}}}],"result":{"scalar":{}}}
        ]}"#;
        let manifest = PluginManifest::from_json(json.as_bytes()).unwrap();
        assert_eq!(
            manifest.functions[0].params[0].kind,
            ManifestValueKind::Scalar(ManifestMonomial::default())
        );
        assert!(manifest.functions[0].dim_vars.is_empty());
    }

    #[test]
    fn unit_kinds_decode_from_bare_strings() {
        let json = r#"{"abi_version":2,"functions":[
            {"name":"f","params":[{"name":"n","kind":"int"},{"name":"b","kind":"bool"}],"result":"int"}
        ]}"#;
        let manifest = PluginManifest::from_json(json.as_bytes()).unwrap();
        assert_eq!(manifest.functions[0].params[0].kind, ManifestValueKind::Int);
        assert_eq!(
            manifest.functions[0].params[1].kind,
            ManifestValueKind::Bool
        );
        assert_eq!(manifest.functions[0].result, ManifestValueKind::Int);
    }

    #[test]
    fn future_abi_version_is_reported_before_shape_errors() {
        // The future shape is unknown; only `abi_version` must be readable.
        let json = r#"{"abi_version":3,"modules":{"totally":"different"}}"#;
        let err = PluginManifest::from_json(json.as_bytes()).unwrap_err();
        assert_eq!(
            err,
            ManifestDecodeError::UnsupportedAbiVersion {
                found: 3,
                supported: crate::ABI_VERSION,
            }
        );
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let json = r#"{"abi_version":2,"functions":[],"extra":true}"#;
        assert!(matches!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Json { .. }
        ));
    }

    #[test]
    fn empty_function_list_is_rejected() {
        let json = r#"{"abi_version":2,"functions":[]}"#;
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::NoFunctions)
        );
    }

    #[test]
    fn duplicate_function_names_are_rejected() {
        let mut manifest = lerp_manifest();
        manifest.functions.push(manifest.functions[0].clone());
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::DuplicateFunction {
                name: "lerp".to_string()
            })
        );
    }

    #[test]
    fn duplicate_dim_vars_are_rejected() {
        let mut manifest = lerp_manifest();
        manifest.functions[0].dim_vars.push("D".to_string());
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::DuplicateDimVar {
                function: "lerp".to_string(),
                var: "D".to_string()
            })
        );
    }

    #[test]
    fn duplicate_params_are_rejected() {
        let mut manifest = lerp_manifest();
        manifest.functions[0].params[1].name = "a".to_string();
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::DuplicateParam {
                function: "lerp".to_string(),
                param: "a".to_string()
            })
        );
    }

    #[test]
    fn duplicate_monomial_vars_are_rejected() {
        let mut manifest = lerp_manifest();
        let ManifestValueKind::Scalar(monomial) = &mut manifest.functions[0].params[0].kind else {
            panic!("expected scalar kind");
        };
        monomial.vars.push(ManifestVarPower {
            var: "D".to_string(),
            pow: rational(2, 1),
        });
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::DuplicateMonomialVar {
                function: "lerp".to_string(),
                var: "D".to_string()
            })
        );
    }

    #[test]
    fn duplicate_fixed_dims_are_rejected() {
        let mut manifest = lerp_manifest();
        let ManifestValueKind::Scalar(monomial) = &mut manifest.functions[0].params[2].kind else {
            panic!("expected scalar kind");
        };
        for _ in 0..2 {
            monomial.fixed.push(ManifestDimPower {
                dim: "Length".to_string(),
                pow: rational(1, 1),
            });
        }
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::DuplicateFixedDim {
                function: "lerp".to_string(),
                dim: "Length".to_string()
            })
        );
    }

    #[test]
    fn non_positive_denominators_are_rejected() {
        for den in [0, -2] {
            let mut manifest = lerp_manifest();
            let ManifestValueKind::Scalar(monomial) = &mut manifest.functions[0].params[0].kind
            else {
                panic!("expected scalar kind");
            };
            monomial.vars[0].pow = rational(1, den);
            let json = manifest.to_json().unwrap();
            assert_eq!(
                PluginManifest::from_json(json.as_bytes()).unwrap_err(),
                ManifestDecodeError::Invalid(ManifestValidationError::NonPositiveDenominator {
                    function: "lerp".to_string(),
                    num: 1,
                    den,
                })
            );
        }
    }

    #[test]
    fn zero_powers_are_rejected() {
        let mut manifest = lerp_manifest();
        let ManifestValueKind::Scalar(monomial) = &mut manifest.functions[0].params[0].kind else {
            panic!("expected scalar kind");
        };
        monomial.vars[0].pow = rational(0, 1);
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::ZeroPower {
                function: "lerp".to_string(),
            })
        );
    }

    #[test]
    fn empty_names_are_rejected_with_their_role() {
        let mut manifest = lerp_manifest();
        manifest.functions[0].params[0].name = String::new();
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::EmptyName {
                function: Some("lerp".to_string()),
                role: NameRole::Param,
            })
        );
    }

    #[test]
    fn embeds_into_and_extracts_from_a_wasm_module() {
        let manifest = lerp_manifest();
        let wasm = manifest.embed_into(&crate::section::EMPTY_MODULE).unwrap();
        let decoded = PluginManifest::from_wasm(&wasm).unwrap();
        assert_eq!(decoded, manifest);
    }

    fn smooth_manifest() -> PluginManifest {
        PluginManifest {
            abi_version: crate::ABI_VERSION,
            functions: vec![ManifestFunction {
                name: "smooth".to_string(),
                dim_vars: vec!["D".to_string()],
                index_vars: vec!["I".to_string()],
                params: vec![
                    ManifestParam {
                        name: "xs".to_string(),
                        kind: ManifestValueKind::Array {
                            element: ManifestMonomial {
                                vars: vec![ManifestVarPower {
                                    var: "D".to_string(),
                                    pow: rational(1, 1),
                                }],
                                fixed: Vec::new(),
                            },
                            index: "I".to_string(),
                        },
                    },
                    ManifestParam {
                        name: "window".to_string(),
                        kind: ManifestValueKind::Scalar(ManifestMonomial::default()),
                    },
                ],
                result: ManifestValueKind::Array {
                    element: ManifestMonomial {
                        vars: vec![ManifestVarPower {
                            var: "D".to_string(),
                            pow: rational(1, 1),
                        }],
                        fixed: Vec::new(),
                    },
                    index: "I".to_string(),
                },
            }],
        }
    }

    #[test]
    fn array_kinds_roundtrip_through_json() {
        let manifest = smooth_manifest();
        let json = manifest.to_json().unwrap();
        assert!(json.contains(r#""index_vars":["I"]"#), "{json}");
        assert!(json.contains(r#""array""#), "{json}");
        let decoded = PluginManifest::from_json(json.as_bytes()).unwrap();
        assert_eq!(decoded, manifest);
    }

    #[test]
    fn duplicate_index_vars_are_rejected() {
        let mut manifest = smooth_manifest();
        manifest.functions[0].index_vars = vec!["I".to_string(), "I".to_string()];
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::DuplicateIndexVar {
                function: "smooth".to_string(),
                var: "I".to_string(),
            })
        );
    }

    #[test]
    fn empty_array_index_names_are_rejected() {
        let mut manifest = smooth_manifest();
        manifest.functions[0].result = ManifestValueKind::Array {
            element: ManifestMonomial::default(),
            index: String::new(),
        };
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::EmptyName {
                function: Some("smooth".to_string()),
                role: NameRole::IndexVar,
            })
        );
    }
}
