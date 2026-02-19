//! Convert a JSON input file into parameter overrides (`HashMap<DeclName, Expr>`).
//!
//! The JSON schema (Option E) uses expression strings for scalar leaves and
//! JSON objects for structs, tagged unions, and named-label indexed params.
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
//! | `object` with `"fields"` (no `"variant"`) | Single-variant struct |
//! | `object` (other keys) | Named-label indexed param |

use std::collections::HashMap;
use std::fmt;

use graphcal_syntax::ast::{
    self, DeclKind, Expr, ExprKind, FieldInit, IndexDeclKind, MapEntry, MapEntryKey, TypeExprKind,
};
use graphcal_syntax::names::{
    DeclName, FieldName, IndexName, Spanned, StructTypeName, VariantName,
};
use graphcal_syntax::span::Span;

/// A synthetic span used for all AST nodes constructed from JSON input.
const SYNTH_SPAN: Span = Span::new(0, 0);

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur when converting a JSON input file to overrides.
#[derive(Debug)]
pub enum JsonInputError {
    /// The JSON file could not be read.
    Io(std::io::Error),
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
    /// A structured object was used but the param has no type annotation in the AST.
    MissingTypeAnnotation { param: String },
    /// The type annotation cannot be resolved to a struct type name.
    CannotInferTypeName { param: String },
    /// Attempted to override a range-indexed param, which is not supported.
    RangeIndexNotSupported { param: String, index: String },
    /// The `"variant"` field is not a string.
    InvalidVariant { param: String },
    /// The `"fields"` field is not an object.
    InvalidFields { param: String },
    /// The type annotation is not an indexed type.
    NotAnIndexedType { param: String },
}

impl fmt::Display for JsonInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "cannot read input file: {e}"),
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
            Self::MissingTypeAnnotation { param } => {
                write!(
                    f,
                    "param `{param}` not found in .gcl file (needed for structured JSON input)"
                )
            }
            Self::CannotInferTypeName { param } => {
                write!(
                    f,
                    "cannot determine struct type name from type annotation of `{param}`"
                )
            }
            Self::RangeIndexNotSupported { param, index } => {
                write!(
                    f,
                    "overriding range-indexed param `{param}` (index `{index}`) is not supported"
                )
            }
            Self::InvalidVariant { param } => {
                write!(f, "`variant` field for `{param}` must be a string")
            }
            Self::InvalidFields { param } => {
                write!(f, "`fields` field for `{param}` must be an object")
            }
            Self::NotAnIndexedType { param } => {
                write!(
                    f,
                    "param `{param}` is not an indexed type but JSON value looks like a map"
                )
            }
        }
    }
}

impl std::error::Error for JsonInputError {}

impl From<std::io::Error> for JsonInputError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for JsonInputError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

// ---------------------------------------------------------------------------
// AST info extraction
// ---------------------------------------------------------------------------

/// Extract param name → type annotation from a parsed AST.
fn extract_param_type_anns(file: &ast::File) -> HashMap<&str, &ast::TypeExpr> {
    let mut map = HashMap::new();
    for decl in &file.declarations {
        if let DeclKind::Param(p) = &decl.kind {
            map.insert(p.name.value.as_str(), &p.type_ann);
        }
    }
    map
}

