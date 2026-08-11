//! Convert a JSON input file into parameter overrides (`HashMap<DeclName, Expr>`).
//!
//! The JSON schema uses expression strings for leaf values and
//! JSON objects for structs, tagged unions, and named-label indexed params.
//! Nominal constructor/index spellings are explicit in JSON. The prepared
//! evaluator supplies the expected parameter type and recursively validates the
//! resulting closed value; JSON never introduces an independent computation.
//!
//! ## Schema
//!
//! | JSON shape | Interpretation |
//! |---|---|
//! | `string` | Parsed as closed GCL value syntax via `parse_single_expr()` |
//! | `bool` | `ExprKind::Bool` |
//! | `number` (integer) | `ExprKind::Integer` |
//! | `number` (float) | `ExprKind::Number` (dimensionless) |
//! | `object` with `"variant"` | Tagged union variant (+ optional `"fields"`) |
//! | `object` with `"type"` and `"fields"` | Single-variant struct |
//! | `object` with `"index"` and `"entries"` | Named-label indexed param |

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::Deserialize;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;

use graphcal_compiler::syntax::ast::{
    Expr, ExprKind, FieldInit, Ident, IdentPath, MapEntry, MapEntryIndex, MapEntryKey,
};
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::index_name::{IndexEntryKey, IndexVariantName};
use graphcal_compiler::syntax::names::{NameAtom, NameAtomError, NamePath};
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::syntax::span::{Span, Spanned};
use graphcal_compiler::syntax::type_name::FieldName;

/// A synthetic span used for all AST nodes constructed from JSON input.
const SYNTH_SPAN: Span = Span::new(0, 0);

fn synth_ident_path(
    name: &str,
    param: &str,
    role: &'static str,
) -> Result<IdentPath, JsonInputError> {
    let segments = parse_name_atoms(name, param, role)?;
    Ok(IdentPath::new(segments.map(|name| Ident {
        name,
        span: SYNTH_SPAN,
    })))
}

fn synth_name_path(
    name: &str,
    param: &str,
    role: &'static str,
) -> Result<NamePath, JsonInputError> {
    parse_name_atoms(name, param, role).map(NamePath::new)
}

fn parse_name_atoms(
    name: &str,
    param: &str,
    role: &'static str,
) -> Result<NonEmpty<NameAtom>, JsonInputError> {
    let atoms = name
        .split('.')
        .map(|segment| {
            NameAtom::parse(segment).map_err(|reason| JsonInputError::InvalidName {
                param: param.to_string(),
                role,
                value: name.to_string(),
                reason,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    NonEmpty::try_from_vec(atoms).map_err(|_| JsonInputError::InvalidName {
        param: param.to_string(),
        role,
        value: name.to_string(),
        reason: NameAtomError::Empty,
    })
}

// ---------------------------------------------------------------------------
// Exact JSON boundary
// ---------------------------------------------------------------------------

/// A structured location in the JSON input.
///
/// Keeping path segments typed lets duplicate-key and number diagnostics retain
/// object/array structure without building and later splitting a composite key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonPath {
    segments: Vec<JsonPathSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum JsonPathSegment {
    ObjectKey(String),
    ArrayIndex(usize),
}

impl JsonPath {
    const fn root() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    fn object_key(&self, key: &str) -> Self {
        let mut path = self.clone();
        path.segments
            .push(JsonPathSegment::ObjectKey(key.to_string()));
        path
    }

    fn array_index(&self, index: usize) -> Self {
        let mut path = self.clone();
        path.segments.push(JsonPathSegment::ArrayIndex(index));
        path
    }
}

impl fmt::Display for JsonPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("$")?;
        self.segments.iter().try_for_each(|segment| match segment {
            JsonPathSegment::ObjectKey(key) => write!(formatter, "[{key:?}]"),
            JsonPathSegment::ArrayIndex(index) => write!(formatter, "[{index}]"),
        })
    }
}

/// Why an exact JSON number cannot cross into Graphcal's numeric domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonNumberError {
    /// Integer-syntax JSON numbers are accepted only in Graphcal's signed range.
    IntegerOutOfRange,
    /// A decimal/exponent token overflowed or otherwise lacked a finite binary64 value.
    FloatOutOfRange,
    /// A nonzero decimal/exponent token underflowed to binary64 zero.
    FloatUnderflow,
}

impl fmt::Display for JsonNumberError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::IntegerOutOfRange => "integer is outside the signed 64-bit range",
            Self::FloatOutOfRange => "decimal/exponent value is outside the finite binary64 range",
            Self::FloatUnderflow => "nonzero decimal/exponent value underflows to binary64 zero",
        })
    }
}

