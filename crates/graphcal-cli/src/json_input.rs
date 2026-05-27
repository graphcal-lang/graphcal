//! Convert a JSON input file into parameter overrides (`HashMap<DeclName, Expr>`).
//!
//! The JSON schema uses expression strings for scalar leaves and
//! JSON objects for structs, tagged unions, and named-label indexed params.
//! All type information is provided explicitly in the JSON — no AST lookup is needed.
//! Type and dimension validation is deferred to the evaluator.
//!
//! ## Schema
//!
//! | JSON shape | Interpretation |
//! |---|---|
//! | `string` | Parsed as GCL expression via `parse_single_expr()` |
//! | `bool` | `ExprKind::Bool` |
//! | `number` (integer) | `ExprKind::Integer` |
//! | `number` (float) | `ExprKind::Number` (dimensionless) |
//! | `object` with `"variant"` | Tagged union variant (+ optional `"fields"`) |
//! | `object` with `"type"` and `"fields"` | Single-variant struct |
//! | `object` with `"index"` and `"entries"` | Named-label indexed param |

use std::collections::HashMap;
use std::fmt;

use graphcal_compiler::syntax::ast::{
    Expr, ExprKind, FieldInit, MapEntry, MapEntryIndex, MapEntryKey,
};
use graphcal_compiler::syntax::names::{
    ConstructorName, DeclName, FieldName, IndexName, IndexVariantName,
};
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::syntax::span::{Span, Spanned};

/// A synthetic span used for all AST nodes constructed from JSON input.
const SYNTH_SPAN: Span = Span::new(0, 0);

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
    /// A GCL expression string could not be parsed.
    ParseFailed { param: String, message: String },
    /// A JSON number is neither i64 nor f64.
    InvalidNumber { param: String },
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
}

impl fmt::Display for JsonInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(e) => write!(f, "invalid JSON: {e}"),
            Self::TopLevelNotObject => write!(f, "top-level JSON value must be an object"),
            Self::ParseFailed { param, message } => {
                write!(f, "failed to parse value for `{param}`: {message}")
            }
            Self::InvalidNumber { param } => {
                write!(f, "invalid number for `{param}`")
            }
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
        }
    }
}

impl std::error::Error for JsonInputError {}

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
/// All type information (struct type names, index names) must be provided
/// explicitly in the JSON. Type and dimension validation is deferred to the evaluator.
pub fn json_to_overrides(json_str: &str) -> Result<HashMap<DeclName, Expr>, JsonInputError> {
    let json: serde_json::Value = serde_json::from_str(json_str)?;
    let obj = json.as_object().ok_or(JsonInputError::TopLevelNotObject)?;

    let mut overrides = HashMap::new();
    for (name, value) in obj {
        let expr = convert_value(value, name)?;
        overrides.insert(DeclName::new(name), expr);
    }
    Ok(overrides)
}

// ---------------------------------------------------------------------------
// Value conversion
// ---------------------------------------------------------------------------

/// Convert a single JSON value into an AST `Expr`.
fn convert_value(value: &serde_json::Value, param_name: &str) -> Result<Expr, JsonInputError> {
    match value {
        serde_json::Value::String(s) => convert_string(s, param_name),
        serde_json::Value::Bool(b) => Ok(synth_expr(ExprKind::Bool(*b))),
        serde_json::Value::Number(n) => convert_number(n, param_name),
        serde_json::Value::Object(obj) => convert_object(obj, param_name),
        serde_json::Value::Null => Err(JsonInputError::UnsupportedJsonType {
            param: param_name.to_string(),
            kind: "null",
        }),
        serde_json::Value::Array(_) => Err(JsonInputError::UnsupportedJsonType {
            param: param_name.to_string(),
            kind: "array",
        }),
    }
}

/// Parse a string as a GCL expression.
fn convert_string(s: &str, param_name: &str) -> Result<Expr, JsonInputError> {
    graphcal_compiler::syntax::parser::Parser::new(s)
        .parse_single_expr()
        .map_err(|e| JsonInputError::ParseFailed {
            param: param_name.to_string(),
            message: e.to_string(),
        })
}

