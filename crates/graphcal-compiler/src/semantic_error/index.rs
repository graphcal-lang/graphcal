//! Index diagnostics: unknown, missing, and mismatched index variants and bindings.
//!
//! Each variant is a typed payload located by a
//! [`Diagnostic`](crate::diagnostic::Diagnostic); its stable code, labels,
//! and help are described by its [`DiagnosticKind`] implementation.

use thiserror::Error;

use crate::diagnostic::{DiagnosticKind, SecondaryLabel};
use crate::semantic::checked_type::IndexDisplayName;
use crate::syntax::ast::NatExpr;
use crate::syntax::index_name::{IndexEntryKey, IndexName, IndexVariantName};
use crate::syntax::names::NameAtom;

/// A Nat written where an explicit Index is required.
#[derive(Debug, Clone)]
pub enum FoundNat {
    /// A Nat expression, such as `3` or `N + 1`.
    Expression(NatExpr),
    /// A generic Nat parameter named where an Index parameter is required.
    Parameter(NameAtom),
}

/// Two found Nats are equal when they are the same expression up to source
/// layout, or name the same parameter.
impl PartialEq for FoundNat {
    fn eq(&self, other: &Self) -> bool {
        use crate::syntax::format_equivalent::FormatEquivalent as _;
        match (self, other) {
            (Self::Expression(lhs), Self::Expression(rhs)) => lhs.format_equivalent(rhs),
            (Self::Parameter(lhs), Self::Parameter(rhs)) => lhs == rhs,
            (Self::Expression(_), Self::Parameter(_))
            | (Self::Parameter(_), Self::Expression(_)) => false,
        }
    }
}

impl Eq for FoundNat {}

impl std::fmt::Display for FoundNat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Expression(expression) => expression.fmt(f),
            Self::Parameter(name) => name.fmt(f),
        }
    }
}

/// Index diagnostics: unknown, missing, and mismatched index variants and bindings.
#[derive(Debug, Clone, Error)]
pub enum IndexError {
    #[error("unknown index `{name}`")]
    UnknownIndex { name: IndexDisplayName },
    #[error("unknown variant `{variant_name}` in index `{index_name}`")]
    UnknownVariant {
        index_name: IndexDisplayName,
        variant_name: IndexVariantName,
    },
    #[error(
        "missing variant(s) [{}] in map literal for index `{index_name}`",
        format_index_entry_keys(missing)
    )]
    MissingVariants {
        index_name: IndexDisplayName,
        missing: Vec<IndexEntryKey>,
    },
    #[error(
        "extra variant(s) [{}] in map literal for index `{index_name}`",
        format_index_entry_keys(extra)
    )]
    ExtraVariants {
        index_name: IndexDisplayName,
        extra: Vec<IndexEntryKey>,
    },
    #[error("index mismatch: expected `{expected}`, found `{found}`")]
    IndexMismatch {
        expected: IndexDisplayName,
        found: IndexDisplayName,
    },
    #[error("coordinate index `{name}`: {message}")]
    CoordinateIndexDimensionMismatch { name: IndexName, message: String },
    #[error("coordinate index `{name}`: {message}")]
    CoordinateIndexInvalid {
        name: IndexName,
        message: String,
        help: String,
    },
    #[error("expected Index, found Nat `{expression}`")]
    ExpectedIndexFoundNat { expression: FoundNat },
    #[error(
        "index dimension mismatch: `{dep_index}` requires dimension {expected_dim} but `{bound_index}` has dimension {found_dim}"
    )]
    IndexBindingDimensionMismatch {
        dep_index: String,
        expected_dim: String,
        bound_index: String,
        found_dim: String,
    },
    /// A required typed Static input was not bound at a DAG instantiation boundary.
    #[error("required {kind} `{name}` must be bound at DAG instantiation")]
    RequiredStaticInputNotBound {
        kind: crate::static_interface::StaticInputKind,
        name: String,
    },
}