/// A number whose source syntax and target representation agree.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ExactJsonNumber {
    Integer(i64),
    FiniteFloat(f64),
}

/// JSON data after duplicate-key and exact-number validation.
#[derive(Debug, Clone, PartialEq)]
enum ExactJsonValue {
    String(String),
    Bool(bool),
    Number(ExactJsonNumber),
    Object(ExactJsonObject),
    Null,
    Array,
}

/// An object whose private constructor rejects duplicate keys.
#[derive(Debug, Clone, PartialEq)]
struct ExactJsonObject {
    entries: Vec<(String, ExactJsonValue)>,
}

impl ExactJsonObject {
    fn get(&self, key: &str) -> Option<&ExactJsonValue> {
        self.entries
            .iter()
            .find_map(|(candidate, value)| (candidate == key).then_some(value))
    }

    fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    fn keys(&self) -> impl Iterator<Item = &String> {
        self.entries.iter().map(|(key, _)| key)
    }

    fn iter(&self) -> impl Iterator<Item = (&String, &ExactJsonValue)> {
        self.entries.iter().map(|(key, value)| (key, value))
    }
}

impl ExactJsonValue {
    fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            Self::Bool(_) | Self::Number(_) | Self::Object(_) | Self::Null | Self::Array => None,
        }
    }

    const fn as_object(&self) -> Option<&ExactJsonObject> {
        match self {
            Self::Object(object) => Some(object),
            Self::String(_) | Self::Bool(_) | Self::Number(_) | Self::Null | Self::Array => None,
        }
    }
}

/// Temporary object entries preserving every occurrence and raw value token.
struct RawObjectEntries<'a>(Vec<(String, &'a RawValue)>);

impl<'de> Deserialize<'de> for RawObjectEntries<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RawObjectVisitor;

        impl<'de> Visitor<'de> for RawObjectVisitor {
            type Value = RawObjectEntries<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut entries = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    entries.push((key, map.next_value::<&'de RawValue>()?));
                }
                Ok(RawObjectEntries(entries))
            }
        }

        deserializer.deserialize_map(RawObjectVisitor)
    }
}

/// Temporary array entries used only to validate nested object keys/numbers.
struct RawArrayEntries<'a>(Vec<&'a RawValue>);

impl<'de> Deserialize<'de> for RawArrayEntries<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RawArrayVisitor;

        impl<'de> Visitor<'de> for RawArrayVisitor {
            type Value = RawArrayEntries<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON array")
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut entries = Vec::new();
                while let Some(value) = sequence.next_element::<&'de RawValue>()? {
                    entries.push(value);
                }
                Ok(RawArrayEntries(entries))
            }
        }

        deserializer.deserialize_seq(RawArrayVisitor)
    }
}

fn parse_exact_json(source: &str) -> Result<ExactJsonValue, JsonInputError> {
    let raw: &RawValue = serde_json::from_str(source)?;
    parse_exact_value(raw, &JsonPath::root())
}

fn parse_exact_value(raw: &RawValue, path: &JsonPath) -> Result<ExactJsonValue, JsonInputError> {
    let source = raw.get().trim_start();
    match source.as_bytes().first() {
        Some(b'"') => serde_json::from_str(source)
            .map(ExactJsonValue::String)
            .map_err(JsonInputError::from),
        Some(b'{') => parse_exact_object(source, path).map(ExactJsonValue::Object),
        Some(b'[') => {
            let entries: RawArrayEntries<'_> = serde_json::from_str(source)?;
            entries
                .0
                .into_iter()
                .enumerate()
                .try_for_each(|(index, value)| {
                    parse_exact_value(value, &path.array_index(index)).map(|_| ())
                })?;
            Ok(ExactJsonValue::Array)
        }
        Some(b't') => Ok(ExactJsonValue::Bool(true)),
        Some(b'f') => Ok(ExactJsonValue::Bool(false)),
        Some(b'n') => Ok(ExactJsonValue::Null),
        Some(_) => parse_exact_number(source, path).map(ExactJsonValue::Number),
        None => Err(JsonInputError::EmptyRawValue),
    }
}

