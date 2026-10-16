//! The view model's wire types: what the CLI, the language server and the
//! browser engine serialize. Plain data, already rendered as strings; the
//! projection in [`super::project`] fills them in.
//!
//! Identities are opaque keys ([`ViewNodeId`], [`ViewIndexId`], entry keys):
//! a reader joins on them and never parses them. Everything a reader shows
//! travels in a field of its own (`label`, `display`, `name`).

use graphcal_compiler::text_position::{Utf16Position, Utf16Range};
use serde::Serialize;

use super::wire_key::{ViewIndexId, ViewNodeId};

/// The view of one module: one `.gcl` file.
#[derive(Debug, Clone, Serialize)]
pub struct ViewModel {
    /// Schema version of this model.
    pub version: u8,
    /// The module this view presents.
    pub module: ViewModule,
    /// Every index that appears as an axis of a node, with its entries in
    /// declaration order.
    pub indexes: Vec<ViewIndex>,
    /// The module's declarations in source order, followed by the
    /// declarations of other modules they read.
    pub nodes: Vec<ViewNode>,
}

/// Identity of the module a view presents.
#[derive(Debug, Clone, Serialize)]
pub struct ViewModule {
    /// The module path, e.g. `power_project.power_max`.
    pub id: String,
    /// Source path relative to the project root.
    pub file: String,
    /// The owning package.
    pub package: String,
}

/// One index axis.
#[derive(Debug, Clone, Serialize)]
pub struct ViewIndex {
    /// The key nodes' `axes` refer to.
    pub id: ViewIndexId,
    /// The axis as source spells it: `Component`, `Fin(3)`.
    pub name: String,
    /// The module path of the declaring module; `None` for a structural
    /// `Fin(N)` axis, which no module declares.
    pub module: Option<String>,
    /// The entries in declaration order; empty for an axis whose entries are
    /// not known (a required index, a generic parameter).
    pub entries: Vec<ViewIndexEntry>,
}

/// One entry of an index axis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViewIndexEntry {
    /// The key the entries of an indexed value refer to; opaque.
    pub key: String,
    /// The entry as `graphcal eval` shows it: a variant name, a coordinate
    /// (`0.5 s`), or a position (`#2`).
    pub display: String,
}

/// The declaration kind behind a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewNodeKind {
    Param,
    Const,
    Node,
    Assert,
}

/// Where a node's declaration comes from, as the module reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ViewOrigin {
    /// A declaration of the module itself.
    Local,
    /// A declaration of another module, read through an `import`.
    Imported,
    /// A declaration of another module, read through an `include` instance.
    Included { instance: ViewInstance },
}

/// One declaration as the view presents it.
#[derive(Debug, Clone, Serialize)]
pub struct ViewNode {
    /// Opaque key, unique within the model and stable across evaluations.
    pub id: ViewNodeId,
    /// The node as the module spells it: `margin`, an import binding
    /// (`num`), or an include member (`e::p_budget`).
    pub label: String,
    /// The declaration's own name.
    pub name: String,
    /// The module path of the declaring module (the template of an include
    /// instance).
    pub module: String,
    pub kind: ViewNodeKind,
    pub origin: ViewOrigin,
    /// Index axes, outermost first; empty for a scalar.
    pub axes: Vec<ViewIndexId>,
    /// Declared element type, e.g. `Power`, `Dimensionless`, `Int`; absent
    /// for an assertion.
    pub dimension: Option<String>,
    /// The unit the values are shown in, when every value agrees on one.
    pub unit: Option<String>,
    /// Whether the declaration is a public output of its module.
    #[serde(rename = "pub")]
    pub is_public: bool,
    /// Ids of the nodes this declaration reads via `@`.
    pub deps: Vec<ViewNodeId>,
    /// Source text of the defining expression (const body, param default,
    /// node formula, assertion operands).
    pub expr: Option<String>,
    /// The aggregation that removed an axis of a dependency. Not derived yet.
    pub reduce: Option<ViewReduce>,
    /// The declaration's `///` documentation: one line per `///` line,
    /// joined with newlines. The view shows its first line beside the name.
    pub doc: Option<String>,
    /// Where the declaration lives.
    pub source: Option<ViewSource>,
    /// The evaluated value, when available.
    pub value: Option<ViewValue>,
}

/// An include instance a node is read through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViewInstance {
    /// The instance's name: `eps` in `include power_project.eps() …`, or
    /// the alias of `include … as e`.
    pub name: String,
    /// Whether the include binds any of the template's params or Static
    /// parameters. A bound instance's values differ from those of its
    /// module evaluated alone, which is all a view of that module shows.
    pub bound: bool,
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
    /// A value with one axis; nested for further axes. The entries follow
    /// the node's axes outermost first.
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
    /// The key of the axis entry ([`ViewIndexEntry::key`]) this value sits
    /// at.
    pub key: String,
    pub value: ViewValue,
}