/// Convert a JSON number to an integer or dimensionless float.
///
/// Tries, in order: `i64` → `u64` (converted to `i64`) → `f64`.
/// Returns an error if no representation works, or if a `u64` value
/// exceeds `i64::MAX` (which would lose precision when cast to `f64`).
fn convert_number(n: &serde_json::Number, param_name: &str) -> Result<Expr, JsonInputError> {
    // Try i64 first (most common integer case).
    if let Some(i) = n.as_i64() {
        return Ok(synth_expr(ExprKind::Integer(i)));
    }
    // Try u64 for large unsigned integers (e.g., 9_999_999_999_999_999_999).
    if let Some(u) = n.as_u64() {
        return i64::try_from(u).map_or_else(
            |_| {
                Err(JsonInputError::InvalidNumber {
                    param: param_name.to_string(),
                })
            },
            |i| Ok(synth_expr(ExprKind::Integer(i))),
        );
    }
    // Fall back to f64 for floating-point values.
    n.as_f64().map_or_else(
        || {
            Err(JsonInputError::InvalidNumber {
                param: param_name.to_string(),
            })
        },
        |f| Ok(synth_expr(ExprKind::Number(f))),
    )
}

/// Dispatch a JSON object to the appropriate converter.
fn convert_object(
    obj: &serde_json::Map<String, serde_json::Value>,
    param_name: &str,
) -> Result<Expr, JsonInputError> {
    if obj.contains_key("variant") {
        convert_tagged_union(obj, param_name)
    } else if obj.contains_key("type") && obj.contains_key("fields") {
        convert_struct(obj, param_name)
    } else if obj.contains_key("fields") {
        Err(JsonInputError::MissingTypeKey {
            param: param_name.to_string(),
        })
    } else if obj.contains_key("index") && obj.contains_key("entries") {
        convert_indexed(obj, param_name)
    } else {
        Err(JsonInputError::AmbiguousObject {
            param: param_name.to_string(),
        })
    }
}

/// Convert `{"variant": "Name", "fields": {...}}` to a `ConstructorCall` expr.
///
/// Also handles bare variants: `{"variant": "Nominal"}`.
fn convert_tagged_union(
    obj: &serde_json::Map<String, serde_json::Value>,
    param_name: &str,
) -> Result<Expr, JsonInputError> {
    let variant_name = obj["variant"]
        .as_str()
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
        constructor: Spanned::new(ConstructorName::new(variant_name), SYNTH_SPAN),
        generic_args: Vec::new(),
        fields,
    }))
}

/// Convert `{"type": "TypeName", "fields": {"x": "...", ...}}` to a `ConstructorCall` expr.
///
/// The struct type name is provided explicitly via the `"type"` key.
fn convert_struct(
    obj: &serde_json::Map<String, serde_json::Value>,
    param_name: &str,
) -> Result<Expr, JsonInputError> {
    let type_name_str = obj["type"]
        .as_str()
        .ok_or_else(|| JsonInputError::InvalidType {
            param: param_name.to_string(),
        })?;

    let fields_obj = obj["fields"]
        .as_object()
        .ok_or_else(|| JsonInputError::InvalidFields {
            param: param_name.to_string(),
        })?;
    let fields = convert_field_inits(fields_obj, param_name)?;

    Ok(synth_expr(ExprKind::ConstructorCall {
        constructor: Spanned::new(ConstructorName::new(type_name_str), SYNTH_SPAN),
        generic_args: Vec::new(),
        fields,
    }))
}