fn parse_exact_object(source: &str, path: &JsonPath) -> Result<ExactJsonObject, JsonInputError> {
    let raw_entries: RawObjectEntries<'_> = serde_json::from_str(source)?;
    if let Some(key) = duplicate_key(&raw_entries.0) {
        return Err(JsonInputError::DuplicateKey {
            path: path.clone(),
            key,
        });
    }
    let entries = raw_entries
        .0
        .into_iter()
        .map(|(key, value)| {
            let child_path = path.object_key(&key);
            parse_exact_value(value, &child_path).map(|value| (key, value))
        })
        .collect::<Result<_, _>>()?;
    Ok(ExactJsonObject { entries })
}

fn duplicate_key(entries: &[(String, &RawValue)]) -> Option<String> {
    let mut seen = HashSet::new();
    entries
        .iter()
        .find_map(|(key, _)| (!seen.insert(key.as_str())).then(|| key.clone()))
}

fn parse_exact_number(source: &str, path: &JsonPath) -> Result<ExactJsonNumber, JsonInputError> {
    if !source.contains(['.', 'e', 'E']) {
        return source
            .parse::<i64>()
            .map(ExactJsonNumber::Integer)
            .map_err(|_| JsonInputError::InvalidNumber {
                path: path.clone(),
                value: source.to_string(),
                reason: JsonNumberError::IntegerOutOfRange,
            });
    }

    let value = source
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| JsonInputError::InvalidNumber {
            path: path.clone(),
            value: source.to_string(),
            reason: JsonNumberError::FloatOutOfRange,
        })?;
    let nonzero_significand = source.split(['e', 'E']).next().is_some_and(|significand| {
        significand
            .bytes()
            .any(|digit| matches!(digit, b'1'..=b'9'))
    });
    if value == 0.0 && nonzero_significand {
        return Err(JsonInputError::InvalidNumber {
            path: path.clone(),
            value: source.to_string(),
            reason: JsonNumberError::FloatUnderflow,
        });
    }
    Ok(ExactJsonNumber::FiniteFloat(value))
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur when converting a JSON input file to overrides.
#[derive(Debug)]
pub enum JsonInputError {
    /// The JSON file could not be parsed.
    Json(serde_json::Error),
    /// The top-level JSON value is not an object.
    TopLevelNotObject,
    /// A JSON-provided name was not a valid Graphcal leaf/path.
    InvalidName {
        param: String,
        role: &'static str,
        value: String,
        reason: NameAtomError,
    },
    /// A GCL expression string could not be parsed.
    ///
    /// Carries the typed [`graphcal_compiler::syntax::parser::ParseError`]
    /// (like the sibling `--set` path)
    /// instead of a pre-rendered message, so miette can render the span
    /// and source context.
    ParseFailed {
        param: String,
        source: Box<graphcal_compiler::syntax::parser::ParseError>,
    },
    /// A duplicate object key would otherwise be collapsed by a map representation.
    DuplicateKey {
        /// The object containing the repeated key.
        path: JsonPath,
        /// The repeated decoded key.
        key: String,
    },
    /// A JSON number cannot be represented under the explicit input policy.
    InvalidNumber {
        /// Structured location of the numeric token.
        path: JsonPath,
        /// Exact numeric source token.
        value: String,
        /// The failed numeric invariant.
        reason: JsonNumberError,
    },
    /// An empty raw value escaped `serde_json`'s syntax validation.
    EmptyRawValue,
    /// An unsupported JSON type was encountered (null or array).
    UnsupportedJsonType { param: String, kind: &'static str },
    /// The `"variant"` field is not a string.
    InvalidVariant { param: String },
    /// The `"fields"` field is not an object.
    InvalidFields { param: String },
    /// The `"type"` field is not a string.
    InvalidType { param: String },
    /// A struct object has `"fields"` but no `"type"` key.
    MissingTypeKey { param: String },
    /// The `"index"` field is not a string.
    InvalidIndex { param: String },
    /// The `"entries"` field is not an object.
    InvalidEntries { param: String },
    /// A JSON object has unrecognized structure.
    AmbiguousObject { param: String },
    /// A JSON object mixes mutually-exclusive shape keys.
    ConflictingObjectKeys { param: String },
    /// A JSON object contains a key that is not part of its declared shape.
    UnknownObjectKey { param: String, key: String },
}

impl fmt::Display for JsonInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(e) => write!(f, "invalid JSON: {e}"),
            Self::TopLevelNotObject => write!(f, "top-level JSON value must be an object"),
            Self::InvalidName {
                param,
                role,
                value,
                reason,
            } => {
                write!(f, "invalid {role} name `{value}` for `{param}`: {reason}")
            }
            Self::ParseFailed { param, source } => {
                write!(f, "failed to parse value for `{param}`: {source}")
            }
            Self::DuplicateKey { path, key } => {
                write!(f, "duplicate JSON key {key:?} in object at {path}")
            }
            Self::InvalidNumber {
                path,
                value,
                reason,
            } => {
                write!(f, "invalid JSON number {value:?} at {path}: {reason}")
            }
            Self::EmptyRawValue => write!(f, "invalid JSON: empty raw value"),
            Self::UnsupportedJsonType { param, kind } => {
                write!(f, "unsupported JSON type `{kind}` for `{param}`")
            }
            Self::InvalidVariant { param } => {
                write!(f, "`variant` field for `{param}` must be a string")
            }
            Self::InvalidFields { param } => {
                write!(f, "`fields` field for `{param}` must be an object")
            }
            Self::InvalidType { param } => {
                write!(f, "`type` field for `{param}` must be a string")
            }
            Self::MissingTypeKey { param } => {
                write!(
                    f,
                    "struct object for `{param}` has `fields` but no `type` key; \
                     add `\"type\": \"TypeName\"` to specify the struct type"
                )
            }
            Self::InvalidIndex { param } => {
                write!(f, "`index` field for `{param}` must be a string")
            }
            Self::InvalidEntries { param } => {
                write!(f, "`entries` field for `{param}` must be an object")
            }
            Self::AmbiguousObject { param } => {
                write!(
                    f,
                    "unrecognized JSON object shape for `{param}`; expected one of: \
                     {{\"variant\": ..., \"fields\": ...}}, \
                     {{\"type\": ..., \"fields\": ...}}, or \
                     {{\"index\": ..., \"entries\": ...}}"
                )
            }
            Self::ConflictingObjectKeys { param } => {
                write!(
                    f,
                    "JSON object for `{param}` mixes mutually-exclusive shape keys"
                )
            }
            Self::UnknownObjectKey { param, key } => {
                write!(f, "unknown key `{key}` in JSON object for `{param}`")
            }
        }
    }
}

