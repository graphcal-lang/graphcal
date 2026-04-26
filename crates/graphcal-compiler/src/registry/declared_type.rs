//! Declared type of a const/param/node.

use crate::syntax::dimension::Dimension;
use crate::syntax::names::{IndexName, StructTypeName};

use crate::registry::time_scale::TimeScale;
use crate::registry::types::DimensionRegistry;

/// The declared type of a const/param/node: either a scalar with a dimension, a bool, or a struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclaredType {
    Scalar(Dimension),
    Bool,
    Int,
    /// A datetime instant in a specific time scale. `Datetime(UTC)` is the default for civil use.
    Datetime(TimeScale),
    /// A label of a named index (e.g., `Maneuver.Departure` has type `Label(Maneuver)`).
    Label(IndexName),
    /// A struct type, optionally with concrete type arguments for generic structs.
    Struct(StructTypeName, Vec<Self>),
    Indexed {
        element: Box<Self>,
        index: IndexName,
    },
}

impl DeclaredType {
    /// Format as a human-readable string for diagnostics (e.g. `"Length / Time"`, `"Bool"`).
    #[must_use]
    pub fn format(&self, dims: &DimensionRegistry) -> String {
        match self {
            Self::Scalar(d) => dims.format_dimension(d),
            Self::Bool => "Bool".to_string(),
            Self::Int => "Int".to_string(),
            Self::Datetime(scale) => {
                if scale.is_utc() {
                    "Datetime".to_string()
                } else {
                    format!("Datetime<{scale}>")
                }
            }
            Self::Label(index) => format!("Label({index})"),
            Self::Struct(name, args) => {
                if args.is_empty() {
                    name.to_string()
                } else {
                    let args_str: Vec<String> = args.iter().map(|a| a.format(dims)).collect();
                    format!("{name}<{}>", args_str.join(", "))
                }
            }
            Self::Indexed { element, index } => {
                format!("{}[{index}]", element.format(dims))
            }
        }
    }
}
