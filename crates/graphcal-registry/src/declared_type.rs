//! Declared type of a const/param/node.

use graphcal_syntax::dimension::Dimension;
use graphcal_syntax::names::{IndexName, StructTypeName};

use crate::time_scale::TimeScale;

/// The declared type of a const/param/node: either a scalar with a dimension, a bool, or a struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclaredType {
    Scalar(Dimension),
    Bool,
    Int,
    /// A datetime instant in a specific time scale. `Datetime(UTC)` is the default for civil use.
    Datetime(TimeScale),
    /// A label of a named index (e.g., `Maneuver::Departure` has type `Label(Maneuver)`).
    Label(IndexName),
    /// A struct type, optionally with concrete type arguments for generic structs.
    Struct(StructTypeName, Vec<Self>),
    Indexed {
        element: Box<Self>,
        index: IndexName,
    },
}
