//! The view model's wire types: what the CLI, the language server and the
//! browser engine serialize. Plain data, already rendered as strings; the
//! projection in [`super::project`] fills them in.

use graphcal_compiler::text_position::{Utf16Position, Utf16Range};
use serde::Serialize;

/// The view of one module: one `.gcl` file.
#[derive(Debug, Clone, Serialize)]
pub struct ViewModel {
    /// Schema version of this model.
    pub version: u8,
    /// The module this view presents.
    pub module: ViewModule,
    /// Every index that appears as an axis of a node, with its labels in
    /// declaration order.
    pub indexes: Vec<ViewIndex>,
    /// The module's declarations in source order, followed by the
    /// declarations of other modules they read.
    pub nodes: Vec<ViewNode>,
}

/// Identity of the module a view presents.
#[derive(Debug, Clone, Serialize)]
pub struct ViewModule {
    /// Canonical module id, e.g. `power_project.power_max`.
    pub id: String,
    /// Source path relative to the project root.
    pub file: String,
    /// The owning package.
    pub package: String,
}

/// One index axis.
#[derive(Debug, Clone, Serialize)]
pub struct ViewIndex {
    pub name: String,
    /// The module declaring the index.
    pub module: String,
    /// Entry labels in declaration order.
    pub labels: Vec<String>,
}

/// The declaration kind behind a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewNodeKind {
    Param,
    Const,
    Node,
    Assert,
    /// A declaration of another module that this module reads.
    External,
}

/// One declaration as the view presents it.
#[derive(Debug, Clone, Serialize)]
pub struct ViewNode {
    /// Identifier unique within the model: the bare name for this module's
    /// declarations, `module::name` for other modules'.
    pub id: String,
    pub name: String,
    /// Canonical id of the declaring module.
    pub module: String,
    pub kind: ViewNodeKind,
    /// Index axes, outermost first; empty for a scalar.
    pub axes: Vec<String>,
    /// Declared element type, e.g. `Power`, `Dimensionless`, `Int`; absent
    /// for an assertion.
    pub dimension: Option<String>,
    /// The unit the values are shown in, when every value agrees on one.
    pub unit: Option<String>,
    /// Whether the declaration is a public output of its module.
    #[serde(rename = "pub")]
    pub is_public: bool,
    /// Ids of the nodes this declaration reads via `@`.
    pub deps: Vec<String>,
    /// Source text of the defining expression (const body, param default,
    /// node formula, assertion operands).
    pub expr: Option<String>,
    /// The aggregation that removed an axis of a dependency. Not derived yet.
    pub reduce: Option<ViewReduce>,
    /// For an external read through an `include`: the instance's name
    /// (`eps` in `include power_project.eps() …`, or the alias of
    /// `include … as e`). `None` for the module's own declarations and for
    /// imports, which have no instance.
    pub instance: Option<String>,
    /// Where the declaration lives.
    pub source: Option<ViewSource>,
    /// The evaluated value, when available.
    pub value: Option<ViewValue>,
}

/// An aggregation over one axis.
#[derive(Debug, Clone, Serialize)]
pub struct ViewReduce {
    pub op: String,
    pub axis: String,
}

/// Source position of a declaration.
#[derive(Debug, Clone, Serialize)]
pub struct ViewSource {
    /// Source path relative to the project root; `None` when the file is not
    /// part of the loaded project.
    pub file: Option<String>,
    /// One-based line of the declaration.
    pub line: u32,
    /// Editor range of the defining expression, for in-place edits.
    pub expr_range: Option<ViewRange>,
}

/// A half-open editor range: zero-based lines and UTF-16 columns, as the
/// Language Server Protocol counts them.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ViewRange {
    pub start: ViewPosition,
    pub end: ViewPosition,
}

/// A zero-based line and UTF-16 column.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ViewPosition {
    pub line: u32,
    pub character: u32,
}

impl From<Utf16Position> for ViewPosition {
    fn from(position: Utf16Position) -> Self {
        Self {
            line: position.line,
            character: position.character,
        }
    }
}

impl From<Utf16Range> for ViewRange {
    fn from(range: Utf16Range) -> Self {
        Self {
            start: range.start.into(),
            end: range.end.into(),
        }
    }
}

/// An evaluated value shaped for display.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ViewValue {
    /// A real quantity, shown in `unit`.
    Quantity {
        display: String,
        /// The number in `unit`, formatted.
        magnitude: String,
        /// The value in base SI units.
        si_value: f64,
        /// The unit `magnitude` is expressed in; `None` for dimensionless.
        unit: Option<String>,
    },
    Int {
        display: String,
    },
    Bool {
        display: String,
        value: bool,
    },
    /// One entry of a named index.
    Label {
        display: String,
        index: String,
        variant: String,
    },
    /// A value with one axis; nested for further axes.
    Indexed {
        display: String,
        index: String,
        entries: Vec<ViewEntry>,
    },
    /// Anything the view has no richer shape for (structs, complex numbers,
    /// datetimes), rendered as text.
    Text {
        display: String,
    },
}

/// One entry of an indexed value.
#[derive(Debug, Clone, Serialize)]
pub struct ViewEntry {
    pub key: String,
    pub value: ViewValue,
}