/// Extract index name → `is_range` from a parsed AST.
fn extract_index_kinds(file: &ast::File) -> HashMap<&str, bool> {
    let mut map = HashMap::new();
    for decl in &file.declarations {
        if let DeclKind::Index(idx) = &decl.kind {
            let is_range = matches!(idx.kind, IndexDeclKind::Range { .. });
            map.insert(idx.name.value.as_str(), is_range);
        }
    }
    map
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Convert a JSON input string into a `HashMap` of parameter overrides.
///
/// The `ast` is the parsed `.gcl` file, used to extract type annotations
/// for struct type names and index names.
pub fn json_to_overrides(
    json_str: &str,
    ast: &ast::File,
) -> Result<HashMap<DeclName, Expr>, JsonInputError> {
    let json: serde_json::Value = serde_json::from_str(json_str)?;
    let obj = json.as_object().ok_or(JsonInputError::TopLevelNotObject)?;

    let type_anns = extract_param_type_anns(ast);
    let index_kinds = extract_index_kinds(ast);

    let mut overrides = HashMap::new();
    for (name, value) in obj {
        let type_ann = type_anns.get(name.as_str()).copied();
        let expr = convert_value(value, type_ann, &index_kinds, name)?;
        overrides.insert(DeclName::new(name), expr);
    }
    Ok(overrides)
}

// ---------------------------------------------------------------------------
// Value conversion
// ---------------------------------------------------------------------------

/// Convert a single JSON value into an AST `Expr`.
fn convert_value(
    value: &serde_json::Value,
    type_ann: Option<&ast::TypeExpr>,
    index_kinds: &HashMap<&str, bool>,
    param_name: &str,
) -> Result<Expr, JsonInputError> {
    match value {
        serde_json::Value::String(s) => convert_string(s, param_name),
        serde_json::Value::Bool(b) => Ok(synth_expr(ExprKind::Bool(*b))),
        serde_json::Value::Number(n) => convert_number(n, param_name),
        serde_json::Value::Object(obj) => convert_object(obj, type_ann, index_kinds, param_name),
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
    graphcal_syntax::parser::Parser::new(s)
        .parse_single_expr()
        .map_err(|e| JsonInputError::ParseFailed {
            param: param_name.to_string(),
            message: e.to_string(),
        })
}

/// Convert a JSON number to an integer or dimensionless float.
fn convert_number(n: &serde_json::Number, param_name: &str) -> Result<Expr, JsonInputError> {
    n.as_i64().map_or_else(
        || {
            n.as_f64().map_or_else(
                || {
                    Err(JsonInputError::InvalidNumber {
                        param: param_name.to_string(),
                    })
                },
                |f| Ok(synth_expr(ExprKind::Number(f))),
            )
        },
        |i| Ok(synth_expr(ExprKind::Integer(i))),
    )
}

/// Dispatch a JSON object to the appropriate converter.
fn convert_object(
    obj: &serde_json::Map<String, serde_json::Value>,
    type_ann: Option<&ast::TypeExpr>,
    index_kinds: &HashMap<&str, bool>,
    param_name: &str,
) -> Result<Expr, JsonInputError> {
    if obj.contains_key("variant") {
        convert_tagged_union(obj, param_name)
    } else if obj.contains_key("fields") {
        convert_struct(obj, type_ann, index_kinds, param_name)
    } else {
        convert_indexed(obj, type_ann, index_kinds, param_name)
    }
}

/// Convert `{"variant": "Name", "fields": {...}}` to a `StructConstruction` expr.
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

    Ok(synth_expr(ExprKind::StructConstruction {
        type_name: Spanned::new(StructTypeName::new(variant_name), SYNTH_SPAN),
        type_args: Vec::new(),
        fields,
    }))
}

/// Convert `{"fields": {"x": "...", ...}}` to a `StructConstruction` expr.
///
/// Requires the param's type annotation to determine the struct type name.
fn convert_struct(
    obj: &serde_json::Map<String, serde_json::Value>,
    type_ann: Option<&ast::TypeExpr>,
    index_kinds: &HashMap<&str, bool>,
    param_name: &str,
) -> Result<Expr, JsonInputError> {
    let type_ann = type_ann.ok_or_else(|| JsonInputError::MissingTypeAnnotation {
        param: param_name.to_string(),
    })?;

    let (type_name_str, type_args) = extract_struct_type_name(type_ann, index_kinds, param_name)?;

    let fields_val = &obj["fields"];
    let fields_obj = fields_val
        .as_object()
        .ok_or_else(|| JsonInputError::InvalidFields {
            param: param_name.to_string(),
        })?;
    let fields = convert_field_inits(fields_obj, param_name)?;

    Ok(synth_expr(ExprKind::StructConstruction {
        type_name: Spanned::new(StructTypeName::new(type_name_str), SYNTH_SPAN),
        type_args,
        fields,
    }))
}

/// Convert a plain JSON object to a `MapLiteral` expr (named-label indexed param).
fn convert_indexed(
    obj: &serde_json::Map<String, serde_json::Value>,
    type_ann: Option<&ast::TypeExpr>,
    index_kinds: &HashMap<&str, bool>,
    param_name: &str,
) -> Result<Expr, JsonInputError> {
    let type_ann = type_ann.ok_or_else(|| JsonInputError::MissingTypeAnnotation {
        param: param_name.to_string(),
    })?;

    let index_name = extract_index_name(type_ann, param_name)?;

    // Reject range indexes
    if let Some(&is_range) = index_kinds.get(index_name.as_str())
        && is_range
    {
        return Err(JsonInputError::RangeIndexNotSupported {
            param: param_name.to_string(),
            index: index_name.clone(),
        });
    }

    let entries = obj
        .iter()
        .map(|(variant, value)| {
            let value_expr = convert_value(
                value,
                None,
                index_kinds,
                &format!("{param_name}[{variant}]"),
            )?;
            Ok(MapEntry {
                keys: vec![MapEntryKey {
                    index: Spanned::new(IndexName::new(&index_name), SYNTH_SPAN),
                    variant: Spanned::new(VariantName::new(variant), SYNTH_SPAN),
                }],
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
            let field_expr = convert_value(
                field_val,
                None,
                &HashMap::new(),
                &format!("{param_name}.{field_name}"),
            )?;
            Ok(FieldInit {
                name: Spanned::new(FieldName::new(field_name), SYNTH_SPAN),
                value: Some(field_expr),
            })
        })
        .collect()
}

/// Extract the struct type name and type args from a type annotation.
///
/// Handles:
/// - `TypeApplication { name: "Vec3", type_args: [Length, Eci] }` → generic struct
/// - `DimExpr` with a single term (e.g., `TransferResult`) → simple struct name
fn extract_struct_type_name(
    type_ann: &ast::TypeExpr,
    index_kinds: &HashMap<&str, bool>,
    param_name: &str,
) -> Result<(String, Vec<ast::TypeExpr>), JsonInputError> {
    match &type_ann.kind {
        TypeExprKind::TypeApplication { name, type_args } => {
            Ok((name.name.clone(), type_args.clone()))
        }
        TypeExprKind::DimExpr(dim_expr)
            if dim_expr.terms.len() == 1 && dim_expr.terms[0].term.power.is_none() =>
        {
            let name = &dim_expr.terms[0].term.name.name;
            // Make sure this isn't actually an index name (which would be an indexed param,
            // not a struct).
            if index_kinds.contains_key(name.as_str()) {
                return Err(JsonInputError::CannotInferTypeName {
                    param: param_name.to_string(),
                });
            }
            Ok((name.clone(), Vec::new()))
        }
        _ => Err(JsonInputError::CannotInferTypeName {
            param: param_name.to_string(),
        }),
    }
}

/// Extract the index name from an indexed type annotation like `Velocity[Maneuver]`.
fn extract_index_name(
    type_ann: &ast::TypeExpr,
    param_name: &str,
) -> Result<String, JsonInputError> {
    match &type_ann.kind {
        TypeExprKind::Indexed { indexes, .. } => indexes.first().map_or_else(
            || {
                Err(JsonInputError::NotAnIndexedType {
                    param: param_name.to_string(),
                })
            },
            |first| Ok(first.name.clone()),
        ),
        _ => Err(JsonInputError::NotAnIndexedType {
            param: param_name.to_string(),
        }),
    }
}

/// Create an `Expr` with a synthetic span.
const fn synth_expr(kind: ExprKind) -> Expr {
    Expr {
        kind,
        span: SYNTH_SPAN,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]

    use super::*;
    use graphcal_syntax::parser::Parser;

    /// Parse a `.gcl` source into an AST for test use.
    fn parse_gcl(source: &str) -> ast::File {
        Parser::new(source).parse_file().unwrap()
    }

    #[test]
    fn scalar_with_units() {
        let ast = parse_gcl("param dry_mass: Mass = 1200.0 kg;");
        let overrides = json_to_overrides(r#"{"dry_mass": "1500.0 kg"}"#, &ast).unwrap();
        assert!(overrides.contains_key(&DeclName::new("dry_mass")));
        let expr = &overrides[&DeclName::new("dry_mass")];
        assert!(matches!(expr.kind, ExprKind::UnitLiteral { .. }));
    }

    #[test]
    fn bool_value() {
        let ast = parse_gcl("param enabled: Bool = true;");
        let overrides = json_to_overrides(r#"{"enabled": false}"#, &ast).unwrap();
        let expr = &overrides[&DeclName::new("enabled")];
        assert!(matches!(expr.kind, ExprKind::Bool(false)));
    }

    #[test]
    fn integer_value() {
        let ast = parse_gcl("param count: Int = 10;");
        let overrides = json_to_overrides(r#"{"count": 42}"#, &ast).unwrap();
        let expr = &overrides[&DeclName::new("count")];
        assert!(matches!(expr.kind, ExprKind::Integer(42)));
    }

    #[test]
    fn dimensionless_float() {
        let ast = parse_gcl("param ratio: Dimensionless = 1.0;");
        let overrides = json_to_overrides(r#"{"ratio": 3.33}"#, &ast).unwrap();
        let expr = &overrides[&DeclName::new("ratio")];
        assert!(matches!(expr.kind, ExprKind::Number(f) if (f - 3.33).abs() < f64::EPSILON));
    }

    #[test]
    fn struct_single_variant() {
        let ast = parse_gcl(
            r"
            dimension Velocity = Length / Time;
            type TransferResult { dv1: Velocity, dv2: Velocity }
            param transfer: TransferResult = TransferResult { dv1: 100.0 m/s, dv2: 200.0 m/s };
            ",
        );
        let json = r#"{"transfer": {"fields": {"dv1": "150.0 m/s", "dv2": "250.0 m/s"}}}"#;
        let overrides = json_to_overrides(json, &ast).unwrap();
        let expr = &overrides[&DeclName::new("transfer")];
        match &expr.kind {
            ExprKind::StructConstruction {
                type_name, fields, ..
            } => {
                assert_eq!(type_name.value.as_str(), "TransferResult");
                assert_eq!(fields.len(), 2);
            }
            other => panic!("expected StructConstruction, got {other:?}"),
        }
    }

    #[test]
    fn tagged_union_with_fields() {
        let ast = parse_gcl(
            r"
            dimension Velocity = Length / Time;
            dimension Force = Mass * Length / Time^2;
            type ManeuverKind {
                Impulsive { delta_v: Velocity }
                LowThrust { thrust: Force, duration: Time }
            }
            param maneuver: ManeuverKind = Impulsive { delta_v: 100.0 m/s };
            ",
        );
        let json = r#"{"maneuver": {"variant": "LowThrust", "fields": {"thrust": "0.5 N", "duration": "3600.0 s"}}}"#;
        let overrides = json_to_overrides(json, &ast).unwrap();
        let expr = &overrides[&DeclName::new("maneuver")];
        match &expr.kind {
            ExprKind::StructConstruction {
                type_name, fields, ..
            } => {
                assert_eq!(type_name.value.as_str(), "LowThrust");
                assert_eq!(fields.len(), 2);
            }
            other => panic!("expected StructConstruction, got {other:?}"),
        }
    }

    #[test]
    fn bare_variant_object() {
        let ast = parse_gcl(
            r"
            type Status { Nominal  Warning { code: Dimensionless } }
            param status: Status = Nominal;
            ",
        );
        let json = r#"{"status": {"variant": "Nominal"}}"#;
        let overrides = json_to_overrides(json, &ast).unwrap();
        let expr = &overrides[&DeclName::new("status")];
        match &expr.kind {
            ExprKind::StructConstruction {
                type_name, fields, ..
            } => {
                assert_eq!(type_name.value.as_str(), "Nominal");
                assert!(fields.is_empty());
            }
            other => panic!("expected StructConstruction, got {other:?}"),
        }
    }

    #[test]
    fn bare_variant_string() {
        let ast = parse_gcl(
            r"
            type Status { Nominal  Warning { code: Dimensionless } }
            param status: Status = Nominal;
            ",
        );
        // String "Nominal" is parsed as a GCL expression, which produces a ConstRef
        // (PascalCase identifier). The evaluator handles this as a bare variant.
        let json = r#"{"status": "Nominal"}"#;
        let overrides = json_to_overrides(json, &ast).unwrap();
        assert!(overrides.contains_key(&DeclName::new("status")));
    }

    #[test]
    fn named_label_indexed() {
        let ast = parse_gcl(
            r"
            dimension Velocity = Length / Time;
            index Maneuver = { Departure, Correction, Insertion }
            param delta_v: Velocity[Maneuver] = {
                Maneuver::Departure: 2.46 km/s,
                Maneuver::Correction: 0.12 km/s,
                Maneuver::Insertion: 1.83 km/s,
            };
            ",
        );
        let json = r#"{"delta_v": {"Departure": "3.0 km/s", "Correction": "0.2 km/s", "Insertion": "2.0 km/s"}}"#;
        let overrides = json_to_overrides(json, &ast).unwrap();
        let expr = &overrides[&DeclName::new("delta_v")];
        match &expr.kind {
            ExprKind::MapLiteral { entries } => {
                assert_eq!(entries.len(), 3);
                assert_eq!(entries[0].keys[0].index.value.as_str(), "Maneuver");
            }
            other => panic!("expected MapLiteral, got {other:?}"),
        }
    }

    #[test]
    fn range_index_rejected() {
        let ast = parse_gcl(
            r"
            index Step = range(0.0 s, 1.0 s, step: 0.25 s);
            param y: Dimensionless[Step] = unfold(10.0, |prev_t, t| 0.0);
            ",
        );
        let json = r#"{"y": {"0 s": "10.0", "0.25 s": "9.5"}}"#;
        let result = json_to_overrides(json, &ast);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, JsonInputError::RangeIndexNotSupported { .. }),
            "expected RangeIndexNotSupported, got {err}"
        );
    }

    #[test]
    fn unsupported_null() {
        let ast = parse_gcl("param x: Dimensionless = 1.0;");
        let result = json_to_overrides(r#"{"x": null}"#, &ast);
        assert!(result.is_err());
    }

    #[test]
    fn unsupported_array() {
        let ast = parse_gcl("param x: Dimensionless = 1.0;");
        let result = json_to_overrides(r#"{"x": [1, 2, 3]}"#, &ast);
        assert!(result.is_err());
    }

    #[test]
    fn top_level_not_object() {
        let ast = parse_gcl("param x: Dimensionless = 1.0;");
        let result = json_to_overrides(r#""not an object""#, &ast);
        assert!(matches!(result, Err(JsonInputError::TopLevelNotObject)));
    }

    #[test]
    fn expression_string_with_arithmetic() {
        let ast = parse_gcl("param x: Dimensionless = 1.0;");
        let overrides = json_to_overrides(r#"{"x": "2.0 + 3.0"}"#, &ast).unwrap();
        let expr = &overrides[&DeclName::new("x")];
        assert!(matches!(expr.kind, ExprKind::BinOp { .. }));
    }
}