/// Convert `{"index": "IndexName", "entries": {"Variant": ..., ...}}` to a `MapLiteral` expr.
///
/// The index name is provided explicitly via the `"index"` key.
fn convert_indexed(
    obj: &serde_json::Map<String, serde_json::Value>,
    param_name: &str,
) -> Result<Expr, JsonInputError> {
    let index_name = obj["index"]
        .as_str()
        .ok_or_else(|| JsonInputError::InvalidIndex {
            param: param_name.to_string(),
        })?;

    let entries_obj = obj["entries"]
        .as_object()
        .ok_or_else(|| JsonInputError::InvalidEntries {
            param: param_name.to_string(),
        })?;

    let entries = entries_obj
        .iter()
        .map(|(variant, value)| {
            let value_expr = convert_value(value, &format!("{param_name}[{variant}]"))?;
            Ok(MapEntry {
                keys: NonEmpty::singleton(MapEntryKey {
                    index: Spanned::new(
                        MapEntryIndex::Named(IndexName::new(index_name).into()),
                        SYNTH_SPAN,
                    ),
                    variant: Spanned::new(IndexVariantName::new(variant), SYNTH_SPAN),
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
    fields_obj: &serde_json::Map<String, serde_json::Value>,
    param_name: &str,
) -> Result<Vec<FieldInit>, JsonInputError> {
    fields_obj
        .iter()
        .map(|(field_name, field_val)| {
            let field_expr = convert_value(field_val, &format!("{param_name}.{field_name}"))?;
            Ok(FieldInit {
                name: Spanned::new(FieldName::new(field_name), SYNTH_SPAN),
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
    fn scalar_with_units() {
        let overrides = json_to_overrides(r#"{"dry_mass": "1500.0 kg"}"#).unwrap();
        assert!(overrides.contains_key(&DeclName::new("dry_mass")));
        let expr = &overrides[&DeclName::new("dry_mass")];
        assert!(matches!(expr.kind, ExprKind::UnitLiteral { .. }));
    }

    #[test]
    fn bool_value() {
        let overrides = json_to_overrides(r#"{"enabled": false}"#).unwrap();
        let expr = &overrides[&DeclName::new("enabled")];
        assert!(matches!(expr.kind, ExprKind::Bool(false)));
    }

    #[test]
    fn integer_value() {
        let overrides = json_to_overrides(r#"{"count": 42}"#).unwrap();
        let expr = &overrides[&DeclName::new("count")];
        assert!(matches!(expr.kind, ExprKind::Integer(42)));
    }

    #[test]
    fn dimensionless_float() {
        let overrides = json_to_overrides(r#"{"ratio": 3.33}"#).unwrap();
        let expr = &overrides[&DeclName::new("ratio")];
        assert!(matches!(expr.kind, ExprKind::Number(f) if (f - 3.33).abs() < f64::EPSILON));
    }

    #[test]
    fn struct_single_variant() {
        let json = r#"{"transfer": {"type": "TransferResult", "fields": {"dv1": "150.0 m/s", "dv2": "250.0 m/s"}}}"#;
        let overrides = json_to_overrides(json).unwrap();
        let expr = &overrides[&DeclName::new("transfer")];
        match &expr.kind {
            ExprKind::ConstructorCall {
                constructor,
                fields,
                ..
            } => {
                assert_eq!(constructor.value.as_str(), "TransferResult");
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
    fn tagged_union_with_fields() {
        let json = r#"{"maneuver": {"variant": "LowThrust", "fields": {"thrust": "0.5 N", "duration": "3600.0 s"}}}"#;
        let overrides = json_to_overrides(json).unwrap();
        let expr = &overrides[&DeclName::new("maneuver")];
        match &expr.kind {
            ExprKind::ConstructorCall {
                constructor,
                fields,
                ..
            } => {
                assert_eq!(constructor.value.as_str(), "LowThrust");
                assert_eq!(fields.len(), 2);
            }
            other => panic!("expected ConstructorCall, got {other:?}"),
        }
    }

    #[test]
    fn bare_variant_object() {
        let json = r#"{"status": {"variant": "Nominal"}}"#;
        let overrides = json_to_overrides(json).unwrap();
        let expr = &overrides[&DeclName::new("status")];
        match &expr.kind {
            ExprKind::ConstructorCall {
                constructor,
                fields,
                ..
            } => {
                assert_eq!(constructor.value.as_str(), "Nominal");
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
        assert!(overrides.contains_key(&DeclName::new("status")));
    }

    #[test]
    fn named_label_indexed() {
        let json = r#"{"delta_v": {"index": "Maneuver", "entries": {"Departure": "3.0 km/s", "Correction": "0.2 km/s", "Insertion": "2.0 km/s"}}}"#;
        let overrides = json_to_overrides(json).unwrap();
        let expr = &overrides[&DeclName::new("delta_v")];
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
        let expr = &overrides[&DeclName::new("x")];
        assert!(matches!(expr.kind, ExprKind::BinOp { .. }));
    }
}