impl DiagnosticKind for IndexError {
    fn code(&self) -> &'static str {
        match self {
            Self::UnknownIndex { .. } => "graphcal::I001",
            Self::UnknownVariant { .. } => "graphcal::I002",
            Self::MissingVariants { .. } => "graphcal::I003",
            Self::ExtraVariants { .. } => "graphcal::I004",
            Self::IndexMismatch { .. } => "graphcal::I005",
            Self::CoordinateIndexDimensionMismatch { .. } => "graphcal::I006",
            Self::CoordinateIndexInvalid { .. } => "graphcal::I007",
            Self::ExpectedIndexFoundNat { .. } => "graphcal::I008",
            Self::IndexBindingDimensionMismatch { .. } => "graphcal::I009",
            Self::RequiredStaticInputNotBound { .. } => "graphcal::I010",
        }
    }

    fn primary_label(&self) -> Option<String> {
        match self {
            Self::UnknownIndex { .. } => Some("unknown index".to_owned()),
            Self::UnknownVariant { index_name, .. } => {
                Some(format!("not a variant of `{index_name}`"))
            }
            Self::MissingVariants { .. } => Some("incomplete map literal".to_owned()),
            Self::ExtraVariants { .. } => Some("unexpected variants".to_owned()),
            Self::IndexMismatch { .. } => Some("wrong index".to_owned()),
            Self::CoordinateIndexDimensionMismatch { .. }
            | Self::IndexBindingDimensionMismatch { .. } => Some("dimension mismatch".to_owned()),
            Self::CoordinateIndexInvalid { .. } => Some("invalid coordinate index".to_owned()),
            Self::ExpectedIndexFoundNat { .. } => {
                Some("Nat is not implicitly converted to Index".to_owned())
            }
            Self::RequiredStaticInputNotBound { kind, .. } => {
                Some(format!("required {kind} input is not bound"))
            }
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::UnknownIndex { .. } => Some("declare a named or coordinate index, or write `Fin(N)` explicitly for a structural axis; coordinate constructors are `range(start, end, step: delta)` and `linspace(start, end, points: N)`".to_owned()),
            Self::UnknownVariant { .. }
            | Self::IndexMismatch { .. } => None,
            Self::MissingVariants { .. } => Some("map literals must cover all variants of the index".to_owned()),
            Self::ExtraVariants { .. } => Some("only variants declared in the index are allowed".to_owned()),
            Self::CoordinateIndexDimensionMismatch { .. } => Some("coordinate constructor arguments must have exactly the same dimension".to_owned()),
            Self::CoordinateIndexInvalid { help, .. } => Some(help.clone()),
            Self::ExpectedIndexFoundNat { expression, .. } => Some(format!("write `Fin({expression})` for an explicit finite structural index")),
            Self::IndexBindingDimensionMismatch { .. } => Some("coordinate-index bindings must have matching dimensions".to_owned()),
            Self::RequiredStaticInputNotBound { kind, .. } => Some(format!("bind the input with its explicit `{kind}` marker at the include or direct-call site")),
        }
    }

    fn secondary_labels(&self) -> Vec<SecondaryLabel> {
        match self {
            Self::UnknownIndex { .. }
            | Self::UnknownVariant { .. }
            | Self::MissingVariants { .. }
            | Self::ExtraVariants { .. }
            | Self::IndexMismatch { .. }
            | Self::CoordinateIndexDimensionMismatch { .. }
            | Self::CoordinateIndexInvalid { .. }
            | Self::ExpectedIndexFoundNat { .. }
            | Self::IndexBindingDimensionMismatch { .. }
            | Self::RequiredStaticInputNotBound { .. } => Vec::new(),
        }
    }
}

fn format_index_entry_keys(keys: &[IndexEntryKey]) -> String {
    keys.iter()
        .map(|key| format!("\"{key}\""))
        .collect::<Vec<_>>()
        .join(", ")
}
