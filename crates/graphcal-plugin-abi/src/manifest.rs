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
//! This module validates only *wire shape*: bounded payload/list/name sizes,
//! JSON well-formedness, a known [`ABI_VERSION`](crate::ABI_VERSION), non-empty
//! names, positive denominators, non-zero powers, and no duplicate keys.
//! Everything the compiler can check better is left to the compiler.

use std::collections::HashSet;
use std::io::{self, Write};

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

impl ManifestFunction {
    /// Count the raw WebAssembly parameters required by this signature.
    ///
    /// Returns `None` only if counts overflow the host's address-sized
    /// arithmetic; such a manifest is invalid.
    #[must_use]
    pub fn abi_parameter_slots(&self) -> Option<usize> {
        let params = self.params.iter().try_fold(0_usize, |slots, param| {
            slots.checked_add(kind_abi_parameter_slots(&param.kind)?)
        })?;
        params.checked_add(match &self.result {
            ManifestValueKind::Array { .. } | ManifestValueKind::Struct { .. } => 1,
            ManifestValueKind::Quantity(_) | ManifestValueKind::Bool | ManifestValueKind::Int => 0,
        })
    }
}

const fn kind_abi_parameter_slots(kind: &ManifestValueKind) -> Option<usize> {
    match kind {
        ManifestValueKind::Quantity(_) | ManifestValueKind::Bool | ManifestValueKind::Int => {
            Some(1)
        }
        ManifestValueKind::Array { indexes, .. } => indexes.len().checked_add(1),
        // Structs are result-only after semantic conversion; retaining one
        // pointer slot here keeps malformed wire values bounded as well.
        ManifestValueKind::Struct { .. } => Some(1),
    }
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
/// JSON encoding is externally tagged: `{"quantity": {…}}`, `"bool"`, `"int"`,
/// `{"array": {"element": {…}, "indexes": ["I", "J"]}}`,
/// `{"struct": {"fields": [{"name": "root", "kind": {"quantity": {}}}]}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestValueKind {
    /// A quantity with the dimension described by the monomial.
    Quantity(ManifestMonomial),
    /// A boolean value (crosses the ABI as `1.0`/`0.0`).
    Bool,
    /// An integer value (crosses the ABI as an exactly-representable `f64`).
    Int,
    /// An array of quantities over one or more index variables (crosses the
    /// ABI as a dense row-major `f64` buffer plus one extent per axis).
    Array {
        /// The element dimension monomial.
        element: ManifestMonomial,
        /// Index variables in row-major axis order, each matching an entry in
        /// [`ManifestFunction::index_vars`]. Never empty after validation.
        indexes: Vec<String>,
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

/// The kind of one struct-result field: concrete quantities only (no
/// dimension variables), or `Bool`/`Int` with the usual slot encodings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestFieldKind {
    /// A quantity whose dimension is fixed (the monomial must have no
    /// dimension-variable factors).
    Quantity(ManifestMonomial),
    /// A boolean slot (`1.0`/`0.0`).
    Bool,
    /// An integer slot (exactly-representable `f64`).
    Int,
}

/// A dimension monomial: a product of dimension-variable powers and fixed
/// prelude base-dimension powers, e.g. `D1 * D2^(1/2) * Length^-1`.
///
/// The dimensionless quantity monomial is the empty product: `{"quantity": {}}`.
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

/// Version-only fallback probe used when full decoding fails, so that a
/// manifest written for a different (possibly shape-incompatible) ABI version
/// reports [`ManifestDecodeError::UnsupportedAbiVersion`] instead of a shape
/// error. Valid current-shape manifests do not need this second pass.
#[derive(Deserialize)]
struct VersionProbe {
    abi_version: u32,
}

/// JSON output sink that refuses to retain more than the manifest wire limit.
#[derive(Default)]
struct ManifestJsonWriter {
    bytes: Vec<u8>,
    exceeded_limit: bool,
}

impl Write for ManifestJsonWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > crate::MAX_MANIFEST_BYTES.saturating_sub(self.bytes.len()) {
            self.exceeded_limit = true;
            return Err(io::Error::other("plugin manifest exceeds its byte limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl PluginManifest {
    /// Encode this manifest as compact JSON, the byte payload of the
    /// [`MANIFEST_SECTION`](crate::MANIFEST_SECTION) custom section.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestEncodeError`] if JSON serialization fails or the
    /// encoded payload would exceed [`MAX_MANIFEST_BYTES`](crate::MAX_MANIFEST_BYTES).
    pub fn to_json(&self) -> Result<String, ManifestEncodeError> {
        let mut writer = ManifestJsonWriter::default();
        match serde_json::to_writer(&mut writer, self) {
            Ok(()) => String::from_utf8(writer.bytes).map_err(|err| ManifestEncodeError::Json {
                message: err.to_string(),
            }),
            Err(_) if writer.exceeded_limit => Err(ManifestEncodeError::PayloadTooLarge {
                max: crate::MAX_MANIFEST_BYTES,
            }),
            Err(err) => Err(ManifestEncodeError::Json {
                message: err.to_string(),
            }),
        }
    }

    /// Decode and shape-validate a manifest from its JSON payload.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestDecodeError`] when the payload is not valid JSON for
    /// the manifest shape, was written for a different ABI version, or
    /// violates a wire-shape rule.
    pub fn from_json(payload: &[u8]) -> Result<Self, ManifestDecodeError> {
        if payload.len() > crate::MAX_MANIFEST_BYTES {
            return Err(ManifestDecodeError::PayloadTooLarge {
                bytes: payload.len(),
                max: crate::MAX_MANIFEST_BYTES,
            });
        }
        let manifest = match serde_json::from_slice::<Self>(payload) {
            Ok(manifest) => manifest,
            Err(manifest_error) => {
                // Only malformed current-shape payloads take this bounded
                // fallback pass. It preserves the more useful ABI-version
                // diagnostic when a future version has an unknown shape,
                // while valid current manifests deserialize exactly once.
                if let Ok(probe) = serde_json::from_slice::<VersionProbe>(payload)
                    && probe.abi_version != crate::ABI_VERSION
                {
                    return Err(ManifestDecodeError::UnsupportedAbiVersion {
                        found: probe.abi_version,
                        supported: crate::ABI_VERSION,
                    });
                }
                return Err(ManifestDecodeError::Json {
                    message: manifest_error.to_string(),
                });
            }
        };
        if manifest.abi_version != crate::ABI_VERSION {
            return Err(ManifestDecodeError::UnsupportedAbiVersion {
                found: manifest.abi_version,
                supported: crate::ABI_VERSION,
            });
        }
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
        validate_list_count(
            self.functions.len(),
            crate::MAX_MANIFEST_FUNCTIONS,
            ManifestListRole::Functions,
            None,
        )?;
        if self.functions.is_empty() {
            return Err(ManifestValidationError::NoFunctions);
        }
        let mut function_names = HashSet::with_capacity(self.functions.len());
        for function in &self.functions {
            validate_name(&function.name, NameRole::Function, None)?;
            if !function_names.insert(function.name.as_str()) {
                return Err(ManifestValidationError::DuplicateFunction {
                    name: function.name.clone(),
                });
            }
            validate_function(function)?;
        }
        Ok(())
    }
}

fn validate_function(function: &ManifestFunction) -> Result<(), ManifestValidationError> {
    validate_list_count(
        function.dim_vars.len(),
        crate::MAX_MANIFEST_VARIABLES,
        ManifestListRole::DimensionVariables,
        Some(&function.name),
    )?;
    let mut seen_vars = HashSet::with_capacity(function.dim_vars.len());
    for var in &function.dim_vars {
        validate_name(var, NameRole::DimVar, Some(&function.name))?;
        if !seen_vars.insert(var.as_str()) {
            return Err(ManifestValidationError::DuplicateDimVar {
                function: function.name.clone(),
                var: var.clone(),
            });
        }
    }

    validate_list_count(
        function.index_vars.len(),
        crate::MAX_MANIFEST_VARIABLES,
        ManifestListRole::IndexVariables,
        Some(&function.name),
    )?;
    let mut seen_index_vars = HashSet::with_capacity(function.index_vars.len());
    for var in &function.index_vars {
        validate_name(var, NameRole::IndexVar, Some(&function.name))?;
        if !seen_index_vars.insert(var.as_str()) {
            return Err(ManifestValidationError::DuplicateIndexVar {
                function: function.name.clone(),
                var: var.clone(),
            });
        }
    }

    validate_list_count(
        function.params.len(),
        crate::MAX_MANIFEST_PARAMETERS,
        ManifestListRole::Parameters,
        Some(&function.name),
    )?;
    let mut seen_params = HashSet::with_capacity(function.params.len());
    for param in &function.params {
        validate_name(&param.name, NameRole::Param, Some(&function.name))?;
        if !seen_params.insert(param.name.as_str()) {
            return Err(ManifestValidationError::DuplicateParam {
                function: function.name.clone(),
                param: param.name.clone(),
            });
        }
        validate_kind(function, &param.kind)?;
    }

    validate_kind(function, &function.result)?;
    let slots = function.abi_parameter_slots().unwrap_or(usize::MAX);
    if slots > crate::MAX_ABI_FUNCTION_PARAMS {
        return Err(ManifestValidationError::TooManyAbiParameters {
            function: function.name.clone(),
            slots,
            max: crate::MAX_ABI_FUNCTION_PARAMS,
        });
    }
    Ok(())
}

fn validate_kind(
    function: &ManifestFunction,
    kind: &ManifestValueKind,
) -> Result<(), ManifestValidationError> {
    let monomial = match kind {
        ManifestValueKind::Quantity(monomial) => monomial,
        ManifestValueKind::Array { element, indexes } => {
            validate_list_count(
                indexes.len(),
                crate::MAX_MANIFEST_ARRAY_AXES,
                ManifestListRole::ArrayAxes,
                Some(&function.name),
            )?;
            if indexes.is_empty() {
                return Err(ManifestValidationError::EmptyArrayAxes {
                    function: function.name.clone(),
                });
            }
            for index in indexes {
                validate_name(index, NameRole::IndexVar, Some(&function.name))?;
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
    validate_list_count(
        fields.len(),
        crate::MAX_MANIFEST_STRUCT_FIELDS,
        ManifestListRole::StructFields,
        Some(&function.name),
    )?;
    if fields.is_empty() {
        return Err(ManifestValidationError::EmptyStruct {
            function: function.name.clone(),
        });
    }
    let mut seen_fields = HashSet::with_capacity(fields.len());
    for field in fields {
        validate_name(&field.name, NameRole::Field, Some(&function.name))?;
        if !seen_fields.insert(field.name.as_str()) {
            return Err(ManifestValidationError::DuplicateStructField {
                function: function.name.clone(),
                field: field.name.clone(),
            });
        }
        match &field.kind {
            ManifestFieldKind::Bool | ManifestFieldKind::Int => {}
            ManifestFieldKind::Quantity(monomial) => {
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
    let factor_count = monomial.vars.len().saturating_add(monomial.fixed.len());
    validate_list_count(
        factor_count,
        crate::MAX_MANIFEST_MONOMIAL_FACTORS,
        ManifestListRole::MonomialFactors,
        Some(&function.name),
    )?;

    let mut seen_vars = HashSet::with_capacity(monomial.vars.len());
    for factor in &monomial.vars {
        validate_name(&factor.var, NameRole::MonomialVar, Some(&function.name))?;
        if !seen_vars.insert(factor.var.as_str()) {
            return Err(ManifestValidationError::DuplicateMonomialVar {
                function: function.name.clone(),
                var: factor.var.clone(),
            });
        }
        validate_power(function, factor.pow)?;
    }

    let mut seen_dims = HashSet::with_capacity(monomial.fixed.len());
    for factor in &monomial.fixed {
        validate_name(&factor.dim, NameRole::FixedDim, Some(&function.name))?;
        if !seen_dims.insert(factor.dim.as_str()) {
            return Err(ManifestValidationError::DuplicateFixedDim {
                function: function.name.clone(),
                dim: factor.dim.clone(),
            });
        }
        validate_power(function, factor.pow)?;
    }
    Ok(())
}

fn validate_list_count(
    count: usize,
    max: usize,
    role: ManifestListRole,
    function: Option<&str>,
) -> Result<(), ManifestValidationError> {
    if count > max {
        return Err(ManifestValidationError::TooManyItems {
            function: function.map(str::to_owned),
            role,
            count,
            max,
        });
    }
    Ok(())
}

fn validate_name(
    name: &str,
    role: NameRole,
    function: Option<&str>,
) -> Result<(), ManifestValidationError> {
    if name.is_empty() {
        return Err(ManifestValidationError::EmptyName {
            function: function.map(str::to_owned),
            role,
        });
    }
    if name.len() > crate::MAX_MANIFEST_NAME_BYTES {
        return Err(ManifestValidationError::NameTooLong {
            function: function.map(str::to_owned),
            role,
            bytes: name.len(),
            max: crate::MAX_MANIFEST_NAME_BYTES,
        });
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

/// Which bounded manifest list exceeded its protocol count limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestListRole {
    /// The manifest's exported functions.
    Functions,
    /// A function's declared dimension variables.
    DimensionVariables,
    /// A function's declared index variables.
    IndexVariables,
    /// A function's named parameters.
    Parameters,
    /// One array kind's ordered axes.
    ArrayAxes,
    /// One struct result's flattened fields.
    StructFields,
    /// One dimension monomial's combined variable and fixed factors.
    MonomialFactors,
}

impl std::fmt::Display for ManifestListRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Functions => "functions",
            Self::DimensionVariables => "dimension variables",
            Self::IndexVariables => "index variables",
            Self::Parameters => "parameters",
            Self::ArrayAxes => "array axes",
            Self::StructFields => "struct fields",
            Self::MonomialFactors => "monomial factors",
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
    /// The encoded payload would exceed the protocol byte limit.
    #[error("plugin manifest JSON exceeds the encoded payload limit of {max} bytes")]
    PayloadTooLarge {
        /// Maximum accepted encoded payload size.
        max: usize,
    },
}

/// Error from [`PluginManifest::from_json`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ManifestDecodeError {
    /// The encoded payload exceeds the protocol byte limit.
    #[error("plugin manifest payload is {bytes} bytes, exceeding the limit of {max} bytes")]
    PayloadTooLarge {
        /// Encoded payload size received from the caller.
        bytes: usize,
        /// Maximum accepted encoded payload size.
        max: usize,
    },
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
    /// A bounded list has more entries than the protocol permits.
    #[error(
        "plugin manifest contains {count} {role}{}, exceeding the limit of {max}",
        function_context(function.as_deref())
    )]
    TooManyItems {
        /// The declaring function, when the list is inside one.
        function: Option<String>,
        /// Which typed list exceeded its limit.
        role: ManifestListRole,
        /// Number of decoded entries.
        count: usize,
        /// Maximum accepted entry count.
        max: usize,
    },
    /// A name slot holds an empty string.
    #[error("plugin manifest contains an empty {role}{}", function_context(function.as_deref()))]
    EmptyName {
        /// The declaring function, when the slot is inside one.
        function: Option<String>,
        /// Which name slot was empty.
        role: NameRole,
    },
    /// A name's UTF-8 encoding is longer than the protocol permits.
    #[error(
        "plugin manifest contains a {role} of {bytes} bytes{}, exceeding the limit of {max}",
        function_context(function.as_deref())
    )]
    NameTooLong {
        /// The declaring function, when the slot is inside one.
        function: Option<String>,
        /// Which name slot was oversized.
        role: NameRole,
        /// UTF-8 byte length of the decoded name.
        bytes: usize,
        /// Maximum accepted UTF-8 byte length.
        max: usize,
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
    /// An array kind declares no axes.
    #[error("function `{function}` declares an array with no axes")]
    EmptyArrayAxes {
        /// The declaring function.
        function: String,
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
    /// A function's lowered WebAssembly signature exceeds the host's
    /// compilation limit.
    #[error(
        "function `{function}` requires {slots} raw ABI parameter slots, exceeding the limit of {max}"
    )]
    TooManyAbiParameters {
        /// The declaring function.
        function: String,
        /// Raw WebAssembly parameter slots required by the signature.
        slots: usize,
        /// Maximum accepted raw parameter slots.
        max: usize,
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

    fn quantity_var(var: &str) -> ManifestValueKind {
        ManifestValueKind::Quantity(ManifestMonomial {
            vars: vec![ManifestVarPower {
                var: var.to_string(),
                pow: rational(1, 1),
            }],
            fixed: Vec::new(),
        })
    }

    fn scalar_function(name: impl Into<String>) -> ManifestFunction {
        ManifestFunction {
            name: name.into(),
            dim_vars: Vec::new(),
            index_vars: Vec::new(),
            params: Vec::new(),
            result: ManifestValueKind::Quantity(ManifestMonomial::default()),
        }
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
                        kind: quantity_var("D"),
                    },
                    ManifestParam {
                        name: "b".to_string(),
                        kind: quantity_var("D"),
                    },
                    ManifestParam {
                        name: "t".to_string(),
                        kind: ManifestValueKind::Quantity(ManifestMonomial::default()),
                    },
                ],
                result: quantity_var("D"),
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
                        kind: ManifestValueKind::Quantity(ManifestMonomial {
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
                result: ManifestValueKind::Quantity(ManifestMonomial {
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
    fn dimensionless_quantity_is_the_empty_monomial() {
        let json = r#"{"abi_version":4,"functions":[
            {"name":"tanh","params":[{"name":"x","kind":{"quantity":{}}}],"result":{"quantity":{}}}
        ]}"#;
        let manifest = PluginManifest::from_json(json.as_bytes()).unwrap();
        assert_eq!(
            manifest.functions[0].params[0].kind,
            ManifestValueKind::Quantity(ManifestMonomial::default())
        );
        assert!(manifest.functions[0].dim_vars.is_empty());

        let encoded = manifest.to_json().unwrap();
        assert!(encoded.contains(r#""quantity""#));
        assert!(!encoded.contains(r#""scalar""#));
    }

    #[test]
    fn legacy_v2_quantity_kind_is_rejected() {
        let json = r#"{"abi_version":4,"functions":[
            {"name":"tanh","params":[{"name":"x","kind":{"scalar":{}}}],"result":{"quantity":{}}}
        ]}"#;
        assert!(matches!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Json { .. }
        ));
    }

    #[test]
    fn unit_kinds_decode_from_bare_strings() {
        let json = r#"{"abi_version":4,"functions":[
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
        let json = r#"{"abi_version":5,"modules":{"totally":"different"}}"#;
        let err = PluginManifest::from_json(json.as_bytes()).unwrap_err();
        assert_eq!(
            err,
            ManifestDecodeError::UnsupportedAbiVersion {
                found: 5,
                supported: crate::ABI_VERSION,
            }
        );
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let json = r#"{"abi_version":4,"functions":[],"extra":true}"#;
        assert!(matches!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Json { .. }
        ));
    }

    #[test]
    fn encoded_payload_limit_is_checked_before_json_decoding() {
        let mut payload = lerp_manifest().to_json().unwrap().into_bytes();
        payload.resize(crate::MAX_MANIFEST_BYTES, b' ');
        PluginManifest::from_json(&payload).unwrap();

        payload.push(b' ');
        assert_eq!(
            PluginManifest::from_json(&payload).unwrap_err(),
            ManifestDecodeError::PayloadTooLarge {
                bytes: crate::MAX_MANIFEST_BYTES + 1,
                max: crate::MAX_MANIFEST_BYTES,
            }
        );
    }

    #[test]
    fn json_encoding_stops_at_the_payload_limit() {
        let mut manifest = lerp_manifest();
        manifest.functions[0].name = "x".repeat(crate::MAX_MANIFEST_BYTES);
        assert_eq!(
            manifest.to_json().unwrap_err(),
            ManifestEncodeError::PayloadTooLarge {
                max: crate::MAX_MANIFEST_BYTES,
            }
        );
    }

    #[test]
    fn accepts_the_function_limit_with_unique_names_and_rejects_one_more() {
        let mut manifest = PluginManifest {
            abi_version: crate::ABI_VERSION,
            functions: (0..crate::MAX_MANIFEST_FUNCTIONS)
                .map(|index| scalar_function(format!("f{index}")))
                .collect(),
        };
        let json = manifest.to_json().unwrap();
        PluginManifest::from_json(json.as_bytes()).unwrap();

        manifest.functions.push(scalar_function("overflow"));
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::TooManyItems {
                function: None,
                role: ManifestListRole::Functions,
                count: crate::MAX_MANIFEST_FUNCTIONS + 1,
                max: crate::MAX_MANIFEST_FUNCTIONS,
            })
        );
    }

    #[test]
    fn names_are_bounded_by_utf8_bytes() {
        let mut manifest = lerp_manifest();
        manifest.functions[0].params[0].name =
            "é".repeat(crate::MAX_MANIFEST_NAME_BYTES / "é".len());
        let json = manifest.to_json().unwrap();
        PluginManifest::from_json(json.as_bytes()).unwrap();

        manifest.functions[0].params[0].name.push('x');
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::NameTooLong {
                function: Some("lerp".to_string()),
                role: NameRole::Param,
                bytes: crate::MAX_MANIFEST_NAME_BYTES + 1,
                max: crate::MAX_MANIFEST_NAME_BYTES,
            })
        );
    }

    #[test]
    fn empty_function_list_is_rejected() {
        let json = r#"{"abi_version":4,"functions":[]}"#;
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
        let ManifestValueKind::Quantity(monomial) = &mut manifest.functions[0].params[0].kind
        else {
            panic!("expected quantity kind");
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
        let ManifestValueKind::Quantity(monomial) = &mut manifest.functions[0].params[2].kind
        else {
            panic!("expected quantity kind");
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
            let ManifestValueKind::Quantity(monomial) = &mut manifest.functions[0].params[0].kind
            else {
                panic!("expected quantity kind");
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
        let ManifestValueKind::Quantity(monomial) = &mut manifest.functions[0].params[0].kind
        else {
            panic!("expected quantity kind");
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
                            indexes: vec!["I".to_string()],
                        },
                    },
                    ManifestParam {
                        name: "window".to_string(),
                        kind: ManifestValueKind::Quantity(ManifestMonomial::default()),
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
                    indexes: vec!["I".to_string()],
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
    fn variable_lists_accept_the_limit_and_reject_one_more() {
        let mut manifest = PluginManifest {
            abi_version: crate::ABI_VERSION,
            functions: vec![scalar_function("bounded")],
        };
        manifest.functions[0].dim_vars = (0..crate::MAX_MANIFEST_VARIABLES)
            .map(|index| format!("D{index}"))
            .collect();
        let json = manifest.to_json().unwrap();
        PluginManifest::from_json(json.as_bytes()).unwrap();

        manifest.functions[0]
            .dim_vars
            .push("D_overflow".to_string());
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::TooManyItems {
                function: Some("bounded".to_string()),
                role: ManifestListRole::DimensionVariables,
                count: crate::MAX_MANIFEST_VARIABLES + 1,
                max: crate::MAX_MANIFEST_VARIABLES,
            })
        );

        manifest.functions[0].dim_vars.clear();
        manifest.functions[0].index_vars = (0..crate::MAX_MANIFEST_VARIABLES)
            .map(|index| format!("I{index}"))
            .collect();
        let json = manifest.to_json().unwrap();
        PluginManifest::from_json(json.as_bytes()).unwrap();

        manifest.functions[0]
            .index_vars
            .push("I_overflow".to_string());
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::TooManyItems {
                function: Some("bounded".to_string()),
                role: ManifestListRole::IndexVariables,
                count: crate::MAX_MANIFEST_VARIABLES + 1,
                max: crate::MAX_MANIFEST_VARIABLES,
            })
        );
    }

    #[test]
    fn arrays_without_axes_are_rejected() {
        let mut manifest = smooth_manifest();
        manifest.functions[0].result = ManifestValueKind::Array {
            element: ManifestMonomial::default(),
            indexes: Vec::new(),
        };
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::EmptyArrayAxes {
                function: "smooth".to_string(),
            })
        );
    }

    #[test]
    fn empty_array_index_names_are_rejected() {
        let mut manifest = smooth_manifest();
        manifest.functions[0].result = ManifestValueKind::Array {
            element: ManifestMonomial::default(),
            indexes: vec![String::new()],
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

    #[test]
    fn array_axes_accept_the_limit_and_reject_one_more() {
        let indexes: Vec<String> = (0..crate::MAX_MANIFEST_ARRAY_AXES)
            .map(|index| format!("I{index}"))
            .collect();
        let mut manifest = PluginManifest {
            abi_version: crate::ABI_VERSION,
            functions: vec![ManifestFunction {
                name: "array".to_string(),
                dim_vars: Vec::new(),
                index_vars: indexes.clone(),
                params: vec![ManifestParam {
                    name: "xs".to_string(),
                    kind: ManifestValueKind::Array {
                        element: ManifestMonomial::default(),
                        indexes,
                    },
                }],
                result: ManifestValueKind::Bool,
            }],
        };
        let json = manifest.to_json().unwrap();
        PluginManifest::from_json(json.as_bytes()).unwrap();

        let overflow = "I_overflow".to_string();
        manifest.functions[0].index_vars.push(overflow.clone());
        let ManifestValueKind::Array { indexes, .. } = &mut manifest.functions[0].params[0].kind
        else {
            panic!("test parameter must remain an array");
        };
        indexes.push(overflow);
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::TooManyItems {
                function: Some("array".to_string()),
                role: ManifestListRole::ArrayAxes,
                count: crate::MAX_MANIFEST_ARRAY_AXES + 1,
                max: crate::MAX_MANIFEST_ARRAY_AXES,
            })
        );
    }

    #[test]
    fn struct_fields_accept_the_limit_and_reject_one_more() {
        let fields = (0..crate::MAX_MANIFEST_STRUCT_FIELDS)
            .map(|index| ManifestField {
                name: format!("field{index}"),
                kind: ManifestFieldKind::Bool,
            })
            .collect();
        let mut manifest = PluginManifest {
            abi_version: crate::ABI_VERSION,
            functions: vec![ManifestFunction {
                result: ManifestValueKind::Struct { fields },
                ..scalar_function("record")
            }],
        };
        let json = manifest.to_json().unwrap();
        PluginManifest::from_json(json.as_bytes()).unwrap();

        let ManifestValueKind::Struct { fields } = &mut manifest.functions[0].result else {
            panic!("test result must remain a struct");
        };
        fields.push(ManifestField {
            name: "overflow".to_string(),
            kind: ManifestFieldKind::Int,
        });
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::TooManyItems {
                function: Some("record".to_string()),
                role: ManifestListRole::StructFields,
                count: crate::MAX_MANIFEST_STRUCT_FIELDS + 1,
                max: crate::MAX_MANIFEST_STRUCT_FIELDS,
            })
        );
    }

    #[test]
    fn monomial_factors_accept_the_limit_and_reject_one_more() {
        let fixed = (0..crate::MAX_MANIFEST_MONOMIAL_FACTORS)
            .map(|index| ManifestDimPower {
                dim: format!("Base{index}"),
                pow: rational(1, 1),
            })
            .collect();
        let mut manifest = PluginManifest {
            abi_version: crate::ABI_VERSION,
            functions: vec![ManifestFunction {
                result: ManifestValueKind::Quantity(ManifestMonomial {
                    vars: Vec::new(),
                    fixed,
                }),
                ..scalar_function("factors")
            }],
        };
        let json = manifest.to_json().unwrap();
        PluginManifest::from_json(json.as_bytes()).unwrap();

        let ManifestValueKind::Quantity(monomial) = &mut manifest.functions[0].result else {
            panic!("test result must remain a quantity");
        };
        monomial.fixed.push(ManifestDimPower {
            dim: "Overflow".to_string(),
            pow: rational(1, 1),
        });
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::TooManyItems {
                function: Some("factors".to_string()),
                role: ManifestListRole::MonomialFactors,
                count: crate::MAX_MANIFEST_MONOMIAL_FACTORS + 1,
                max: crate::MAX_MANIFEST_MONOMIAL_FACTORS,
            })
        );
    }

    #[test]
    fn raw_abi_parameter_limit_counts_scalar_arity() {
        let mut manifest = lerp_manifest();
        {
            let function = &mut manifest.functions[0];
            function.params = (0..crate::MAX_ABI_FUNCTION_PARAMS)
                .map(|index| ManifestParam {
                    name: format!("p{index}"),
                    kind: ManifestValueKind::Quantity(ManifestMonomial::default()),
                })
                .collect();
            assert_eq!(
                function.abi_parameter_slots(),
                Some(crate::MAX_ABI_FUNCTION_PARAMS)
            );
        }
        let json = manifest.to_json().unwrap();
        PluginManifest::from_json(json.as_bytes()).unwrap();

        manifest.functions[0].params.push(ManifestParam {
            name: "overflow".to_string(),
            kind: ManifestValueKind::Bool,
        });
        let json = manifest.to_json().unwrap();
        assert_eq!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::TooManyItems {
                function: Some("lerp".to_string()),
                role: ManifestListRole::Parameters,
                count: crate::MAX_MANIFEST_PARAMETERS + 1,
                max: crate::MAX_MANIFEST_PARAMETERS,
            })
        );
    }

    #[test]
    fn raw_abi_parameter_limit_counts_array_axes_and_result_pointer() {
        let mut manifest = smooth_manifest();
        let accepted_rank = crate::MAX_ABI_FUNCTION_PARAMS - 2;
        let indexes: Vec<String> = (0..accepted_rank)
            .map(|index| format!("I{index}"))
            .collect();
        {
            let function = &mut manifest.functions[0];
            function.index_vars.clone_from(&indexes);
            function.params = vec![ManifestParam {
                name: "xs".to_string(),
                kind: ManifestValueKind::Array {
                    element: ManifestMonomial::default(),
                    indexes: indexes.clone(),
                },
            }];
            function.result = ManifestValueKind::Array {
                element: ManifestMonomial::default(),
                indexes: indexes.clone(),
            };
            assert_eq!(
                function.abi_parameter_slots(),
                Some(crate::MAX_ABI_FUNCTION_PARAMS)
            );
        }
        let json = manifest.to_json().unwrap();
        PluginManifest::from_json(json.as_bytes()).unwrap();

        let overflow_index = format!("I{accepted_rank}");
        let function = &mut manifest.functions[0];
        function.index_vars.push(overflow_index.clone());
        let ManifestValueKind::Array { indexes, .. } = &mut function.params[0].kind else {
            panic!("test parameter must remain an array");
        };
        indexes.push(overflow_index);
        let json = manifest.to_json().unwrap();
        assert!(matches!(
            PluginManifest::from_json(json.as_bytes()).unwrap_err(),
            ManifestDecodeError::Invalid(ManifestValidationError::TooManyAbiParameters {
                slots,
                max,
                ..
            }) if slots == crate::MAX_ABI_FUNCTION_PARAMS + 1
                && max == crate::MAX_ABI_FUNCTION_PARAMS
        ));
    }
}