impl std::error::Error for JsonInputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ParseFailed { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for JsonInputError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Convert a JSON input string into a `HashMap` of parameter overrides.
///
/// Constructor and index spellings must be explicit in JSON. Concrete expected
/// types, dimensions, completeness, and constraints are checked by the prepared evaluator.
pub fn json_to_overrides(json_str: &str) -> Result<HashMap<DeclName, Expr>, JsonInputError> {
    let json = parse_exact_json(json_str)?;
    let ExactJsonValue::Object(obj) = json else {
        return Err(JsonInputError::TopLevelNotObject);
    };

    let mut overrides = HashMap::new();
    for (name, value) in obj.iter() {
        let expr = convert_value(value, name)?;
        let decl_name =
            DeclName::try_new(name.clone()).map_err(|reason| JsonInputError::InvalidName {
                param: name.clone(),
                role: "parameter",
                value: name.clone(),
                reason,
            })?;
        overrides.insert(decl_name, expr);
    }
    Ok(overrides)
}

// ---------------------------------------------------------------------------
// Value conversion
// ---------------------------------------------------------------------------

/// Convert a single JSON value into an AST `Expr`.
fn convert_value(value: &ExactJsonValue, param_name: &str) -> Result<Expr, JsonInputError> {
    match value {
        ExactJsonValue::String(value) => convert_string(value, param_name),
        ExactJsonValue::Bool(value) => Ok(synth_expr(ExprKind::Bool(*value))),
        ExactJsonValue::Number(number) => Ok(convert_number(*number)),
        ExactJsonValue::Object(object) => convert_object(object, param_name),
        ExactJsonValue::Null => Err(JsonInputError::UnsupportedJsonType {
            param: param_name.to_string(),
            kind: "null",
        }),
        ExactJsonValue::Array => Err(JsonInputError::UnsupportedJsonType {
            param: param_name.to_string(),
            kind: "array",
        }),
    }
}

/// Parse a string as GCL value syntax; the prepared binding compiler later
/// rejects references, computations, and other non-closed forms.
fn convert_string(s: &str, param_name: &str) -> Result<Expr, JsonInputError> {
    graphcal_compiler::syntax::parser::Parser::new(s)
        .parse_single_expr()
        .map_err(|e| JsonInputError::ParseFailed {
            param: param_name.to_string(),
            source: Box::new(e),
        })
}

/// Convert an already validated JSON number to an AST numeric literal.
const fn convert_number(number: ExactJsonNumber) -> Expr {
    match number {
        ExactJsonNumber::Integer(value) => synth_expr(ExprKind::Integer(value)),
        ExactJsonNumber::FiniteFloat(value) => synth_expr(ExprKind::Number(value)),
    }
}

/// Dispatch a JSON object to the appropriate converter.
fn convert_object(obj: &ExactJsonObject, param_name: &str) -> Result<Expr, JsonInputError> {
    let has_variant = obj.contains_key("variant");
    let has_type = obj.contains_key("type");
    let has_index = obj.contains_key("index");
    let shape_count = [has_variant, has_type, has_index]
        .into_iter()
        .filter(|present| *present)
        .count();
    if shape_count > 1 {
        return Err(JsonInputError::ConflictingObjectKeys {
            param: param_name.to_string(),
        });
    }

    if has_variant {
        reject_unknown_keys(obj, param_name, &["variant", "fields"])?;
        convert_tagged_union(obj, param_name)
    } else if has_type && obj.contains_key("fields") {
        reject_unknown_keys(obj, param_name, &["type", "fields"])?;
        convert_struct(obj, param_name)
    } else if obj.contains_key("fields") {
        Err(JsonInputError::MissingTypeKey {
            param: param_name.to_string(),
        })
    } else if has_index && obj.contains_key("entries") {
        reject_unknown_keys(obj, param_name, &["index", "entries"])?;
        convert_indexed(obj, param_name)
    } else {
        Err(JsonInputError::AmbiguousObject {
            param: param_name.to_string(),
        })
    }
}

fn reject_unknown_keys(
    obj: &ExactJsonObject,
    param_name: &str,
    allowed: &[&str],
) -> Result<(), JsonInputError> {
    obj.keys()
        .find(|key| !allowed.contains(&key.as_str()))
        .map_or(Ok(()), |key| {
            Err(JsonInputError::UnknownObjectKey {
                param: param_name.to_string(),
                key: key.clone(),
            })
        })
}

/// Convert `{"variant": "Name", "fields": {...}}` to a `ConstructorCall` expr.
///
/// Also handles bare variants: `{"variant": "Nominal"}`.
fn convert_tagged_union(obj: &ExactJsonObject, param_name: &str) -> Result<Expr, JsonInputError> {
    let variant_name = obj
        .get("variant")
        .and_then(ExactJsonValue::as_str)
        .ok_or_else(|| JsonInputError::InvalidVariant {
            param: param_name.to_string(),
        })?;

    let fields = if let Some(fields_val) = obj.get("fields") {
        let fields_obj = fields_val
            .as_object()
            .ok_or_else(|| JsonInputError::InvalidFields {
                param: param_name.to_string(),
            })?;
        convert_field_inits(fields_obj, param_name)?
    } else {
        Vec::new()
    };

    Ok(synth_expr(ExprKind::ConstructorCall {
        callee: synth_ident_path(variant_name, param_name, "constructor")?,
        generic_args: Vec::new(),
        fields,
    }))
}

/// Convert `{"type": "TypeName", "fields": {"x": "...", ...}}` to a `ConstructorCall` expr.
///
/// The struct type name is provided explicitly via the `"type"` key.
fn convert_struct(obj: &ExactJsonObject, param_name: &str) -> Result<Expr, JsonInputError> {
    let type_name_str = obj
        .get("type")
        .and_then(ExactJsonValue::as_str)
        .ok_or_else(|| JsonInputError::InvalidType {
            param: param_name.to_string(),
        })?;

    let fields_obj = obj
        .get("fields")
        .and_then(ExactJsonValue::as_object)
        .ok_or_else(|| JsonInputError::InvalidFields {
            param: param_name.to_string(),
        })?;
    let fields = convert_field_inits(fields_obj, param_name)?;

    Ok(synth_expr(ExprKind::ConstructorCall {
        callee: synth_ident_path(type_name_str, param_name, "constructor")?,
        generic_args: Vec::new(),
        fields,
    }))
}

/// Convert `{"index": "IndexName", "entries": {"Variant": ..., ...}}` to a `MapLiteral` expr.
///
/// The index name is provided explicitly via the `"index"` key.
fn convert_indexed(obj: &ExactJsonObject, param_name: &str) -> Result<Expr, JsonInputError> {
    let index_name = obj
        .get("index")
        .and_then(ExactJsonValue::as_str)
        .ok_or_else(|| JsonInputError::InvalidIndex {
            param: param_name.to_string(),
        })?;
    let index_path = synth_name_path(index_name, param_name, "index")?;

    let entries_obj = obj
        .get("entries")
        .and_then(ExactJsonValue::as_object)
        .ok_or_else(|| JsonInputError::InvalidEntries {
            param: param_name.to_string(),
        })?;

    let entries = entries_obj
        .iter()
        .map(|(variant, value)| {
            let value_expr = convert_value(value, &format!("{param_name}[{variant}]"))?;
            // Overrides are lowered to HIR (which carries resolution inline),
            // so synthetic spans need no uniqueness tricks here.
            Ok(MapEntry {
                keys: NonEmpty::singleton(MapEntryKey {
                    index: Spanned::new(MapEntryIndex::Named(index_path.clone()), SYNTH_SPAN),
                    additional_index_spans: Vec::new(),
                    variant: Spanned::new(
                        IndexEntryKey::named(IndexVariantName::try_new(variant.clone()).map_err(
                            |reason| JsonInputError::InvalidName {
                                param: format!("{param_name}[{variant}]"),
                                role: "index variant",
                                value: variant.clone(),
                                reason,
                            },
                        )?),
                        SYNTH_SPAN,
                    ),
                }),
                value: value_expr,
            })
        })
        .collect::<Result<Vec<_>, JsonInputError>>()?;

    Ok(synth_expr(ExprKind::MapLiteral { entries }))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a JSON fields object into `Vec<FieldInit>`.
fn convert_field_inits(
    fields_obj: &ExactJsonObject,
    param_name: &str,
) -> Result<Vec<FieldInit>, JsonInputError> {
    fields_obj
        .iter()
        .map(|(field_name, field_val)| {
            let field_expr = convert_value(field_val, &format!("{param_name}.{field_name}"))?;
            let name = FieldName::try_new(field_name.clone()).map_err(|reason| {
                JsonInputError::InvalidName {
                    param: param_name.to_string(),
                    role: "field",
                    value: field_name.clone(),
                    reason,
                }
            })?;
            Ok(FieldInit {
                name: Spanned::new(name, SYNTH_SPAN),
                value: field_expr,
            })
        })
        .collect()
}

/// Create an `Expr` with a synthetic span.
const fn synth_expr(kind: ExprKind) -> Expr {
    Expr::new(kind, SYNTH_SPAN)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantity_with_units() {
        let overrides = json_to_overrides(r#"{"dry_mass": "1500.0 kg"}"#).unwrap();
        assert!(overrides.contains_key(&DeclName::expect_valid("dry_mass")));
        let expr = &overrides[&DeclName::expect_valid("dry_mass")];
        assert!(matches!(expr.kind, ExprKind::QuantityLiteral { .. }));
    }

    #[test]
    fn bool_value() {
        let overrides = json_to_overrides(r#"{"enabled": false}"#).unwrap();
        let expr = &overrides[&DeclName::expect_valid("enabled")];
        assert!(matches!(expr.kind, ExprKind::Bool(false)));
    }

    #[test]
    fn integer_value() {
        let overrides = json_to_overrides(r#"{"count": 42}"#).unwrap();
        let expr = &overrides[&DeclName::expect_valid("count")];
        assert!(matches!(expr.kind, ExprKind::Integer(42)));
    }

    #[test]
    fn dimensionless_float() {
        let overrides = json_to_overrides(r#"{"ratio": 3.33}"#).unwrap();
        let expr = &overrides[&DeclName::expect_valid("ratio")];
        assert!(matches!(expr.kind, ExprKind::Number(f) if (f - 3.33).abs() < f64::EPSILON));
    }

    #[test]
    fn struct_single_variant() {
        let json = r#"{"transfer": {"type": "TransferResult", "fields": {"dv1": "150.0 m/s", "dv2": "250.0 m/s"}}}"#;
        let overrides = json_to_overrides(json).unwrap();
        let expr = &overrides[&DeclName::expect_valid("transfer")];
        match &expr.kind {
            ExprKind::ConstructorCall { callee, fields, .. } => {
                assert_eq!(callee.as_bare().unwrap().name, "TransferResult");
                assert_eq!(fields.len(), 2);
            }
            other => panic!("expected ConstructorCall, got {other:?}"),
        }
    }

    #[test]
    fn struct_missing_type_key() {
        let json = r#"{"transfer": {"fields": {"dv1": "150.0 m/s"}}}"#;
        let result = json_to_overrides(json);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, JsonInputError::MissingTypeKey { .. }),
            "expected MissingTypeKey, got {err}"
        );
    }

    #[test]
    fn object_unknown_key_errors() {
        let json = r#"{"transfer": {"type": "TransferResult", "fields": {}, "extra": 1}}"#;
        let err = json_to_overrides(json).unwrap_err();
        assert!(matches!(err, JsonInputError::UnknownObjectKey { key, .. } if key == "extra"));
    }

    #[test]
    fn object_conflicting_shape_keys_error() {
        let json = r#"{"value": {"variant": "Ok", "type": "Result", "fields": {}}}"#;
        let err = json_to_overrides(json).unwrap_err();
        assert!(matches!(err, JsonInputError::ConflictingObjectKeys { .. }));
    }

    #[test]
    fn tagged_union_with_fields() {
        let json = r#"{"maneuver": {"variant": "LowThrust", "fields": {"thrust": "0.5 N", "duration": "3600.0 s"}}}"#;
        let overrides = json_to_overrides(json).unwrap();
        let expr = &overrides[&DeclName::expect_valid("maneuver")];
        match &expr.kind {
            ExprKind::ConstructorCall { callee, fields, .. } => {
                assert_eq!(callee.as_bare().unwrap().name, "LowThrust");
                assert_eq!(fields.len(), 2);
            }
            other => panic!("expected ConstructorCall, got {other:?}"),
        }
    }

    #[test]
    fn bare_variant_object() {
        let json = r#"{"status": {"variant": "Nominal"}}"#;
        let overrides = json_to_overrides(json).unwrap();
        let expr = &overrides[&DeclName::expect_valid("status")];
        match &expr.kind {
            ExprKind::ConstructorCall { callee, fields, .. } => {
                assert_eq!(callee.as_bare().unwrap().name, "Nominal");
                assert!(fields.is_empty());
            }
            other => panic!("expected ConstructorCall, got {other:?}"),
        }
    }

    #[test]
    fn bare_variant_string() {
        // String "Nominal" is parsed as a GCL expression, which produces a ConstRef
        // (PascalCase identifier). The evaluator handles this as a bare variant.
        let json = r#"{"status": "Nominal"}"#;
        let overrides = json_to_overrides(json).unwrap();
        assert!(overrides.contains_key(&DeclName::expect_valid("status")));
    }

    #[test]
    fn named_label_indexed() {
        let json = r#"{"delta_v": {"index": "Maneuver", "entries": {"Departure": "3.0 km/s", "Correction": "0.2 km/s", "Insertion": "2.0 km/s"}}}"#;
        let overrides = json_to_overrides(json).unwrap();
        let expr = &overrides[&DeclName::expect_valid("delta_v")];
        match &expr.kind {
            ExprKind::MapLiteral { entries } => {
                assert_eq!(entries.len(), 3);
                assert_eq!(entries[0].keys[0].index.value.to_string(), "Maneuver");
            }
            other => panic!("expected MapLiteral, got {other:?}"),
        }
    }

    #[test]
    fn ambiguous_object_rejected() {
        // A plain object with unrecognized keys should be rejected
        let json = r#"{"y": {"Departure": "10.0", "Correction": "9.5"}}"#;
        let result = json_to_overrides(json);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, JsonInputError::AmbiguousObject { .. }),
            "expected AmbiguousObject, got {err}"
        );
    }

    #[test]
    fn unsupported_null() {
        let result = json_to_overrides(r#"{"x": null}"#);
        assert!(result.is_err());
    }

    #[test]
    fn unsupported_array() {
        let result = json_to_overrides(r#"{"x": [1, 2, 3]}"#);
        assert!(result.is_err());
    }

    #[test]
    fn top_level_not_object() {
        let result = json_to_overrides(r#""not an object""#);
        assert!(matches!(result, Err(JsonInputError::TopLevelNotObject)));
    }

    #[test]
    fn expression_string_with_arithmetic() {
        let overrides = json_to_overrides(r#"{"x": "2.0 + 3.0"}"#).unwrap();
        let expr = &overrides[&DeclName::expect_valid("x")];
        assert!(matches!(expr.kind, ExprKind::BinOp { .. }));
    }

    #[test]
    fn structured_json_accepts_qualified_constructor_and_index_paths() {
        let json = r#"{
            "status": {"variant": "lib.Pick", "fields": {"distance": 1}},
            "series": {"index": "lib.Phase", "entries": {"Burn": 1, "Coast": 2}}
        }"#;
        let overrides = json_to_overrides(json).unwrap();

        match &overrides[&DeclName::expect_valid("status")].kind {
            ExprKind::ConstructorCall { callee, .. } => {
                let segments: Vec<_> = callee.segments().iter().map(|s| s.name.as_str()).collect();
                assert_eq!(segments, ["lib", "Pick"]);
            }
            other => panic!("expected ConstructorCall, got {other:?}"),
        }

        match &overrides[&DeclName::expect_valid("series")].kind {
            ExprKind::MapLiteral { entries } => {
                assert_eq!(entries[0].keys[0].index.value.to_string(), "lib.Phase");
            }
            other => panic!("expected MapLiteral, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_keys_are_rejected_at_every_object_level() {
        let top_level = json_to_overrides(r#"{"x": 1, "x": 2}"#).unwrap_err();
        assert!(matches!(
            top_level,
            JsonInputError::DuplicateKey { path, key }
                if path == JsonPath::root() && key == "x"
        ));

        let nested =
            json_to_overrides(r#"{"record": {"type": "Pair", "fields": {"x": 1, "x": 2}}}"#)
                .unwrap_err();
        assert!(matches!(
            nested,
            JsonInputError::DuplicateKey { path, key }
                if path.to_string() == "$[\"record\"][\"fields\"]" && key == "x"
        ));

        let decoded_duplicate = json_to_overrides(r#"{"x": 1, "\u0078": 2}"#).unwrap_err();
        assert!(matches!(
            decoded_duplicate,
            JsonInputError::DuplicateKey { path, key }
                if path == JsonPath::root() && key == "x"
        ));
    }

    #[test]
    fn integer_tokens_are_accepted_only_in_the_i64_domain() {
        for (source, expected) in [
            (r#"{"x": -9223372036854775808}"#, i64::MIN),
            (r#"{"x": 9223372036854775807}"#, i64::MAX),
        ] {
            let overrides = json_to_overrides(source).unwrap();
            assert!(
                matches!(overrides[&DeclName::expect_valid("x")].kind, ExprKind::Integer(value) if value == expected)
            );
        }

        for source in [
            r#"{"x": -9223372036854775809}"#,
            r#"{"x": 9223372036854775808}"#,
            r#"{"x": 18446744073709551615}"#,
            r#"{"x": -18446744073709551617}"#,
        ] {
            let error = json_to_overrides(source).unwrap_err();
            assert!(matches!(
                error,
                JsonInputError::InvalidNumber {
                    reason: JsonNumberError::IntegerOutOfRange,
                    ..
                }
            ));
        }
    }

    #[test]
    fn decimal_tokens_require_a_finite_non_underflowing_binary64_value() {
        let rounded = json_to_overrides(r#"{"x": 9223372036854775808.0}"#).unwrap();
        assert!(matches!(
            rounded[&DeclName::expect_valid("x")].kind,
            ExprKind::Number(value)
                if value.to_bits() == 9_223_372_036_854_775_808.0_f64.to_bits()
        ));

        assert!(matches!(
            json_to_overrides(r#"{"x": 1e309}"#).unwrap_err(),
            JsonInputError::InvalidNumber {
                reason: JsonNumberError::FloatOutOfRange,
                ..
            }
        ));
        assert!(matches!(
            json_to_overrides(r#"{"x": 1e-400}"#).unwrap_err(),
            JsonInputError::InvalidNumber {
                reason: JsonNumberError::FloatUnderflow,
                ..
            }
        ));
        assert!(json_to_overrides(r#"{"x": 0e-400}"#).is_ok());
    }

    #[test]
    fn duplicates_inside_an_unsupported_array_are_still_detected() {
        let error = json_to_overrides(r#"{"x": [{"field": 1, "field": 2}]}"#).unwrap_err();
        assert!(matches!(
            error,
            JsonInputError::DuplicateKey { path, key }
                if path.to_string() == "$[\"x\"][0]" && key == "field"
        ));
    }

    #[test]
    fn dotted_parameter_names_are_reported_not_panicked() {
        let result = json_to_overrides(r#"{"module.x": 1}"#);
        assert!(
            matches!(
                result,
                Err(JsonInputError::InvalidName {
                    role: "parameter",
                    ..
                })
            ),
            "expected InvalidName for dotted parameter key, got {result:?}",
        );
    }
}
