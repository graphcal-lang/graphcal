//! Diagnostics of nominal struct types, their fields, and constructors.
//!
//! Each variant is a typed payload located by a
//! [`Diagnostic`](crate::diagnostic::Diagnostic); its stable code, labels,
//! and help are described by its [`DiagnosticKind`] implementation.

use thiserror::Error;

use crate::diagnostic::{DiagnosticKind, SecondaryLabel};
use crate::syntax::type_name::{ConstructorName, FieldName, StructTypeName};

/// A member a nominal-type diagnostic names: a payload field, or a
/// constructor that does not belong to the scrutinized type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NominalMember {
    Field(FieldName),
    Constructor(ConstructorName),
}

impl std::fmt::Display for NominalMember {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Field(name) => name.fmt(f),
            Self::Constructor(name) => name.fmt(f),
        }
    }
}

/// Diagnostics of nominal struct types, their fields, and constructors.
#[derive(Debug, Clone, Error)]
pub enum StructError {
    #[error("unknown struct type `{name}`")]
    UnknownStructType { name: String },
    #[error("unknown field `{member}` on struct `{type_name}`")]
    UnknownField {
        type_name: StructTypeName,
        member: NominalMember,
    },
    #[error("missing field(s) {missing:?} in construction of `{type_name}`")]
    MissingFields {
        type_name: StructTypeName,
        missing: Vec<FieldName>,
    },
    #[error("missing field(s) {missing:?} in match pattern for `{constructor}`")]
    MissingPatternFields {
        constructor: ConstructorName,
        missing: Vec<FieldName>,
    },
    #[error("constructor `{constructor}` cannot use empty parentheses")]
    EmptyParenthesizedConstructor { constructor: ConstructorName },
    #[error("extra field(s) {extra:?} in construction of `{type_name}`")]
    ExtraFields {
        type_name: StructTypeName,
        extra: Vec<FieldName>,
    },
    #[error("field `{field_name}` of `{type_name}`: expected dimension {expected}, found {found}")]
    FieldDimensionMismatch {
        type_name: StructTypeName,
        field_name: FieldName,
        expected: String,
        found: String,
    },
    #[error("cannot access field of non-struct value `{name}`")]
    NotAStruct { name: String },
    #[error("unknown local variable `{name}`")]
    UnknownLocalRef { name: String },
}

impl DiagnosticKind for StructError {
    fn code(&self) -> &'static str {
        match self {
            Self::UnknownStructType { .. } => "graphcal::S002",
            Self::UnknownField { .. } => "graphcal::S003",
            Self::MissingFields { .. } => "graphcal::S004",
            Self::MissingPatternFields { .. } => "graphcal::S009",
            Self::EmptyParenthesizedConstructor { .. } => "graphcal::S010",
            Self::ExtraFields { .. } => "graphcal::S005",
            Self::FieldDimensionMismatch { .. } => "graphcal::S006",
            Self::NotAStruct { .. } => "graphcal::S007",
            Self::UnknownLocalRef { .. } => "graphcal::S008",
        }
    }

    fn primary_label(&self) -> Option<String> {
        match self {
            Self::UnknownStructType { .. } | Self::UnknownLocalRef { .. } => {
                Some("not found".to_owned())
            }
            Self::UnknownField { .. } => Some("no such field".to_owned()),
            Self::MissingFields { .. } => Some("incomplete construction".to_owned()),
            Self::MissingPatternFields { .. } => Some("incomplete pattern".to_owned()),
            Self::EmptyParenthesizedConstructor { .. } => {
                Some("empty parentheses are invalid here".to_owned())
            }
            Self::ExtraFields { .. } => Some("unexpected fields".to_owned()),
            Self::FieldDimensionMismatch { found, .. } => Some(format!("has dimension {found}")),
            Self::NotAStruct { .. } => Some("not a struct".to_owned()),
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::UnknownStructType { .. } => Some("struct types must be declared with `type` before use".to_owned()),
            Self::UnknownField { .. }
            | Self::FieldDimensionMismatch { .. } => None,
            Self::MissingFields { .. } => Some("all fields are required when constructing a struct".to_owned()),
            Self::MissingPatternFields { .. } => Some("all constructor fields must be bound as `field: variable` or discarded with `field: _`".to_owned()),
            Self::EmptyParenthesizedConstructor { constructor, .. } => Some(format!("write a unit constructor as `{constructor}`; payload constructors require named field arguments")),
            Self::ExtraFields { .. } => Some("only fields declared in the struct type are allowed".to_owned()),
            Self::NotAStruct { .. } => Some("field access `.field` is only valid on struct values".to_owned()),
            Self::UnknownLocalRef { .. } => Some("local variables are introduced by `for`, `scan`, `unfold`, `match`, or function parameters".to_owned()),
        }
    }

    fn secondary_labels(&self) -> Vec<SecondaryLabel> {
        match self {
            Self::UnknownStructType { .. }
            | Self::UnknownField { .. }
            | Self::MissingFields { .. }
            | Self::MissingPatternFields { .. }
            | Self::EmptyParenthesizedConstructor { .. }
            | Self::ExtraFields { .. }
            | Self::FieldDimensionMismatch { .. }
            | Self::NotAStruct { .. }
            | Self::UnknownLocalRef { .. } => Vec::new(),
        }
    }
}
